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
