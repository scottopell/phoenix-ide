# PR Association — Executive Summary

## What This Spec Covers

PR association is the WorkScope-owned link between a Work or Branch conversation and the GitHub
pull requests Phoenix has observed for it. Increment 1 broadens the normative model from a hidden
singular-primary world to a plural association world with one explicit active PR. This spec owns:

- **Observed-branch history** — durable WorkScope-keyed history of settled task-branch heads
  observed at supported reconciliation boundaries.
- **PR ↔ work-scope association** — durable, WorkScope-keyed history of observed PRs, learned
  from `gh` observations and reused until fresher facts replace them.
- **Active-PR selection** — explicit active PR targeting, including inferred versus pinned
  provenance.
- **Compatibility primary projection** — singular ranked projection retained only for unmigrated
  or compatibility surfaces.
- **PR status observation and refresh** — explicit fresh / stale-not-found / unavailable
  semantics for one explicit PR target, including refresh-by-number against durable PR identity so
  branch drift does not strand the user.
- **The Address-CI auto-fix affordance** — whether the "Address CI & comments" control is
  enabled or disabled and which PR it targets, derived from the active-PR status view.
- **PR feedback freshness and baseline** — the agent-facing advisory marker, the baseline of
  what Phoenix last handed the agent, and the bounded poll that gates full feedback fetches.

It does **not** own:

- **Terminal-action git side effects and PR-merge-state-as-cleanup-gate** — abandon and mark-
  as-merged worktree/branch disposition, and PR *merge* state as the cleanup label. The
  `work-lifecycle` spec owns these and consumes this spec's PR status as its cleanup gate.
- **Transition legality** — bedrock's `TaskResolved` rule and its
  `TerminalActionRequiresNoContinuation` invariant.
- **The work-actions-bar UI surface composition** — button labels, action zones, tooltips. The
  `work-actions-bar` spec consumes this spec's auto-fix affordance result and renders it.

## Surface Attribution

Two surfaces consume this spec's PR identity and must stay distinct:

- The **StateBar** renders the **PR badge / PR identity link** (`StateBarPrBadge`) — number,
  title, state. A StateBar concern.
- The **work actions bar** renders the **Address-CI auto-fix affordance**
  (`WorkActions.tsx` → `PrRemediationActions`), including the freshness advisory marker. The
  StateBar has no auto-fix logic.

Both target the same explicit active PR when one is selected; the freshness advisory
(REQ-PRA-001) lives next to the Address-CI action, not on the StateBar badge. A singular
primary-PR view may remain as compatibility current reality during migration, but the normative
owner for PR-specific targeting is the explicit active PR.

## User Need

A developer needs Phoenix to remember which PRs belong to their work, preserve the branch and PR
history that led there, keep status fresh without rate-limiting itself, point PR-specific actions
at the correct explicit PR, and flag when review feedback has arrived since the last time it
handed that feedback to the agent — without ever blocking ordinary work or treating feedback
freshness as branch health.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-PRA-000 | Durable branch-first association and explicit active PR — settled branch observations feed plural PR history; one explicit active PR targets PR-specific surfaces |
| REQ-PRA-000a | Active-PR inference order — pinned first, then latest observed actionable match, then only-actionable fallback, then still-valid prior inference, else unset |
| REQ-PRA-000b | PR-specific surfaces share one explicit target — StateBar identity, status, freshness, auto-fix, and sibling PR-specific surfaces stay aligned |
| REQ-PRA-000c | Compatibility primary projection is non-authoritative — singular projection may exist, but does not override explicit active-PR selection |
| REQ-PRA-001 | PR feedback freshness indicator — compact advisory near Address-CI; never branch health; never a lifecycle gate |
| REQ-PRA-002 | Agent-facing PR context baseline — record each successful remediation capture as the freshness baseline; new = identities absent from baseline; no baseline → no count |
| REQ-PRA-003 | Bounded PR feedback refresh — keep routine polls light; fetch full surfaces only when evidence says they changed; report a coverage gap (REQ-PRA-004) rather than coarsening freshness when a surface can't be read, and log |
| REQ-PRA-004 | PR feedback coverage health — a signal distinct from freshness when a surface can't be read; `auth_required` (user-fixable) vs `incomplete` (transient); any concurrent freshness count is a lower bound; no lifecycle authority |

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-PRA-000 | Specified for increment 1; implementation pending | Normative change only in this increment |
| REQ-PRA-000a | Specified for increment 1; implementation pending | Normative change only in this increment |
| REQ-PRA-000b | Specified for increment 1; implementation pending | Normative change only in this increment |
| REQ-PRA-000c | Specified for increment 1; implementation pending | Normative change only in this increment |
| REQ-PRA-001 | Implemented for the singular current-reality flow | `ui/src/components/WorkActions.tsx` (`PrRemediationActions`); freshness advisory marker |
| REQ-PRA-002 | Implemented for the singular current-reality flow | `crates/phoenix-ide/src/api/pr_monitoring.rs` (WorkScope-keyed baseline persistence) |
| REQ-PRA-003 | Implemented for the singular current-reality flow | `crates/phoenix-ide/src/api/pr_monitoring.rs` (gated full-feedback fetch) |
| REQ-PRA-004 | Implemented for the singular current-reality flow | `crates/phoenix-ide/src/api/pr_monitoring.rs` (coverage-health classification); `PrFeedbackCoverageHealth` in `api/types.rs` |

Current reality is still primarily singular: one ranked primary PR drives the implemented status
and action surfaces. This executive document records that mismatch explicitly. The normative spec
now defines the increment-1 target model — observed branch history, plural association history,
and explicit active-PR selection — so follow-on implementation work can migrate surfaces without
leaving the singular-primary behavior as the only written contract.

## Provenance

PR-association requirements were previously expressed in two layers of project history:

1. Project-scope requirements REQ-PROJ-030 through REQ-PROJ-032 described the singular
   feedback-freshness flow now carried as REQ-PRA-001 through REQ-PRA-004.
2. The older singular-primary association model lived as current behavior and design material in
   the `projects` and `pr-association` artifacts.

This increment keeps the renumbered freshness requirements and adds explicit plural-PR ownership
for branch observation, active selection, and compatibility projection so PR identity, status,
auto-fix targeting, and feedback freshness own a spec independent of project detection, mode
selection, and worktree creation. The sibling `work-lifecycle` spec continues to own terminal
actions and PR-merge-state-as-cleanup-gate behavior.

| Prior source | New ID | Subject |
|-------------|--------|---------|
| project-scope singular behavior | REQ-PRA-000 | Durable branch-first association and explicit active PR |
| project-scope singular behavior | REQ-PRA-000a | Active-PR inference order |
| project-scope singular behavior | REQ-PRA-000b | One explicit active target for PR-specific surfaces |
| project-scope singular behavior | REQ-PRA-000c | Compatibility primary projection is non-authoritative |
| REQ-PROJ-030 | REQ-PRA-001 | PR feedback freshness indicator |
| REQ-PROJ-031 | REQ-PRA-002 | Agent-facing PR context baseline |
| REQ-PROJ-032 | REQ-PRA-003 | Bounded PR feedback refresh |
| REQ-PROJ-032 extension | REQ-PRA-004 | PR feedback coverage health |
