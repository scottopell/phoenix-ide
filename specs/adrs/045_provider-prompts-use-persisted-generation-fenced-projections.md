# ADR-045: Provider prompts use persisted generation-fenced projections

- **Status:** Accepted
- **Date:** 2026-08-30
- **Affects:** REQ-BED-018A, REQ-BED-020, REQ-BED-030A

## Context

Provider requests need complete message content, including normalized attachment children, but repeatedly loading an entire growing transcript before every LLM round makes each round proportional to total history. Keeping a process-local message vector as prompt authority avoids those reads but creates a second transcript representation that can diverge after restart, wake adoption, ownership transfer, or an in-place message mutation.

A cursor alone is insufficient. Appends can be read after the cursor, but ownership and prompt-visible row changes can alter history behind it. Any projection also has to freeze before asynchronous provider work starts, including continuation-summary generation, or a background history read can race with later persistence and send a prompt that did not correspond to one admitted durable state.

## Options considered

1. **Reload full durable history for every request** — preserves database authority but performs parent and child work proportional to total transcript length on every round.
2. **Use the runtime's accumulated messages as authority** — makes steady-state dispatch cheap, but restart and durable mutations can produce prompts that differ from committed history.
3. **Snapshot once, then consume generation-fenced durable tails** — hydrate one transactional snapshot, append only rows beyond a durable cursor while generation is unchanged, and rebuild after any non-append prompt mutation. This retains database authority and bounds steady-state reads, at the cost of explicit mutation invalidation and sequence invariants.

## Decision

Phoenix derives provider-visible transcript history exclusively from persisted message parents and normalized attachment children. A runtime hydrates one transactionally consistent snapshot of transcript generation, ordered parents, and children, then reads only rows beyond its cursor while the generation fence remains current. Parent and attachment hydration use a constant query shape independent of transcript length.

Conversation-local persisted message sequences are unique and strictly increasing for new inserts. Released databases are migrated by preserving all rows, relocating only duplicate later identities above the prior maximum, then installing schema-level uniqueness and monotonic-insert enforcement.

Every prompt-visible non-append mutation atomically advances transcript generation for each affected conversation. Ownership transfer advances both source and destination generations. A stale projection is rebuilt from durable authority once; malformed provider-bound rows or failed snapshot, tail, or rebuild reads fail closed through the typed LLM error path rather than dispatching substituted or partial history.

Ordinary and continuation provider requests render an owned provider-neutral request directly from the borrowed frozen projection before the provider task is spawned. Continuation compacts the current transcript member only; ProductConversation membership and continuation topology do not flatten all member transcripts into one provider prompt.

## Consequences

- **Positive:** Steady-state prompt reads are bounded by newly appended rows rather than total history.
- **Positive:** Database rows remain the sole provider-prompt authority across restart, adoption, transfer, and in-place mutation.
- **Positive:** Transactional snapshots prevent mixed generations, parent sets, and attachment children.
- **Positive:** Strict provider-bound decoding prevents recovery-oriented substitutions from reaching an LLM.
- **Negative:** Every prompt-visible ownership or in-place mutation must participate in generation invalidation in the same transaction.
- **Negative:** The append schema rejects callers that attempt duplicate or behind-cursor sequences even if their process-local broadcaster would otherwise tolerate them.

## References

- `specs/bedrock/requirements.md`
- `specs/bedrock/executive.md`
- ADR-025 (continuation compaction is an idempotent durable operation)
- ADR-031 (ProductConversation persistence uses staged single authority)
- ADR-034 (compatibility guarantees are explicit and data-aware)
