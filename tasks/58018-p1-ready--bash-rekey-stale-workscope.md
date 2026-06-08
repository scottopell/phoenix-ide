Bash handle rekey on Explore->Work approval leaves a stale WorkScope on each Handle.

When an Explore conversation is approved, `runtime/executor.rs` rekeys pre-approval bash
handles from `WorkScope::Conversation(id)` to the new worktree scope, but the move only
changes the registry map key — each `Handle` still stores its old `work_scope` field.
When a migrated process later exits, the waiter emits its terminal lifecycle event using
`handle.work_scope`, so the work-scope bridge broadcasts the OLD conversation scope (which
no live runtime resolves to) instead of the new worktree scope. The work-scope
observability panel then keeps showing the handle as running until a manual/poll refresh.

## Fix
Update each `Handle`s stored `work_scope` when rekeying, the way the tmux/browser rekey
paths already do. See `crates/phoenix-tools/src/bash/registry.rs` (~line 343) and the
rekey call in `crates/phoenix-ide/src/runtime/executor.rs`.

## Context
Surfaced by codex review on PR #232 (conversation-retrieval). Relevant to the chains
work-scope panel (specs/chains/ REQ-CHN-008, Phase 3): a dead bash process shown as live
is exactly the stale state that panel must avoid.
