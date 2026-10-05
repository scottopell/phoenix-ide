# ADR-080: Close stops bound fsmonitor daemons instead of reconfiguring Git

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** REQ-WL-002b, Close retirement's exact-worktree quarantine and descriptor scan

## Context

With `core.fsmonitor` enabled (often globally, by an IDE or package-manager Git install), Git commands start a detached `fsmonitor--daemon` per repository. The daemon outlives the command and holds open directory descriptors inside its worktree. Each initialized submodule is its own repository and gets its own daemon.

Close retirement moves a confirmed worktree into a private quarantine directory, then scans for any process with a descriptor inside it, and treats a hit as a conflicting external writer. On macOS a daemon exits by itself once its root is renamed, but not instantly. So a scan that runs right after quarantine races that exit. On hosts with fsmonitor enabled, Close then reports a false conflict and leaves the worktree in repair. This happens whether the daemon was started by Phoenix's own Git commands, by the user's Git usage, or by a submodule's commands.

Any fix has to avoid the obvious workaround of changing the user's Git behavior. Running Git with `core.fsmonitor=false` makes index-writing commands such as `git add` drop the index's `FSMN` extension even with `GIT_OPTIONAL_LOCKS=0`. That silently degrades fsmonitor for a user who enabled it on purpose.

## Options considered

1. **Disable fsmonitor in Phoenix's shared Git constructor.** This stops Phoenix from starting daemons. But every Phoenix-issued index write strips the user's `FSMN` extension, which mutates user state in exactly the large repositories fsmonitor exists for. It also does nothing about daemons the user's own Git usage started.
2. **Disable fsmonitor in the shared constructor and suppress optional index locks.** This protects read-only commands, but mandatory-lock writes still strip `FSMN`, so user state still changes.
3. **Teach the descriptor scan to ignore Git's fsmonitor daemon.** No configuration changes. But it adds a process-identity allow-list to a safety check, and a daemon is not proof that no other process holds the same tree.
4. **Stop every fsmonitor daemon bound to the worktree and its initialized submodules immediately before quarantine.** This removes the one benign descriptor holder at its source and leaves configuration and index state untouched. The scan stays as strict as before.

## Decision

Choose option 4. The false positive comes from a specific benign process being alive at scan time, not from how Phoenix configures Git. Stopping that process before quarantine fixes the cause for every origin of the daemon. Phoenix's other Git commands keep behaving exactly like the user's own Git. The exact step and its ordering are normative in `specs/work-lifecycle/work-lifecycle.allium`.

## Consequences

- **Positive:** Close converges on hosts with fsmonitor enabled, including worktrees with initialized submodules.
- **Positive:** Phoenix never changes the user's fsmonitor configuration or index state.
- **Positive:** The descriptor scan keeps its full strictness; no process allow-list.
- **Negative:** Retirement makes two more best-effort Git invocations per attempt.
- **Negative:** The user's fsmonitor daemon for a retired worktree is stopped. That worktree is being deleted, so this has no lasting effect.
- **Neutral:** A daemon the user restarts between the stop and the scan is still reported as an external writer, which is correct: something other than Phoenix touched the tree.

## References

- ADR-042: Close directory retirement trusts its private namespace
- `specs/work-lifecycle/requirements.md`
- `specs/work-lifecycle/work-lifecycle.allium`
- Key symbols: `close_retirement::stop_bound_fsmonitor_daemons_best_effort`, `close_retirement::quarantine_and_remove_exact_worktree`, `close_retirement::quarantine_has_open_descriptors`
