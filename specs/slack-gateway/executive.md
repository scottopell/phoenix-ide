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
and exact request coverage. They have at most three durably claimed attempts,
including pre-dispatch failures; definitely-unsent exhaustion records failure and
queues one private notice. Negative acknowledgment envelopes remain unconfirmed
when their error detail is missing or malformed.

Deleted runtime code includes receipt inspection, inspection scheduling, private
retry handling, question-text rendering and question-retirement synchronization.
Obsolete retry tests are replaced by single-attempt failure/restart regressions.
ADR-188 removes prototype compatibility. One final `142/slack_gateway_v1`
schema replaces the unmerged migration chain. Obsolete question associations,
question snapshots, notification inspection metadata and historical trigger
provenance are absent. Existing lineage preflight refuses prototype identities
before mutation. Upstream migration history through 141 is unchanged.

## Validation

Creation-receipt retention migration147 changes the deletion expectation in `request_status_links_require_native_publication`: deleting the predecessor retains its published creation receipt and original identities. The test still requires ordinary native routing and Slack product links to resolve a remaining live successor, then requires both request/return links to become unavailable after last-transcript deletion while the same receipt remains. Coordinator creation history remains tied to the original transcript and does not inherit that successor route. Hosted qualification at head `4e0857e662203` (synthetic merge `d7e4e2dd282a6`) exposed the obsolete cascade-deletion assertion; the corrected test and whole owning Slack DB module await new exact-source qualification. Earlier validation below is historical, not PASS for the retention successor.

All 292 Rust migration, lineage, gateway, Goal and replay tests pass on the
reconciled schema.
Coverage includes fresh installation, prototype refusal before mutation, native
attribution, restart admission and delivery, exact request coverage, generic
clarification handoffs, bounded reply attempts and one-attempt signals.

Eleven UI/static checks pass; seven final static checks also pass after test
fixture consolidation. The full developer-tool suite and spec-shape checks pass
with native Bash/OpenSSL and without a shared Cargo target override. Upstream
migration definitions through 141 are byte-for-byte unchanged.

Integration with main preserves canonical checkpoint transactions and their
receipt schema. Briefing completion remains atomic and checkpoint replay cannot
duplicate its private notice. Ten UI/static checks and three final static checks
pass. Hosted CI and normal automatic review run on the reconciled candidate
before merge.

## Production boundary

The enabled production pilot is an earlier accepted build. Source IDs, recipient
routing, credentials, provider adapters and private operational records stay outside
the public repository. The redesigned settings and later hardening are not claimed
deployed by pilot evidence.

The public feature supports only the final schema on canonical Phoenix data.
The pilot and other unmerged development ledgers require separate offline
reconciliation and paired rollback qualification. No prototype conversion or new
production deployment is implied. The protected deployment gate stays intact.
