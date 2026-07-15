# Make the repository mutation lock test deterministic

## Root cause

`runtime::creation_worker::repository_lock_tests::repository_mutation_lock_serializes_live_holders` is a test flake under machine load, not evidence of a production repository-lock failure.

The production guard resolves the repository common directory, opens one shared lock file, and blocks in `fs2::FileExt::lock_exclusive`. The failing test starts a worker thread and signals `started` before that worker runs `git rev-parse`, opens the lock file, or enters the blocking lock call. It then assumes the worker will finish all of that within a 100 ms observation window and, after releasing the first guard, will acquire and report within 2 seconds. During a full `./dev.py prod deploy`, CPU and process scheduling pressure can exhaust the latter deadline; the reported failure was the post-release `recv_timeout(Duration::from_secs(2))` at `creation_worker.rs:1254`.

The test also has a weaker correctness problem: if the worker does not reach `lock_exclusive` during the initial 100 ms, its first assertion passes vacuously and the first guard may be dropped before any actual contention occurs.

## Narrow change

- Factor the existing lock-file resolution/opening portion of `RepositoryMutationLock::acquire` into a private helper without changing its path, error mapping, blocking behavior, or call sites.
- Rewrite only `repository_mutation_lock_serializes_live_holders` to avoid threads and elapsed-time assertions:
  1. acquire the first production `RepositoryMutationLock` guard;
  2. open a second descriptor through the factored production helper;
  3. assert `try_lock_exclusive` reports contention while the first guard is live;
  4. drop the first guard;
  5. assert the second descriptor can acquire the exclusive lock, then release it.
- Keep production locking semantics and deploy/check concurrency unchanged. Do not broaden this into the separate systematic timer-test effort.

## Verification

- Run the focused test repeatedly.
- Run the surrounding `runtime::creation_worker::repository_lock_tests` target.
- Run the repository-standard Rust checks through `./dev.py check` (scoped by normal gating where applicable).
