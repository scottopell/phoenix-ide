# ADR-217: Reminders submit through standard input admission

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-REM-001 through REQ-REM-009

## Context

The user requested a native scheduled reminder after an agent could only promise
to remember a future Safari inspection/storage-screenshot task. The user selected
both a visible reminder and an agent follow-up, with closed-app phone alerts
deferred. After reviewing wake history, the user explicitly clarified that there
is no Idle-only delivery requirement: standard direct-turn admission should queue
the alert appropriately and deliver it when the conversation is ready.

ADR-098 removed the old wake execution feature. Its implicit process enrollment
had generated semantically duplicate results and model turns, and even the
explicit replacement required another authority/execution path before its first
useful journey. Calendar reminders need neither process observation nor wake's
handle-ownership, transfer and terminal-result lifecycle.

## Options considered

1. Restore wake with a clock condition and reminder profile. This would reintroduce
   a retired execution lifecycle and unrelated process-wait semantics.
2. Create a reminder-specific delivery queue, runtime owner and Idle gate. This
   duplicates standard input acceptance and imposes a product restriction the user
   did not request.
3. Persist reminder time eligibility, then submit one immutable occurrence through
   existing standard input admission. This adds the missing time trigger while
   retaining existing queueing, continuation, Stop, recovery and lifecycle owners.

## Decision

Choose option 3. A reminder row owns only saved reminder intent, scheduling
revision, cancellation and one immutable due occurrence. A clock observation
freezes a scheduled revision into due state in SQLite. Direct display reads that
fact independently of provider or model success.

Creation is replay-safe under the trusted transcript, assistant-message and tool-call identity and an
immutable fingerprint of normalized unresolved request arguments. The fingerprint
describes the original operation; the mutable deadline describes the current
schedule. Comparing the original request prevents a retried relative duration
from moving its accepted deadline after a lost acknowledgement.

Due delivery uses a deterministic reminder occurrence key with standard input
admission. Direct Turn records and durable steering acceptance receipts remain
the authority for whether input was accepted, including after a queued input was
removed or its execution was stopped. Reminder rows do not copy accepted/runtime
state. Their terminal due shape remains unchanged after acceptance or model failure.

Scheduling discovery uses the existing server worker authority and durable times,
with bounded keyset reads so a refused early recipient does not starve later
recipients. Existing ProductConversation admission owns Close/History decisions;
reminders add neither a Close settlement participant nor an Idle check. Historical
wake records remain inert.

The forward migration creates normalized reminder storage and preserves existing
non-reminder records. Accepted reminder intent survives ordinary supported
restart; overdue reminders remain due rather than expiring. This decision grants
no new downgrade, live-database replacement or cross-version replay guarantee.

## Consequences

- **Positive:** The missing time trigger has one small durable owner and reuses
  ordinary conversation input semantics.
- **Positive:** A visible due reminder survives client disconnect and model failure.
- **Positive:** Cancelling an accepted queued input or stopping its turn cannot
  cause reminder-specific resubmission.
- **Negative:** Cancellation/rescheduling after due commitment reports already due;
  it cannot pretend to retract an observable occurrence.
- **Verification obligation:** Prove create replay, immutable due commitment,
  cancellation/reschedule races, late restart, ordinary busy queueing, continuation,
  admission replay and visible reminder behavior without reviving retired wake state.

## References

- ADR-098: retired wake execution/history policy.
- ADR-070: trusted input provenance and ordinary input admission for factual events.
- ADR-074: existing Direct Turn as restart owner.
- `../reminders/requirements.md`, `../reminders/reminders.allium`.
- `../durable-workflows/requirements.md`, `../steering-messages/steering-messages.allium`.
