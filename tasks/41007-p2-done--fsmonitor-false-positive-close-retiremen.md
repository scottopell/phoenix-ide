Close retirement false-positived "open descriptors can still modify the confirmed worktree" on hosts with core.fsmonitor enabled. A Git fsmonitor daemon bound to the worktree (or to an initialized submodule) still held descriptors when the post-quarantine scan ran; on macOS the daemon exits after its root is renamed, but not before the scan.

Fix: Close retirement stops every fsmonitor daemon bound to the worktree and its initialized submodules immediately before quarantine. Phoenix's Git configuration is unchanged, so user fsmonitor/index state is never touched (an earlier global core.fsmonitor=false attempt stripped the index FSMN extension on writes and was rejected in review). Decision: ADR-080. Sequence: work-lifecycle.allium @guidance on AmbiguousPrivateDirectoryIdentityPreservesSafeLeftover.

PR #820.
