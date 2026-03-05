---
created: 2026-03-05
number: 603
priority: p1
status: ready
slug: projects-m3-worktree-isolation
title: "Projects M3: Git worktree isolation for Work mode"
---

# Projects M3: Worktree Isolation

## Summary

On task approval, create a git worktree. Work mode tools operate in the worktree,
not the main checkout. Multiple Work conversations get separate worktrees.

## Context

Read first:
- `specs/projects/requirements.md` — REQ-PROJ-005, REQ-PROJ-007, REQ-PROJ-008, REQ-PROJ-015
- `specs/subagents/requirements.md` — REQ-SA-007, REQ-SA-008 (tier/mode for sub-agents)

## Dependencies

- Task 0602 (M2: task approval)

## What to Do

### Backend

1. **Worktree creation on approval:** When the user approves a task, create a git
   worktree at `.phoenix/worktrees/{conversation-id}/` with a new branch
   `phoenix/{task-id}--{slug}` from current main HEAD. Ensure `.phoenix/worktrees/`
   is in `.gitignore`.

2. **Worktree registry (REQ-PROJ-015):** Add to project record: a list of active
   worktrees with task_id, path, branch, conversation_id, timestamp. Update on
   create/delete. Reconcile on startup.

3. **Work mode tool scoping:** When in Work mode, set the conversation's working
   directory to the worktree path. All tools (bash, patch, etc.) operate relative
   to the worktree, not the main checkout.

4. **Sub-agent worktree inheritance (REQ-PROJ-008):** Work sub-agents share the
   parent's worktree (one at a time). Explore sub-agents from a Work parent get
   the worktree as their read-only cwd.

5. **Write boundary enforcement:** Work mode tools that attempt to write outside
   the worktree path get blocked with a descriptive error.

### Frontend

6. **Worktree indicator:** Work conversations show the worktree path and branch
   name in the UI (e.g., in the state bar).

## Acceptance Criteria

- [ ] Task approval creates a git worktree at `.phoenix/worktrees/{conv-id}/`
- [ ] A new branch `phoenix/{task-id}--{slug}` is created from main HEAD
- [ ] Work mode conversation operates in the worktree directory
- [ ] Multiple Work conversations get separate worktrees
- [ ] Worktree registry tracks all active worktrees
- [ ] Startup reconciles registry against disk
- [ ] Work sub-agents operate in parent's worktree
- [ ] Writes outside worktree are blocked
- [ ] `./dev.py check` passes

## Value Delivered

Physical isolation. Main branch stays clean. Multiple tasks can proceed in parallel
without conflicting. Each worktree is disposable.
