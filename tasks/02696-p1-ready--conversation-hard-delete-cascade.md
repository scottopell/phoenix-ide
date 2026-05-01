---
created: 2026-05-01
priority: p1
status: ready
artifact: src/api/handlers.rs
---

Replace the one-line `delete_conversation` handler with the cascade orchestrator from REQ-BED-032: cancel-or-reject if busy, run cleanup functions in order, delete row, broadcast SSE event. Wires together the bash, tmux, and projects cleanup paths into a single user-facing hard-delete flow.

## In scope

- `src/api/handlers.rs` — replace the existing `delete_conversation` body with the orchestrator:
  - Step 1: cancel-or-reject — if conversation is busy (`is_busy` true), return 409 with `error: "cancel_first"` (the `RejectHardDeleteWhileBusy` rule). Choose the reject branch over the cancel-and-wait branch for v1 — simpler implementation, clearer UX.
  - Step 2: `cascade_bash_on_delete(&state.bash_handles, &conv_id).await` — kill live handles, drop in-memory tombstones. Failures log at WARN with `{conv_id, live_handle_pids, live_handle_pgids, kill_pending_kernel_pids}`.
  - Step 3: `cascade_tmux_on_delete(&state.tmux_registry, &conv_id).await` — `tmux -S <sock> kill-server`, unlink the socket file, drop the registry entry. Failures log at WARN with `{conv_id, socket_path, kill_server_status}`.
  - Step 4: `cascade_projects_on_delete(&state.projects, &conv_id).await` — for non-terminal conversations with worktrees, run the equivalent of `ConfirmAbandon`'s worktree-removal flow without the diff snapshot (hard-delete is not abandon). For Direct-mode and already-terminal conversations, no-op. Failures log at WARN with `{conv_id, worktree_path, branch_name}`.
  - Step 5: `state.runtime.db().delete_conversation(&conv_id).await?` — SQLite ON DELETE CASCADE removes messages, tool calls, and other dependent rows.
  - Step 6: `state.sse.broadcast_to_user(ConversationHardDeleted { conversation_id: conv_id })` — for sidebar / navigation refresh.
- `src/tools/bash/registry.rs` — add `cascade_bash_on_delete(&Arc<BashHandleRegistry>, &str)` function that snapshots live pgids, sends SIGKILL to each, removes the conversation entry, and returns structured errors on failure.
- `src/tools/tmux/registry.rs` — add `cascade_tmux_on_delete(&Arc<TmuxRegistry>, &str)` function. `kill-server` is best-effort (errors are non-fatal); `remove_file` is also best-effort.
- New cleanup function in projects (likely `src/projects/abandon.rs` or wherever `ConfirmAbandon` lives) — `cascade_projects_on_delete(&ProjectState, &str)` calling the worktree-removal flow.
- No event-bus, no subscriber registration, no dynamic dispatch — just direct function calls in the handler.

## Out of scope

- The wire-type definition for `ConversationHardDeleted` and the UI sidebar refresh handler (task 02697 / Migration plumbing). For now the SSE broadcast can carry an untyped JSON payload; task 02697 adds the typed variant.
- The cancel-and-wait branch (deferred). v1 is reject-only.

## Specs to read first

- `specs/bedrock/requirements.md` REQ-BED-032 (the canonical contract).
- `specs/bedrock/design.md` "Conversation Hard-Delete Cascade" section (the pseudocode this task implements).
- `specs/bedrock/bedrock.allium`: `UserHardDeletesConversationRule` and `RejectHardDeleteWhileBusy` rules.
- `specs/bash/requirements.md` REQ-BASH-006 cascade clauses; `specs/bash/bash.allium` `HandlesRemovedByConversationDelete`.
- `specs/tmux-integration/requirements.md` REQ-TMUX-007; `specs/tmux-integration/tmux-integration.allium` `ServerKilledByConversationDelete`.
- `specs/projects/projects.allium` `WorktreeRemovedByConversationDelete` (the new subscriber rule from the final-pass commit).

## Dependencies

- 02694 (Bash operations) — `cascade_bash_on_delete` lives in the bash module subtree and uses the `BashHandleRegistry` types.
- 02695 (Tmux + terminal) — `cascade_tmux_on_delete` lives in the tmux module subtree and uses the `TmuxRegistry`.
- The projects cleanup call reuses the existing `ConfirmAbandon` worktree-removal helpers; no new projects-side dependency on the new specs beyond reading `WorktreeRemovedByConversationDelete`.

## Done when

- `./dev.py check` passes.
- Integration test creates a conversation with: a live bash handle running a long sleep, a tmux session with a window running a server, and a worktree (Work mode). User triggers hard-delete. Verify:
  - Reject path: while the conversation is mid-tool, the delete returns 409 with `error: "cancel_first"`. After cancellation, the same delete succeeds.
  - Successful path: the bash process is gone, the tmux server is killed, the socket file is unlinked, the worktree is removed, the branch is deleted (Work mode), the conversation row is gone, and an SSE event was broadcast.
  - Partial-failure path: simulate a failed `tmux kill-server` (e.g., temp permission error on the socket dir); the cascade continues, WARN log captures the orphan info, the row is still deleted.
- Property test: for any sequence of hard-deletes on conversations with arbitrary state, the post-state never contains a Handle / TmuxServer / Worktree referencing the deleted conversation in the in-memory registries.
