# Unified Close acceptance milestone

## Current checkpoint

- Branch: `feat/unified-close-receipts`.
- Published HEAD: `28dac533252755af1ee1fe936f6a3c32cc98db22`.
- Automatic hosted run: [45078608171](https://github.com/scottopell/phoenix-ide/actions/runs/45078608171), exact published HEAD above: **completed / failure**, personally classified from `/tmp/close-28dac5332/final-run.json` and `final-failed.log`. Single watcher exited with status 1; no replacement watcher for that run. Runtime matcher/actor module: 43 passed / 2 failed (live Bash group missing ordinary registry membership; stale-source fixture repolled an already-consumed oneshot). Exact formatting also failed. All later focused steps passed. Pending repair includes the reviewed membership handback, invalid second oneshot poll removal, and hosted formatting proposal.
- Pending continuation/Bash handback freeze: `/tmp/close-continuation-and-bash-handback-v3.patch`, SHA-256 `42582e9241f1b5bc389d113ca6004bb6b4a603d020f7b7be8c16a1c3dbd6b418`. Independent static review found no blocker; no execution qualification.

## Ownership and actual work

Integrator owns shared `native_initial/worktree.rs`, executor/post-retirement wiring, workflow, review and publication gates.

One parallel batch of two write-capable `gpt-6.1-sol` workers returned; neither is still writing:

1. Physical adapter: new `native_initial/worktree/physical.rs` and shared `close_retirement.rs`. Returned **residual-only scaffold**, not a physical deleter. Shared ignored-content status leaf was extracted. Not wired or accepted.
2. Public fixture: new `native_close_admission_tests/worktree_physical.rs`. Initial partial registered public-tool journey used disposable real linked Worktrees. A bounded recovery worker completed the manual API audit and corrected the source. Not wired, compiled, executed or accepted.

Do not mistake these partial sources for a passed acceptance journey. Existing published Worktree adapter remains read-only/status-only, with physical resource retirement blocked.

## Evidence boundary

- Existing process-only/public fixtures and unknown-release tests are not proof of real Worktree removal.
- Pending Bash fix restores the exact original handle membership only after confirmed nonentry and explicit successful compensation; scope remains fenced. Unknown release retains custody. Static review only.
- Pending Worktree continuation preserves the original owner, receipt, source worker and absolute deadline across status operations. New real Git admission tests are authored but unexecuted.
- **No disposable registered-public-Close journey has yet proven physical Worktree removal, independent WorkScope retirement and correct public terminal state.**

## Concrete blocker and next acceptance

The legacy exact-removal graph contains nested Git/fsmonitor children, quarantine/private renames, descriptor unlink operations and administrative deletion. Each needs admission through the same original DB/live-source/deadline context, with classified writer release before another leaf. Wrapping the whole graph in one callback would retain stale authority and deadlock on nested metadata writes. Unknown release/metadata/child completion must retain custody, not become a retryable ordinary failure.

Next: finish safe leaf plumbing and public fixture API audit; install Worktree-only source control and retained post-retirement operation; record exact original Worktree success only after proven checkout plus admin retirement, then WorkScope success and finalization. The current exact-head terminal run is classified; next publication is limited to reviewed repairs. Physical milestone source remains separate and unaccepted.

## Restrictions

Hosted-only execution: no local Cargo/build/check/test/fmt/rustfmt/dev.py, SQL/process fixtures or native fixture execution. No manual CI rerun/cancel/dispatch, duplicate watcher, real-user destructive fixture, merge/rebase/release/deployment. Retain `retry_close_safely` until full acceptance and release/install gate. No new production authority.
