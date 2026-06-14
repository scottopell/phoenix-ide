# PR Association — Executive Summary

## What This Spec Covers

PR association is the WorkScope-owned link between a Work or Branch conversation and the GitHub
pull requests Phoenix has observed for it. This spec owns:

- **PR ↔ work-scope association** — durable, WorkScope-keyed history of observed PRs, learned
  from `gh` observations and reused until fresher facts replace them.
- **Primary-PR derivation** — selecting the one authoritative PR per scope, ranked by display
  state and tie-broken by update time. The StateBar PR badge and the Address-CI auto-fix
  action target this same primary.
- **PR status observation and refresh** — explicit fresh / stale-not-found / unavailable
  semantics, including refresh-by-number against a persisted primary so branch drift does not
  strand the user.
- **The Address-CI auto-fix affordance** — whether the "Address CI & comments" control is
  enabled or disabled and which PR it targets, derived from the PR status view.
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

Both target the same primary PR; the freshness advisory (REQ-PRA-001) lives next to the
Address-CI action, not on the StateBar badge.

## User Need

A developer needs Phoenix to remember which PR belongs to their work, keep its status fresh
without rate-limiting itself, point "Address CI & comments" at the correct PR, and flag when
review feedback has arrived since the last time it handed that feedback to the agent — without
ever blocking ordinary work or treating feedback freshness as branch health.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-PRA-001 | PR feedback freshness indicator — compact advisory near Address-CI; never branch health; never a lifecycle gate |
| REQ-PRA-002 | Agent-facing PR context baseline — record each successful remediation capture as the freshness baseline; new = identities absent from baseline; no baseline → no count |
| REQ-PRA-003 | Bounded PR feedback refresh — keep routine polls light; fetch full surfaces only when evidence says they changed; report a coverage gap (REQ-PRA-004) rather than coarsening freshness when a surface can't be read, and log |
| REQ-PRA-004 | PR feedback coverage health — a signal distinct from freshness when a surface can't be read; `auth_required` (user-fixable) vs `incomplete` (transient); any concurrent freshness count is a lower bound; no lifecycle authority |

## Implementation Status

| Requirement | Status | Surface |
|-------------|--------|---------|
| REQ-PRA-001 | Implemented | `ui/src/components/WorkActions.tsx` (`PrRemediationActions`); freshness advisory marker |
| REQ-PRA-002 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (WorkScope-keyed baseline persistence) |
| REQ-PRA-003 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (gated full-feedback fetch) |
| REQ-PRA-004 | Implemented | `crates/phoenix-ide/src/api/pr_monitoring.rs` (coverage-health classification); `PrFeedbackCoverageHealth` in `api/types.rs` |

The broader association, primary-derivation, status, and auto-fix behaviour on which these
requirements build is modelled normatively in `pr-association.allium`; REQ-PRA-001..004 are
the agent-facing feedback-freshness layer over it.

## Provenance

PR-association requirements were previously expressed as project-scope requirements
REQ-PROJ-030 through REQ-PROJ-032 in the `projects` spec. They are carried here, renumbered to
the REQ-PRA-* prefix, so that PR identity, status freshness, auto-fix targeting, and feedback
freshness own a spec independent of project detection, mode selection, and worktree creation.
The sibling `work-lifecycle` spec carved out the terminal-action and PR-merge-state-as-cleanup-
gate requirements (REQ-PROJ-010/011/026/027 → REQ-WL-001..003) from the same project-scope
source.

| Prior ID | New ID | Subject |
|----------|--------|---------|
| REQ-PROJ-030 | REQ-PRA-001 | PR feedback freshness indicator |
| REQ-PROJ-031 | REQ-PRA-002 | Agent-facing PR context baseline |
| REQ-PROJ-032 | REQ-PRA-003 | Bounded PR feedback refresh |
