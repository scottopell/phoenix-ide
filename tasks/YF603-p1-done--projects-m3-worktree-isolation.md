---
created: 2026-03-05
priority: p1
status: done
artifact: pending
---

# Projects M3: Worktree Isolation

## Summary

Replace the shared-checkout branch model (M2) with physical worktree isolation.
Each Work conversation gets its own directory via `git worktree add`, enabling
parallel Work conversations on the same project.

## Context

Read first:
- `specs/projects/requirements.md` -- REQ-PROJ-005, REQ-PROJ-007, REQ-PROJ-008, REQ-PROJ-015
- `specs/subagents/requirements.md` -- REQ-SA-007, REQ-SA-008 (tier/mode for sub-agents)
- Task 0602 (M2) -- establishes branch creation on approve, shared-checkout Work mode

## Dependencies

- Task 0602 (M2: task approval + branch creation)

## What Changes from M2

M2 creates a branch and checks it out in the main checkout. This works but has
two constraints:
1. Only one Work conversation per project (shared checkout)
2. No write boundary enforcement (agent can write anywhere)

M3 replaces the `git checkout` step with `git worktree add`, giving each Work
conversation a physically separate directory.

## What to Do

### Backend

1. **Worktree creation on approve:** Change the M2 approval flow from
   `git checkout task-{NNNN}-{slug}` to
   `git worktree add .phoenix/worktrees/{conversation-id}/ task-{NNNN}-{slug}`.
   The branch was already created in M2's flow -- worktree add attaches to it.
   Ensure `.phoenix/worktrees/` is in `.gitignore`.

2. **CWD change:** When entering Work mode, set the conversation's working
   directory to the worktree path. All tools (bash, patch, etc.) operate relative
   to the worktree, not the main checkout.

3. **Lift one-Work-conv constraint:** Remove the M2 guard that errors on a second
   task approval. Multiple Work conversations can now coexist because each has its
   own directory.

4. **Worktree registry (REQ-PROJ-015):** Track active worktrees in the project
   record: task_id, worktree path, branch name, conversation_id, timestamp.
   Update on create/delete. On startup, reconcile registry against disk -- clean
   orphaned entries, report unregistered worktrees.

5. **Write boundary enforcement (REQ-PROJ-007):** Work mode tools that attempt
   to write outside the worktree path get blocked with a descriptive error.

6. **Sub-agent worktree inheritance (REQ-PROJ-008):** Work sub-agents share the
   parent's worktree as CWD (one Work sub-agent at a time). Explore sub-agents
   from a Work parent get the worktree as read-only CWD.

### Frontend

7. **Worktree indicator:** Work conversations show the worktree path and branch
   name in the conversation header / state bar.

## Acceptance Criteria

- [ ] Task approval creates a git worktree at `.phoenix/worktrees/{conv-id}/`
- [ ] Work mode conversation operates in the worktree directory (CWD changed)
- [ ] Multiple Work conversations get separate worktrees (parallel work)
- [ ] Worktree registry tracks all active worktrees
- [ ] Startup reconciles registry against disk
- [ ] Work sub-agents operate in parent's worktree
- [ ] Writes outside worktree are blocked with descriptive error
- [ ] `.phoenix/worktrees/` in `.gitignore`
- [ ] `./dev.py check` passes

## Value Delivered

Physical isolation. Main checkout stays clean. Multiple tasks proceed in parallel
without conflicting. Each worktree is disposable. Write boundary enforcement
prevents accidental cross-contamination.

## Scope boundary (M3 vs M4)

M3 creates and manages worktrees. It does NOT handle merging worktree branches
back to main or cleaning up after task completion -- that's M4.
