# ADR-054: Automatic continuation admission binds consent and context authority

- **Status:** Accepted
- **Date:** 2026-09-18
- **Affects:** REQ-BED-021, REQ-API-029, REQ-CONV-024

## Context

A generated continuation summary can be accepted automatically only when the user has opted in for the stable ProductConversation or Coordinator aggregate. The preference can change concurrently with summary completion, and generated model text must not acquire the authority of a user-authored instruction merely because it enters the existing continuation dispatch path.

An in-memory callback after context exhaustion would lose work across restart and could retroactively admit an already-exhausted transcript after a later preference change. A separate automatic successor constructor would duplicate the manual continuation topology, WorkScope transfer, and dispatch semantics. Copying the generated handoff into a second admission payload would create two authorities for the same accepted bytes before dispatch.

## Options considered

1. **React or runtime observation starts continuation after seeing context exhaustion** — simple, but consent and admission are not atomic with exhaustion and recovery can manufacture retroactive work.
2. **A separate automatic continuation workflow creates and dispatches the successor** — durable, but duplicates the existing continuation reservation and opening-intent operation.
3. **Commit a normalized automatic admission with context exhaustion, then drive the existing continuation operation** — makes consent sampling durable and prospective while retaining one successor and dispatch path.

## Decision

Choose option 3.

The stable `product_conversations` row owns one default-disabled preference for both ordinary ProductConversations and the Coordinator aggregate. The transaction that commits the continuation summary and `ContextExhausted` state samples that preference and, only when lifecycle and Close fences permit, inserts one automatic admission keyed by the exhausted predecessor.

The admission references the committed continuation summary message and stores deterministic opening-message identity, phase, retry evidence, and `generated_predecessor_context` authority. It does not copy the summary text. Enabling the preference after exhaustion cannot create an admission; disabling it after admission cannot revoke the accepted operation.

Successor reservation and opening dispatch continue through the existing manual continuation operation. Manual intents carry `user_authorized_instruction`; automatic intents carry the admission's `generated_predecessor_context`. Provider projection must preserve that distinction so generated predecessor context cannot approve work or issue a new user command.

## Consequences

- **Positive:** Preference sampling, context exhaustion, summary identity, and automatic admission have one crash-safe linearization point.
- **Positive:** Restart recovery can enumerate only admitted operations and cannot infer consent from historical exhausted rows.
- **Positive:** Manual and automatic races converge through existing continuation topology and dispatch idempotency.
- **Positive:** Generated handoff text has one pre-dispatch authority and explicit non-user provenance.
- **Negative:** Automatic continuation needs a normalized operation record and phase reconciliation in addition to the existing transient dispatch intent.
- **Neutral:** Existing aggregates remain disabled and existing continuation intents migrate as user-authorized.

## References

- ADR-025: Continuation compaction is an idempotent durable operation
- ADR-026: Product conversation lifecycle is separate from WorkScope resource ownership
- ADR-031: ProductConversation persistence uses staged single authority
- ADR-045: Provider prompts use persisted generation-fenced projections
- ADR-046: ProductConversation owns aggregate presentation
- ADR-053: Invalid continuation intents retire without fabricated identity
- `specs/bedrock/requirements.md`
- `specs/bedrock/bedrock.allium`
