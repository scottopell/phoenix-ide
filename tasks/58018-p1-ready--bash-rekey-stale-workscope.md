Explore->Work approval rekeys resources to the new worktree scope, but three
rekey paths leave stale work-scope state behind. Each is independently a source
of the "panel shows dead/old state as live" bug that the chains work-scope panel
(specs/chains/ REQ-CHN-008, Phase 3) must avoid.

## Facet 1 — Bash handles keep their stale `work_scope` field (P2)

When an Explore conversation is approved, `runtime/executor.rs` rekeys
pre-approval bash handles from `WorkScope::Conversation(id)` to the new worktree
scope, but the move only changes the registry map key — each `Handle` still
stores its old `work_scope` field. When a migrated process later exits, the
waiter (`crates/phoenix-tools/src/bash/operations.rs`) emits its terminal
lifecycle event using `handle.work_scope`, so the work-scope bridge broadcasts
the OLD conversation scope (which no live runtime resolves to) instead of the
new worktree scope. The observability panel then keeps showing the handle as
running until a manual/poll refresh.

Fix: update each `Handle`'s stored `work_scope` when rekeying, the way the
tmux/browser rekey paths already do. See
`crates/phoenix-tools/src/bash/registry.rs` (~line 343) and the rekey call in
`crates/phoenix-ide/src/runtime/executor.rs`.

## Facet 2 — `work_scope_key` not propagated on the mode-change update (P2)

Full conversation snapshots now carry `work_scope_key`
(`crates/phoenix-ide/src/runtime.rs` ~line 692), but the Explore->Work approval
path emits a `ConversationUpdate` carrying the new `worktree_path` WITHOUT the
derived `work_scope_key`, and the frontend reducer only shallow-merges the
supplied fields. So the left work-scope panel keeps querying the old
`conversation:<id>` inventory while the runtime has rekeyed resources to
`worktree:<path>`. The fresh-scope SSE snapshot can then be overwritten by the
panel's old-scope poll until a full conversation snapshot/list poll corrects it.

Fix: include the new `work_scope_key` in the mode-change metadata
`ConversationUpdate` (or recompute it when `worktree_path` changes) so the panel
switches to the new scope immediately. See the update emission near
`crates/phoenix-ide/src/runtime.rs:684`.

## Facet 3 — Original browser profile dir not cleaned up after rekey (P3)

When a browser session opened before approval is moved from the conversation
scope to the worktree scope, `BrowserSessionManager::rekey_scope`
(`crates/phoenix-tools/src/browser/session.rs` ~line 1056) updates only the map
key and `ScopedSession::scope`. The underlying `BrowserSession` was launched with
the old scope key, so its Chrome user-data directory lives under
`user_data_dir_for_key(old_key)`. A later `kill_session(new_scope)` removes
`user_data_dir_for_key(new_key)`, leaving the original profile directory orphaned
on disk after archive/delete/idle cleanup for any pre-approval browser use.

Fix: either migrate the on-disk profile directory to the new key during
`rekey_scope`, or record the launch-time key on the session so cleanup removes
the correct directory.

## Context
All three surfaced by codex review on PR #232 (conversation-retrieval), as
out-of-scope observations against the #230 work-scope-rekey code (already merged
to main). They share one root cause: Explore->Work rekey updates the keying layer
but not every place the old scope is still referenced.
