# ADR-052: Question incarnations and durable dismissal resumption

- **Status:** Accepted
- **Date:** 2026-09-14
- **Affects:** REQ-AUQ-004, REQ-AUQ-007, REQ-AUQ-011, REQ-AUQ-012, REQ-COMP-006
- **Refines:** ADR-051 request identity and dismissal resumption

## Context

Implementation review established that provider tool-use identifiers can repeat
across turns. Binding a response only to that identifier cannot distinguish two
question incarnations, even when their displayed questions are identical.

Dismissal also exposed a durable ordering requirement. Already-accepted steering
must remain paused until the user sends a new message. The normal chat admission
path appends that message behind the existing queue. Treating the last hidden
dismissal marker as a permanent pause leaves the new explicit message blocked;
a process-local bypass would lose its authorization on restart.

## Options considered

1. Continue using provider identity and a marker-based runtime exception. This
   does not establish incarnation identity or durable resumption.
2. Assign a server-generated question incarnation and represent dismissal pause
   ownership in a normalized row. Existing FIFO acceptance and drain own delivery.
3. Add operation receipts, cancellable answer revisions, and durable client drafts.
   These expand the product contract beyond the required request isolation.

## Decision

Choose option 2. Every entry into AwaitingUserResponse receives a fresh UUID
`request_id`; `tool_use_id` remains provider provenance. Mutation payloads and
consumption checks require `request_id`. No provider-only fallback is accepted.
Forward migration preserves pending question content and assigns missing durable
identities before typed state loading; live deserialization has no absence default.

A `question_dismissal_pauses` row belongs to its conversation by cascading foreign
key. Dismissal creates that row in the same transaction as its hidden transcript
marker, Idle projection, and durable turn settlement. A newly accepted explicit
queued user message removes the pause in its acceptance transaction. Deferred
creation-objective delivery is a distinct admission source and cannot remove it.
Direct user-turn materialization also removes the pause transactionally. Startup
and live queue drain consult the durable pause owner, so accepted inputs resume
in FIFO order without depending on the enqueue notification surviving.

The marker remains historical explanation for dismissal; pause rows own whether
queued work may resume. Migration creates pause ownership for a latest
question-dismissal marker on an idle conversation only when its queue is empty.
Databases without pause ownership retain FIFO eligibility for existing queued
inputs. The baseline `dd54a4fb` executor's `commit_startup_steering_queue` and
`prepare_immediate_steering_drain` resume any nonempty idle queue without checking
the dismissal marker. Legacy queue rows contain neither acceptance time nor
admission source, and conversation `updated_at` also records unrelated changes;
they cannot prove whether an explicit message was accepted after dismissal.
Preserving the established restart behavior avoids inventing that evidence or
blocking an accepted message whose wake notification was lost. New dismissals
use transactional pause ownership regardless of queue contents.
Question settlement continues
to follow ADR-036: classify a lost local SQLite result once from exact durable
evidence or close admission/publication and fail stop.

## Consequences

- Reused provider identifiers cannot authorize old clients to consume new questions.
- Existing pending questions and dismissed queues survive forward upgrade.
- All supported clients must send `request_id`; older callers receive update guidance.
- Explicit resumption preserves older queued inputs and one combined dispatch.
- No durable drafts, receipt store, or answer revision/cancellation contract is added.
