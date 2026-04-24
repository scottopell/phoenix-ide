---
created: 2026-04-24
priority: p1
status: ready
artifact: ui/src/pages/ConversationPage.tsx
---

Continuing a Work-mode (or Branch-mode) conversation via "Continue in
new conversation" landed the new agent in a Direct conversation on
`main` in the main repo checkout, instead of a fresh worktree on the
parent's task branch.

## Observed (2026-04-23/24)

Previous conversation:
- Work mode, branch `task-24694-distill-sse-wire-allium-spec`
- Worktree at `.phoenix/worktrees/8f82c521-…`
- Had uncommitted changes implementing the task 02679 fix (~7 call
  sites across db.rs, runtime/traits.rs, runtime/executor.rs,
  runtime/testing.rs, api/lifecycle_handlers.rs)

Clicked "Continue in new conversation". New conversation:
- Direct mode
- `cwd` = main repo root
- Parent's worktree was deleted as part of cleanup
- **Uncommitted changes in the parent worktree were destroyed** (not
  stashed; task 08678 is the fix for that but hasn't shipped)

Net effect: two bugs compounded — lost work + wrong-mode continuation.
The fix is now being re-implemented from a written summary. Painful.

## Suspect code

`ui/src/pages/ConversationPage.tsx:789-791`:

```tsx
const branchForContinuation =
  conversation.branch_name ?? conversation.base_branch ?? null;
const mode: 'branch' | 'direct' = branchForContinuation ? 'branch' : 'direct';
```

For a Work-mode parent, `conversation.branch_name` should be the
managed task branch (e.g. `task-24694-…`) and this ought to produce
`mode='branch'`. If it instead produced `mode='direct'`, either:

1. `branch_name` and `base_branch` were both null/empty on the parent
   object at the moment of the click (state shape bug, or the
   conversation row for a Work-mode conv doesn't expose branch_name
   to the FE the way this code expects).
2. Server rejected the branch-mode create (e.g. ConflictError or git
   error) and fell through without navigating to the right place —
   though the catch block at 817-820 does navigate on conflict.
3. A different "Continue" entry point was used (not the
   context-exhausted banner button at `ConversationPage.tsx:775`) —
   trace which one the user actually clicked.

Also: the server accepts modes `auto | branch | managed | direct`
(see `src/api/handlers.rs:510-621`). `work` is not an option on the
create endpoint. The FE maps Work-mode continuation to `branch`,
which is probably fine, but the mapping needs an explicit audit —
does a Work-mode parent's managed branch round-trip cleanly through
branch-mode create in a fresh worktree?

## Related / prior art

- Task 08678 (ready): auto-stash on ContextExhausted worktree cleanup.
  Would have preserved the uncommitted fix across the worktree
  deletion. Independently worth shipping; does not obviate this task.
  The two bugs are orthogonal — fix both.
- Task 24692 (done): worktree cleanup on context-exhausted. Introduced
  the destructive cleanup path.

## What to investigate

1. Instrument the Continue button(s) to log `conversation.mode`,
   `branch_name`, `base_branch` at click time. Find out which field
   was null/undefined in the observed case.
2. Audit every "Continue in new conversation" entry point in the UI
   (context-exhausted banner, conversation list, possibly others).
   Are they consistent? Do any pass `mode='direct'` unconditionally?
3. For a Work-mode parent with a managed task branch, verify the
   server's `GET /api/conversations/<id>` response includes
   `branch_name` in the shape the FE reads.

## Acceptance

- Continuing any conversation that had a worktree (Work or Branch
  mode) lands in a fresh worktree on the same branch, regardless of
  entry point.
- Regression test: mock a Work-mode parent, click Continue, assert
  the POST body has `mode: 'branch'` and the correct branch.

## References

- `ui/src/pages/ConversationPage.tsx:775-830` — context-exhausted
  Continue button; mode-selection logic at 789-791.
- `src/api/handlers.rs:510-621` — server-side mode resolution.
- Task 08678 — auto-stash on context-exhausted (preserves uncommitted
  work across cleanup).
- Task 02679 — the underlying fix that was lost to this bug.
