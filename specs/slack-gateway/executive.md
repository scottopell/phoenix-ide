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

Production runs the canonical public schema, with one custom emoji configured as
an operational workaround for the OR-filter defect. The single-field API and
storage simplification require a new candidate deployment. No prototype conversion
is needed. Private configuration, recipients, and operational records stay outside
the public repository.

Discovery remains bounded by the connector's pagination, the four-calendar-date
window, and the existing call allowance. Removing multi-emoji support fixes the
unsupported OR query; it does not remove the connector's page limit.
