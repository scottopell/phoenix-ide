# Work Lifecycle — Executive Summary

## What This Spec Covers

The work lifecycle defines the user-facing **terminal actions** on Work and Branch
conversations and the **git side effects** they produce. A Work or Branch conversation
enters the lifecycle when the user declares the work shipped (mark as merged) or discards it
(abandon). This spec owns:

- **Action semantics** — what each terminal action means, what git state it leaves behind,
  and the disposition of the worktree and branch per conversation mode.
- **Git side effects** — worktree removal, branch deletion (Managed/Work mode) or
  preservation (Branch mode), the diff-snapshot captured on abandon, and the confirmation
  that gates the irreversible abandon deletion.
- **PR-state-as-cleanup-gate** — `gh`-observed PR merge state labels and guides which
  cleanup affordance is presented, without ever creating an automatic lifecycle transition.

It does **not** own:

- **Transition legality** — when a terminal action is permitted based on conversation state
  (`core_status`, `parent_status`, `mode`, continuation pointer). That is bedrock's
  `TaskResolved` rule and its `TerminalActionRequiresNoContinuation` invariant (REQ-BED-029,
  REQ-BED-031). This spec's handlers validate against that same gate but do not define it.
- **PR feedback freshness, explicit active-PR targeting, auto-fix, or remediation context** —
  the `pr-association` spec.
- **UI surface composition** — button labels, action zones, disposition derivation, tooltips.
  The `work-actions-bar` spec owns these.

## User Need

A developer using PhoenixIDE needs a clear, safe way to close out Work and Branch
conversations. *Clear* means the user always knows what cleanup will happen before they
commit to it. *Safe* means no irreversible git operation runs without confirmation, a
worktree still in use by another live conversation is never destroyed, and the branch
disposition matches the mode: Phoenix-created task branches are removed, user-owned PR
branches are kept.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WL-001 | Abandon a conversation — confirmation, diff snapshot, mode-dependent branch disposition |
| REQ-WL-002 | Mark as merged — worktree cleanup, mode-dependent branch disposition, no squash, no push |
| REQ-WL-003 | PR merge state is the cleanup gate — advisory only, never a lifecycle trigger |

Increment 1 clarifies that plural PR association does not create plural cleanup ownership.
Cleanup remains one task/worktree action even when multiple associated PRs exist. Mixed PR states
may be summarized by sibling UI surfaces, and an explicit active PR may be named for clarity, but
cleanup does not merge, close, or retarget PRs and does not treat feedback freshness as a cleanup
gate.

Both terminal actions (REQ-WL-001, REQ-WL-002) are organized by **action**, with conversation
mode appearing as a row in each action's disposition table rather than as a separate
requirement. The disposition is identical in shape across the two actions: the worktree is
always deleted; the branch is deleted for Managed (Work) mode and kept for Branch mode.

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-WL-001 | Implemented | `POST /api/conversations/:id/abandon-task`; `git_ops::CapturedDiff` |
| REQ-WL-002 | Implemented | `POST /api/conversations/:id/mark-merged` |
| REQ-WL-003 | Implemented | `GET /api/conversations/:id/pr-status` |

## Provenance

Work-lifecycle requirements were previously expressed as project-scope requirements in the
`projects` spec, carved out here so that action semantics and git side effects own their own
spec independent of project detection, mode selection, and worktree creation. The standing
mapping from the prior project-scope IDs is:

| Prior ID | New ID | Mapping |
|----------|--------|---------|
| REQ-PROJ-010 | REQ-WL-001 | Abandon — confirmation and diff snapshot, both modes |
| REQ-PROJ-026 (abandon rows) | REQ-WL-001 (Branch row) | Branch-mode abandon: worktree deleted, branch kept |
| REQ-PROJ-027 (abandon rows) | REQ-WL-001 (Work row) | Managed-mode abandon: worktree and task branch deleted |
| REQ-PROJ-026 (merge rows) | REQ-WL-002 (Branch row) | Branch-mode mark-merged: worktree deleted, branch kept |
| REQ-PROJ-027 (merge rows) | REQ-WL-002 (Work row) | Managed-mode mark-merged: worktree and task branch deleted; no squash, no push |
| REQ-PROJ-011 | REQ-WL-003 | PR merge state as the cleanup gate; freshness/auto-fix excluded (see `pr-association`) |

REQ-PROJ-009 (Complete via squash merge) is deprecated and superseded by REQ-PROJ-027: the
push-branch model under which the user merges through their own PR workflow. Work-lifecycle
therefore carries the no-squash-merge prohibition (folded into REQ-WL-002) but defines no
"Complete (squash merge)" action.

REQ-PROJ-030 through REQ-PROJ-032 (PR feedback freshness, agent-facing context baseline,
bounded refresh) are deliberately not carried here — they belong to the `pr-association` spec.
