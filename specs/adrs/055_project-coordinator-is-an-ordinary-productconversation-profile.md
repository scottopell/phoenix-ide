# ADR-055: Project Coordinator is an ordinary ProductConversation profile

- **Status:** Accepted
- **Date:** 2026-09-17
- **Affects:** REQ-PCO-001 through REQ-PCO-007; `ProductConversation`, continuation compaction, provider prompt composition

## Context

Phoenix already has two relevant but distinct foundations. Ordinary ProductConversations own durable user-facing identity and continuation lifecycle, while the singleton Global Coordinator is a privileged Phoenix-wide identity with specialized prompt and tool behavior. Users also need an ordinary conversation to retain project-specific coordination instructions without turning it into another global identity or copying mutable instructions into every transcript segment.

The charter must be human-editable and authoritative across restart and continuation. Existing same-user HTTP authentication can identify an authenticated client but cannot prove that a biological human, rather than software holding the same capability, initiated a request. The product can structurally exclude charter writes from its supported LLM tools and message-processing paths without inventing actor attestation.

## Options considered

1. **Reuse the Global Coordinator kind or runtime role.** This would inherit singleton assumptions and privileged prompt/tool behavior, conflating an ordinary opt-in purpose with Phoenix-wide coordination authority.
2. **Copy the charter into opening messages or continuation handoffs.** This would make stale transcript copies compete with the editable charter and make a later edit ineffective until another handoff.
3. **Store profile and charter in a schemaless settings blob.** This would detach the data from its ProductConversation owner and encode addressable fields in serde rather than relational schema.
4. **Add a structured policy model with approvals or actor provenance.** This could express more rules or stronger claims but introduces a policy/authorization framework beyond the required trust boundary.
5. **Use a normalized optional profile owned by an ordinary ProductConversation.** Row existence represents opt-in, one scalar column stores the sole charter, a revision fences concurrent human saves, turns resolve it through stable aggregate identity, and compaction selects role-appropriate wording without receiving the charter.

## Decision

Choose option 5.

A Project Coordinator is an optional profile of an `ordinary` ProductConversation. It does not create another ProductConversation kind, runtime role, lifecycle, hierarchy, singleton, or permission set. Multiple ordinary aggregates may independently carry the profile. The existing Global Coordinator remains structurally separate and ineligible for the profile.

Persist the profile as a one-to-one normalized child of `product_conversations`. Row existence is the only opt-in representation. The row contains one plain-text charter, a monotonic revision used for compare-and-swap saves, and an integer Unix-microsecond update timestamp. Existing databases acquire no rows, so existing behavior remains default-off. Supported forward migration preserves valid rows; downgrade and rollback retain the project-wide offline boundaries from ADR-034.

Fresh ordinary turns resolve the profile from `Conversation.product_conversation_id`, append concise generic coordination guidance, and append the current charter as a distinct provider system block. No transcript message or handoff stores another charter representation. The continuation operation and generation fences remain unchanged; profile presence selects coordination-oriented summarization instructions that preserve evolving mission and delivery state while explicitly excluding charter reproduction.

The mutation surface is one authenticated human-facing HTTP settings action with optimistic revision fencing. Phoenix registers no LLM tool for it and does not connect chat, assistant messages, tool calls, tool results, Global Coordinator capabilities, or Project Coordinator behavior to the write path. This supports a narrow claim about Phoenix's supported LLM mutation surfaces. It does not attest biological-human presence or defend against arbitrary misuse of an authenticated HTTP client.

## Consequences

- **Positive:** Stable aggregate identity owns one charter authority across restart and continuation.
- **Positive:** Ordinary lifecycle, WorkScope, mode, tool admission, and permissions remain unchanged by construction.
- **Positive:** Compare-and-swap saves expose concurrent edits rather than silently overwriting them.
- **Positive:** Generic product guidance remains independent of Phoenix repositories, roadmap systems, providers, and machine policy.
- **Negative:** A caller holding the user's authenticated HTTP capability can invoke the endpoint; stronger actor provenance would require a separately approved authorization design.
- **Negative:** Prompt construction performs one ProductConversation profile read for each fresh provider request unless later optimization earns a generation-fenced aggregate projection.
- **Neutral:** Removing the optional row on opt-out also removes its charter; no disabled-charter archive is retained.

## References

- `specs/project-coordinator/requirements.md`
- ADR-025: continuation compaction is an idempotent durable operation
- ADR-026: ProductConversation lifecycle is separate from WorkScope resource ownership
- ADR-031: ProductConversation persistence uses staged single authority
- ADR-034: compatibility guarantees are explicit and data-aware
- ADR-045: provider prompts use persisted generation-fenced projections
- ADR-046: ProductConversation owns aggregate presentation without duplicating transcript authority
- ADR-049: compaction protects accepted handoffs and Coordinator context
