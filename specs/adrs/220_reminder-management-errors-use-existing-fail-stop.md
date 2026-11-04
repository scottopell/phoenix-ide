# ADR-220: Reminder management errors use existing fail-stop authority

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-REM-006, REQ-DWF-043, REQ-BED-033

## Context

Reminder cancellation and rescheduling return typed results when their SQLite
commands finish normally. After a library error, a reminder snapshot cannot
identify which caller committed a mutation. Distinct cancellations can share an
expected revision and wall-clock microsecond; distinct reschedules can share an
expected revision, due instant and timezone. Matching those values does not
establish the failed command's result.

REQ-DWF-043 permits one exact classification query but does not require recovery
when that query cannot establish the necessary command authority. Creation has
an immutable trusted invocation identity and request fingerprint; management
does not have a separately persisted per-command identity.

## Options considered

1. Infer management success from matching timestamps, revision or schedule values.
   This cannot distinguish the winning command from another matching request.
2. Persist another management command identity or journal. This adds storage and
   a recovery contract that reminder management does not require.
3. Preserve normal typed results and use existing fail-stop authority when a
   management command returns no classified result.

## Decision

Choose option 3. Cancel and reschedule retain their normal typed Updated,
revision-conflict and terminal-state results. Known validation, target and
lifecycle refusals remain established errors. SQLite, serialization and other
unrecognized infrastructure errors become DurableFactUnclassified; the existing
ReminderWritePass closes local authority. No management snapshot query infers
command ownership.

Creation retains its invocation/fingerprint replay query. Promotion already uses
fail-stop for unclassified results. All four writes retain abandoned-owner
protection. No additional command ID, journal, schema, wire or client field is
introduced, and no new compatibility or recovery guarantee is granted.

## Consequences

- **Positive:** A competing command's matching snapshot cannot be reported as
  this command's success, and reminder storage remains limited to saved intent.
- **Negative:** A management infrastructure error can stop the serving process
  even when a later read would be available. A restarted process reconstructs
  committed reminder facts under the existing persistence contract.

## References

- `../reminders/requirements.md`, `../durable-workflows/requirements.md`,
  `../bedrock/requirements.md`.
- `ReminderWritePass`, `classify_management_result`, `create_command`.
