A ContextExhausted parent conversation deliberately preserves its worktree for the Continue flow, but the scope-liveness check treats it as not-live, so deleting/archiving a sibling force-removes that worktree (uncommitted work lost).

`is_terminal()` includes ContextExhausted/HandedOff (phoenix-core sm_state.rs ~1559). `scope_has_live_conversation_inner` (runtime.rs ~1349) skips terminal rows when deciding scope ownership. A Work-mode sub-agent inherits the parent worktree_path, is not a chain member, not busy, and can be hard-deleted/archived via API. Its cascade (handlers.rs cascade_projects_on_delete ~3400) finds no live sibling -> concludes the scope is unowned -> `git worktree remove --force` + `git branch -D` on the parent worktree/branch. A later Continue then points at a worktree that no longer exists. HandedOff is protected only because its continuation is live; ContextExhausted-without-continuation is the hole.

Fix: in scope_has_live_conversation_inner, count ContextExhausted (and HandedOff with no live continuation) rows as live owners — these states mean "worktree preserved pending user action."

Related git-data-loss to fold in or split: mark_merged (lifecycle_handlers.rs ~585) force-destroys branch+worktree with no merge verification and no diff snapshot (abandon captures one); archive cascade (handlers.rs ~2957) likewise destroys branch+worktree with no snapshot despite reversible-sounding semantics.

Found in spiritual-core audit 2026-06-10.
