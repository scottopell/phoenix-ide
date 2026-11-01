Coverage mapping for the replacement of
`api::handlers::hard_delete_cascade_tests::archive_chain_captures_first_pass_close_inventory`
with a direct production-capture regression test, done on branch
close-property-test-boundary.

Old test (removed):
- `api::handlers::hard_delete_cascade_tests::archive_chain_captures_first_pass_close_inventory`
  Built a real Git worktree + repo on disk via
  `build_workmode_chain_with_shared_worktree`, ran the full
  `archive_chain_handler` over a 3-member continuation chain, and asserted
  end-to-end convergence (worktree gone, branch kept, rows archived). It
  exercised `RuntimeManager::capture_close_retirement_inventory` only
  indirectly, as one step inside the full handler call, and paid for a real
  repo + tempdir to do it.

New test (added):
- `runtime::close_retirement::tests::prepare_retirement_captures_and_seals_one_exact_shared_worktree`
  Drives the real production `RuntimeManager::capture_close_retirement_inventory`
  directly against a committed `RetirementRequested` Close attempt built
  entirely from production DB commands (`begin_close_foundation`,
  `confirm_close_stop_work`, `begin_close_active_work_settlement`,
  `advance_close_settlement_when_quiescent`, `replace_close_inspection`).
  The sealed worktree identity names an inert sentinel path
  (`/inert/sentinel/worktree-never-touched`) that is never created. The test
  asserts only committed DB state (captured scope/worktree identity, sealed
  targets, idempotent re-capture); it runs no Git, filesystem, or process scan
  and makes no filesystem-absence assertion about the sentinel path.
  New fixture `build_close_attempt_with_sentinel_worktree` adapts the
  continuation-membership wiring from `build_workmode_chain_with_shared_worktree`
  (3 members sharing one WorkScope), dropping the real git/tempdir setup.
  Asserts: inventory incomplete before capture, complete after; exactly one
  captured scope/worktree target across the 3 continuation members; sealed
  snapshot and worktree identity match exactly; repeat capture against the
  same snapshot is idempotent (no duplicate sealed resources); zero
  retirement evidence rows (capture never retires); scope spawn admission is
  fenced; the aggregate does not reach History; and the obligation phase is
  unchanged by capture alone.

Tests kept unchanged (same coverage, not touched by this change):
- `api::handlers::hard_delete_cascade_tests::archive_chain_handler_completes_one_aggregate_close`
  (daemon-free full chain-archive lifecycle through the handler)
- `api::handlers::hard_delete_cascade_tests::delete_chain_cleans_shared_worktree_and_branch_once`
  and `cascade_skips_worktree_when_continuation_exists` (still use
  `build_workmode_chain_with_shared_worktree`, the real-worktree fixture,
  for hard-delete and cascade-skip coverage respectively -- unrelated to
  Close inventory capture, left in place)
- `phoenix_db::close_foundation::tests::initial_cleanup_failure_terminalizes_both_certainties_and_exact_replay`
  and `initial_cleanup_failure_remaining_inventory_spans_scopes_and_excludes_success`
  (DB-level capture/replay/terminalization invariants)

The replacement calls the production `prepare_close_retirement_targets` boundary,
which performs the first-pass incomplete-inventory guard, captures through the
normal runtime method, and then lists the sealed targets. The real retirement
orchestrator calls the same boundary before resource effects. This slice does
not claim to test live Git deletion or host watcher shutdown; those are not
inventory properties. Later work must cover evidence-to-effect sequencing at
the production boundary rather than restore the broad integration fixture.

The removed test also exercised handler-to-worktree cleanup and branch
preservation. This inventory property deliberately does not claim that live
qualification; it asserts the exact targets returned to production retirement
instead. Separate adapter safety tests remain.
