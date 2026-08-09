# ADR-027: Write-capable ProductConversations use bounded global evidence

- **Status:** Accepted
- **Date:** 2026-08-08
- **Supersedes:** ADR-022's Coordinator-exclusive placement of global evidence tools
- **Affects:** REQ-GR-004, REQ-GR-007, REQ-GR-012

## Context

ADR-022 placed bounded relational evidence, conversation history search, and singular cross-conversation messaging on the Coordinator. Ordinary coding agents consequently could not inspect earlier ProductConversations or continuation predecessors without the user manually carrying context into the active conversation.

ProductConversation unifies continuation-linked execution rows as one product aggregate and removes Direct, Work, and Branch as product-facing identity. That lifecycle simplification does not require global evidence to remain exclusive to Coordinator. The capability boundary instead needs to distinguish a write-capable parent agent, which acts for the user on an explicit turn, from restricted planning conversations and subordinate agents.

## Options considered

1. **Keep global evidence Coordinator-only.** Users would continue to relay predecessor and related-conversation evidence manually.
2. **Inject other conversations as ambient memory.** Ordinary agents would gain context without an explicit tool call, but provenance, token cost, and authority would become implicit.
3. **Give write-capable parent agents the bounded host tools.** Agents can explicitly search and inspect relevant evidence while preserving the existing read-only SQL authority, budgets, citations, and singular message acceptance path.
4. **Give every execution kind the tools.** This is uniform, but unnecessarily expands authority to restricted planning conversations and sub-agents.

## Decision

Phoenix chooses option 3. A write-capable ordinary ProductConversation receives `search_conversations`, `read_conversation`, `query_database`, and `send_conversation_message`. During the mode transition, the existing top-level Direct, Work, and Branch runtime registries represent that write-capable parent boundary. A restricted planning conversation receives the tools only when it transitions to write capability. Sub-agents never receive them.

The Coordinator uses the same four tool implementations and additionally retains `resolve_reference`, its bounded relational snapshot, and optional WorkScope-targeted sandboxed Bash. Global evidence remains explicit and host-bound; it is not injected into prompts as ambient memory and does not run autonomously.

`query_database` retains one-statement read-only SQLite authority and all existing object, function, work, row, column, byte, and duration limits. `send_conversation_message` retains one non-empty text target per call and delegates to the ordinary chat acceptance service. It rejects the originating conversation, Coordinator-chain members, and sub-agents before dispatch where applicable.

## Consequences

- **Positive:** A coding agent can discover and inspect predecessors and related ProductConversations without manual transcript transfer.
- **Positive:** Coordinator and write-capable ordinary agents share one implementation of each global capability.
- **Positive:** Restricted planning conversations and sub-agents retain their smaller authority boundary.
- **Positive:** The ProductConversation refactor can replace transitional mode names without changing the durable capability decision.
- **Negative:** Operator-level database visibility, including sensitive stored records, is available to more parent agents.
- **Negative:** Cross-conversation messaging permits recursive agent interaction, bounded by explicit turns, singular calls, target eligibility, and authoritative recipient acceptance.
- **Neutral:** Internal `coordinator_*` query names may remain until mode-adjacent cleanup; they do not define product authority.

## References

- ADR-022
- ADR-026
- `specs/global-recall/requirements.md`
- `coordinator_tools::writing_tools`
- `ToolRegistryExecutor::upgrade_to_work_mode`
