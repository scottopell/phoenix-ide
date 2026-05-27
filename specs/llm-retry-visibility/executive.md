# LLM Retry Visibility — Executive Summary

## Requirements Summary

Phoenix already retries retryable `LlmError`s
(`Network | RateLimit | ServerError`) in the state machine via
`Effect::ScheduleRetry`, with exponential backoff (1s, 2s, 4s) and
a global cap of `MAX_RETRY_ATTEMPTS = 3`. The retry count is
already on the wire via `StateChange.state.attempt`. What's missing
is *why* a retry is happening and how long until the next one — both
known to the executor at retry-schedule time but never surfaced to
the client.

This spec adds:

- **A new SSE event `LlmAttempt`** carrying `(attempt, max_attempts,
  reason, backing_off_ms, resets_at?)`, emitted from the executor's
  `Effect::ScheduleRetry` handler immediately before the spawned
  backoff sleep. Replayed via the ephemeral replay ring so a client
  reconnecting mid-backoff sees the same retry context as a
  continuously-connected client.
- **A typed `LlmAttemptReason` enum** (`RateLimit | ServerError |
  Network`) that mirrors the retryable subset of `LlmErrorKind`,
  classified by the same `is_retryable()` predicate.
- **A `display_data.retry_count: u32` field** on the persisted
  assistant message so the post-hoc "(retried Nx)" badge survives
  beyond the live retry window. Mirrors the typed-display_data
  side-channel that working-phase-visibility uses for `tool_starts`.
- **The cross-spec contract** with `specs/working-phase-visibility/`:
  this spec owns the producer side of the `RetryContext` value
  (currently inlined as PLACEHOLDER in the sibling) and the
  `TurnRetryContext` populator rule.
- **Explicit out-of-scope** for cross-conversation retry rollups,
  live "backing off Ns" countdowns, and per-provider retry policies.

## Technical Summary

Wire format changes are additive:

- New `SseWireEvent::LlmAttempt { attempt: u32, max_attempts: u32,
  reason: LlmAttemptReason, backing_off_ms: u64, resets_at:
  Option<DateTime<Utc>> }`. Snake-case event tag `"llm_attempt"`.
- New enum `LlmAttemptReason { RateLimit, ServerError, Network }`,
  shared between the runtime and the wire.
- `display_data.retry_count: u32` field on assistant-message
  `MessageDisplayData` (typed; ts-rs-exported). `#[serde(default)]`
  rollout shim for old rows that read as `0`.

Runtime changes are tightly scoped:

- `Effect::ScheduleRetry` widens from `{ delay, attempt }` to
  `{ delay, attempt, reason, resets_at }`.
- `Event::LlmError` gains `resets_at: Option<DateTime<Utc>>` (already
  carries `error_kind`).
- `llm_outcome_to_event` (`transition.rs:2373`) threads
  `error.quota.as_ref().and_then(|q| q.resets_at)` into
  `Event::LlmError.resets_at`.
- `MAX_RETRY_ATTEMPTS` becomes `pub` so the executor's
  `Effect::ScheduleRetry` handler can carry the value on the wire.
- The assistant-message persist path captures the final retry count
  into `display_data.retry_count`.

Client changes:

- Atom field `turnRetryContext: { attempt, max_attempts, reason,
  reason_text, backing_off_ms, resets_at } | null` populated by the
  `llm_attempt` reducer; cleared by `agent_done` and terminal
  `error`.
- `useConnection.ts` registers an explicit `llm_attempt` listener
  (required by native EventSource's lack of wildcard for named
  events) and bumps `lastSseEventAt` via the existing wrapper.
- `MessageComponents.tsx` renders the `(retried Nx)` badge when
  `display_data.retry_count > 0`.

No DB schema changes (`display_data` is JSON-typed; the new field
is one more typed key).

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-LRV-001:** Retry context wire event | ❌ New | `SseWireEvent::LlmAttempt` + emission from `Effect::ScheduleRetry` (`executor.rs:1408`). ts-rs regen + `parity_*` test update required |
| **REQ-LRV-002:** Retry reason classification | ❌ New | `LlmAttemptReason` enum mirrors retryable `LlmErrorKind` subset; exhaustive `match` enforces both sides |
| **REQ-LRV-003:** Cross-spec contract with working-phase-visibility | 🔄 Replace | Removes the PLACEHOLDER block in `working-phase-visibility.allium`; adds `use "../llm-retry-visibility/llm-retry-visibility.allium" as llm_retry` import; this spec owns the canonical `RetryContext` value + `TurnRetryContext` populator |
| **REQ-LRV-004:** Sub-agent retries stay local | ❌ New | Each conversation's SSE stream carries only its own retries; cross-conv rollups deferred |
| **REQ-LRV-005:** Cancellation routes through `Cancelling` | 🔄 Document | Behaviour already correct in the state machine; spec makes the contract explicit |
| **REQ-LRV-006:** Post-hoc retry badge on assistant message | ❌ New | Typed `retry_count: u32` on `MessageDisplayData`; rendered by `MessageComponents.tsx` |
| **REQ-LRV-007:** `LlmAttempt` and `RateLimitSnapshot` distinct | ❌ New | Documents the wire-level distinction; `LlmAttempt` IS in the replay ring, `RateLimitSnapshot` is NOT |

**Progress:** 0 of 7 implemented. Stage B of task 58001 in
`specs/working-phase-visibility/` is gated on this spec landing.

## Cross-Spec Dependencies

- **`specs/working-phase-visibility/`** — the consumer. Imports this
  spec via `use "../llm-retry-visibility/llm-retry-visibility.allium"
  as llm_retry`. The inlined PLACEHOLDER block (`value
  RetryContext`, `render_retry_modifier_for`, `render_frozen_retry_modifier`)
  in `working-phase-visibility.allium` is removed in the same
  change as this spec lands; the canonical `value RetryContext`
  declaration moves here.
- **`specs/sse_wire/`** — wire-format authority. `LlmAttempt` is
  added to the `EphemeralEventAppendedToReplayRing` whitelist and to
  the `PendingEventEntry.event_type` block comment. Replay semantics
  follow the standard ephemeral-event path.
- **`specs/conversation_atom/`** — no atom impact for the wire
  event itself (it populates a working-phase-visibility entity, not
  a message in the atom). The `retry_count` field on
  `display_data` is set at persist time via the existing
  `Effect::persist_agent_message` path, which conversation_atom
  already covers.

## Behavioural Specification

The corresponding Allium spec is
`specs/llm-retry-visibility/llm-retry-visibility.allium`. It models:

- `LlmAttemptReason` enum — the retryable-error classification.
- `LlmAttemptEmission` rule — the executor's emission of
  `SseEvent::LlmAttempt` from `Effect::ScheduleRetry`, including the
  precondition that the error is retryable and the attempt budget
  remains.
- `value RetryContext` — the canonical retry-context value type
  (moved here from the working-phase-visibility PLACEHOLDER).
  Exposed via `surface RetryContextProducer` so the sibling spec
  can `use` it.
- `TurnRetryContextUpdatedOnLlmAttempt` — populator rule that the
  working-phase-visibility spec consumes (its consumer-side
  derivation rules read `TurnRetryContext{view}.retry` and feed
  `render_retry_modifier_for(view)`).
- `LlmAttemptDoesNotEscapeSubAgentBoundary` invariant — REQ-LRV-004.
  The producer emits on its own conversation's stream only.
- `RetryCountPersistedAtTurnEnd` rule — captures the final
  attempt count into the assistant message's
  `display_data.retry_count` field at `LlmResponse`/persist time.

Deferred entries document the policy choices that don't need
modelling at v1 (per-provider retry policies, live backoff
countdown, cross-conversation rollups, per-tool retry badges).

Open questions: none. All decisions are documented in the design.md
"Open Questions" block and in the spec's own `Open questions` block
at the bottom of `llm-retry-visibility.allium`.
