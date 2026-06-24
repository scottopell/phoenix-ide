# Analytics Executive Summary

Phoenix owns its local analytics model. Conversation history and `turn_usage` are the source of truth; Trajectory is an optional downstream projection target, not an upstream capture path.

| Requirement | Summary | Implementation surface |
| --- | --- | --- |
| REQ-AN-001 | Local analytics work without external daemons | `/api/usage`, analytics projection module |
| REQ-AN-002 | Preserve `/usage` behavior | `api::usage` aggregate and drilldown endpoints |
| REQ-AN-003 | Derive from Phoenix history | `analytics::project_session` reads conversations/messages/turn_usage |
| REQ-AN-004 | No duplicate transcript | Analytics records reference source message ids |
| REQ-AN-005 | Persist first-byte timestamp | `turn_usage.first_byte_at` |
| REQ-AN-006 | Pair tool calls/results | `AnalyticsToolCall` projection via `tool_use_id` |
| REQ-AN-007 | Expose fidelity | `AnalyticsFidelity` and pricing-known usage fields |
| REQ-AN-008 | Retry facts deferred until first-class use | Follow-up deferral task |
| REQ-AN-009 | Outcome facts deferred until first-class use | Follow-up deferral task |
| REQ-AN-010 | Export from projection | Trajectory-compatible export endpoint |
