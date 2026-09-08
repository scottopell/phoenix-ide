# Add aggregate-capable ProductConversation Close API

Current `/api/conversations/:id/archive` rejects every member of a continuation chain, including its canonical root, while iOS and other ProductConversation clients need one lifecycle operation over the aggregate. Define and implement a typed ProductConversation Close endpoint keyed by aggregate authority, with the existing Close orchestration/admission/evidence owner rather than bypassing it.

## Acceptance criteria
- Continued ProductConversations have one server-authoritative Close request surface that does not route through per-conversation archive.
- The endpoint composes with the existing ProductConversation Close orchestration, settlement, WorkScope retirement, History transition, and recovery contracts.
- Single-segment and multi-segment aggregate regressions cover success, rejection, and idempotent retry.
- iOS may replace its single-segment-only Close boundary only after this endpoint is normative and shipped.

## Scope
No iOS cache migration. Do not duplicate task 92033's Close coordinator; integrate with that owner.
