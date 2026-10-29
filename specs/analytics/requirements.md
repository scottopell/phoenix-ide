# Analytics Requirements

## User need

Phoenix needs local analytics for conversations, turns, tools, costs, latency, and export without making an external telemetry system the source of truth.

## Requirements

### REQ-AN-001: Local Analytics Independence

Phoenix SHALL provide local analytics without requiring Trajectory, Datadog, or any external daemon.

### REQ-AN-002: Usage Page Preservation

Phoenix SHALL preserve the existing `/usage` page semantics while migrating it onto shared analytics projection helpers.

### REQ-AN-003: Conversation History Source of Truth

Phoenix SHALL derive analytics from persisted Phoenix conversation history and typed durable facts.

### REQ-AN-004: No Duplicate Transcript Store

Phoenix SHALL NOT persist a second full transcript or full tool I/O copy solely for analytics.

### REQ-AN-005: First-Byte Durability

Phoenix SHALL persist the server timestamp of the first streamed LLM token for token-bearing turns when that timestamp is observed.

### REQ-AN-006: Tool Parentage Projection

Phoenix SHALL project tool calls by pairing assistant tool-use blocks with tool-result messages through `tool_use_id` and turn/session membership.

### REQ-AN-007: Explicit Fidelity

Phoenix SHALL mark analytics/export fields as native, derived, estimated, unknown, or unavailable where exactness matters. V1 cost fidelity is represented by pricing-known semantics and unknown-turn counts rather than persisted cost/source columns.

### REQ-AN-008: Retry Facts

Phoenix SHOULD persist LLM attempt/retry facts when retry behavior is used in analytics or export.

### REQ-AN-009: Outcome Facts

Phoenix SHOULD persist commit/PR/yield facts when outcome attribution becomes a first-class analytics surface.

### REQ-AN-010: Trajectory-Compatible Export

Phoenix SHALL provide a Trajectory-compatible export adapter that projects Phoenix analytics sessions into an external session format without making Trajectory the source of truth.

### REQ-AN-011: Explain Capacity and Cache Boundaries

WHEN a request is planned, including a local refusal before provider dispatch
THE SYSTEM SHALL expose content-free diagnostics for selected model and route, effective context limit, preferred output reservation, retained input count and counting method, uncertainty, reduction action and amount, and classified planning outcome
AND SHALL correlate those diagnostics with provider usage and request timing when an attempt exists
AND SHALL represent absence of an attempt or provider usage explicitly rather than fabricate zero usage.

WHEN a historical request prefix changes
THE SYSTEM SHALL identify known causes among clearing, compaction, guidance refresh, model/route switch, and tool-policy change, allowing multiple causes
AND SHALL distinguish a known intentional change from unexplained observed cache loss
AND SHALL NOT claim that a cache miss's provider-internal cause or monetary cost is known without supporting evidence.

THE SYSTEM SHALL extend the existing local request metrics and usage surfaces rather than introduce a second analytics or transcript authority
AND SHALL store independently addressable durable facts relationally
AND SHALL derive aggregates without persisting prompts, reasoning, tool bodies, credentials, or a duplicate transcript.

WHEN evaluating a context policy
THE SYSTEM SHALL report input occupancy, cache reads and writes where available, output usage, counting error where comparable, provider TTFT, and semantic continuity separately
AND SHALL include cold, warm, clearing, compaction, rewarm, and model-switch cases
AND SHALL NOT equate cache hit rate, transport payload savings, or request success with preserved understanding.
