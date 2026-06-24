# Analytics Design

## Source of truth

Phoenix analytics are projections over Phoenix-owned data:

- `conversations` for session identity, lifecycle, project/task/branch/worktree attribution, model, and root/sub-agent grouping;
- `messages` plus attachment child tables for user, assistant, and tool history;
- `turn_usage` for token-bearing LLM turns;
- typed fact tables for values that cannot be reconstructed from existing history.

Phoenix does not maintain a second analytics transcript or full JSONL capture log.

## Persisted shape

`turn_usage` is the canonical token-bearing turn table. It carries nullable first-byte timing metadata:

```sql
ALTER TABLE turn_usage ADD COLUMN first_byte_at TEXT;
```

`first_byte_at` is nullable. It stores the server timestamp at which Phoenix observed the first streamed text token for the LLM request. Historical rows, non-streaming paths, and failed/cancelled requests with no observed token leave it `NULL`.

Cost is not persisted. It is derived from token counts and the model pricing lookup used by `/api/usage`.

## Projection layer

The internal analytics projection builds typed records:

- `AnalyticsSession` for root-session metadata, turns, tool calls, and fidelity;
- `AnalyticsUsageTurn` for one `turn_usage` row, token totals, derived cost, first-byte timestamp, and first-byte latency when an anchor is reconstructable;
- `AnalyticsToolCall` for assistant tool-use blocks paired to tool-result messages by `tool_use_id`.

Tool-call projections reference source message ids instead of copying full input/output text. Deterministic permission denials are surfaced as typed `denied = true` by detecting the structured `command_safety_rejected` result.

## `/usage`

`GET /api/usage` remains an aggregate query over `turn_usage`, with costs derived at presentation time. `GET /api/usage/conversation/:id` remains the per-root-session timeline and includes nullable first-byte fields for drilldown display. Historical nulls render as unavailable rather than zero latency.

## Trajectory-compatible export

The export adapter is a consumer of the analytics projection. It returns a Phoenix-owned session payload with Trajectory-compatible session facts, source metadata, and fidelity metadata. It does not require a running Trajectory daemon and does not write an authoritative downstream store.

## Durable fact boundaries

Additional fact tables are introduced only when analytics surfaces require values that cannot be reconstructed from history:

- `llm_attempts` for retry/attempt analytics;
- turn lifecycle status rows for failed/cancelled/non-token-bearing turns;
- `conversation_interruptions` for interruption analytics;
- `conversation_git_yields` for commit/PR/outcome attribution;
- `analytics_privacy_events` for incognito/export suppression semantics.
