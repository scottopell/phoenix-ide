# Simplify New Conversation around user intent

## Problem

The `/new` view exposes Phoenix implementation concepts directly: Direct, Managed, Branch, Base branch. That makes starting a conversation feel like configuring Phoenix internals instead of expressing what the user wants to do.

## Desired UX

Replace the current three-mode form with two top-level choices:

1. **Direct**
   - Simple/familiar path.
   - Work in the selected directory/checkout directly.
   - Maps to the existing `direct` conversation mode.

2. **From existing work** (label still open to copy iteration)
   - Safer Explore-first path for git repositories.
   - Starts from a Git starting point.
   - Defaults to the remote default branch.
   - Allows override through an omni-picker.
   - Maps to the existing managed Explore → `propose_task` → approve → Work lifecycle.

## V1 scope

### Branch starting points

- Remove the visible `Managed` / `Branch` split from the new-conversation form.
- For git repos, show the second choice as an intent-oriented option instead of Phoenix mode terminology.
- Default the starting point to the remote default branch when available.
- Allow override via a fuzzy picker that includes remote branches and local branches.
- Continue submitting branch starts through the existing managed path (`mode: "managed"`, `base_branch: <selected branch>`), so the conversation begins in Explore mode on the selected starting point.

### Task-file starting points

Task selection is part of v1, not a later iteration.

- The first meaningful omni-picker should include both branch entries and task-file entries.
- The user should be able to start work from a selected task file directly from `/new`, similarly to how continuation can create a successor work conversation.
- Selecting a task should create an Explore conversation.
- The initial prompt should instruct the agent to call `propose_task` with the selected task file path.
- The normal task approval flow remains authoritative: the user reviews/comments/approves, and approval creates the Work conversation/branch through existing mechanics.
- If the task already has an active conversation, the picker should surface that and route the user to the existing conversation rather than creating a duplicate.

## Implementation plan

1. Prototype the interaction visually before touching production UI:
   - create quick standalone HTML/CSS sketches for the `/new` flow
   - iterate with the user on naming, layout, picker behavior, and task/branch presentation
   - only proceed to implementation once the preferred sketch is clear
2. Refactor `ConversationSettings` / `useCreateConversation` to model user intent separately from backend mode:
   - `direct`
   - `fromExistingWork` with a selected starting point (`branch` or `task`).
3. Keep backend mode mapping explicit at submit time:
   - Direct intent → existing `mode: "direct"`.
   - Branch starting point → existing `mode: "managed"` plus `base_branch`.
   - Task starting point → existing `mode: "managed"` plus default/base branch, with an initial prompt that asks the agent to call `propose_task` for the chosen file.
4. Add/extend an API for listing task files for a cwd/project before a conversation exists. Existing `GET /api/conversations/:id/tasks` is conversation-scoped and cannot power `/new` directly.
5. Build the v1 omni-picker data model:
   - branch entries from `GET /api/git/branches?cwd=...&search=...`
   - task entries from the project tasks directory
   - clear labels, type badges, and conflict/active-conversation affordances
6. Update validation/gating:
   - non-git directories only allow Direct
   - `fromExistingWork` requires a resolved branch or task starting point
   - conflict rows disable send and link to the existing conversation
7. Update copy and visual hierarchy so the page communicates user intent, not Phoenix internals.
8. Add focused tests for:
   - Direct submit payload
   - branch starting point defaults to remote default branch
   - branch override submit payload
   - task starting point creates an Explore conversation with a `propose_task` instruction
   - active task/branch conflicts prevent duplicate starts

## Notes

This is primarily a UI/API-shape simplification. It should preserve existing backend lifecycle semantics rather than introducing a new backend conversation mode unless the implementation reveals a structural gap.
