# Qualify assembled ordinary native-retry actor + DB + tmux custody

## Scope and evidence
Independent source review of49a721951bac77ca2ec8706f61a85fb5376e5982 found no new demonstrated safety defect. Retained tmux worker reacquires exact DB/source entry at the actual sink, original deadline, backend fatal latch and retained uncertain source lease. Current DB, registry, backend short-write and lease-custody tests are lower-layer evidence, not assembled actor coverage. This is explicit qualification debt, not three newly invented merge gates or permission to ignore existing failures. Named public Worktree reclaim/History/replay and all selected exact-source tests remain mandatory.

## Acceptance
- Add one bounded ordinary native-retry actor fixture with real DB/registry and injectable backend. Explicitly label any supported legacy/original anchored residual seed; do not claim public native-initial end-to-end coverage from that seed. Do not fabricate native receipts/authority with SQL.
- Hold backend readiness before irreversible entry, ACK actual actor Stop while held, release/join, prove no kill/sink effect and original server survives.
- Causally exercise death/absence versus replacement identity; never kill replacement or confuse registry Gone fencing with process absence proof.
- With a legitimate narrow local unknown-release injection, perform real sink then drop observer; require backend fatal latch and retained lease, no evidence/finalization or next effect. No production ownership framework just to test.
- Add exact hosted inventory/execution checks; no sleep-based synchronization or timeout/drop of mutating DB futures.

## Starting points
RuntimeManager runtime.rs prepare_clean_close_with_tmux; phoenix-db native_close/retry/execution_tests.rs native_tmux_retry_fixture and initial_cleanup_failure_fixture_with_resources; public retry actor invocation and canonical persist acknowledgement in captured_loss_public_retry.rs. Existing helpers are private in their own test modules; assembling supported APIs is nontrivial new fixture work. FakeBackend Stall returns immediately, not a controllable hold. Existing TmuxBackend trait injection can support a test-only wrapper.

## Non-goals
No automatic successor, captured-loss tmux support, physical authority expansion, in-Close file cleanup, universal model transcript-round-trip gate, or claim of general actor DB-contention Stop responsiveness.
