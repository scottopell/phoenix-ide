# LLM Retry Visibility — Executive Summary

## Current Reality

Phoenix exposes retry scheduling through the typed, replay-eligible `LlmAttempt` SSE event. The event carries `attempt`, `max_attempts`, `reason`, `backing_off_ms`, and optional `resets_at`; clients maintain per-conversation retry context and clear it at turn completion or terminal failure.

Two bounded policies feed that event:

- Generic transient failures (`RateLimit`, `ServerError`, `Network`, `TimedOut`) retain 3 total attempts with nominal 2-second and 4-second waits.
- Selected-model capacity uses `ServerOverloaded`, 5 total attempts, nominal 4/8/16/32-second waits with deterministic ±25% jitter, and an absolute 120-second window. A valid `Retry-After` of at most 30 seconds is a floor; a larger hint stops automatic retry visibly.

The overload retry state is durable. Its logical target, target attempt, waiting `retry_at`, first-overload time, deadline, and waiting/in-flight phase survive process restart. Startup rearms a future wait once, dispatches a due or in-flight target once, or expires the operation without dispatch when its original deadline has elapsed.

The StateBar renders `server overloaded`, the target attempt and maximum, and a live remaining-backoff countdown. Restart and reconnect reconstruct the same visible retry from persisted state and the rebroadcast remaining wait. Exhaustion clears the countdown and settles an ordinary request as `Error` or a continuation-summary request as `RecoverableContinuationFailure`.

Cancellation and Close retire retry-timer and provider-admission authority; stale timeout or provider outcomes cannot recreate retry visibility. Automatic continuation opening consumes the request's single overload loop rather than creating another loop.

Assistant messages retain `display_data.retry_count` for the post-turn `(retried Nx)` badge. Retry events remain scoped to their own conversation; parent views do not aggregate sub-agent retry state.

## Status

| Requirement | Status | Current evidence |
| --- | --- | --- |
| REQ-LRV-001 Retry context wire event | Complete | `SseWireEvent::LlmAttempt`, `SseEvent::LlmAttempt`, `Effect::ScheduleRetry` |
| REQ-LRV-002 Retry reason classification | Complete | `LlmAttemptReason` includes generic reasons and `ServerOverloaded`; event carries policy-specific maximum |
| REQ-LRV-003 Working-phase projection | Complete | Per-conversation `TurnRetryContext` drives StateBar retry presentation |
| REQ-LRV-004 Sub-agent locality | Complete | Retry event is emitted on the owning conversation broadcaster |
| REQ-LRV-005 Cancellation and Close retirement | Complete | Retry generation and provider generation invalidate stale outcomes |
| REQ-LRV-006 Persisted retry badge | Complete | Assistant message `display_data.retry_count` |
| REQ-LRV-007 Distinct quota and retry events | Complete | `RateLimitSnapshot` and `LlmAttempt` remain separate wire variants |
| REQ-LRV-008 Overload countdown | Complete | `ServerOverloaded` retry context renders attempt/max and remaining wait |

## Boundaries

- `specs/llm/` owns error taxonomy and retry policy.
- `specs/sse_wire/` owns event envelopes and replay-ring admission.
- `specs/working-phase-visibility/` consumes retry context for StateBar composition.
- This spec owns the retry event payload, client retry-context lifecycle, countdown semantics, and persisted retry-count badge.
