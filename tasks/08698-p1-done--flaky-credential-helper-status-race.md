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

`drain(run_and_stream())` returning does not guarantee the internal
`CredentialStatus` transition `Running -> Valid` is visible to a subsequent
`credential_status().await`. There is a window between the helper stream
completing and the status being committed. The same race likely affects the
sibling tests in this module that follow the `drain(...); assert_eq!(status, ...)`
pattern (e.g. `ttl_zero_expires_valid_to_idle`, `invalidate_clears_valid_*`).

## Fix direction

Make the status transition observable before the stream drain completes (so a
post-drain read is a happens-after), rather than papering over it with a sleep
or retry loop. Treat this as a timer/ordering smell in the credential-helper
state machine, not just a test fix: if the test can observe `Running` after a
full drain, a real caller can too. Verify whether `get()`/`credential_status()`
callers in production rely on the same post-drain ordering.

## Acceptance

- The assertion is deterministic: no sleeps, no retry-until-Valid loops.
- Root cause documented (is the production read path also racy?).
- Run the module under stress to confirm: `cargo test -p phoenix-llm credential_helper -- --test-threads=8` repeated in a loop stays green.
