---
created: 2026-05-05
priority: p2
status: ready
artifact: src/runtime/executor.rs
---

# Conversation cwd: mutable or immutable?

## Framing

A conversation's working directory has historically been mutable: a Managed
conversation could be created at the repo root (Explore, no worktree), and on
task approval the cwd would update to the freshly-created worktree path. This
mutability is the architectural reason the tmux pane diverges from the LLM's
view — tmux locks pane cwd at session-spawn time, while bash tool reads
cwd dynamically per call.

After `feat/managed-requires-base-branch` (commit `79efb788`), Managed mode
always allocates an Explore worktree at conversation creation. The
"non-worktree checkout path" gap is closed for Managed. **Question: are there
any remaining gaps where a conversation's `cwd` needs to mutate after
creation?** If not, we should be able to assert immutability — which would:

- Eliminate the tmux pane / LLM cwd divergence by construction (the pane's
  fixed cwd equals the conv's permanent cwd, no transition to track)
- Simplify the mental model ("a conversation lives in one directory for its
  whole life")
- Remove a class of bugs around tools that pin state at first observation
  (tmux today, plausibly others later)

## Current mutation sites

Inventory of every place `conv.cwd` (DB column) or `context.working_dir`
(in-memory) is written after conversation creation. Each must be either
eliminated or documented as a load-bearing exception before immutability
can be asserted.

### Site 1 — Explore→Work approve_task transition

**File:** `src/runtime/executor.rs:2117` (DB) + `:2126` (in-memory)
**Trigger:** `approve_task` on a Managed-Explore conversation.
**What it does:** writes the Work worktree path, replacing the Explore
worktree path that was set at conversation creation.

**Investigation:** the Explore worktree (created by
`create_managed_explore_worktree_blocking`, `src/api/handlers.rs:856`) is
on a temp branch in `.phoenix/worktrees/<conv_id>/`. The Work worktree
created by `approve_task` is on the real task branch. Can the Explore
worktree be **promoted** in place — i.e., rename the temp branch to the
task branch, keep the same worktree path — so cwd never changes? If yes,
this site eliminates cleanly.

If branch rename is unsafe (committed work on the temp branch, etc.),
document why and document the cwd update as load-bearing.

### Site 2 — `execute_resolve_task` terminal transition

**File:** `src/runtime/executor.rs:2008`
**Trigger:** task completion / mark-merged / abandon flows that resolve
a conversation to terminal state.
**What it does:** resets cwd to `repo_root` after the worktree is torn
down.

**Investigation:** terminal conversations are read-only — they don't run
tools, the agent loop is dead, the worktree is gone. **Why does cwd need
to be valid at that point at all?** If nothing reads cwd post-terminal,
this update is dead code and can be dropped. Verify: search every read of
`conv.cwd` in terminal-state conversations (UI rendering, history loading,
SSE streams). If a downstream consumer needs a cwd post-terminal, surface
the requirement — that's the signal that this update is justified.

### Site 3 — startup worktree recovery

**File:** `src/main.rs:392`
**Trigger:** Phoenix startup; for each conversation, if its worktree is
missing on disk (e.g. user manually deleted `.phoenix/worktrees/`), reset
cwd to the project root.

**Investigation:** this is recovery, not a normal-path mutation. Three
options:
- **a) Refuse to load.** Mark the conversation as `worktree_missing` in
  state; UI shows it as broken and offers a recreate / abandon. Preserves
  cwd immutability at the cost of recovery UX.
- **b) Recreate the worktree.** On startup, if a Work/Branch conversation's
  worktree is missing, reconstruct it from the branch + repo root.
  Preserves cwd and recovery behavior, but adds startup complexity.
- **c) Document as the one allowed mutation.** Recovery is rare; perhaps the
  pragmatic answer is "cwd is immutable except for this exact recovery
  path."

The decision here trades architectural purity against startup robustness.

## Proposed sequence

1. **Site 2 first** (smallest, likely dead code): grep every read of
   `conv.cwd` and `context.working_dir` in terminal-state code paths. If
   none, drop the update. If any, surface the consumer as a follow-up.
2. **Site 1 second** (the one that actually drives the tmux bug): prototype
   the in-place branch rename / worktree promotion. If clean, ship it; the
   tmux pane divergence disappears as a side effect. If unclean, ship the
   tmux `send-keys "cd <path>"` injection as the targeted fix and leave
   site 1 alone.
3. **Site 3 last** (the design call): pick a, b, or c based on what
   sites 1 and 2 produce. If 1 and 2 closed cleanly, c becomes harder to
   justify and a or b is the natural finish. If 1 stayed mutable, c falls
   out for free.
4. **Once all three are settled**, encode the contract: rename
   `update_conversation_cwd` to `update_conversation_cwd_recovery_only`
   (or remove entirely if site 3 closes), add a comment at the schema
   declaration of `cwd`, and consider a debug-assertion that cwd at fetch
   time matches cwd at conversation creation time for any in-memory
   `Conversation` traversed by the runtime.

## What this is NOT

- Not the tmux fix in isolation. If the answer to the framing question is
  "cwd must remain mutable," then a separate task should add `tmux send-keys
  cd` injection at the transition point. This task is about the
  architectural question; the tmux fix is the consolation prize.
- Not blocked on `feat/managed-requires-base-branch`. That branch closes one
  gap (Managed without base_branch). This task asks whether the remaining
  gaps can also be closed.

## Validation

After implementation:
- Every site enumerated above is either gone or documented with its
  load-bearing justification.
- A new test asserts that for a conversation created with cwd=X, every
  fetch of `conv.cwd` returns X across approve_task / mark-merged / abandon
  flows (subject to whatever exceptions site 3's resolution leaves).
- Manual: create a Managed conversation, approve a task, verify the tmux
  pane and LLM bash tool agree on cwd at every step.

## Out of scope

- Sub-agent conversations: their cwd may legitimately differ from parent
  (they often inherit but the spec hasn't been explicit). Treat as a
  separate question if it surfaces during investigation; do not bundle here.
- Direct mode (no worktree, no transitions): already trivially immutable;
  no work needed.
