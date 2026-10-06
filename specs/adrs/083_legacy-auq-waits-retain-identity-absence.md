# ADR-083: Legacy AUQ waits retain identity absence

- **Status:** Accepted
- **Date:** 2026-10-06
- **Affects:** REQ-AUQ-009, REQ-COMP-001; `QuestionRequestId`, `AwaitingUserResponse`

## Context

Binding answers and dismissals to a Phoenix-owned pending-question identity prevents a stale action from settling a newer question when a provider reuses its tool-use identity. Two production databases contain one pending `AwaitingUserResponse` row each that predates this identity. Those rows retain their provider tool-use identity but have no Phoenix request identity.

The upgrade must distinguish true historical absence from a newly identified wait. Fabricating identities for existing rows would make their already-rendered clients unable to answer. Accepting an absent identity for every wait would preserve old clients but defeat stale-action protection for all new questions. Retiring the existing waits would discard real pending user interactions.

## Options considered

1. **Retire identity-absent waits during upgrade** — establishes one strict protocol immediately, but loses the two pending interactions without user action.
2. **Backfill generated identities into identity-absent waits** — gives every row an identity, but clients that observed those waits before upgrade do not possess the generated value and cannot answer them.
3. **Preserve historical absence and require exact optional identity equality** — keeps existing waits answerable while making every newly created wait identity-bound; it carries a bounded compatibility branch until those historical waits settle.

## Decision

Choose option 3.

A pending wait created before request identities retains `request_id = None` in persisted state. It accepts only an answer or dismissal that also omits the identity. Phoenix does not backfill, retire, or otherwise mutate that wait during upgrade.

Every newly created wait stores `Some(QuestionRequestId)` and accepts only the exact submitted identity. A missing identity for such a wait is rejected with an actionable conflict directing the user to update Phoenix or use the web client. A stale identity is rejected as no longer current. Provider tool-use identity is not mutation authority.

This compatibility guarantee lasts for the lifetime of each persisted identity-absent wait. Once such a wait settles, no new identity-absent wait is created. Installed clients that cannot send request identity are not compatible with newly created waits.

## Consequences

- **Positive:** Existing pending interactions remain answerable without fabricating authority or mutating production rows.
- **Positive:** New waits reject tokenless and stale mutations even when provider tool-use identities repeat.
- **Negative:** Mutation handling carries an explicit legacy `None == None` branch until all historical identity-absent waits settle.
- **Negative:** Installed old clients cannot answer or dismiss new identified waits and must surface the server's update-or-web conflict.
- **Neutral:** No database migration or normalized question table is introduced; compatibility uses the existing serialized conversation state.

## References

- `specs/ask-user-question/requirements.md` REQ-AUQ-009
- `specs/compatibility/requirements.md` REQ-COMP-001
- ADR-034: Compatibility guarantees are explicit and data-aware
- `QuestionRequestId`
- `validate_question_request_identity`
