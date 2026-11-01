# Terminal integration tests: PTY I/O paths and lifecycle teardown

## Approved narrow scope: registered detached-child reaping

Implement and qualify REQ-TERM-009 for registered terminal children without
changing disconnect policy or adding idle expiry:

- Install one child-exit observer on successful registry publication, independent
  of WebSocket attachment; publish a read-only retained in-memory exit receipt.
- Reap detached exits without reconnect or Close. Acquire attach authority before
  closing the master and removing only the exact registry instance.
- Preserve live idle detached children and any explicit Close retirement fence.
- For attached natural exit, do **not** send `StopReason::TearDown`; preserve the
  relay's existing EOF/output-drain path. Explicit Close awaits the same receipt
  instead of independently reaping and retains its owning fence.

Candidate source is present in `phoenix-terminal::child_lifecycle`,
`ActiveTerminals::install_child_observer`, and
`ActiveTerminals::complete_retirement_by_observing`. Qualification is pending;
source inspection is not a test pass or production acceptance.

## Existing infrastructure and candidate coverage

The original claim that no real PTY/process test infrastructure exists is stale.
Reuse these tests and fixtures rather than introducing a new smoke-test binary:

- `crates/phoenix-terminal/src/proptests.rs` has real-PTY explicit retirement
  coverage in `complete_retirement_exits_and_reaps_long_lived_detached_direct_pty`
  and candidate detached-natural-exit coverage in
  `detached_direct_shell_exit_is_reaped_without_reconnect_or_close`.
- Candidate registry tests cover both publication paths, live-idle preservation,
  attached authority retention, failed-observer receipts, exact-instance cleanup,
  and Close fences: `detached_exit_cleans_both_publication_paths_without_retiring_live_idle_children`,
  `attached_exit_preserves_relay_until_existing_authority_is_released`,
  `exited_child_rejects_reconnect_and_natural_cleanup_preserves_close_fence_and_receipt`,
  `close_and_monitor_share_one_observer_and_repeated_completion_uses_receipt`,
  `failed_observer_is_not_exit_proof_or_a_relay_teardown_request`, and
  `old_natural_cleanup_never_removes_same_identity_replacement_or_new_close_fence`.
- `crates/phoenix-ide/src/api/terminal_ws.rs` has `reclaim_tests::direct_pty_fixture`,
  `actual_direct_handoff_terminal_events_preserve_pty_for_successor_reclaim`, and
  `direct_work_pty_without_attachment_is_still_retired_explicitly`.
- `crates/phoenix-terminal/src/relay.rs` tests already exercise PTY output/tracker
  forwarding, WebSocket input, resize, detach, teardown, and EOF behavior using
  the relay test seam. This is distinct from end-to-end real WebSocket coverage.

These tests exist in source; the candidate has not been built or run in this
documentation pass. Qualify the exact integrated candidate, including attached
final-output draining, before reporting success. No builds or runtimes are
part of this pass.

## Separately tracked dispositions — outside the approved implementation

1. **Disconnect policy contradiction.** REQ-TERM-008 and Allium
   `UserClosedTerminal` require master closure on every WebSocket disconnect;
   actual `terminal_ws::detach_only` preserves the registered master/child and
   `terminal_ws::reclaim` reattaches. Obtain a separate policy decision and
   reconcile requirements, Allium, legacy `specs/terminal/design.md`, and tests
   together. Do not silently rewrite policy or claim REQ-TERM-008 complete as
   part of the detached-exit fix.
2. **Descriptor inheritance.** Separate task 44019 owns atomic close-on-exec
   PTY allocation and relay duplication under REQ-TERM-024, with cross-exec
   regressions. Independent child observation alone does not fix inherited
   descriptors; this remains outside the detached-exit slice.
3. **Cancelled-spawn reservation/publication.** `terminal_ws::acquire_handle`
   calls `ActiveTerminals::reserve_spawn`, awaits planning and
   `tokio::task::spawn_blocking` around `spawn_pty`, then publishes through
   `ActiveTerminals::insert_reserved`. Cancellation can bypass
   `ActiveTerminals::release_spawn`; the blocking spawn can continue
   after its awaiting future is dropped, before a handle is published and its
   observer installed. The rejected-publication branch handles an explicit
   rejection, not cancellation of the awaiting owner. Track cancellation-safe
   reservation and child ownership/publication separately; this patch only
   covers successfully registered children.

## Remaining broader integration-test inventory

Re-audit rather than repeat the original blanket "untested" claims: full real
PTY/WebSocket forwarding and EIO behavior; executor-to-terminal lifecycle event
routing (`TerminalAbandonedWithConversation`); and `read_terminal`
`wait_for_quiescence` notification/timeout behavior. The relay and real-PTY
fixtures above are available, but do not by themselves establish all these
end-to-end paths. These broader items are not an expansion of the approved
registered detached-child scope.
