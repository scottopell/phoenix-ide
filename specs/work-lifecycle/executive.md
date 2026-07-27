# Work Lifecycle — Executive Summary

## What This Spec Covers

The work lifecycle defines the user-facing **Close conversation** flow for Git-backed
conversations and the **retirement inspection / resource retirement** contract it drives.
This spec owns:

- **Close semantics** — one user-facing lifecycle action that moves the product conversation
  to History without treating branches, tasks, or pull requests as lifecycle owners.
- **Retirement inspection** — exact worktree-loss classification, inspection generation, and
  discard confirmation for destructive teardown.
- **Resource retirement** — idempotent teardown of the attached Git-backed `WorkScope`
  resources and worktree, without creating automatic recovery artifacts or mutating refs / PRs.
- **Advisory PR guidance** — observed PR state can guide whether Close looks timely, but never
  triggers lifecycle change on its own.

It does **not** own:

- **Conversation-state legality, close-obligation sequencing, or ProductConversation lifecycle** —
  bedrock owns those authorities.
- **PR feedback freshness, explicit active-PR targeting, auto-fix, or remediation context** —
  the `pr-association` spec.
- **UI placement, wording variants, or action-surface composition** — sibling UI specs such as
  `work-actions-bar` own those surfaces.

## User Need

A developer using PhoenixIDE needs one clear, safe way to retire a Git-backed conversation.
*Clear* means Phoenix explains whether closing will discard workspace-only state and what exact
loss categories were detected. *Safe* means Close never silently mutates branches or pull requests,
never treats repository facts as lifecycle authority, and can retry teardown without losing track
of partially retired resources.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-WL-001 | Close conversation is the only user-facing terminal lifecycle action for Git-backed conversations |
| REQ-WL-002 | Retirement inspection classifies exact worktree-loss risk before destructive teardown |
| REQ-WL-002a | Discard confirmation binds to one exact inspected workspace generation |
| REQ-WL-002b | Retirement retires owned resources stepwise, idempotently, and without automatic recovery artifacts |
| REQ-WL-003 | Pull-request state guides Close but never triggers it |

ADR-025 is the governing rationale: product lifecycle belongs to the ProductConversation,
`continued_in_conv_id` remains transcript topology, and the attached `WorkScope` owns disposable
resources without becoming the lifecycle root.

## Normative Authority

Load-bearing behavior previously described in the legacy `design.md` now lives in:

- `requirements.md` for the timeless Close / inspection / retirement / PR-guidance contract;
- `specs/bedrock/bedrock.allium` for ProductConversation lifecycle and close-obligation sequencing;
- `specs/adrs/025_workscope-owned-lifecycle-unifies-conversation-handoffs.md` for the ownership split
  between lifecycle, transcript topology, and WorkScope resources.

The legacy `design.md` has been removed.

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-WL-001 | Specified | Bedrock close-obligation flow owns lifecycle sequencing; user-facing Close migration still in progress |
| REQ-WL-002 | Specified | Retirement inspection contract is specified; implementation coverage is tracked separately |
| REQ-WL-002a | Specified | Inspection-generation/fingerprint binding is specified |
| REQ-WL-002b | Specified | Resource-retirement and no-automatic-recovery-artifact contract is specified |
| REQ-WL-003 | Specified | PR guidance contract is specified; no automatic close authority |

## Provenance

This spec supersedes the older abandon / mark-merged split. Legacy clients or persisted intents may
still arrive with deprecated `abandon` / `mark_merged` values, but `requirements.md` now defines
those only as compatibility inputs to the single Close flow rather than as current user-facing
lifecycle verbs.

Completed historical task references remain useful context, but current authority is the REQ /
Allium / ADR package above.

## Validation

All active guidance should now cite `requirements.md`, `bedrock.allium`, or ADR-025 rather than the
removed legacy `design.md`.

Search check used during migration:

- `rg -n 'work-lifecycle/design\.md' specs tasks`

Only historical task references should remain after migration.

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
