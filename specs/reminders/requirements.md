# Conversation Reminders

## User Story

As a Phoenix user, I need to ask a conversation to remind me at a future time,
leave it alone, and find the reminder and a contextual follow-up when that time
arrives, without keeping a browser or model request open.

## Requirements

### REQ-REM-001: Explicit conversation-owned reminders

WHEN a top-level ordinary conversation or the Coordinator explicitly creates a reminder
THE SYSTEM SHALL bind it to that stable ProductConversation and its trusted creating transcript, assistant message and tool invocation
AND SHALL retain its text, resolved due instant, display timezone and revision
AND SHALL require neither a repository nor a WorkScope for reminder ownership
AND SHALL reject subordinate-agent registration and arbitrary cross-conversation targeting.

### REQ-REM-002: Durable and replay-safe receipt

WHEN reminder creation succeeds
THE SYSTEM SHALL have committed the reminder before returning a receipt containing its ID, saved text and exact resolved time/zone
AND SHALL return immediately without keeping a tool call or provider request open, parking the tool round, or changing conversation execution state.

WHEN the same trusted creating transcript/assistant-message/tool-call identity is replayed with the same normalized request
THE SYSTEM SHALL return the same reminder without shifting a relative deadline
AND SHALL reject reuse of that invocation identity with a different normalized request.

### REQ-REM-003: Unambiguous time

THE SYSTEM SHALL accept a relative duration or a fully specified local date/time and IANA timezone
AND SHALL resolve time at the server boundary rather than assume the server's local timezone
AND SHALL reject new or rescheduled dates that are not in the future
AND SHALL return actionable choices for ambiguous local times and reject nonexistent local times.

THE SYSTEM SHALL retain one canonical due instant and derive local display from that instant and the saved timezone.

### REQ-REM-004: One immutable due occurrence

WHEN the server observes an elapsed scheduled reminder
THE SYSTEM SHALL atomically freeze its current revision as one durable due occurrence
AND SHALL retain the original due instant and the actual observation time
AND SHALL make the due reminder queryable independently of client connectivity or model success.

Repeated discovery and clock changes SHALL NOT refire an already committed due occurrence.

### REQ-REM-005: Ordinary input admission

WHEN a due reminder is eligible for conversation input admission
THE SYSTEM SHALL submit its immutable reminder text and scheduled provenance through the standard input admission service under a deterministic occurrence identity
AND SHALL inherit ordinary direct-delivery, steering-queue, readiness, lifecycle and continuation behavior
AND SHALL NOT impose an Idle-only condition or create a reminder-specific runtime owner, acceptance queue or settlement participant.

WHEN normal admission has already accepted the occurrence, including a steering acceptance whose live queue entry has been removed
THE SYSTEM SHALL recognize that existing acceptance and SHALL NOT submit a replacement input merely because a turn failed, was stopped, or the queued input was removed.

### REQ-REM-006: Revision-checked management

WHEN the owner reschedules or cancels a scheduled reminder with its current revision
THE SYSTEM SHALL serialize that mutation with due commitment and apply one winning result.

Rescheduling SHALL advance the revision and invalidate the old deadline.
Cancellation SHALL prevent a scheduled reminder from becoming due.
Stale revisions SHALL return the current record without overwriting it.
An already committed due occurrence SHALL retain its text, time and revision; later cancellation or rescheduling SHALL report that it is already due rather than claim retraction.

THE SYSTEM SHALL expose a bounded owner-scoped list and SHALL NOT allow one conversation to mutate another conversation's reminder.

### REQ-REM-007: Forward restart recovery

WHEN Phoenix restarts with committed reminder records in a supported database
THE SYSTEM SHALL discover elapsed scheduled reminders and due occurrences not yet admitted through ordinary input acceptance without requiring a client connection or retained in-memory timer
AND SHALL deliver late reminders with their original due instant rather than silently expire them.

The reminder migration SHALL preserve preexisting non-reminder data and retired wake history. This feature SHALL NOT resume historical wake records or grant downgrade, live-database replacement, cross-version replay or binary-only rollback guarantees beyond `../compatibility/requirements.md`.

### REQ-REM-008: Visible truth and notification capability

THE SYSTEM SHALL expose upcoming and due reminder state independently of the model's response
AND SHALL identify due inputs as scheduled reminders tied to their original request rather than freshly authored human input
AND SHALL preserve that provenance through persistence, queueing, transcript/wire transport and model rendering.

Browser notification delivery SHALL use the existing permission, settings and focus policy. Receipts SHALL state available notification capability without claiming closed-app phone delivery. A model failure SHALL NOT erase a due reminder.

### REQ-REM-009: Bounded lifecycle ownership

THE SYSTEM SHALL consume existing ProductConversation admission and continuation authority without interpreting individual Close phases or making pending reminders block Close
AND SHALL allow existing permanent aggregate deletion to remove its solely-owned reminder records.

The feature SHALL provide one-shot reminders only. It SHALL NOT schedule arbitrary executable callbacks, enroll ordinary process handles, add recurring schedules, or restore the retired wake feature.

## Related Contracts

- `../durable-workflows/requirements.md`: standard durable turn/input ownership.
- `../steering-messages/steering-messages.allium`: ordinary queued input behavior.
- `../bedrock/requirements.md` and `../work-lifecycle/requirements.md`: conversation and deletion authority.
- `../notifications/requirements.md`: browser notification permission/settings/focus.
- `../wake-contracts/requirements.md`: inert retired wake history.
- `../compatibility/requirements.md`: supported forward persistence and timestamp representation.
