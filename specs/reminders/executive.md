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

Upcoming/due HTTP and UI views and connected-browser notifications are implemented.
The owner view and immutable due cards read saved state independently of model
responses. The native/API restart journey and desktop/narrow browser journey pass;
complete project checks and adversarial review remain pending. Deployment and
device acceptance are not claimed.

## Coverage

| Requirements | Status | Evidence |
|---|---|---|
| REQ-REM-001–002 ownership, durable create and replay | Implemented | Host-bound tool; `Database::create_reminder`, `reminder_for_create_call`; full trusted invocation identity and unresolved-request fingerprint |
| REQ-REM-003 time resolution | Implemented | `ReminderWhen::resolve`; DST gap/fold/offset validation; current UTC time in list replies |
| REQ-REM-004, REQ-REM-006 due commitment and management | Storage implemented | Atomic due/cancel/reschedule transactions; schema-enforced immutable terminal rows |
| REQ-REM-005 ordinary input admission | Implemented | Existing worker and real Send Chat service; direct, steering, refusal/readiness, continuation, removed queue identity and stopped accepted-turn coverage |
| REQ-REM-007 restart recovery | Storage implemented | File-backed reopen and late-discovery regression authored |
| REQ-REM-008 visible reminder and provenance | Implemented; browser journey qualified | Scheduled provenance, canonical reminder API/cards, saved-zone management and global connected-browser notifications; focused component/policy tests and desktop/narrow browser QA |
| REQ-REM-009 lifecycle boundary | Storage implemented | Existing ProductConversation admission; ordinary owner FKs; no Close participant |

Focused storage validation passes 16 tests covering replay/conflict, owner and
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

Focused HTTP validation passes three API tests covering owner isolation, conflict
mapping, durable due visibility and shared DST/reschedule validation. UI validation
passes 131 tests across seven focused files, with TypeScript, ESLint and reminder
CSS checks passing. Core, DB and application library Clippy checks also pass. The updated notification-feed continuation and file-backed
settings persistence tests also pass. No full project-check pass is claimed;
broader checks and independent adversarial review remain required before PR readiness. The management host helpers expose the same
exact-probe/fail-stop behavior to API callers without requiring a tool-call identity.

## Authorities

- `requirements.md`: product contract.
- `reminders.allium`: time-eligibility transitions.
- `../adrs/217_reminders-submit-through-standard-input-admission.md`: bounded architecture decision.


## Web and HTTP slice

The authenticated HTTP surface provides `GET /api/conversations/:id/reminders`, revision-checked `POST /api/conversations/:id/reminders/:reminder_id/cancel` and `/reschedule`, and the read-only `GET /api/reminders/due` feed. Owner lists select at most 50 rows independently for scheduled, due and cancelled state. Native tool lists use 10 rows per state and return `limit_per_state` metadata. The global feed pages at 100 rows through due-time/ID cursors within the last day of actual occurrence observation; each connected-page refresh restarts its scan and caps at ten pages. It resolves current continuation/Coordinator heads and excludes History without consulting model admission receipts.

The conversation panel displays saved-zone exact times, immutable due cards and scheduled-only management. Editing retains the opening revision; validation exposes IANA zone and optional UTC offset, and conflicts reload current state. Abortable non-overlapping polling refreshes at five seconds and on visibility/focus/network return. Browser alerts use the existing notification store/policy, a server-persisted `notify_reminder` key defaulting enabled, an initially quiet baseline and independent per-reminder dedupe. This does not provide closed-app phone notifications.

Focused API, persistence, component, policy, navigation and stale-refresh regressions accompany this slice. Integrated API/browser journey validation passes; broad checks and adversarial review remain pending.

## API journey qualification

The isolated real-binary `tests/e2e/run.py --scenario reminders` journey passes.
An opt-in mock provider calls the native tool with production invocation identity;
the test verifies the committed receipt and upcoming HTTP record, restarts the
server before due, reschedules through HTTP, observes the due fact and a normal
model follow-up with scheduled provenance, then reconnects and restarts after
acceptance without creating another input. It also covers stale-revision and
wrong-owner management failures and cancellation through the real routes.
All 18 mock-provider tests, 46 E2E harness self-tests, seven panel tests, reminder
CSS lint and the production frontend build pass.

Chrome browser QA observed the upcoming card and reschedule form. Its desktop
screenshot exposed native button styling; reminder controls now use the
application's theme variables and focus styles. A parent-agent pass through the
in-app browser verified real rescheduling with America/New_York, cancellation,
immutable due-card visibility, the scheduled input label and ordinary model
follow-up. At 393×852 the panel was within the viewport (x=16, width=361,
document width=393). Native date-field keyboard editing worked; earlier CLI
fill/capture/resize attempts were inconclusive. Notification permission delivery
and physical devices were not exercised. The complete project check and
independent adversarial review remain pending. No deployment was performed.
