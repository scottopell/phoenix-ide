---
created: 2026-04-08
priority: p2
status: ready
artifact: ui/src/components/ConversationSettings.tsx
---

# Branch selection for Managed mode conversations

## Problem

When starting a Managed conversation, the worktree is always created from
whatever branch is currently checked out. The backend supports `base_branch`
in `ConvMode::Work` and uses it as the merge target for Complete, but there's
no UI to choose a different base branch at conversation creation time.

Use case: user wants to work against a feature branch, not main. Currently
they must manually `git checkout feature-branch` before starting the
conversation.

## What to build

### Frontend

In ConversationSettings, when Managed mode is selected for a git repo:
- Show a branch selector dropdown (populated from `git branch --list`)
- Default to the currently checked out branch
- Pass the selected branch to the backend

### Backend

- New field on CreateConversationRequest: `base_branch: Option<String>`
- When Managed mode + base_branch specified, checkout that branch before
  task approval creates the worktree
- Or: pass base_branch through to the task approval flow so the worktree
  is created from the right starting point

### API

- New endpoint or extend existing: `GET /api/git/branches?cwd=...` to
  list available branches for a directory
- Returns branch names + which one is currently checked out

## Testing needed

The Managed workflow (Explore -> propose_task -> approve -> Work -> Complete/Abandon)
has not been thoroughly tested with non-default base branches. Specifically:

- [ ] Worktree created from a non-main branch
- [ ] Complete squash-merges back to the correct base branch (not main)
- [ ] Abandon restores the correct base branch
- [ ] Commits-behind/ahead indicators work relative to the chosen base
- [ ] Branch name in StateBar reflects the correct base

## Done when

- [ ] Branch selector in Managed mode conversation creation
- [ ] Worktree created from selected branch
- [ ] Complete/Abandon target the correct base branch
- [ ] Non-default base branch workflow tested end-to-end
