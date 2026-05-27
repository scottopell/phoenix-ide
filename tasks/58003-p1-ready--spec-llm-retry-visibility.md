# Spec: llm-retry-visibility (sibling spec to working-phase-visibility)

## Problem

LLM retries (rate-limits, 5xx, network errors) currently happen
silently inside the executor's retry loop. A 45-second "thinking..."
silence could be a slow model call, a retry storm, or a wedged
server — the user has no way to tell. REQ-WPV-003 (in the sibling
spec, task 58002) requires retry to surface as a modifier on the
phase base reason: `executing bash 12s (retry 2/5 after 429)`.

This task drafts the sibling spec that owns:

- The wire-format change (new SSE event `LlmAttempt` or similar) for
  retry visibility
- The producer-side rules describing when/how it's emitted from the
  executor's `Effect::ScheduleRetry` handler
- The cross-spec contract with working-phase-visibility's
  `TurnRetryContext` and `RetryContext` value (currently inlined as
  `PLACEHOLDER` in working-phase-visibility.allium)

## Grounded findings from producer-side code reads

The retry-loop architecture is a common misread. Findings below are
from actual reads of `crates/phoenix-ide/src/llm/` and
`crates/phoenix-ide/src/runtime/`, not from imagination. Use these
as the spec's grounding.

### Retry loop is NOT in the LLM client

The LLM client functions (`anthropic::complete_streaming`,
`openai::complete_streaming`, `fireworks` via the OpenAI path) each
make **one** attempt and return `Result<LlmResponse, LlmError>`. No
retry loop, no sleep, no exponential backoff inside the client.

The retry is driven by the **executor + state machine**:

1. `LlmRequesting { attempt: N }` is the active state.
2. LLM client returns `Err(LlmError)`.
3. Executor maps `LlmErrorKind` → `LlmOutcome`
   (``llm_error_to_llm_outcome` in executor.rs`):
   - `RateLimit` → `LlmOutcome::RateLimited { retry_after }`
   - `ServerError` / `ServerOverloaded` → `LlmOutcome::ServerError { ... }`
   - `Network` → retryable
   - Others (`Auth`, `UsageLimitReached`, `ContentFilter`,
     `ContextWindowExceeded`, `InvalidRequest`) → non-retryable
4. State machine handles retryable error in `handle_core_llm_error`
   (``handle_core_error_retry` in transition.rs`):
   ```rust
   CoreState::AwaitingContinuation {
       rejected_tool_calls: ...,
       attempt: new_attempt,   // = N + 1
   }
   .with_effect(Effect::ScheduleRetry {
       delay,
       attempt: new_attempt,
   })
   ```
5. Executor handles `Effect::ScheduleRetry` (`executor.rs:1408-1418`):
   ```rust
   Effect::ScheduleRetry { delay, attempt } => {
       let outcome_tx = self.outcome_tx.clone();
       tokio::spawn(async move {
           tokio::time::sleep(delay).await;
           let _ = outcome_tx
               .send(EffectOutcome::RetryTimeout { attempt })
               .await;
       });
       Ok(None)
   }
   ```
6. State machine transitions `AwaitingContinuation → LlmRequesting`
   on `RetryTimeout` (`the `RetryTimeout` case in `handle_core_error_retry``).

### What's already on the wire

The `attempt` counter is **already exposed** via existing
`StateChange` events. `ConvState::LlmRequesting { attempt }` and
`ConvState::AwaitingContinuation { attempt }` are serialised as part
of `state` on every `StateChange`. The display layer can read
`state.attempt` from the existing payload — no new field needed for
the counter alone.

### What's NOT on the wire yet

These drive the user-facing `(retry 2/5 after 429)` display but are
currently lost between executor and client:

1. **Retry reason** (RateLimit / ServerError / Network) — classified
   by `llm_error_to_llm_outcome` but only stored as an `LlmOutcome`
   variant that doesn't propagate into the next state.
2. **Backoff delay** (`delay: Duration` in `Effect::ScheduleRetry`) —
   only known to the executor's spawned tokio task.
3. **resets_at quota timestamp** — already formatted into user-facing
   error message strings (`the `retry_suffix` / `retry_suffix_after_or` helpers in llm/error.rs`), but only surfaced
   when the conversation terminates with `UserFacingError`, NOT
   during the retry loop.
4. **Max attempts** — implicit somewhere in the state machine. Need
   to grep for the retry policy when drafting.

### Overlap question: LlmAttempt vs RateLimitSnapshot

`SseWireEvent::RateLimitSnapshot { snapshot: QuotaDetails }`
(`wire.rs:328-331`) already exists. Currently:

- Emitted only from the codex backend
- Mid-stream — during an active LLM call, NOT during backoff
- Ephemeral, not persisted

This is overlapping ground with the proposed `LlmAttempt`. Spec
must decide:

- **Distinguish**: `RateLimitSnapshot` = quota during a successful
  LLM call; `LlmAttempt` = retry context during backoff. Or:
- **Unify**: one event with mode discriminator.

### Sub-agent retries

Sub-agents have their own state machine instance
(`the `SubAgentState::Core` conversions in state.rs`); each runs `AwaitingContinuation { attempt }`
independently.

**Spec decision needed**: if a sub-agent retries, does the parent's
StateBar reflect it? **Recommended initial answer: parent stays
mute.** Each conversation surfaces its own retry context. Cross-
conversation retry rollups are deferred.

### Cancellation during backoff

`Effect::ScheduleRetry`'s `tokio::spawn` is fire-and-forget — no
JoinHandle is tracked. If the user clicks Cancel during the sleep,
the spawned task keeps sleeping and eventually sends `RetryTimeout`
to a state machine that has transitioned to `Cancelling` or `Idle`.
The state machine filters stale `RetryTimeout` events
(`the `Idle`+`LlmResponse` stale absorber in transition.rs`).

**For the spec**: cancellation IS already correctly modelled at the
state machine level. The display-side answer is "during backoff,
show `awaiting_continuation` with the new attempt number"; on
cancel, `StateChange` to `Cancelling` fires and the display
transitions naturally. No new flow needed.

### LlmErrorKind classification (from `llm/error.rs:81-105`)

```rust
pub enum LlmErrorKind {
    Network,           // retryable
    RateLimit,         // retryable with backoff (transient)
    UsageLimitReached, // NOT retryable (plan-level cap)
    ServerError,       // retryable (5xx)
    ServerOverloaded,  // NOT retryable
    Auth,              // NOT retryable
    InvalidRequest,    // NOT retryable
    ContentFilter,     // NOT retryable
    ContextWindowExceeded, // NOT retryable
}
```

The user-visible "reason" should map:
- `RateLimit` → "rate limit"
- `ServerError` → "server error"
- `Network` → "network error"

Other variants never reach the retry loop (they terminate
immediately).

## Proposed wire shape (preview — to be refined when drafting)

```rust
LlmAttempt {
    sequence_id: i64,
    attempt: u32,           // 1-indexed; matches state.attempt
    max_attempts: u32,      // from the executor's retry policy
    reason: LlmAttemptReason, // RateLimit | ServerError | Network
    backing_off_ms: u64,    // delay until next attempt
    resets_at: Option<DateTime<Utc>>, // when quota window opens
}
```

Emitted from the executor's `Effect::ScheduleRetry` handler
(`executor.rs:1408-1418`), immediately before the `tokio::spawn`.
Ephemeral (per `RateLimitSnapshot`); not persisted.

## What this task delivers

The spec set at `specs/llm-retry-visibility/`:

- `requirements.md` — 5-7 requirements (REQ-LRV-*)
- `design.md` — wire/runtime/client changes
- `executive.md` — status table + cross-spec dependencies
- `llm-retry-visibility.allium` — formal behavioural spec

Plus updates to `specs/working-phase-visibility/`:
- Replace the inline `value RetryContext` and helper PLACEHOLDERs
  with an import: `use "../llm-retry-visibility/llm-retry-visibility.allium" as llm_retry`
- Update file-header cross-references

Plus updates to `specs/sse_wire/sse_wire.allium`:
- Add `llm_attempt` to the `EphemeralEventAppendedToReplayRing`
  whitelist (per the cross-spec checklist embedded there)

## Open questions to resolve when drafting

1. `LlmAttempt` vs `RateLimitSnapshot` — unify, or keep distinct
   with clear lifecycle differences?
2. Where is `max_attempts` configured? Per-provider / per-call /
   global? Needs a grep for the retry policy.
3. Persisting attempt metadata after success — the "(retried 2x)"
   badge on the persisted assistant message is the post-hoc
   surface. Probable home: `display_data.retry_count` typed field
   on the assistant message, mirroring `tool_starts` and
   `duration_ms`.
4. Sub-agent retries — explicit `deferred` entry in the Allium spec
   with "parent stays mute" recorded.
5. Cancellation-during-backoff display — confirm `Cancelling`
   transition is the right exit and document it explicitly.

## Code paths to verify when drafting

- `executor.rs:1408-1418` — `Effect::ScheduleRetry` handler
- `the `complete_streaming` call site in executor.rs` — `complete_streaming` call site
- ``llm_error_to_llm_outcome` in executor.rs` — `LlmError` → `LlmOutcome` mapping
- ``handle_core_error_retry` in transition.rs` — `handle_core_llm_error` retry transition
- ``handle_core_continuation` in transition.rs` — continuation retry handler
- ``ConvState::AwaitingContinuation` in state.rs` — `AwaitingContinuation { rejected_tool_calls,
  attempt }`
- `llm/error.rs:81-115` — `LlmErrorKind` + `is_retryable`
- `wire.rs:328-331` — `RateLimitSnapshot` for overlap analysis
- `llm/anthropic.rs:343-` — `complete_streaming` (single attempt)
- ``complete_streaming` in llm/openai.rs (~L452-)` — same shape
- `llm/service.rs:138-158` — dispatch layer

## Pre-flight checklist

Before pushing the spec, run `specs/AUTHORING.md`'s pre-flight
(see task 58004). Don't repeat the 8-round review cycle PR #155
went through.
