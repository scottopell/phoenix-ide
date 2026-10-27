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

## Simplification

ADR-155 replaces independent signal recovery with single-attempt behavior.
Private commands record submission, rejection or uncertainty without receipt
inspection or automatic retry. Eyes is claimed once before calling Slack; failure
or interruption cannot retry it. These signals never block an otherwise settled
account change. Clarifications contain only a generic Phoenix handoff and link;
question bodies and options remain native. Thread replies retain durable delivery
and exact request coverage.

Deleted runtime code includes receipt inspection, inspection scheduling, private
retry handling, question-text rendering and question-retirement synchronization.
Obsolete retry tests are replaced by single-attempt failure/restart regressions.
Existing migration history remains unchanged; retained private retries stop visibly
and definitely-unsent legacy question-text returns become distinct generic handoffs.

## Validation

All 606 affected Rust tests pass, covering MCP, gateway, native routing,
migrations, creation replay and deletion. All ten local check types pass,
including full UI tests, type checking, linting, Clippy, formatting, Allium and
structural checks. The 86-test focused run also passes. Regression coverage
includes single-attempt command outcomes across settings changes/restart,
interrupted eyes claims, discovery after failed acknowledgments, generic handoffs
across question changes, exact connector pagination and OAuth callback grace.

The 1,200-test developer-tool run had three environment-sensitive failures;
all three pass with native Bash/OpenSSL and the shared Cargo target override
removed. The full developer-tool run was not repeated. Hosted CI and posted
review feedback remain gates for the new head.

## Production boundary

The enabled production pilot is an earlier accepted build. Source IDs, recipient
routing, credentials, provider adapters and private operational records stay outside
the public repository. The redesigned settings and later hardening are not claimed
deployed by pilot evidence.

Public migrations 132–136 follow upstream migration 131; the pilot used a distinct
pre-publication migration-130 ledger. Direct pilot-to-public upgrade is unqualified
and requires offline reconciliation plus paired rollback qualification. Protected
deployment limits remain intact. No merge, new deployment or live integration
writes are implied by local tests or this simplification.
