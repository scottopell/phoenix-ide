# ADR-049: Compaction protects accepted handoffs and Coordinator context

- **Status:** Accepted
- **Date:** 2026-09-13
- **Affects:** REQ-BED-020, REQ-GR-011, REQ-GR-013

## Context

The continuation pipeline summarizes a bounded newest suffix of the current transcript member. Repeated compaction can evict the opening handoff that contains unresolved work from earlier members. The accepted opening text may include user edits and differs from the predecessor's generated summary. Prompt reset and generation fencing also determine which text remains legitimate input.

The global Coordinator is the user's interface across several workstreams and delegates, but its summary instructions describe a single coding session in a working directory. Ordinary chats can act as project coordinators informally; that role is not a distinct product identity.

## Options considered

1. **Change only the Coordinator prompt.** Small, but cannot preserve context absent from the summarizer's input.
2. **Reserve a bounded fraction for an older handoff and clip it when necessary.** Retains more recent history, but silently sacrifices parts of the very baseline meant to survive compaction.
3. **Protect the full accepted handoff and specialize only the global Coordinator instructions.** Gives both ordinary and Coordinator conversations the same input protection, with explicit failure when it cannot fit.
4. **Add a purpose selector or separate commitment store.** Supports broader role modeling or durable memory, but introduces authority and lifecycle machinery beyond the requested change.

## Decision

Use option 3. Every continuation-capable parent conversation protects the full actual accepted predecessor handoff when acceptance provenance proves its message identity and that message remains in the frozen current-member prompt projection. Use the accepted text, including user edits, exactly once by message identity. Allocate the remaining request budget to the newest remaining history. Do not flatten older transcript members into the request or resurrect reset text.

If acceptance provenance is missing or the accepted message is absent from the frozen projection, use bounded history with an explicit missing-protection notice; do not guess from the first user message or substitute the predecessor's generated summary. If the full protected text and mandatory overhead cannot fit provider token or request-shape limits, fail through the existing recoverable continuation path before provider dispatch. There is no clipping percentage, secondary summarization, alternate memory store, or automatic fallback.

The runtime's existing Coordinator identity selects coordination instructions. Ordinary handoff instructions remain unchanged. The Coordinator instructions preserve unresolved obligations, delegation relationships, evidence, corrections, scoped authority, and relevant implementation work, while distinguishing delivery acceptance from completion and delegate requests from user instructions. Completed detail yields to unresolved work. Summary generation remains tool-free and does not receive the live activity snapshot; the resumed Coordinator refreshes relevant current status on its ordinary turn.

No purpose setting, ad-hoc project-coordinator classification, new persistence field, capability, or background monitoring is added. The existing durable operation and stale-result fencing remain shared. This decision does not establish a cross-version prompt-replay guarantee beyond the compatibility requirements.

## Consequences

- **Positive:** The summarizer receives the accepted baseline across successive compactions, including user edits, without a second mutable representation of memory.
- **Positive:** Global coordination instructions reflect ownership and obligations rather than assuming one repository task.
- **Negative:** A large accepted handoff reduces space for recent history and can prevent compaction until the request can fit. Failure remains visible and recoverable; no lossy rescue path is supplied.
- **Negative:** Full input protection cannot guarantee that the LLM retains every fact in its output or restore already-lost context. Repeated-compaction evaluation remains necessary.
- **Neutral:** Ordinary chats receive the retention improvement without acquiring a coordination role or different handoff instructions.

## References

- [Bedrock requirements](../bedrock/requirements.md), REQ-BED-020
- [Coordinator requirements](../global-recall/requirements.md), REQ-GR-011 and REQ-GR-013
- [ADR-025](025_continuation-compaction-is-an-idempotent-durable-operation.md): durable continuation operation
- [ADR-045](045_provider-prompts-use-persisted-generation-fenced-projections.md): prompt projection authority
- [Compatibility requirements](../compatibility/requirements.md)
- `ConversationRuntime::request_continuation`, `plan_continuation_history`, `ProductConversation`
