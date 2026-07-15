# P0: Add LLM request hang detection and retry

## Problem

A production conversation (`mobile-multi-pr-ui-fixtures`, conversation id `84d9fd99-401e-4dd5-bf0f-67ae2043fe76`) remained in persisted state:

```json
{"type":"llm_requesting","attempt":1}
```

for roughly an hour after a successful `patch` tool result. The UI showed the turn as waiting on the LLM indefinitely.

This was not a hung tool call: the final tool result had already been persisted. The next LLM request was dispatched and never produced a terminal outcome that returned the conversation to a stable state or triggered retry handling.

## Evidence from incident

- Prod HTTPS stayed responsive; this was conversation-local, not a global service outage.
- The conversation state remained `llm_requesting` since `2026-07-15T20:55:29Z`.
- There were outbound Phoenix TCP sockets to Cloudflare/OpenAI/Codex endpoints, including `ESTABLISHED` sockets and stale `CLOSE_WAIT` sockets.
- Provider code has transport-level timeouts:
  - non-streaming OpenAI: 5 minutes
  - streaming HTTP/SSE OpenAI/Anthropic: 10 minutes
  - Codex WebSocket connect: 15 seconds
  - Codex WebSocket frame wait: 10 minutes
- The executor liveness deadline currently covers only `AwaitingSubAgents`, `CancellingTool`, and `CancellingSubAgents`; it does not cover `LlmRequesting`.

## Root cause hypothesis

Provider-level timeouts are insufficient as the only guard. If the spawned LLM task wedges outside the wrapped read/send timeout, loses/drops its outcome, panics in a path not converted into an outcome, or the executor misses the outcome, `LlmRequesting` can persist forever because there is no executor/state-level backstop.

## Required behavior

Phoenix must treat `LlmRequesting` as a bounded waiting state.

## Acceptance criteria

- [ ] Add an executor-level liveness deadline for `ConvState::LlmRequesting`.
- [ ] When the deadline expires, abort/supersede the in-flight LLM generation so stale late outcomes cannot affect later turns.
- [ ] Convert the expiry into a retryable LLM outcome, e.g. `LlmOutcome::NetworkError { message: "LLM request timed out after ..." }`, so existing retry/backoff machinery runs.
- [ ] If retry budget is exhausted, surface a user-visible recoverable error instead of leaving the turn in `llm_requesting`.
- [ ] Log at warn/error level with conversation id, model id, attempt, elapsed duration, and whether Codex WebSocket vs HTTP/SSE transport is known.
- [ ] Reset/poison the Codex WebSocket session on watchdog expiry so future attempts do not reuse a suspected wedged socket.
- [ ] Add tests proving an `LlmRequesting` state cannot remain indefinitely when the LLM task never sends an outcome.
- [ ] Add tests proving late/stale LLM outcomes after watchdog expiry are ignored by generation guards.
- [ ] Confirm cancellation still works and does not double-report if user cancel races with the watchdog.

## Notes

A reasonable initial deadline should be slightly above provider streaming timeouts, e.g. 12-15 minutes, or configurable via env for production tuning. The important invariant is that no conversation can remain in `LlmRequesting` forever.
