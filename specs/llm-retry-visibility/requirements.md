# LLM Retry Visibility

## User Story

As a user watching a Phoenix conversation, I need to see *why* a turn is
taking a long time — specifically, whether the agent is being throttled,
hitting a 5xx server, or losing network — and how many retries remain
before the turn gives up. The current behaviour hides retries entirely
inside the executor's `Effect::ScheduleRetry` handler, so a 45-second
silence could be one slow LLM call or three back-to-back rate-limit
backoffs and I have no way to tell.

## Background

When the LLM client returns a retryable `LlmError`
(`crates/phoenix-ide/src/llm/error.rs:111` — `Network | RateLimit |
ServerError`), the executor maps it to an `LlmOutcome` and feeds an
`Event::LlmError` to the state machine. The state machine
(`handle_core_error_retry` in `transition.rs`, and `handle_core_continuation`
for the post-tool-round path) bumps the `attempt` counter, schedules a
backoff via `Effect::ScheduleRetry { delay, attempt }`, and emits an
`Effect::NotifyStateChange`. The executor's `Effect::ScheduleRetry`
handler (`executor.rs:1408-1418`) spawns a sleep task that emits
`EffectOutcome::RetryTimeout` to drive `RetryTimeout -> RequestLlm`
when the delay elapses.

The state's `attempt` field is already surfaced on the wire as part of
the `StateChange.state` payload (clients can read `state.attempt`).
What's currently *lost* between the executor and the client:

- **The reason** (`RateLimit | ServerError | Network`), classified by
  `llm_error_to_outcome` (`executor.rs:3570`) into an `LlmOutcome`
  variant that has no on-the-wire representation during the retry
  window.
- **The backoff delay** (`delay: Duration` in `Effect::ScheduleRetry`),
  known only to the spawned sleep task.
- **The quota reset timestamp** (`resets_at`) — present on
  `QuotaDetails` and rendered into the user-facing message string when
  the turn *terminates* via `UserFacingError`, but never surfaced
  during the retry loop itself.
- **The maximum attempts** — `MAX_RETRY_ATTEMPTS = 3` in
  `transition.rs:183` — implicit and not on the wire.

This spec adds the wire/runtime contract for surfacing those values
during the backoff window. The display side that consumes them is
governed by `specs/working-phase-visibility/`'s REQ-WPV-003: the
producer here owns the data, the sibling spec owns the rendering.

## Requirements

### REQ-LRV-001: Retry Context Wire Event

WHEN the state machine schedules a retry for a retryable `LlmError`
(`Effect::ScheduleRetry` is emitted from either `handle_core_error_retry`
during `LlmRequesting` or `handle_core_continuation` during
`AwaitingContinuation`)
THE SYSTEM SHALL emit a new SSE event `LlmAttempt` carrying the retry
context, immediately before the spawned backoff sleep begins

**Wire shape (additive — see design.md for exact field types):**

```
LlmAttempt {
    sequence_id: i64,
    attempt: u32,           // 1-indexed, matches state.attempt
    max_attempts: u32,      // MAX_RETRY_ATTEMPTS = 3
    reason: LlmAttemptReason,   // RateLimit | ServerError | Network
    backing_off_ms: u64,    // the delay value in Effect::ScheduleRetry
    resets_at: Option<DateTime<Utc>>,  // RFC3339 string, when known
}
```

**Rationale:** The state's `attempt` counter on `StateChange` answers
"how many tries"; it does not answer "why" or "how long until the
next try". Threading the reason and delay through a typed event keeps
the producer (executor) and the consumer (client renderer) coupled by
data, not by string parsing.

---

### REQ-LRV-002: Retry Reason Classification

WHEN classifying an `LlmError` into an `LlmAttemptReason`
THE SYSTEM SHALL map exactly the three retryable `LlmErrorKind`
variants:

- `LlmErrorKind::RateLimit` -> `LlmAttemptReason::RateLimit`
- `LlmErrorKind::ServerError` -> `LlmAttemptReason::ServerError`
- `LlmErrorKind::Network` -> `LlmAttemptReason::Network`

WHEN an `LlmError` with a non-retryable kind arrives
THE SYSTEM SHALL NOT emit `LlmAttempt` (the state machine terminates
the turn instead — see `transition.rs:1255`)

**Rationale:** `is_retryable()` (`llm/error.rs:111`) is the single
source of truth for which errors enter the retry loop. The wire enum
mirrors that classification exactly so the compiler enforces
exhaustiveness on both sides.

---

### REQ-LRV-003: Cross-Spec Contract with Working-Phase Visibility

WHEN `specs/working-phase-visibility/`'s `TurnRetryContext` entity is
populated
THE SYSTEM SHALL source the population from a received `LlmAttempt`
event (one rule, owned by this spec, that creates or updates
`TurnRetryContext{view}` on each arrival)

WHEN the consumer's `render_retry_modifier_for(view)` helper produces
its `(retry K/N <reason>)` suffix
THE SYSTEM SHALL source `K` from `LlmAttempt.attempt`, `N` from
`LlmAttempt.max_attempts`, and `<reason>` from `LlmAttempt.reason`
formatted by a shared helper owned here (`reason_text(reason) ->
String`)

**Rationale:** The placeholder `RetryContext` value type currently
inlined in `working-phase-visibility.allium` (with PLACEHOLDER
comment) is owned by this spec from now on. The import direction is
working-phase-visibility -> llm-retry-visibility (the consumer
imports the producer); there is no circular dependency.

---

### REQ-LRV-004: Sub-Agent Retries Stay Local

WHEN a sub-agent conversation's state machine schedules a retry
THE SYSTEM SHALL emit `LlmAttempt` on the **sub-agent's** SSE stream
only — NOT on the parent's

WHEN a user views the parent conversation while a sub-agent is
retrying
THE SYSTEM SHALL display the parent's own retry context (which may be
absent), NOT a roll-up of the sub-agent's retries

**Rationale:** Phoenix sub-agents run as independent conversations
with their own SSE streams and their own state machines
(`SubAgentState::Core` in `state.rs:1302`). Cross-conversation retry
rollups (e.g. "this parent's sub-agent is retrying") are a separate
aggregate-dashboard concern (see the
`AggregateActivityDashboard` deferred entry in
`working-phase-visibility.allium`). Keeping each conversation's
retry context isolated avoids leakage of sub-agent details into a
parent view that wasn't asked for them.

---

### REQ-LRV-005: Cancellation During Backoff Routes Through Cancelling

WHEN the user cancels during a retry backoff window (the executor's
spawned `RetryTimeout` task is sleeping)
THE SYSTEM SHALL transition the conversation to a `Cancelling`
variant via the existing state-machine cancel path, and the
ScheduledRetry task's eventual `RetryTimeout` event SHALL be filtered
by the state machine as stale

WHEN the conversation is in any `Cancelling*` variant with a
`TurnRetryContext` populated
THE SYSTEM SHALL continue to display the retry suffix until either
(a) the turn reaches a terminal state (`AgentDone` clears it per the
working-phase-visibility spec) or (b) a fresh `LlmAttempt` arrives

**Rationale:** The state machine already correctly handles
cancellation during backoff: `handle_core_cancel*` paths transition
out of `LlmRequesting`/`AwaitingContinuation`, and stale
`RetryTimeout` events are absorbed by the catch-all idle path. The
spec exists to make explicit that no new "abort retry" event is
needed — the existing cancellation flow is the abort signal, and the
display naturally tracks it via the same `TurnRetryContext` lifecycle
working-phase-visibility already specifies.

---

### REQ-LRV-006: Post-Hoc Retry Badge on Assistant Message

WHEN an assistant message is persisted after a turn that retried at
least once
THE SYSTEM SHALL record the final retry count on the persisted
message's `display_data` as a typed `retry_count: u32` field
(reusing the typed display_data side-channel pattern that
working-phase-visibility uses for `tool_starts: BTreeMap<String,
i64>`)

WHEN the UI renders an assistant message whose `display_data.retry_count`
is greater than zero
THE SYSTEM SHALL display a `(retried Nx)` badge on the message
header

**Rationale:** During the turn, the retry context appears in the
StateBar as a live modifier. After the turn completes, the StateBar
clears (per
`TurnRetryContextClearedOnAgentDone` in working-phase-visibility)
and the audit trail of "this answer took 2 retries" would
otherwise vanish. The typed field on the persisted message is
the long-lived record. Keying it on `display_data` mirrors the
existing `duration_ms`-on-tool-result convention (`schema.rs:649`)
and the `tool_starts`-on-assistant-message convention from the
sibling spec — same side-channel, same shape, one mental model.

---

### REQ-LRV-007: LlmAttempt and RateLimitSnapshot Are Distinct

THE SYSTEM SHALL emit `LlmAttempt` and `RateLimitSnapshot` as
distinct SSE wire variants, with non-overlapping lifecycles:

- `RateLimitSnapshot` is emitted mid-call (on a successful turn from
  parsed `x-codex-*` headers, or on a terminal 429 replayed from
  `UsageLimitReached.details`). It carries the full `QuotaDetails`.
  Like all `send_seq` broadcasts it **is appended to the replay ring**
  and whitelisted in `sse_wire.allium` (`EphemeralEventAppendedToReplayRing`),
  but its semantics are **point-in-time**: a reconnecting client treats
  a replayed snapshot as possibly-stale quota data (the live quota may
  have moved since), so dropping or ignoring a replayed snapshot is
  acceptable.
- `LlmAttempt` is emitted at retry-schedule time (from
  `Effect::ScheduleRetry`). It carries only the per-attempt context
  needed to render the retry modifier on the StateBar. It is **also in
  the replay ring**, and unlike `RateLimitSnapshot` it is
  **load-bearing on reconnect**: a mid-backoff reconnect must replay it
  to reconstruct the in-flight retry display.

**Rationale:** Both events involve rate-limit data, but they answer
different questions ("what is the current quota?" vs "what is the
current retry context?") and have different staleness properties on
reconnect — `RateLimitSnapshot` is informational/point-in-time while
`LlmAttempt` is load-bearing for reconstructing the retry suffix.
Unifying them would mean attaching retry context to the QuotaDetails
carrier (which structurally implies a quota snapshot exists, and it
doesn't for non-codex providers) or coercing the load-bearing retry
event into the point-in-time staleness contract. Keeping them distinct
preserves both contracts.

---

## Out of Scope

- Per-provider retry policies (different `max_attempts` for Anthropic
  vs OpenAI vs Fireworks). `MAX_RETRY_ATTEMPTS = 3` is global; if a
  provider-specific policy emerges, the wire field `max_attempts` is
  already per-event so no schema change is needed — only the runtime
  policy.
- Showing a live "backing off Ns" countdown derived from
  `LlmAttempt.backing_off_ms`. V1 displays the static modifier
  `(retry K/N <reason>)`; if a countdown is added, it derives from the
  `LlmAttempt` event's own arrival timestamp client-side, not from a
  server-pushed clock.
- A retry-events query API ("show me all retries for conversation X").
  The `display_data.retry_count` badge (REQ-LRV-006) plus
  server-side logs cover diagnostics needs at v1.
- Cross-conversation retry rollups (parent showing sub-agent's
  retries) — deferred per REQ-LRV-004's rationale.
- Per-tool retry badges (a tool widget showing "this tool's
  underlying LLM turn retried 2x") — deferred per
  `working-phase-visibility.allium`'s `PerArtifactRetryBadge`
  deferred entry.
