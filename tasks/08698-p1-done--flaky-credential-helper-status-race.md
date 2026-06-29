# Flaky test: credential_helper status race (Running vs Valid)

`phoenix-llm` test `credential_helper::tests::expire_if_needed_is_noop_while_valid`
fails intermittently under load (observed in `./dev.py check --lanes rust` and a
full CI rust lane). Passes 3/3 when run in isolation, so it is a timing flake,
not a logic bug.

## Symptom

```
thread 'credential_helper::tests::expire_if_needed_is_noop_while_valid' panicked at
crates/phoenix-llm/src/credential_helper.rs:586:9:
assertion `left == right` failed
  left: Running
 right: Valid
```

The test spawns a helper subprocess (`printf 'TOK\n'`), `drain()`s its event
stream, then asserts `credential_status() == Valid`. Under parallel-test load it
observes `Running` — the status has not yet transitioned out of `Running` at the
moment of the assertion.

## Root cause (to confirm)

Static analysis of `spawn_helper_task` shows the state is replaced with
`Valid` *before* `Complete` is sent to subscribers, so a subscriber that
receives `Complete` should already observe `Valid`. The literal “stream
completed but status still `Running`” window therefore needs confirmation.

The more likely mechanism is that `drain` returns *without* a terminal event:
it loops on `timeout(TICK, s.next())` and exits on timeout or stream close.
Under heavy parallel-test load the helper subprocess or tokio task may not run
within the 5-second `TICK`, so `drain` returns early while the helper is still
`Running`. The subsequent `credential_status()` then sees `Running`.

The same pattern affects sibling tests that follow `drain(...); assert_eq!(status, ...)`
(e.g. `ttl_zero_expires_valid_to_idle`, `invalidate_clears_valid_*`).

## Confirmation plan

1. Instrument the failing test to record whether `drain` observed `Complete`,
   `Error`, timeout, or stream-close.
2. Reproduce under stress: `cargo test -p phoenix-llm credential_helper -- --test-threads=8` in a loop.
3. Verify whether failures correlate with `drain` returning a non-terminal
   event list.
4. Audit production callers in `registry.rs` (`get()`, `is_recovering()`,
   `wait_for_settlement()`) to determine whether any production path assumes
   a post-stream `Running -> Valid` ordering.

## Fix direction

Do not paper over the flake with sleeps or retry-until-Valid loops. Once the
mechanism is confirmed, choose one of:

- Make the test wait for the helper to settle after draining (e.g. assert that
  `drain` produced a terminal event, then use `wait_for_settlement()` before the
  status assertion), if the race is a test-only timeout issue.
- Make the status transition a structural happens-before of stream completion
  if production callers also need the guarantee.

Treat the outcome as a timer/ordering smell in the credential-helper state
machine, not just a test fix.

## Acceptance

- The assertion is deterministic: no sleeps, no retry-until-Valid loops.
- Root cause documented (is the production read path also racy?).
- Run the module under stress to confirm: `cargo test -p phoenix-llm credential_helper -- --test-threads=8` repeated in a loop stays green.

## Resolution

Confirmed mechanism: `spawn_helper_task` sets the inner state to `Valid` (or `Failed`) before sending `Complete`/`Error` to subscribers and before notifying `settled`. Therefore, if a test observes `Running` after `drain` returns, `drain` must have exited without seeing a terminal event — its `timeout(TICK, s.next())` loop returns early when the subprocess is slow to spawn under parallel-test load.

Production callers audited in `registry.rs`:
- `LlmAuth::resolve()` calls `get()` then `is_recovering()`; it never assumes a post-stream status.
- `CredentialHelper::get()` is explicitly fire-and-forget and returns `None` while `Running`.
- `is_recovering()` only polls the current state.

No production path requires a structural happens-before guarantee beyond what `wait_for_settlement()` already provides, so the fix is test-only.

Fix: make the test helper `drain` take `&CredentialHelper` and call `wait_for_settlement()` internally after collecting events. This composes the production primitives (`run_and_stream` for event delivery + `wait_for_settlement` for the state transition) inside the helper, so any test that calls `drain` and then asserts `credential_status()` / `get()` is guaranteed to see a settled state — structurally, not by discipline. No new methods are added to the production `CredentialHelper` type; the settlement logic lives entirely in the test-local `drain` helper.

Tests that need to observe the intermediate `Running` state (`second_subscriber_joins_running_helper_and_replays_buffer`) already use `run_and_stream()` directly and poll the stream manually — the production streaming API — without `drain`. This is the explicitly unsettled path; `drain` is the settled default.

Stress validation: `cargo test -p phoenix-llm credential_helper -- --test-threads=8` passed 10/10 consecutive runs. `cargo fmt --all -- --check` and `cargo clippy -p phoenix-llm --tests -- -D warnings` are clean.
