# ADR-113: Coordinator API recipes are non-exhaustive

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-GR-007; REQ-AUQ-009; `PhoenixApiCreationRecipe`, `PhoenixApiDestinationAuthority`

## Context

The Coordinator's built-in API skill supplies careful lifecycle recipes, but its source-inspection restriction and the generated prompt's “documented operations” language can turn examples into an accidental endpoint allowlist. The public structured-question response route is a concrete supported operation omitted from those recipes. Its request identity prevents stale answers; it does not provide an idempotent replay receipt or enforce answer completeness.

## Options considered

1. **Exhaustive recipe allowlist:** require a guidance release for every supported public operation; easy to enumerate but unnecessarily blocks user-authorized APIs and discourages contract inspection.
2. **Unrestricted API improvisation:** allow inferred endpoints and retry semantics; flexible but loses deployment, authority, and safety boundaries.
3. **Non-exhaustive, contract-grounded recipes:** allow supported public operations after inspecting applicable deployment/source contracts and establishing user authority, preserving all existing transport and mutation boundaries.

## Decision

Choose option 3. Treat recipes as best practices, not an endpoint allowlist. Source inspection establishes facts, never user authority or proof that local source matches the selected deployment. Keep explicit active WorkScope execution, separate destination authority, authenticated encrypted nonlocal transport, destination-bound nonserialized credentials, TLS verification, immutable targets and intent, endpoint-specific idempotency, fresh state reads, and server enforcement. Private handlers and database mutation are not substitutes for public APIs. Gap inventories are illustrative and qualified by inspected deployment/source, not categorical denylists: verified supported alternatives and bounded, deadline-limited state reads remain available within user authority. Never invent guarantees or receipts, bypass enforcement, or create background monitors to substitute for missing server semantics.

Document question responses as submitting exact user-provided answers or decisions made within explicit delegated decision scope for every exact full question text, using the pending opaque request identity rather than the provider tool-use identity. Generic inspect/unblock instructions are not decision authority. Never fabricate approvals or extend delegation; ask the user when authority is missing. Require a fresh matching read, complete answers, one POST, and resulting-state observation. After uncertainty retain the original target/identity, do not automatically replay or transplant answers, and do not treat 409 as success. Optional notes are supported; submitted annotation previews are not consumed by the response transition.

## Consequences

- **Positive:** supported public APIs remain usable without an exhaustive recipe catalog; the question recipe captures concrete wire and decision-authority pitfalls.
- **Negative:** operators must establish deployment/source correspondence and exact user answers or explicit delegated decision scope; uncertain non-idempotent responses may need explicit follow-up rather than automatic retry.
- **Neutral:** guidance changes no runtime behavior, tool grants, authorization enforcement, deployment state, or compatibility guarantee.

## References

- [Coordinator requirements](../global-recall/requirements.md), REQ-GR-007.
- [Question requirements](../ask-user-question/requirements.md), REQ-AUQ-009; ADR-082 preserves legacy identity absence without extending this recipe to legacy requests.
- `respond_to_question`, `RespondToQuestionPayload`, `validate_question_request_identity`, `UserQuestionResponse`, `QuestionPanel.buildAnswerMap`.
