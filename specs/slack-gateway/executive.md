# Slack gateway status

## Scope

One owner's emoji gestures and optional Saved-message captures create or reuse ordinary local Phoenix
conversations through shared native services and the existing Slack MCP connection.
Explicit requests receive intentional source-thread answers. Other-author reactions
prepare separate private briefings. Thread reuse and exact reply coverage remain.

## Requirements

| Requirement | Status | Evidence boundary |
|---|---|---|
| REQ-SG-001 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-002 | Partial | Implemented; current candidate validation and production limits below. |
| REQ-SG-018: Configure one trigger emoji | Implemented | Single-string configuration, narrow queries, list removal, and preserving upgrade pass focused checks and local adversarial review. Hosted qualification and deployment remain separate. |
| REQ-SG-019: Capture a discussion privately with Save for later | Implemented; locally verified | Durable Saved scope/baseline, private outcomes, retained progress and shared polling pass focused tests and local review. Phoenix-owned live qualification and activation remain separate. |
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
All 63 gateway tests, 31 Slack database tests, 156 migration tests, three
migration-dependent tests and 36 settings/compatibility tests pass. These include
Saved initialization, any-age/any-author source verification, private failure and
question outcomes, scope separation, restart continuity, shared allowance and
provider-ceiling incompleteness. Clippy and the selected static/spec/UI checks
pass; ast-grep is unavailable locally. Local adversarial review findings are fixed
and the final focused recheck is clean. Hosted qualification and production
deployment remain separate.

## Production boundary

Saved capture is an opt-in addition under [ADR-250](../adrs/250_saved-messages-create-private-any-age-captures.md).
It accepts any source age, initializes independently, and has no source-message
acknowledgment or thread response. Neither Saved discovery nor this policy has
been activated in production.

Production runs the canonical public schema, with one custom emoji configured as
an operational workaround for the OR-filter defect. The single-field API and
storage simplification require a new candidate deployment. No prototype conversion
is needed. Private configuration, recipients, and operational records stay outside
the public repository.

Discovery remains bounded by the connector's pagination and existing call allowance.
Emoji discovery retains its four-calendar-date window; Saved discovery has no
source-message age cutoff. Removing multi-emoji support fixes the
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

The Saved candidate adds normalized scan progress and a distinct source scope,
with a preserving migration for accepted identities, return coverage and native
provenance. It shares the serial worker, 16-call allowance, backoff and accepted-work
reserve with emoji discovery. First initialization baselines existing saves;
re-enabling and retained-state restart preserve progress and handled identities.
Emoji-setting changes do not reset Saved state. No source acknowledgment or thread
return is created by Saved work, including questions, refusal or failure.

Spec-only adversarial reviews found and resolved observation-authority,
metadata-scope and fresh-prefix ordering inconsistencies. The project-level spec
review is clean. Implementation review found a mismatched test assertion and a
missing compatibility probe argument; both are corrected with regression coverage.
A follow-up check also retains the workspace-identity argument form, which omits
that optional argument. Its regression test and final review recheck pass.

Before activation, qualify reads through the Phoenix-owned MCP connection and
approve a controlled live capture test. The existing read-only checks use the chat
connector and do not establish Phoenix-owned transport qualification. Local tests
exercise any-age and any-author sources, independent scopes, restart continuity,
private outcomes and shared scheduling. A simulated collection beyond the
connector's page ceiling must remain visibly incomplete, preserve readiness and
accepted work, and leave the last-complete marker unchanged. No range-based paging
continuation or fixed discovery latency is promised.
