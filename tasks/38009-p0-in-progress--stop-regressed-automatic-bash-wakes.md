# Stop regressed automatic bash wakes and retire stranded contracts

## Observed journey

In production conversation `explore-conversation-work-tool-access-8` (`bc396732-9534-4fda-b32d-c8b0a0f1ee48`), the wake status bar showed a bash wake expiring in roughly 24 hours even though the associated WorkScope command had terminated. The wake was created without an explicit wake command: an ordinary `bash op="run"` used `wait_seconds: 0`, returned a background handle, and automatically registered a durable wake.

The same behavior reproduced during investigation when another `bash op="run", wait_seconds: 0` created workflow 173.

## Verified findings

- Production workflow 164 binds contract `bash:call_5oa8fBDvjlVo3NODddC9edAI:b-c5e49fb3-3416-4729-837b-1c3bfd43b975` to the conversation and remains `Active` with no terminal receipt.
- Persisted message 7794 shows the producer was `bash op="run"`, command `uv run tests/e2e/run.py`, `wait_seconds: 0`; message 7795 contains `wake_registration.workflow_id = 164`.
- Persisted message 7804 shows a subsequent ordinary `bash wait` observed that exact handle as tombstoned after successful exit (`exit_code: 0`). The contract nevertheless remained active.
- `WakeStatusBar` polls `GET /api/conversations/:id/wake`; the endpoint queries active workflows without terminal receipts. The displayed wake therefore reflects durable backend state, not an invented UI-only timer.
- `RuntimeRegistryInspector` can convert a tombstoned bash handle into terminal wake evidence, but `AppState::new` starts the wake worker only when `AGENT_FACING_WAKE_REGISTRATION.0` is true. That constant is false.
- `RuntimeManager::new_with_message_retriever` nevertheless always constructs and exposes `ProductionWakeRegistrar`. `bash::race_run_response` calls `background_run_response(..., register_wake = true)` whenever the initial foreground wait expires, including immediately for `wait_seconds: 0`. This creates durable contracts while their only consumer is disabled.
- Commit `27af8f1d1` / PR #555 explicitly removed automatic background-handle wakes and added `background_run_does_not_register_or_acknowledge_wake`. Commit `104e02518` / PR #557 later reintroduced the old registration implementation and inverted that regression test while replacing derived WorkScope identities.
- Normative `REQ-BASH-014` states that background-command handles remain available for manual `peek`, `wait`, and `kill` **without registering a durable WorkScope wake**. Ordinary `bash wait` itself does not register a wake; the bug is the preceding background transition in `bash run`.
- `AppState::new` already calls `WakeRepository::retire_all_registrations` before runtime startup, so a deployment/restart after the code fix should terminalize contracts stranded by the regression.

## Failure model and owning invariant

The wake feature gate controls only the consumer worker, not the producer capability injected into tools. Separately, the bash background response contains an implicit wake producer that normative behavior had removed. A WorkScope refactor resurrected that producer during conflict resolution. Consequently, a normal background bash command persists a 24-hour wake contract while no worker is running to observe process termination.

Owning invariant: backgrounding and synchronous polling are handle lifecycle operations only. They must never create a durable wake. Wake registration must occur solely through the explicit agent-facing wake operation when that operation and its worker are enabled together.

## Proposed scope

1. Remove the implicit durable-wake registration path from `crates/phoenix-tools/src/bash/operations.rs`:
   - make `race_run_response` return the ordinary still-running response on timeout;
   - remove `register_wake` branching, bash wake expiry/fingerprint construction, and `wake_registration` response enrichment from `background_run_response`;
   - retain manual handle control and existing cancellation behavior.
2. Restore a regression test equivalent to `background_run_does_not_register_or_acknowledge_wake`, with a registrar present in `ToolContext`, proving that `wait_seconds: 0` neither calls it nor emits `wake_registration` in provider/display output.
3. Add or strengthen an integration-level assertion at the runtime/tool-registry seam so future WorkScope/runtime refactors cannot re-enable implicit registration merely because a registrar is injected.
4. Verify `retire_all_registrations` covers the stranded active workflows on startup and preserves idempotence. Add focused coverage only if the existing repository tests do not prove active unresolved automatic registrations become terminal/non-visible.
5. Update the wake-contract executive status if its current implementation map claims automatic bash registration remains removed; do not change timeless requirements, which already specify the correct behavior.

## Acceptance evidence

- A backgrounded `bash run` with `wait_seconds: 0` returns `still_running` and a handle, with no wake registration in output and no `WakeRegistrar::register` call.
- `bash peek`, `wait`, and `kill` continue to work for that handle.
- An explicitly registered wake remains outside this change and is not conflated with ordinary bash lifecycle operations.
- The wake-status API reports no contract created by ordinary background bash.
- Existing stranded contracts are retired by startup during deployment and disappear from `WakeStatusBar` after a successful refresh.
- Focused Rust tests plus applicable `./dev.py check` lanes pass.

## Risks and non-goals

- Do not enable the agent-facing wake feature or start the wake worker as a workaround; that would preserve behavior forbidden by `REQ-BASH-014`.
- Do not make ordinary `bash wait` cancel or resolve durable wakes; it should remain a synchronous handle observation operation.
- Do not redesign explicit wake contracts, delivery, expiry, or UI presentation.
- Do not manually mutate production wake rows. Existing startup retirement is the durable cleanup path; verify it and let deployment invoke it.
