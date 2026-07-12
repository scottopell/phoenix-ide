# PR Association — Executive Summary

## What This Spec Covers

PR association is the WorkScope-owned link between a Work or Branch conversation and the GitHub
pull requests Phoenix has observed for it. Phoenix persists plural association history and one
explicit active PR for PR-specific targeting. This spec owns:

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
primary-PR view may exist as a compatibility projection for singular consumers, but the
authoritative owner for PR-specific targeting is the explicit active PR.

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
| REQ-PRA-000 | Implemented | `crates/phoenix-ide/src/runtime.rs` (`reconcile_scope_pr_observations_after_terminal_edge`, `qualify_observed_branch_for_conversation`); `crates/phoenix-ide/src/api/git_handlers.rs` (`selection_envelope_for_scope` test coverage for plural associations and latest observed branch) |
| REQ-PRA-000a | Implemented | `crates/phoenix-ide/src/runtime.rs` (`reconcile_scope_pr_observations_after_terminal_edge` → `derive_active_work_scope_pr_selection`); `crates/phoenix-ide/src/api/git_handlers.rs` (`resume_active_pr_selection`, `pin_active_pr_selection`, `active_selection_target_uses_explicit_selection_not_ranked_primary`) |
| REQ-PRA-000b | Implemented | `crates/phoenix-ide/src/api/git_handlers.rs` (`get_pr_status`, `create_pr_auto_fix_context`, `get_active_pr_diff` all target `active_selection_target_for_scope`); `ui/src/components/WorkActions.test.tsx` (active PR interactions + diff targeting); `ui/src/components/StateBar.tsx` (active PR selector) |
| REQ-PRA-000c | Implemented | `crates/phoenix-ide/src/api/git_handlers.rs` (`active_selection_target_for_scope`, `active_selection_target_uses_explicit_selection_not_ranked_primary`); compatibility projection remains non-authoritative |
| REQ-PRA-001 | Implemented | `ui/src/components/WorkActions.tsx` (`PrRemediationActions`); `ui/src/components/WorkActions.test.tsx` (freshness marker scenarios) |
| REQ-PRA-002 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (WorkScope-keyed baseline persistence and refresh classification) |
| REQ-PRA-003 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (gated full-feedback fetch during status refresh) |
| REQ-PRA-004 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (`coverage_health`); `crates/phoenix-ide/src/api/types.rs` (`PrFeedbackCoverageHealth`); `ui/src/components/WorkActions.test.tsx` (coverage marker scenarios) |

Phoenix now implements the plural-association and explicit-active-selection model across the PR
selection envelope, terminal-edge observation reconciliation, active-selection mutation surfaces,
and PR-specific targeting. Singular ranked-primary behavior remains only as a compatibility
projection for consumers that still need a singular view.

## Provenance

PR-association requirements were previously expressed in two layers of project history:

1. Project-scope requirements REQ-PROJ-030 through REQ-PROJ-032 described the singular
   feedback-freshness flow now carried as REQ-PRA-001 through REQ-PRA-004.
2. The older singular-primary association model lived as current behavior and design material in
   the `projects` and `pr-association` artifacts.

This spec keeps the renumbered freshness requirements and adds explicit plural-PR ownership for
branch observation, active selection, and compatibility projection so PR identity, status,
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
