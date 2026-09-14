# ADR-053: Question interactions bind explicit answers to request identity

- **Status:** Accepted
- **Date:** 2026-09-14
- **Affects:** REQ-AUQ-001, REQ-AUQ-002, REQ-AUQ-003, REQ-AUQ-004,
  REQ-AUQ-007, REQ-AUQ-009, REQ-AUQ-010, REQ-AUQ-011, REQ-AUQ-012,
  REQ-AUQ-013, REQ-KB-001, REQ-KB-004, REQ-KB-008, REQ-COMP-006

## Context

The question panel audit reproduced custom-answer omission, stale previews,
offscreen focus, unavailable notes, unlabeled choices, and inefficient long-text
layout. The existing panel combines virtual focus, hover previews, and timed
advancement. Its persisted pending state owns `tool_use_id`, but response and
dismissal callers name only the conversation, allowing stale operations to target
a subsequent question set. A responsive visual redesign therefore also requires
an explicit mutation boundary to preserve the answer the user intended to send.
The user approved revision 3 of the audited proposal for full implementation.

## Options considered

1. **Repair isolated visual defects:** lower immediate cost, but preserve the
   virtual keyboard model and ambiguous request targeting.
2. **Native explicit form with request identity:** unify answering across desktop
   and narrow browser panes, keep selected previews and per-question notes, and
   bind mutations to the existing tool request. This requires all supported
   callers to adapt together.
3. **Durable drafts and authoritative operation receipts:** allow richer offline
   recovery and editing after uncertainty, at the cost of storage, ownership,
   retention, and cross-device conflict contracts.

## Decision

Choose the native explicit form and request-bound mutation model. Selection does
not send or advance. Previews follow selection, all answer modes accept notes,
and custom answer inclusion is explicit. An 840 px container breakpoint fits
readable side-by-side columns; narrow panes keep choices stationary above one
preview. One answer-body scroller and an in-flow footer at short heights avoid
competing scrolling regions. Native keyboard behavior replaces timed advancement
and Tab-based wizard navigation.

Reuse `(conversation ID, tool_use_id)` through clients, API admission, and the
consuming transition. Missing identity is an actionable no-mutation rejection,
not a legacy fallback. Bind asynchronous completion and focus effects to the
originating request, so a fast next question cannot be erased by an earlier POST.

An uncertain operation retains its frozen snapshot. Explicit retries repeat only
that snapshot or dismissal identity. A rejection of one retry cannot establish
that an earlier attempt did not commit; editing remains locked until authoritative
resolution or proof all attempts cannot mutate. This bounded policy avoids
adding a receipt store or automatic mutation retry engine to a UX improvement.

## Consequences

- **Positive:** Answer payload, preview, and user-visible inclusion agree;
  keyboard behavior is predictable; stale requests cannot consume new questions.
- **Negative:** Web, CLI, and native iOS require coordinated protocol adaptation.
  Identity-free clients receive reload/update errors. Unresolved transport
  uncertainty can keep editing locked; leaving can discard local drafts.
- **Neutral:** Native iOS receives protocol qualification without the browser
  visual redesign. Durable drafts, offline synchronization, draggable splitters,
  detached previews, and revise/cancel-after-uncertainty are excluded. Approval
  covers implementation, not merge or production deployment.

## References

- [Approved proposal](../../docs/proposals/ask-user-question.md)
- [Ask User Question requirements](../ask-user-question/requirements.md)
- [Keyboard interaction requirements](../keyboard-interaction/requirements.md)
- [Compatibility requirements](../compatibility/requirements.md)
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- `ConvState::AwaitingUserResponse`, `QuestionPanel`, `respond_to_question`,
  `dismiss_question`
