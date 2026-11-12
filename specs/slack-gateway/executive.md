# Slack gateway status

## Scope

One owner's three Slack gestures create or reuse ordinary local Phoenix
conversations through shared native services and the existing Slack MCP connection.
Explicit requests receive intentional source-thread answers. Other-author reactions
prepare separate private briefings. Thread reuse and exact reply coverage remain.

## Requirements

| Requirement | Status | Evidence boundary |
|---|---|---|
| REQ-SG-001 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-002 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-018: Configure one trigger emoji | Implemented | Single-string configuration, narrow queries, list removal, and preserving upgrade pass focused checks and local adversarial review. Hosted qualification and deployment remain separate. |
| REQ-SG-019: Capture a discussion privately with Save for later | Specified; not implemented | Controlled Saved membership, paging, and reply identity checks pass through the chat connector. Phoenix-owned transport and runtime qualification remain open. |
| REQ-SG-003 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-004 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-005 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-006 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-007 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-008 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-009 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-010 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-011 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-012 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-013 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-014 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-015 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-016 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-017 | Partial | Implemented; current candidate validation and production limits below. |

## Current behavior

[ADR-249](../adrs/249_slack-uses-one-configurable-trigger-emoji.md) replaces
multi-emoji aliases with one configurable Slack name. Prefix and owner-reaction
queries each use that name directly. Changing the name establishes a new baseline;
saving the same normalized name preserves progress. Accepted work, source identity,
thread reuse, and delivery duties remain unchanged.

The settings retain the three workflow explanations: an owner prefix or reaction
to the owner's message requests a Slack reply; an owner reaction to another
person's message prepares a private briefing. Plain replies do not trigger work.
Eyes and private notifications remain single-attempt, best-effort signals.
Clarifications direct the owner into Phoenix. Accepted work and intentional
thread replies retain their existing durable recovery.

## Validation

Focused checks cover scalar configuration and invalid/list input, all three
custom-emoji workflows, narrow search terms, baseline changes, and upgrade/reopen
from the published list schema. Upgrade keeps the first saved name, preserves
single-emoji progress, and discards unsupported multi-emoji query positions.
Baseline readiness, accepted requests, return coverage, and backoff survive.
All 58 gateway tests, 29 Slack database tests, 23 settings tests, and 156 migration
and migration-dependent tests pass. The selected static/UI checks pass, with
ast-grep unavailable locally. Local adversarial review found no actionable defects.
Hosted qualification and production deployment remain separate.

## Production boundary

Saved capture is a spec-only addition under [ADR-250](../adrs/250_saved-messages-create-private-any-age-captures.md).
It accepts any source age, initializes independently, and has no source-message
acknowledgment or thread response. Neither Saved discovery nor this policy has
been activated in production.

Production runs the canonical public schema, with one custom emoji configured as
an operational workaround for the OR-filter defect. The single-field API and
storage simplification require a new candidate deployment. No prototype conversion
is needed. Private configuration, recipients, and operational records stay outside
the public repository.

Discovery remains bounded by the connector's pagination, the four-calendar-date
window, and the existing call allowance. Removing multi-emoji support fixes the
unsupported OR query; it does not remove the connector's page limit.

## Saved capture verification and open qualification

Controlled read-only checks observe zero, one, two, then one saved message as the
owner saves and unsaves test sources. The removed reply remains readable without
the Saved predicate, excluding access loss as the explanation for its disappearance.
Two one-result pages produce distinct messages followed by a terminal empty page.
Own-author and other-author sources are covered; the saved reply retains the right
parent and supports an exact bounded source-plus-parent read. The connected owner
matches production, whose current MCP catalog exposes the same search inputs.

No Saved timestamp is returned. No new source messages, reactions, notifications,
native conversations, production setting changes or application changes were made
for this verification. Test source IDs and content are not public evidence.

Before activation, qualify reads through the Phoenix-owned MCP connection and
test any-age eligibility, private outcomes including failure, independent baseline
and deduplication scopes, restart continuity and the shared call allowance. Exercise
a collection beyond the connector's page ceiling: the one-result page strategy
can exhaust that ceiling, and the tiny controlled sample does not prove capacity
or latency at larger collection sizes. An incomplete scan must be visible and
cannot finish initialization or update a last-complete marker.


Spec-only adversarial review found three issues, now reconciled: membership is
not discovery evidence, metadata expiry must not impose an age limit or cross
workflow scopes, and prefix freshness must not bypass pending Saved work. Saved
admission now requires a verified observation; timing metadata is best effort
with the existing seven-day expiry, while accepted work and scan state remain
durable. Removing the catch-up bypass avoids another scheduler-state predicate.

Additional read-only probes show that search time bounds behave at whole-second
precision. A possible range-based paging continuation must overlap boundary
seconds and refuse to claim completeness when it cannot advance safely. Local
algorithm experiments cross a simulated 20-page ceiling with 100 distinct-time
records and preserve boundary-second overlap; dense equal-timestamp cases remain
explicitly incomplete. These are model experiments, not live Saved-capacity or
Phoenix-runtime qualification, and no paging implementation is selected by them.

The second spec-only adversarial pass found no actionable issues after those
fixes. All 62 Allium files validate without errors; all four selected spec checks
pass. This qualifies the proposal's consistency, not runtime or production behavior.
