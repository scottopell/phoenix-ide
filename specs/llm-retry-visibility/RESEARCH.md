# llm-retry-visibility — Producer Code Research

**Status:** research notes; NOT the spec yet. Captures the producer-side
code shape so the spec drafts against actual types and control flow,
not imagination. Once the spec drafts use these as ground truth,
the spec files themselves replace this doc as the source of truth.

## Key finding: retries are NOT in the LLM client

The retry loop architecture is a common misread. The LLM client functions
(`anthropic::complete_streaming`, `openai::complete_streaming`,
`fireworks` via the OpenAI path) each make **one** attempt and return
`Result<LlmResponse, LlmError>`. There is no retry loop, no sleep, no
exponential backoff inside the client.

The retry is driven by the **executor + state machine**. The relevant
sequence:

1. `LlmRequesting { attempt: N }` is the active state.
2. LLM client returns `Err(LlmError)`.
3. Executor maps `LlmErrorKind` → `LlmOutcome` (`executor.rs:3549-3578`):
   - `RateLimit` → `LlmOutcome::RateLimited { retry_after }`
   - `ServerError` / `ServerOverloaded` → `LlmOutcome::ServerError { ... }`
   - `Network` → also retryable
   - Others (`Auth`, `UsageLimitReached`, `ContentFilter`,
     `ContextWindowExceeded`, `InvalidRequest`) → non-retryable
4. State machine handles the retryable error in `handle_core_llm_error`
   (`transition.rs:1223-1278`):
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
   on `RetryTimeout` (`transition.rs:1273-1278`).

## What's already on the wire

The `attempt` counter is **already exposed** via `StateChange` events.
The runtime state variants `LlmRequesting { attempt }` and
`AwaitingContinuation { attempt }` are serialized as part of
`ConvState`, and `StateChange.state` carries that struct verbatim.

**Implication:** the attempt counter does NOT need a new wire field.
The display layer can read `state.attempt` from the existing
`StateChange.state` payload.

## What's NOT on the wire yet

These are the values that drive the user-facing "(retry 2/5 after 429)"
display but are currently lost between executor and client:

1. **The retry reason** (RateLimit / ServerError / Network / etc.) —
   classified at the executor by `llm_error_to_llm_outcome` but only
   stored as the `LlmOutcome` variant, which doesn't propagate into the
   next state.

2. **The backoff delay** (`delay: Duration` in `Effect::ScheduleRetry`)
   — only known to the executor's spawned tokio task; the client has no
   way to know "we're sleeping for 4s before the next attempt".

3. **The resets_at quota timestamp** (provider-supplied) — already
   formatted into the user-facing error message strings
   (`llm/error.rs:206-222`), but only surfaced when the conversation
   terminates with `UserFacingError`, NOT during the retry loop.

4. **The max attempt** — there's a max retry count somewhere in the
   state machine but it's currently implicit. The display "(retry 2/5)"
   wants both numerator and denominator.

## Existing wire surface for quota

`SseWireEvent::RateLimitSnapshot { snapshot: QuotaDetails }`
(`wire.rs:328-331`) already exists. It's described as "mid-stream
quota snapshot from the codex backend. Ephemeral." So the wire variant
exists; current production:

- Emitted only from the codex backend (not anthropic/openai direct)
- Mid-stream — during an active LLM call, not during backoff
- Ephemeral — not persisted

This is overlapping ground for `LlmAttempt`. The two should be
distinguished in the spec:
- `RateLimitSnapshot` = quota info during a successful LLM call
- `LlmAttempt` = retry context during a backoff window

Or they should be unified into one event with both modes.

## Existing wire surface for user-facing errors

`SseWireEvent::Error { message, error: UserFacingError }`
(`wire.rs:293-302`) is emitted on terminal failures only — when the
state machine gives up retrying and the conversation enters an error
state. NOT during the retry loop. Different lifecycle.

## Sub-agent retries

Sub-agents have their own state machine instance (`state.rs:1345`),
so `AwaitingContinuation { attempt }` works the same way. Each
sub-agent runs its own retry loop independently.

**Parent display question:** if a sub-agent retries, should the
parent's StateBar reflect that? Today the parent shows
`awaiting_sub_agents` with no retry info; the sub-agent's own
conversation view shows the retry. The spec should decide whether
parent rolls up sub-agent retries or stays mute.

Recommended initial answer: parent stays mute. Each conversation
shows its own retry context. Cross-conversation retry rollups are
deferred.

## Cancellation during backoff

The `ScheduleRetry` tokio::spawn is fire-and-forget — no JoinHandle
is stored on the executor:

```rust
tokio::spawn(async move {
    tokio::time::sleep(delay).await;
    let _ = outcome_tx
        .send(EffectOutcome::RetryTimeout { attempt })
        .await;
});
```

If the user clicks Cancel during the sleep, the spawned task keeps
sleeping and eventually sends `RetryTimeout` to a channel whose receiver
is in a different state. The state machine handles this via the
"stale LlmResponse after cancel" / similar handlers in
`transition.rs:588` — a `RetryTimeout` arriving in (say) `Cancelling`
or `Idle` is filtered.

**For the spec:** the producer-side cancellation works at the state
machine level. The display-side question is "during backoff, what does
the StateBar show?" — currently `awaiting_continuation` with the new
attempt number. If the user cancels during this window, the state
transitions to `Cancelling`, the StateChange fires, and the
backoff-display ends naturally.

So cancellation IS already correctly modelled — we don't need a new
cancellation flow, just a clear `awaiting_continuation` display.

## LlmErrorKind classification

From `llm/error.rs:81-105`:

```rust
pub enum LlmErrorKind {
    Network,           // retryable
    RateLimit,         // retryable with backoff (transient)
    UsageLimitReached, // NOT retryable (plan-level cap)
    ServerError,       // retryable (5xx)
    ServerOverloaded,  // NOT retryable (server_is_overloaded / slow_down)
    Auth,              // NOT retryable
    InvalidRequest,    // NOT retryable
    ContentFilter,     // NOT retryable
    ContextWindowExceeded, // NOT retryable
}
```

`is_retryable()` returns true for `Network | RateLimit | ServerError`
only.

The user-visible "reason" string for `LlmAttempt` should map from these
classifications:
- `RateLimit` → "rate limit"
- `ServerError` → "server error"
- `Network` → "network error"

The other LlmErrorKinds never reach the retry loop (they terminate the
conversation immediately).

## Proposed wire shape (preview)

Not yet specified — to be drafted in the actual spec — but the
producer constraints suggest something like:

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
(`executor.rs:1408-1418`), immediately before the tokio::spawn that
schedules the timeout. Ephemeral (per `RateLimitSnapshot`); not
persisted.

The state machine's existing `attempt` field becomes the "live"
counter; `LlmAttempt` carries the per-attempt metadata that doesn't
live in the state itself.

## Loose ends to resolve when drafting the spec

1. **`LlmAttempt` vs `RateLimitSnapshot` overlap** — unify, or keep
   distinct with clear lifecycle differences?
2. **Max attempts** — where is it configured? Per-provider, per-call,
   global? Needs a `grep` for the retry policy.
3. **Persisting attempt metadata after success** — the retry "(retried
   2x)" badge on the persisted assistant message is the post-hoc
   surface. Decide where this is stored — probably the assistant
   message's `display_data.retry_count` (typed field, mirroring
   `tool_starts` and `duration_ms`).
4. **Sub-agent retries — explicit deferred entry** in the Allium
   spec, with the "parent stays mute" decision recorded.
5. **Cancellation-during-backoff display** — confirm that the existing
   `Cancelling` transition is the right exit and document it.

## Code paths to verify when drafting

- `executor.rs:1408-1418` — `Effect::ScheduleRetry` handler
- `executor.rs:1819` — `complete_streaming` call site
- `executor.rs:3549-3578` — `LlmError` → `LlmOutcome` mapping
- `transition.rs:1223-1278` — `handle_core_llm_error` retry transition
- `transition.rs:1290-1322` — continuation retry handler
- `state.rs:885-891` — `AwaitingContinuation { rejected_tool_calls,
  attempt }`
- `llm/error.rs:81-115` — `LlmErrorKind` + `is_retryable`
- `wire.rs:328-331` — `RateLimitSnapshot` for overlap analysis
- `llm/anthropic.rs:343-` — `complete_streaming` (single attempt,
  no retry)
- `llm/openai.rs:471-` — same shape
- `llm/service.rs:138-158` — dispatch layer (no retry, but invalidates
  auth cache on auth failure)
