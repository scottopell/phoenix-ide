# Conversation Reminders — Executive Summary

## Current Reality

Reminder domain types, normalized storage and migration 150 implement registration,
owner-scoped reads, revision-checked cancellation/rescheduling, immutable due
commitment, restart discovery and standard-acceptance lookup. Registration is
bound to a trusted top-level transcript and tool invocation; ordinary conversations
and the Coordinator are supported without a reminder WorkScope dependency.

Due discovery derives accepted status from existing Direct Turn identities and
durable steering acceptance receipts rather than copying runtime execution state
into reminder rows. Keyset discovery permits callers to advance past temporarily
refused recipients. Reminder code does not interpret Close phases.

Tool/runtime admission, scheduled input provenance, API/UI visibility and browser
notification integration remain subsequent implementation slices. No end-to-end
reminder delivery, deployment or device acceptance is claimed by this storage slice.

## Coverage

| Requirements | Status | Evidence |
|---|---|---|
| REQ-REM-001–002 ownership, durable create and replay | Storage implemented | `Database::create_reminder`, `reminder_for_create_call`; trusted source/fingerprint constraints |
| REQ-REM-003 time resolution | Partial | Validated timestamp/timezone domain types; local-time/DST tool resolution remains pending |
| REQ-REM-004, REQ-REM-006 due commitment and management | Storage implemented | Atomic due/cancel/reschedule transactions; schema-enforced immutable terminal rows |
| REQ-REM-005 ordinary input admission | Pending integration | Discovery reads standard durable turn and steering acceptance identities; no new runtime owner |
| REQ-REM-007 restart recovery | Storage implemented | File-backed reopen and late-discovery regression authored |
| REQ-REM-008 visible reminder and provenance | Pending integration | Queryable due rows exist; tool/wire/UI and model origin integration pending |
| REQ-REM-009 lifecycle boundary | Storage implemented | Existing ProductConversation admission; ordinary owner FKs; no Close participant |

Focused storage validation passes 11 tests covering replay/conflict, owner and
subagent boundaries, Coordinator ownership, revision/deadline replacement,
cancellation/due arbitration, durable steering receipts, keyset discovery,
file-backed restart, foreign-key cleanup and backward clock adjustment. The core
value-validation test also passes. Reminder Allium analysis has zero diagnostics
and findings when checked with its governing Bedrock import; formatting, diff
whitespace and task metadata validate.

A filtered project spec check passed artifact shape, but its original Allium pass
preceded the corrected surfaces and failed four unreachable-trigger findings. Its
nested dev.py unit suite spawned an unplanned full build and was stopped; no full
project-check pass is claimed. Broader checks and independent adversarial review
remain required before PR readiness.

## Authorities

- `requirements.md`: product contract.
- `reminders.allium`: time-eligibility transitions.
- `../adrs/217_reminders-submit-through-standard-input-admission.md`: bounded architecture decision.
