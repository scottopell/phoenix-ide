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
| REQ-AN-011 | Explain capacity and cache boundaries | Relational prepared-plan diagnostics and existing `/usage` overview/conversation projections; expandable capacity UI |

REQ-AN-011 has source-ready backend diagnostics and a capacity section in the existing Usage overview and conversation drilldown. The UI labels its bounded recent sample, separates local refusals from unknown dispatch, distinguishes conservative local estimates, direct provider counts, and Anthropic estimates, and preserves missing observations separately from reported zero. Expandable details show full input/output allocation, reduction, comparable counting errors, planning time, provider TTFT, observed cache usage, known prefix-change causes, and exact preparation identity. Complete input already includes cached input. Prefix changes do not establish provider cache-miss causes or preserved understanding.

Deterministic UI tests cover bounded sampling, missing versus zero usage/timing, complete-input accounting, local refusal versus unknown ready dispatch, multiple prefix-change causes, counting sources, and overview/drilldown integration. Shared typecheck/build and browser qualification remain integration gates; no production deployment, measured cache evaluation, or semantic-continuity acceptance is claimed. Task 44018 tracks the broader implementation and qualification.
