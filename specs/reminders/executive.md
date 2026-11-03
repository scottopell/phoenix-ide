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

The host-bound `reminders` tool supports strict create/list/reschedule/cancel
requests, local-time/DST resolution, full transcript/assistant-message/tool-call
create identity and committed agent receipts. Top-level Direct, Explore, Work and
Coordinator runtimes receive it; subordinate registries do not. Explore-to-Work
upgrades retain the bound capability.

The existing Direct Turn worker promotes due facts and submits immutable text
through ordinary Send Chat admission. Busy inputs use its steering queue;
awaiting-user and task-approval refusals leave reminders pending until ordinary
admission accepts them. No reminder-specific readiness policy is introduced.
Scheduled-reminder provenance is normalized through durable turns, steering,
messages, read/replay paths and wire types; model rendering identifies saved
reminder data without granting new human authority.

Upcoming/due API and UI views and connected-browser notifications remain the
next implementation slice. No full end-to-end reminder journey, deployment or
device acceptance is claimed.

## Coverage

| Requirements | Status | Evidence |
|---|---|---|
| REQ-REM-001–002 ownership, durable create and replay | Implemented | Host-bound tool; `Database::create_reminder`, `reminder_for_create_call`; full trusted invocation identity and unresolved-request fingerprint |
| REQ-REM-003 time resolution | Implemented | `ReminderWhen::resolve`; DST gap/fold/offset validation; current UTC time in list replies |
| REQ-REM-004, REQ-REM-006 due commitment and management | Storage implemented | Atomic due/cancel/reschedule transactions; schema-enforced immutable terminal rows |
| REQ-REM-005 ordinary input admission | Implemented | Existing worker and real Send Chat service; direct, steering, refusal/readiness, continuation, removed queue identity and stopped accepted-turn coverage |
| REQ-REM-007 restart recovery | Storage implemented | File-backed reopen and late-discovery regression authored |
| REQ-REM-008 visible reminder and provenance | Partial | Scheduled provenance persists and renders through provider/wire/input attribution; due views and browser notifications remain pending |
| REQ-REM-009 lifecycle boundary | Storage implemented | Existing ProductConversation admission; ordinary owner FKs; no Close participant |

Focused storage validation passes 13 tests covering replay/conflict, owner and
subagent boundaries, Coordinator ownership, revision/deadline replacement,
cancellation/due arbitration, durable steering receipts, keyset discovery,
file-backed restart, foreign-key cleanup, backward clock adjustment, full-call
identity and scheduled origin round-trips. Focused runtime validation passes nine
tests covering real standard admission, persistent steering cancellation,
awaiting-user/approval refusal followed by readiness, continuation, Stop,
model-independent due facts, create replay and management write classification/fail-stop.
Three tool tests and seven core input-origin tests pass. Workspace type export,
UI TypeScript checking and tool-crate Clippy pass. The core value-validation test
also passes. Reminder Allium analysis has zero diagnostics
and findings when checked with its governing Bedrock import; formatting, diff
whitespace and task metadata validate.

A filtered project spec check passed artifact shape, but its original Allium pass
preceded the corrected surfaces and failed four unreachable-trigger findings. Its
nested dev.py unit suite spawned an unplanned full build and was stopped; no full
project-check pass is claimed. Broader checks and independent adversarial review
remain required before PR readiness. The management host helpers expose the same
exact-probe/fail-stop behavior to API callers without requiring a tool-call identity.

## Authorities

- `requirements.md`: product contract.
- `reminders.allium`: time-eligibility transitions.
- `../adrs/217_reminders-submit-through-standard-input-admission.md`: bounded architecture decision.
