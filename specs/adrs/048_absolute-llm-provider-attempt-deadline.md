# ADR-048: Bound each LLM provider attempt with one absolute service deadline

- **Status:** Accepted
- **Date:** 2026-09-09
- **Affects:** REQ-LLM-011, REQ-LLM-012, REQ-LLM-013; `LlmAttemptReason`

## Context

Provider adapters combine operations with different local liveness guards. The OpenAI Responses adapter can connect and stream over WebSocket, retry once on a fresh socket, and fall back to HTTP/SSE. A timeout around each connect, send, frame read, or HTTP request bounds that operation but does not bound their sequence. Frequent nonterminal frames can also make every individual frame wait succeed indefinitely.

The conversation reducer owns semantic state and the durable direct-turn aggregate owns accepted-turn identity, generation, terminal settlement, and conversation ownership. A second conversation-level timer would create competing terminal authorities. Conversely, a timeout buried in one parser or transport would not cover credential resolution, recovery, or fallback.

A timeout outcome is persisted in `llm_request_metrics`. Internal SQLite structure has no general downgrade or cross-version compatibility promise under ADR-034, but forward migration must preserve existing rows and must be explicit rather than relying on permissive string storage.

## Options considered

1. **One absolute deadline at the service boundary** — bounds the complete provider attempt while keeping durable terminal authority in the existing reducer and direct-turn aggregate.
2. **Independent transport deadlines** — simple locally, but recovery and fallback renew the logical lifetime and separate providers can drift.
3. **A conversation-level watchdog** — can bound the UI state, but creates a second terminal authority outside the generation-fenced provider-result path.
4. **Visible-text idle detection** — detects one symptom but incorrectly penalizes valid reasoning, tool, and structured generation and provides no total bound.

## Decision

`LlmServiceImpl` owns a non-optional provider-attempt deadline policy. At each `complete` or `complete_streaming` dispatch it captures one Tokio absolute deadline and awaits the complete provider future beneath it. The production duration is ten minutes per provider attempt.

The deadline includes credential resolution, provider selection, WebSocket connect/send/read, fresh-socket recovery, HTTP/SSE fallback, stream consumption, and response normalization. Provider activity does not renew it. Narrower transport timeouts remain valid early-failure diagnostics.

Deadline expiration produces exhaustive typed values:

- `LlmErrorKind::TimedOut` at the provider boundary;
- `LlmAttemptOutcome::TimedOut` in request metrics;
- `LlmOutcome::TimedOut` at the reducer boundary;
- the existing `ErrorKind::TimedOut` in durable conversation state.

The attempt capture is first-terminal-wins. Timeout, cancellation, and ordinary finalization return the already-finalized metric if another terminal path won. The metrics table is migrated forward to admit `timed_out` while copying existing rows unchanged and retaining a closed outcome constraint.

Timeout is auto-retryable through the existing finite retry policy. Retry remains within the same accepted durable turn and canonical user-message identity while each provider attempt receives its own request identity. The runtime's existing request generation rejects late results before response persistence or tool execution. Retry exhaustion uses the existing atomic direct-turn failed settlement to persist terminal state and release conversation ownership.

Dropping a timed-out provider future is the cancellation mechanism. The OpenAI WebSocket `AttemptMarker` already marks a dropped in-progress pooled session dirty, so a later request cannot reuse it as clean continuation state.

## Consequences

- An active provider stream cannot retain a conversation indefinitely.
- Three attempts can consume approximately thirty minutes plus bounded backoff before durable terminal failure; this ADR bounds each provider attempt, not the entire accepted turn.
- A provider may continue remote computation after client disconnect. Phoenix guarantees at-most-once durable commitment through generation and turn authority, not provider-side exactly-once execution.
- Existing rows keep their outcome values. Downgrade after migration is not guaranteed.
- Tests inject short policies and use Tokio virtual time; production callers cannot construct a service without a deadline policy.
- Transport-local telemetry remains content-free. Cross-transport telemetry history is a separate observability concern and is not duplicated into parallel representations by this change.

## References

- ADR-034: internal persistence has no compatibility contract
- ADR-045: provider prompts use persisted generation-fenced projections
- `LlmServiceImpl::complete` and `LlmServiceImpl::complete_streaming`
- `LlmAttemptCapture`
- `ConversationRuntime::process_generation_tagged_llm_outcome`
- `specs/llm/requirements.md`
- `specs/durable-workflows/direct-chat-profile.allium`

## Rejected alternatives

### Reset an idle timer only when visible text is absent

Rejected because reasoning, tool, and structured generation can legitimately be non-visible, and an idle heuristic does not establish a total bound.

### Put independent deadlines in WebSocket and SSE parsers

Rejected because reconnect and fallback could renew the logical request lifetime and because competing timer semantics would drift.

### Add a conversation-level watchdog

Rejected because the reducer/direct-turn aggregate already owns terminal meaning. A second authority could race provider settlement and recovery outside the generation-fenced result path.

### Classify deadline expiry as a network error

Rejected because it makes cancellation policy and operational telemetry ambiguous and permits persisted outcomes to misrepresent Phoenix-enforced liveness.

### Disable WebSocket fallback

Rejected because fallback is useful and safe before public text emission; the defect is the absent shared budget, not fallback itself.
