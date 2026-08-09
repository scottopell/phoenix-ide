# ADR-030: iOS prose-review authority survives the composer handoff

- **Status:** Accepted
- **Date:** 2026-08-09
- **Affects:** REQ-IOS-002, REQ-IOS-003, REQ-IOS-021; `ProseReviewAuthority`

## Context

ADR-029 makes reader notes session-scoped and transfers their formatted text to
the editable conversation composer. That transfer clears the reader notes and
their exact `WorkScope`, file, and content-revision bindings before the user
actually submits the message. The user may edit the draft while the live message
target or file revision changes, so formatted path and line text alone cannot
prove that the eventual message still refers to current, agent-accessible
content.

The ordinary local message queue remains the durable delivery owner. The prose
review flow needs to retain authority until queue submission without creating a
second message or comment delivery lifecycle.

## Options considered

1. **Retain ADR-029's handoff-only binding** — keeps the composer simple, but
   loses the facts needed to revalidate the review at actual submission.
2. **Treat formatted path and line text as authority** — requires no additional
   draft state, but cannot distinguish current content from stale text and makes
   invalid review targets representable.
3. **Attach typed review authority to the editable draft** — preserves each
   injected bundle's exact scope, file, and revision binding across editing,
   revalidates every binding at submission, and still delegates durable delivery
   to the ordinary queue.

## Decision

Choose typed draft authority. Each reader-to-composer handoff creates a typed
`ProseReviewAuthority` associated with the injected review contribution and
carrying its exact `WorkScope`, file identity, and content revision. Editing the
visible draft does not remove that authority.

Actual message submission revalidates every attached authority against ordinary
chat capability, the server-declared live message target, and the current file
revision. A valid draft enters the ordinary durable message queue and releases
its review authority only after queue acceptance. A failed revalidation changes
neither the draft nor its authority; the user may refresh and re-anchor the
affected contribution or explicitly remove that contribution while preserving
unrelated draft text.

## Consequences

- **Positive:** Scope and revision safety survive arbitrary composer editing and
  are checked at the last point before durable submission.
- **Negative:** The composer must retain typed metadata for review contributions
  and provide recovery UI when an authority becomes invalid.
- **Neutral:** Reader notes remain session-scoped, and the ordinary local message
  queue still exclusively owns delivery, retry, and reconciliation.

## References

- ADR-029, superseded by this decision.
- ADR-026, for ProductConversation lifecycle and WorkScope ownership boundaries.
- `specs/ios_client/requirements.md`
- `specs/ios_client/ios_prose_feedback.allium`
- `specs/prose-feedback/requirements.md`, especially REQ-PF-009 through REQ-PF-011.
- `specs/user_message_queue/user_message_queue.allium`
