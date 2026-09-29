# Make the bounded-Git descendant test observe execution, not PID existence

## Contextualized flake assessment

Basis: checkout `924fb0b01bd8a133ed4c8c4b72331647496ac34a`, named historical task/commit history, retained coordinator deployment logs, the hosted #808 Rust receipt, current source/tests, and live PR status checked on 2026-09-27. PR #808 remains open and frozen at `a22461cf2c9c7f1497c54c2db7042ccba9e5399b`; this task neither edits nor depends on its worktree. A later green run is stability evidence, not causal proof, and expected/caught panic text in a passing negative test is not counted as a failed test.

### Implementation defects and fixture races

| Class / observed symptom | Mechanism and confidence | Prior fix and coverage | What remains / priority | Sources |
|---|---|---|---|---|
| Browser resize test hung twice for >300s on first browser use | **High:** browser session initialization awaited CDP phases without bounds. | #183 (`b7a8a6810`) bounded launch, new-page, listener, navigate/eval/resize, and later shutdown phases. Current phase bounds survive in `BrowserSession`; resize regression remains. | Specific hang is covered. Whole-test protection is still coarse (Rust lane ceiling), and first browser download is intentionally unbounded. Do not inflate timeouts; instrument the active phase if this recurs. | [task 45001](45001-p1-done--fix-browser-resize-test-hang.md), [`session.rs`](../crates/phoenix-browser/src/session.rs), [`tools.rs`](../crates/phoenix-tools/src/browser/tools.rs) |
| Fresh tmux session returned success but immediate cwd query produced empty output | **High:** `new-session -d` acceptance preceded pane readiness under load. | #183 added `wait_for_spawned_pane`: success requires observable nonempty `list-panes`, with a 5s outer ceiling. | Current test-control/watchdog spawn path bypasses that normal-path helper, though end-to-end cwd coverage passes. Only add direct normal-path coverage if this symptom recurs. | [task 62006](62006-p1-done--fix-tmux-test-flake.md), [`wait_for_spawned_pane`](../crates/phoenix-tools/src/tmux/registry.rs) |
| Tmux watchdog left owned processes/roots after socket loss; later concurrent cleanup reported `query/late-pane-birth-unavailable` | **High for #781 socket-loss defect and #808 deterministic reproduction:** socket-path discovery lost authority; later, a listed short-lived pane exited before birth lookup and was treated as live-unverifiable. | #781 (`590b15e76`) introduced exact PID+birth+token/control-inode containment and fail-closed replacement protection. Frozen #808 proves late-pane absence independently via ESRCH in retirement and pre-sweep, adds bounded reason receipts, and synchronizes replacement spawn on tri-state identity. Before-fix causal regression failed; fixed form passed 20/20. Four post-fix complete tmux slices passed (808 nonignored tests); a fifth replacement test failed and was retained; final captured slices were 3 × 203 pass/1 ignored. | Do not duplicate #808. The two unchanged-`924fb0b` deployment reds (7.07s and 6.80s watchdog exit 1) predate reason receipts, so they are historical lookalikes, not proof of the same cause. Three earlier registry reds lack complete assertion receipts; `unlinked_live_server_is_not_accepted_as_absent` is explicitly not causally covered by #808. Reclassify only on a new receipt after #808. | [#781](https://github.com/scottopell/phoenix-ide/pull/781), [#808](https://github.com/scottopell/phoenix-ide/pull/808), retained coordinator logs under `.phoenix/evidence/prod-deploy-20260927-924fb0b/` |

### Brittle tests and false PASS coverage

| Observed symptom | Mechanism and confidence | Prior fix and coverage | What remains / priority | Sources |
|---|---|---|---|---|
| Broad E2E/UI/browser timing audit found fixed sleeps, microtask drains, teardown overlap, and negative readiness predicates | **High:** elapsed time was being used as readiness/completion authority. | #498/#501 replaced console, screencast, validation, PR-status, and keepalive waits with event or exact-turn witnesses; task 36016 added semantic Rust test timing lint. | The lint prevents recognized new/changed patterns but intentionally grandfathers legacy findings and cannot judge whether a bounded witness has the right identity. Open residuals include screencast lifecycle serialization and identity-bound mid-stream cancel. | [task 36011](36011-p1-done--audit-e2e-flake-risk.md), [task 36016](36016-p2-done--rust-test-aware-timing-lint.md), [`check_rust_test_timing.py`](../scripts/check_rust_test_timing.py) |
| `profile_trace_stop_long_task_real` passes when output says `Long tasks (>50ms): 0` | **Certain false PASS:** it checks only that the first count character is a digit; its 120ms `Runtime.evaluate` busy loop historically did not become a >50ms trace event. | A later `run_scenario` test asserts a page `PerformanceObserver` sees `>=1`, but it does not cover trace-stop extraction. | Still open at HEAD. After the recent red is fixed, this is a high-value coverage repair: drive a page-originated trace event, parse the count, and require `>=1`; do not merely tighten the current workload into a deterministic failure. | [task 02717](02717-p3-ready--profile-trace-long-task-weak-assertion.md), [`test_browser_profile_trace_stop_long_task_real`](../crates/phoenix-tools/src/browser/tests.rs) |
| Process inspector scrollback test intermittently exceeded Vitest 5s | **High:** seven large timer-driven renders created ~27k DOM operations to test a 5,000-entry cap. | #249 (`346e1c765`) changed it to one oversized update while preserving trim-boundary assertions. | Low residual: it still renders 5,000 rows and duplicates the cap literal. A pure accumulation test is optional only if cost recurs. | [task 58022](58022-p1-done--flaky-process-inspector-scrollback-timeo.md), [`ProcessInspectorPanel.test.tsx`](../ui/src/components/ProcessInspectorPanel.test.tsx) |
| Hosted #808 Rust job failed `run_git_bounded_kills_spawned_descendants_on_timeout` at `descendant survived timeout kill tree` | **High-confidence test-semantics defect; production defect unproven:** after group `SIGKILL`, `kill(pid, 0)` succeeds for a dead-but-unreaped zombie. The helper returned in 0.437s despite the descendant's 30s sleep and joins pipe readers; a genuinely executing inherited-pipe holder would normally delay return. Exact state/PGID was not captured, so a live survivor remains the falsifier. | Bounded Git/process-group implementation (`10acf20a5`, later integration #727) correctly isolates a group and signals it on timeout; no later fix changed the immediate PID-existence assertion. | **Highest-value narrow intervention now:** make the test distinguish executing descendants from terminated zombies, assert the exact command-timeout path, and retain state/PPID/PGID/elapsed diagnostics on failure. Do not weaken production signaling absent evidence of an executing survivor. | [hosted job](https://github.com/scottopell/phoenix-ide/actions/runs/36333172898/job/108659047831), [`git_ops.rs`](../crates/phoenix-ide/src/git_ops.rs) |

### Environmental and operational failures

| Observed symptom | Mechanism / confidence | Prior handling | Remaining action | Sources |
|---|---|---|---|---|
| Browser eval test hung once on iteration 20/20 | **Medium-high environmental classification:** eight Chromium tests plus another compile drove load averages `34.9 / 109 / 124` on a 10-core host; 19/20 passed. Navigate/eval/session phases are bounded. | Browser isolation/shutdown and `dev.py` CPU+memory test-thread caps reduce starvation. | Keep open as recurrence-triggered diagnostics; capture the active awaited phase on an idle-host recurrence. No blanket timeout increase. | [task 02718](02718-p2-ready--browser-eval-test-hang-under-load.md) |
| React profiling navigation timed out fetching unpkg | **High:** non-hermetic test depended on a live third-party CDN; network precheck did not prove unpkg availability. | #249 vendored pinned React/scheduler/ReactDOM fixtures. | Functional risk is low. Remove stale “unpkg” failure wording when nearby code is touched. | [task 58021](58021-p1-done--flaky-browser-test-unpkg-cdn-dependency.md), [`fixtures/`](../crates/phoenix-tools/src/browser/fixtures/) |
| #808 owner encountered ENOSPC; production attempt 2 passed all 20 checks and built/signed, then activation lost its uv Python | **Certain environmental/operational incidents:** disk exhaustion and Global cache pruning, not assertion races. | Bounded unused-artifact cleanup recovered ENOSPC. Preserved deploy log shows missing `~/.cache/uv/archive-v0/.../bin/python3` after a 575.2s all-green gate and 3m37s build. | No source/test attribution; never purge live uv/cache/build artifacts from this stream. Global retains deploy authority. | coordinator evidence `prod-deploy-20260927-924fb0b/deploy-attempt2.log` |
| SQLite `two_connections_record_overlapping_native_reads` failed once in a full gate, then passed isolated | **Unclassified:** exact assertion/output is still missing. Current test uses two connection-local native barrier participants and a per-database collector; no timing sleep or global collector was found. | Historical `33701b6c3` already replaced a 100ms overlap inference with the deterministic native barrier. | Do not invent a fix. On recurrence retain the exact assertion. If it hangs, add bounded rendezvous diagnostics; if peak or writer accounting differs, expose the corresponding test-only collector state. | [`sqlite_native_statement.rs`](../crates/phoenix-db/src/sqlite_native_statement.rs) |

### Newly introduced diagnostics defects

| Observed symptom | Mechanism / confidence | Present containment | Remaining action | Sources |
|---|---|---|---|---|
| Frozen #808 quarantine-hook timeout recovery can restore/report an unrelated replacement path | **High from review:** after one identity check, an adversarial hook can replace `root_quarantine`; unchecked `os.replace(root_quarantine, root)` then moves the wrong object and may leave the owned root undisclosed. | Test-only diagnostic/recovery path; normal exact retirement and replacement protection are unchanged. Risk was explicitly accepted for stabilization. | Follow-up only after the current Git test repair unless it produces real leaks: reauthenticate without following symlinks or restore through retained authority. Keep receipts bounded; do not build post-deadline reconstruction machinery. | [#808 final review](https://github.com/scottopell/phoenix-ide/pull/808) |

## Observed journey

- On hosted PR #808 head `b1f6e9db99cd14d4c649e5e7e902393e7fd107cc`, the Rust lane ran 1,219 of 2,230 tests before fail-fast: 1,218 passed and this one test failed in 0.437s; 1,011 were not run.
- `run_git_bounded_with_env` starts a Git alias in a dedicated process group. The alias starts `sleep 30`, records its PID, and waits. The helper times out at 200ms, group-kills, reaps the direct Git child, joins stdout/stderr readers, and returns a timeout error.
- The test immediately calls `kill(descendant_pid, 0)` and treats any existing PID—including a zombie awaiting the host reaper—as a live process leak.

## Verified findings

- `kill(pid, 0)` tests PID existence/permission, not whether the process can execute; zombies can make it return success.
- The hosted helper returned in 0.437s, far before the fixture descendant's 30s natural exit.
- The descendant inherits the helper's stdout/stderr pipes. An actually executing `sleep` would ordinarily keep those descriptors open and block the helper's reader joins until natural exit; a zombie holds no descriptors.
- Production process-group setup and kill code are unchanged by #808, and no retained receipt ties the tested PID to tmux ownership. This supports independence but does not prove it from diff alone.

## Inferences and unknowns

- **Inference, high confidence:** the hosted PID was a killed zombie, making the test assertion false. Falsifier: a diagnostic repetition observes state `R`/`S` (not `Z`), unexpected PGID, or continued fixture execution after the helper returns.
- **Unknown:** the historical job did not record descendant state, PPID, PGID, process birth, group-kill result, or exact helper error text.
- **Unknown/unrelated:** the SQLite failure remains unclassified until its exact assertion is recovered.

## Interaction map

`Git alias fixture` → `dedicated Unix process group` → `run_git_bounded_with_env timeout/group SIGKILL` → `direct-child reap + inherited-pipe closure` → `test observation of descendant termination` → `host orphan reaper eventually removes zombie PID`

The defective boundary is the final observation: immediate PID existence is not execution/liveness.

## Proposed scope

1. Before changing semantics, run the exact test in bounded serial and concurrent batches and, on any extant PID, capture sanitized test-only `pid`, state, PPID, PGID, elapsed time, and exact returned error. Record selected-test counts so zero-test invocations cannot be counted.
2. Replace `kill(pid, 0) == ESRCH` as the pass/fail authority with a narrow test-only termination observation that accepts non-executing zombie state but still fails for running/sleeping descendants. Prefer direct process-state observation available on supported Unix targets; keep any platform fallback explicit and bounded.
3. Tighten the regression to require the exact command-timeout error path, not a generic string that could also match pipe-drain timeout. Assert the helper returns well before the fixture's natural exit so inherited-pipe closure remains part of the property.
4. Preserve or improve failure diagnostics enough to distinguish real process-group escape/signal failure from delayed host reaping, without emitting environment values or creating a process-inspection framework.
5. If diagnostics show a genuinely executing descendant, stop treating this as test-only and make the smallest production correction: surface group-signal failure and verify group membership/termination. Do not suppress that evidence by accepting every extant PID.

### Validation

- Report actual denominators, failures, durations, and platform for a bounded baseline and fixed run; target 100 serial exact selections plus 4 workers × 50 exact selections under contention if runtime remains practical.
- Verify every invocation selects exactly one test; exclude zero-test commands.
- Confirm no executing fixture descendants remain after each batch; distinguish zombie from running/sleeping state.
- Run the owning `phoenix_ide` test target/lane, then one `./dev.py check --all` qualification. Later green runs are evidence of stability, not a claim that all descendant cleanup flakes are eliminated.
- Publish the follow-up branch/PR for review, but do not merge or deploy without authorization. Coordinate results through Phoenix owner `7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f`.

### Non-goals

- No edits to frozen PR #808 or its old worktree; no merge/deploy authority.
- No production tmux changes, quarantine diagnostic expansion, SQLite rewrite, global scheduler/build slots, retries-until-green, blanket timeout increases, or weakened assertions.
- No universal process supervisor/subreaper or wholesale test framework.
- No cache/uv/build/DB/source purge and no secret payload capture.
