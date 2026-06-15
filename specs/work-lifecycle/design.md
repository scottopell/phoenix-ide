# Work Lifecycle — Technical Design

## Architecture Overview

The work lifecycle is implemented at the executor/handler layer, not in the pure state
machine. The state machine has no knowledge of git, worktrees, or branch names — those are
handler concerns. A handler runs the git cleanup, then resolves the conversation by sending
bedrock's `TaskResolved` event, which transitions the conversation's `parent_status` to
`terminal`.

The two terminal actions are user-initiated HTTP calls:

- `POST /api/conversations/:id/abandon-task`
- `POST /api/conversations/:id/mark-merged`

The PR-state gate is a read-only query:

- `GET /api/conversations/:id/pr-status`

The scope boundary — what this spec owns versus what bedrock, `pr-association`, and
`work-actions-bar` own — is stated canonically in `requirements.md` and is not re-duplicated
here.

## Legality Gate (Bedrock's Responsibility)

Both terminal actions share one legality gate, enforced by bedrock's `TaskResolved` rule
(REQ-BED-029, REQ-BED-031). Each handler validates against it and rejects with HTTP 409 on
failure:

- `core_status ∈ {idle, error}` — the agent is not running. `idle` is the ordinary disposable
  state; `error` is a stuck, non-running state (for example a usage-limit window the user
  merged around externally). An errored conversation executes nothing, so destructive cleanup
  is safe — the user must not be forced to coax out a successful LLM turn just to dispose of
  work that is already done or externally merged.
- `parent_status ∈ {absent, context_exhausted}` — normal idle, or a paused context-exhausted
  parent. Excludes `awaiting_recovery` / `awaiting_task_approval` / `awaiting_user_response`
  (these need user attention first) and `terminal` (already resolved).
- `mode ∈ {work, branch}` — Direct and Explore conversations have no worktree and are not
  subjects of these actions.
- `continued_in_conv_id` absent — a context-exhausted parent that has already been continued
  cannot be acted on from the parent; the continuation is the live conversation and any
  terminal decision belongs there (`TerminalActionRequiresNoContinuation`).

## Mode-Dependent Branch Disposition

The mode recorded on the conversation at cleanup time determines branch fate, identically for
both actions:

| Mode | Worktree | Branch |
|------|----------|--------|
| `work` (Managed) | Deleted | Deleted (`git branch -D {branch_name}`) |
| `branch` | Deleted | Kept |

The structural reason: Managed-mode branch names (`task-{ID}-{slug}`) are Phoenix artifacts
created by the Explore→Work approval flow; `branch`-mode conversations check out a pre-existing
user branch belonging to the user's PR, not to Phoenix.

## Mark as Merged — Git Sequence

1. Validate the legality gate. Reject with HTTP 409 on failure.
2. Check for a live worktree co-owner (see [Worktree Ownership Guard](#worktree-ownership-guard)).
   Skip worktree/branch removal if one exists.
3. `git worktree remove {worktree_path} --force` (the worktree may have uncommitted files).
   On failure: remove the directory directly + `git worktree prune`.
4. If Managed (Work) mode: `git branch -D {branch_name}` — non-fatal; log at debug on failure
   and continue. The important resource is the worktree directory, not the branch ref.
5. Send `TaskResolved` with outcome `merged`; emit the system message.

"Mark as merged" requires no separate confirmation dialog when the PR-state gate is present:
the "Clean up merged PR" label is confirmation-sufficient for the merged case. The
`work-actions-bar` spec owns the exact confirmation affordances.

## Abandon — Git Sequence

1. Validate the legality gate (same gate as Mark as Merged). Reject with HTTP 409 on failure.
2. After the user confirms, capture the diff snapshot (best-effort), then check for a live
   worktree co-owner. Skip worktree/branch removal if one exists.
3. `git worktree remove {worktree_path} --force`. On failure: remove the directory directly +
   `git worktree prune`.
4. If Managed (Work) mode: `git branch -D {branch_name}` — non-fatal.
5. Send `TaskResolved` with outcome `abandoned`; emit the system message.

The diff snapshot is captured before step 3 specifically because the worktree (and its git
objects) will not exist afterward. All uncommitted work in the worktree is permanently lost
after worktree removal; the snapshot is the recovery artifact.

## Diff Snapshot Capture

The abandon snapshot is captured by `git_ops::capture_branch_diff`, which produces a
`CapturedDiff`. The same capture function and structure back both the abandon snapshot and the
conversation diff endpoint, so they cannot diverge. The capture is taken relative to
`effective_base_ref` — `origin/{base_branch}` when that ref resolves, otherwise the bare base
branch name.

`CapturedDiff` records the work as committed and uncommitted state relative to the comparator:

- the commit log of commits on the branch not yet in the comparator (subject lines only),
- the committed diff (`base...HEAD`),
- the uncommitted diff (`HEAD` against the working tree, including untracked files surfaced
  through a temporary index so the real index is never mutated).

Each diff section is captured through a bounded streaming read: the in-memory buffer stops
growing past a per-section byte cap, while a counter continues up to a hard limit so the
truncation indicator can report an accurate total. Past the hard limit the count becomes a
lower bound. Each section therefore carries a total-bytes count and a saturation flag, and the
persisted snapshot marks truncation rather than silently dropping content. Capture is
best-effort: a failure here is logged at `debug` and does not block worktree removal.

Keeping the branch instead of snapshotting was rejected: it would require a new kind of
"orphaned branch" management outside the worktree model. The bounded snapshot is reviewable
from the conversation record without git infrastructure; for very large diffs the user can
`git stash` before abandoning if they want full preservation.

## PR State as Cleanup Gate

`GET /api/conversations/:id/pr-status` queries `gh` for the PR associated with the branch
checked out in the conversation's worktree, off the request thread. The response is normalized
to a `PrStatusResult`:

```
PrStatusResult {
    found: Boolean
    display_state: open | draft | merged | closed       -- when found
    check_state:   passing | pending | failing | unknown -- when found
    number: Integer?                                     -- when found
    unavailable_reason: String?                          -- when not found
}
```

The `work-actions-bar` spec owns how this maps to UI affordances (labels, disable states,
explanatory text). This spec owns the contract:

- `display_state = merged` → the "Clean up merged PR" happy path is appropriate.
- `display_state ∈ {open, draft, closed}` (or `check_state ∈ {pending, failing}`) → a
  discouraging note is appropriate; the user may opt into a manual fallback.
- `found = false` → no note; manual fallback available without friction.

`gh` failures (unavailable, unauthenticated, non-git directory, command failure) yield
`found: false` with an `unavailable_reason`, are logged at `debug`, and surface as compact,
non-blocking UI hints. The conversation page remains usable without `gh`.

PR state is **advisory only**: it labels and guides the cleanup affordance but does not create
a lifecycle transition. Cleanup occurs when and only when the user initiates a terminal
action. There is no polling loop, no background auto-merge detection, and no automatic
Terminal transition based on observed PR state. Phoenix observes no push event and cannot poll
every branch continuously; more fundamentally, the user is always better positioned to assert
"the PR is merged" than Phoenix is to detect it.

## Worktree Ownership Guard

Before removing a worktree or deleting a branch, both terminal actions apply the
any-live-owner guard: if a live conversation other than the one being acted on resolves to the
same work scope — a continuation that inherited the worktree (REQ-BED-030), or a Work-mode
sub-agent sharing the parent's `worktree_path` (REQ-PROJ-008) — the worktree and branch are
**not** deleted. This is the same preservation signal that gates the bedrock terminal-cascade
(REQ-BED-032), where `cascade_projects_on_delete` skips worktree/branch cleanup when a live
co-owner remains.

"Live" means non-terminal AND not archived, derived from persisted conversation rows — not
from runtime handle presence. A handle-less but non-terminal, non-archived conversation still
owns its worktree.

## Relationship to Bedrock

This spec does not re-specify bedrock's state machine. The interaction boundary is:

1. The work-lifecycle handler validates legality using bedrock's `TaskResolved` preconditions.
2. The handler runs git cleanup (this spec's domain) and, for abandon, persists the diff
   snapshot.
3. The handler sends `TaskResolved(conversation, outcome)` → bedrock transitions
   `parent_status` to `terminal` and emits `ConversationBecameTerminal`.
4. The synthetic system message describing the outcome is a separate effect emitted alongside
   resolution, not an argument to `TaskResolved`.

The transition to `terminal` is bedrock's; this spec owns steps 1 and 2.
