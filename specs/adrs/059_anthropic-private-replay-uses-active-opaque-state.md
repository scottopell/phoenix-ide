# ADR-059: Anthropic private replay uses active opaque state

- **Status:** Accepted
- **Date:** 2026-09-24
- **Affects:** REQ-LLM-004, REQ-LLM-005, REQ-LLM-006

## Context

Claude Opus 5.5 can return signed `thinking` and encrypted `redacted_thinking` blocks during tool use. Anthropic requires clients to replay those blocks unchanged while the tool exchange remains active. The blocks are provider protocol state, not public assistant content. Phoenix also rebuilds system instructions, tools, and bounded history between provider requests, so preserved-thinking prefix binding can become stale.

Phoenix message persistence feeds browser SSE, reconnect snapshots, transcript APIs, search, and provider requests. Storing private blocks in ordinary message content would expose opaque signatures to clients and retain them after their replay obligation ends. Freezing an entire provider request prefix would avoid binding changes but would introduce a second prompt authority and defer normal Phoenix prompt/tool updates.

The replay payload is an ordered provider-owned aggregate. Phoenix always reads, writes, replays, and deletes it as a whole and never addresses block fields in SQL.

## Options considered

1. Store private blocks in ordinary transcript content and sanitize every public projection.
2. Freeze the complete Anthropic request prefix while replay is active.
3. Store only provider-private replay state and request Anthropic to drop blocks after prefix mismatch.

Option 1 creates a permanent client-privacy obligation. Option 2 creates a second prompt authority and delays normal Phoenix prompt/tool changes. Option 3 preserves crash-safe replay without letting provider binding rules dictate Phoenix prompt architecture.

## Decision

Phoenix stores Anthropic private replay material in one conversation-owned `active_provider_replay_state` row only while an unsettled exchange has private blocks requiring replay. Lifecycle metadata remains relational. The ordered private response sets use one closed, total typed JSON payload as a deliberate exception to the normal child-collection rule.

Public message types cannot represent private replay blocks. The provider request projection locates each durable owner by message identity, verifies that its public content remains unchanged, and inserts private blocks at validated original ordinals. Missing owners, rewritten owners, malformed payloads, unknown variants, missing signatures, and invalid ordinals fail explicitly before provider I/O.

Phoenix continues to rebuild normal system instructions, tools, and public history. Supported Anthropic requests enable `prefix_mismatch_behavior: "drop_block"`; Anthropic may discard signed blocks whose bound prefix changed. Phoenix records only bounded transformation counts, never private content.

Accepted provider outcomes stage replay mutation after stale-generation and reducer admission checks. Persistence stores replay with the active execution state before tools run, retains it through same-exchange retries and parked waits, and clears it with terminal or abandonment settlement. Ordinary public assistant/tool-result checkpoint timing remains unchanged.

## Consequences

- Private thinking, signatures, and encrypted data never enter SSE, browser state, public transcript content, search, or exports.
- Restart can recover replay obligations from SQLite without process-local state.
- Prefix changes can lose affected prior reasoning and cache reuse, but they do not require a frozen full-request subsystem or produce prefix-mismatch errors on supported routes.
- The opaque payload codec is part of persisted compatibility. A future incompatible representation owes a migration or backward decoder.
- Chain Q&A uses the same typed replay representation in memory and settles it before its forced-answer tool-surface change; it gains no durability contract.
