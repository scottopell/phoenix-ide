# PR Association — Technical Design

## Architecture Overview

PR association is a WorkScope-owned runtime invariant. PR identity is learned from provider
(`gh`) observations, persisted keyed by **WorkScope** — the durable owner key that resolves a
conversation to either a worktree scope or a conversation scope — and reused by status and
action surfaces until fresher provider facts replace it. Continuations that inherit a worktree
resolve to the same WorkScope as their predecessor, so PR identity survives context-exhaustion
continuations.

The behavioural model lives in `pr-association.allium`. The implementation home is
`crates/phoenix-ide/src/api/pr_monitoring.rs` (observation, persistence, primary derivation,
status/refresh, auto-fix context) and `ui/src/components/WorkActions.tsx`
(`PrRemediationActions`, which renders the Address-CI affordance).

The scope boundary — what this spec owns versus what `work-lifecycle`, `bedrock`, and
`work-actions-bar` own — is stated canonically in `requirements.md` and `executive.md` and is
not re-duplicated here.

## WorkScope-Keyed Observation History

A `WorkScopeRecord` is the durable row for a scope that has actually learned PR facts;
read-only lookups never create it. Each observed PR becomes a `WorkScopePrAssociation` under
that record. Re-observing the same PR updates the mutable PR facts, `rank`, and `last_seen_at`
while `first_seen_at` stays stable. Three invariants hold: one record per scope, one
association per PR per scope, and at most one primary per scope.

## Primary-PR Derivation

`primary_pr(scope)` selects the highest-ranked persisted PR, ranked open-non-draft → draft →
merged → closed, tie-broken by provider update time then observation time. Derivation runs
**after** observation persistence so a fresh branch discovery and any number-refreshed
association are compared in one history before the response is selected. The same primary is
rendered by the StateBar PR badge and targeted by the Address-CI auto-fix affordance —
`StatusAndActionUseSamePrimary` is the contract guaranteeing they never diverge.

## Freshness-Aware PR Status

`GET pr-status` resolves to a `PrStatusView` carrying both the PR identity and explicit
`PrRefreshMetadata`. Freshness is never inferred from the presence or absence of a PR identity
(`FreshnessIsExplicit`). The refresh state distinguishes three cases:

- **fresh** — branch discovery succeeded; the derived primary is returned with fresh metadata.
- **not_found (stale)** — branch discovery completed but found no current-branch PR; a
  persisted open primary is still returned with stale-not-found metadata, because auto-fix can
  refresh that primary **by number**.
- **unavailable** — the provider could not refresh at all; a persisted primary is returned
  stale-with-reason, or, with no persisted primary, the view carries a null PR and the reason.
  `StaleReasonIsStateful`: a not_found response never masquerades as unavailable.

## Auto-Fix Affordance Derivation

The Address-CI affordance (`PrAutoFixAffordance`) is derived from the status view and rendered
by the **work actions bar** (`PrRemediationActions`), not the StateBar. Four disjoint rules
cover it:

- open primary + fresh → **enabled**, targeting the primary.
- open primary + stale not_found → **enabled**, targeting the primary by number. The disabled
  copy must not tell the user to refresh branch status here: branch refresh already said
  not_found, and the available action is number-based auto-fix against the persisted primary
  (`NotFoundStalePrimaryIsActionable`).
- open primary + unavailable refresh → **disabled** with the provider reason surfaced
  (`UnavailableProviderIsNotActionable`).
- no primary, or a non-open primary → **disabled** with the non-open rejection message.

The auto-fix *context* (`create_context`) refreshes the persisted primary by number
independently of branch discovery, so branch/HEAD drift does not strand a user who has an
associated primary. Branch-based discovery is a fallback only when no associated primary
exists.

## PR Feedback Freshness and Baseline (REQ-PRA-001..004)

The agent-facing feedback freshness layer builds on the status/identity model above. It is
modelled in `pr-association.allium` as four `deferred` behavioural extensions
(`PrFeedbackFreshnessAdvisory`, `PrRemediationContextBaseline`, `BoundedPrFeedbackRefresh`,
`PrFeedbackCoverageHealthAdvisory`) because it composes the existing freshness-aware PR identity
rather than introducing new association state-machine transitions.

**Baseline (REQ-PRA-002).** Each *successful* capture of agent-facing PR remediation context
records a baseline keyed by WorkScope and PR number: the capture timestamp, the PR
`updated_at` (the `PrIdentity.github_updated_at` field) when available, and stable feedback
identities/fingerprints when available. The baseline is what Phoenix actually handed the
agent — not what GitHub contained at an unrelated time. Feedback is classified **new** when
current feedback carries stable identities absent from the latest successful baseline. With no
baseline, nothing is "new", so no count is shown.

**Advisory (REQ-PRA-001).** When an open associated PR's feedback differs from the baseline, a
compact count marker (`{N} new` for net-new items, `{N} edited` for changed baseline items)
renders next to the Address-CI Work Action. It is *not* the StateBar branch-health signal (PR
merge state owns that, in `work-lifecycle`) and it never gates cleanup, abandon, or ordinary
conversation use.

**Bounded refresh (REQ-PRA-003).** Routine PR-status polling stays lightweight. Full feedback
surfaces are fetched only when gated by evidence they may have changed — PR `updated_at` newer
than the latest successful baseline, or an explicit remediation-context capture. When a surface
cannot be read during classification, the gap is reported as coverage health (REQ-PRA-004), not
folded into a coarse freshness marker, and the failure is logged — the
capability-gap-is-logged-not-silenced principle applied to rate-limit-sensitive review surfaces.

**Coverage health (REQ-PRA-004).** Coverage health is orthogonal to freshness: freshness answers
"did feedback change?", coverage answers "could we read all of it?". When a surface is unreadable
it is surfaced as a distinct signal — `auth_required` (the GitHub CLI is not authenticated;
user-fixable via `gh auth login`) or `incomplete` (a transient gap) — and any freshness count
shown alongside it is a lower bound. Folding the two together would make an incomplete fetch
indistinguishable from genuinely-fresh feedback; keeping them separate lets the UI show an honest
count and offer a concrete fix for the auth case. Neither signal carries lifecycle authority.

## Abandon Refresh

On abandon of a Work or Branch conversation, Phoenix attempts a bounded, best-effort PR
refresh (`abandon_refresh_deadline_ms`) while the worktree still exists, then continues
cleanup unconditionally. Any failure, unavailable provider, not_found, or deadline expiry is
logged and cannot block worktree/branch cleanup (`PrRefreshNeverBlocksCleanup`). The cleanup
git side effects themselves belong to `work-lifecycle`; this spec owns only the best-effort
enrichment that precedes them.

## Relationship to Other Specs

- **bedrock** — supplies the `Conversation` entity and owns transition legality. This spec's
  surfaces are gated on `conversation.mode ∈ {work, branch}` but do not define the terminal-
  action legality gate.
- **work-lifecycle** — consumes `PrStatusResult` as its cleanup gate (REQ-WL-003: PR *merge*
  state labels the cleanup path) and owns the abandon/mark-merged git side effects. This spec
  provides the status; work-lifecycle decides cleanup. Branch health = PR merge state lives
  there; PR feedback freshness here is deliberately *not* branch health.
- **work-actions-bar** — consumes `PrAutoFixAffordance` and renders the Address-CI control,
  its enable/disable state, the freshness advisory marker, and the copy. This spec derives the
  affordance; work-actions-bar composes the surface.
