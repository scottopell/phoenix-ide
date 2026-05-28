# LLM Retry Visibility — Design

## Overview

One new SSE wire variant (`LlmAttempt`); one new emission site in the
executor (inside `Effect::ScheduleRetry`); one new typed field on
assistant-message `display_data` (`retry_count: u32`); one new
populator rule on the client's `TurnRetryContext` entity (already
declared by the sibling spec, populated here).

The state machine and the LLM clients are unchanged — they already
classify, count, and back off correctly. This spec adds *observability*
of that existing flow, not new flow.

## Wire Format Changes

### `LlmAttempt` (new variant)

```rust
SseWireEvent::LlmAttempt {
    sequence_id: i64,
    /// 1-indexed attempt number this retry is scheduled FOR.
    /// Matches the `attempt` field that lands on the next
    /// `StateChange.state.attempt` after this event.
    attempt: u32,
    /// MAX_RETRY_ATTEMPTS = 3 (from
    /// `state_machine/transition.rs:183`). Carried per-event so a
    /// future per-provider policy is wire-compatible.
    max_attempts: u32,
    /// Why the previous attempt failed retryably.
    reason: LlmAttemptReason,
    /// The `delay: Duration` value from `Effect::ScheduleRetry`,
    /// converted to milliseconds. Informational — the live display
    /// does NOT count down from this; on replay it's already stale.
    backing_off_ms: u64,
    /// When the upstream quota window resets, when known. Populated
    /// only for `LlmErrorKind::RateLimit` errors whose
    /// `LlmError.quota` was non-null; null otherwise. Serialises as
    /// RFC3339 string when present (matching the existing
    /// `QuotaDetails.resets_at: Option<DateTime<Utc>>` shape in
    /// `llm/rate_limit.rs:35`), and is omitted from the JSON when
    /// `None` via `#[serde(skip_serializing_if = "Option::is_none")]`
    /// (which the client treats as `undefined`, the existing
    /// convention across the wire — see
    /// `specs/AUTHORING.md` "Option/null shape" if/when that
    /// doc lands).
    resets_at: Option<DateTime<Utc>>,
}

pub enum LlmAttemptReason {
    RateLimit,
    ServerError,
    Network,
}
```

Variant tag on the wire: `"llm_attempt"` (snake_case, matching the
existing `event_type()` convention in `wire.rs:337-353`).

### Emission site

Inside `Effect::ScheduleRetry`'s handler (`executor.rs:1408-1418`),
immediately before `tokio::spawn`:

```rust
Effect::ScheduleRetry { delay, attempt } => {
    let _ = self.broadcast_tx.send_seq(|seq| SseEvent::LlmAttempt {
        sequence_id: seq,
        attempt,
        max_attempts: MAX_RETRY_ATTEMPTS,
        reason: /* see "Plumbing the reason" below */,
        backing_off_ms: delay.as_millis() as u64,
        resets_at: /* see "Plumbing resets_at" below */,
    });
    let outcome_tx = self.outcome_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = outcome_tx.send(EffectOutcome::RetryTimeout { attempt }).await;
    });
    Ok(None)
}
```

`MAX_RETRY_ATTEMPTS` becomes `pub` in `transition.rs` (currently
`const MAX_RETRY_ATTEMPTS: u32 = 3;`). The change is mechanical;
exposing a single retry-policy constant for cross-module use does not
warrant a new module.

### Plumbing the reason

`Effect::ScheduleRetry` today carries only `{ delay, attempt }`. The
classification (`LlmErrorKind`) lives on the `Event::LlmError`
processed by the state machine immediately before scheduling. The
cleanest threading is to widen the effect:

```rust
// state_machine/effect.rs (or wherever Effect is defined)
Effect::ScheduleRetry {
    delay: Duration,
    attempt: u32,
    reason: LlmAttemptReason,         // new
    resets_at: Option<DateTime<Utc>>, // new — see below
},
```

`handle_core_error_retry` constructs the `reason` from the inbound
`Event::LlmError`'s `error_kind`. A small helper sits next to the
existing `llm_error_to_outcome`:

```rust
fn llm_error_kind_to_attempt_reason(kind: ErrorKind) -> Option<LlmAttemptReason> {
    match kind {
        ErrorKind::Network     => Some(LlmAttemptReason::Network),
        ErrorKind::RateLimit   => Some(LlmAttemptReason::RateLimit),
        ErrorKind::ServerError => Some(LlmAttemptReason::ServerError),
        _ => None,  // non-retryable kinds never reach Effect::ScheduleRetry
    }
}
```

The `None` arm is structurally unreachable (the
`if error_kind.is_retryable() && *attempt < MAX_RETRY_ATTEMPTS` guard
on the rule prevents non-retryable kinds from getting here), but the
helper returns `Option` so the call site can `unwrap_or_else(|| ...)`
with a `tracing::error!` instead of panicking, matching the rest of
the runtime's error-handling style.

### Plumbing resets_at

`Event::LlmError` does not currently carry `quota: Option<QuotaDetails>`.
The 429-classification path that *does* see the parsed quota is
`llm_error_to_outcome` (`executor.rs:3570`); it produces
`LlmOutcome::RateLimited { retry_after: None }`, discarding the
quota information from the original `LlmError.quota` field.

Two options:

**Option A (chosen) — thread `resets_at` through `Event::LlmError`.**
Add `resets_at: Option<DateTime<Utc>>` to `Event::LlmError`, populate
it from `error.quota.as_ref().and_then(|q| q.resets_at)` in
`llm_outcome_to_event` (currently `transition.rs:2373`), and forward
it on the resulting `Effect::ScheduleRetry`.

**Option B — keep `Effect::ScheduleRetry` narrow and look up the
quota from a side channel (e.g. the runtime's quota store).**
Rejected because it introduces a parallel representation of the same
value (one in the error, one in the store) and the producer of
LlmAttempt would need to consult the store at emission time, which
couples the executor to a codex-specific component.

Decision: Option A. The `Event::LlmError` event already carries
context the state machine consumes (`message`, `error_kind`,
`attempt`, `recovery_in_progress`); `resets_at` is one more piece
of context, scoped the same way.

For non-`RateLimit` errors (Network, ServerError), `resets_at`
is always `None`. The field is `Option<DateTime<Utc>>` so the
wire shape encodes "absent" as `undefined` per the
`#[serde(skip_serializing_if = "Option::is_none")]` convention.

### Replay-ring eligibility

`LlmAttempt` IS appended to the replay ring (per
`EphemeralEventAppendedToReplayRing` in `sse_wire.allium` — this
spec adds `"llm_attempt"` to its whitelist as part of the cross-spec
checklist). Rationale:

- Without replay, a client that reconnects mid-backoff sees
  `StateChange(LlmRequesting{attempt:2})` from Init's `conversation.state`
  but has no way to recover the retry reason. The StateBar would
  show "awaiting LLM response Ns" without the "(retry 2/3 after rate limit)"
  suffix — a regression in user trust during exactly the failure mode
  this spec is trying to surface.
- The replay cost is small: `LlmAttempt` is a fixed-size event (no
  blob payload), it's emitted at most `MAX_RETRY_ATTEMPTS - 1 = 2`
  times per turn, and the ring already handles the volume.
- Replay correctness: on the wire, the order is
  `StateChange(...)` then `LlmAttempt(...)`. On replay, the ring
  preserves that order. The client's `TurnRetryContext` populator
  fires on `LlmAttempt`; the consumer's `render_retry_modifier_for`
  reads from it at derivation time. Both rules are idempotent on
  the latest event.

The cross-spec checklist at the top of
`EphemeralEventAppendedToReplayRing` in `specs/sse_wire/sse_wire.allium`
is the canonical procedure for adding a wire variant; this spec
follows it in full, including:

- `wire.rs` enum + `event_type()` + parity test
- `runtime.rs` `SseEvent` + `From<SseEvent>` conversion
- `ui/src/sseSchemas.ts` valibot schema
- `ui/src/hooks/useConnection.ts` explicit listener registration
  (native `EventSource` has no wildcard for named events; see
  the working-phase-visibility design.md "EventSource listener wiring"
  section for the full rationale)
- `sse_wire.allium` whitelist update
- `conversation_atom.allium` reducer (no-op for atom — `LlmAttempt`
  populates a working-phase-visibility entity, not the message
  atom directly)

## Runtime Changes

### State machine

- Add `LlmAttemptReason` to a shared location (`runtime.rs` or
  `state_machine/state.rs`) so both `Event::LlmError` (or its
  surrounding code) and the wire variant can reference it without
  a circular import.
- Widen `Effect::ScheduleRetry` to carry `reason: LlmAttemptReason`
  and `resets_at: Option<DateTime<Utc>>`.
- Widen `Event::LlmError` to carry `resets_at: Option<DateTime<Utc>>`
  (already carries `error_kind`; the reason is derived from that).
- Update both retry call sites (`handle_core_error_retry` and
  `handle_core_continuation`) to populate the new fields.
- `pub` the `MAX_RETRY_ATTEMPTS` constant.

### Executor

- In the `Effect::ScheduleRetry` arm (`executor.rs:1408`), emit the
  new `SseEvent::LlmAttempt` immediately before the
  `tokio::spawn(...)` that schedules the backoff sleep. The emit and
  the spawn happen in the same arm, so there is no ordering window
  in which the spawn fires without the event having been queued for
  broadcast.
- In `llm_outcome_to_event` (`transition.rs:2373` — `RateLimited`
  arm), thread `quota.resets_at` from the original `LlmError` if
  it's reachable. The current code path drops it; the change is to
  carry it forward into `Event::LlmError.resets_at`. `LlmError` already
  has `.quota: Option<Box<QuotaDetails>>` per `llm/error.rs:23`.

### Post-hoc retry_count on assistant message

- Where the assistant message is persisted at `LlmResponse` time
  (effect `Effect::persist_agent_message` in `transition.rs` —
  search the file for the persist path), capture the final
  `attempt` count from the state being transitioned out of and
  write it onto the persisted message's `display_data.retry_count:
  u32`. Like `tool_starts`, this is a typed field on
  `MessageDisplayData` (or its equivalent) with ts-rs export.
- Convention: `retry_count = max(0, final_attempt - 1)`. A turn
  that succeeded on first try has `final_attempt = 1` and
  `retry_count = 0` (badge not shown); a turn that succeeded on the
  third try has `final_attempt = 3` and `retry_count = 2` (badge
  reads "(retried 2x)").
- The field is set only when the turn ends in `Effect::LlmResponse`
  -> `Idle` (success path) or one of the persist-on-end paths;
  cancelled or errored turns either don't persist an assistant
  message at all, or persist a partial one — in those cases
  `retry_count` falls back to `0` (default) since there's no
  successful turn to attribute retries to. Audit of failure-path
  persistence is left to the implementation; for the spec, the
  contract is "iff the message is persisted and the turn retried,
  the field is populated."

## Client Changes

### Atom state additions

`ui/src/conversation/atom.ts` already tracks the working-phase entities
defined by the sibling spec. Add:

```ts
// On the per-conversation atom:
turnRetryContext: {
  attempt: number;
  max_attempts: number;
  reason: 'rate_limit' | 'server_error' | 'network';
  reason_text: string;   // human-rendered, e.g. "rate limit"
  backing_off_ms: number;
  resets_at: number | null;  // unix ms, converted from RFC3339 once at SSE-boundary
} | null;
```

`null` is the no-retry steady state. Set on every `LlmAttempt`
arrival; cleared on `AgentDone` (per
`TurnRetryContextClearedOnAgentDone` in working-phase-visibility) and
on terminal `Error` (per `TurnRetryContextClearedOnTerminalError`).

### Reducer for LlmAttempt

```ts
case 'llm_attempt': {
  atom.turnRetryContext = {
    attempt: event.attempt,
    max_attempts: event.max_attempts,
    reason: event.reason,
    reason_text: reasonText(event.reason),
    backing_off_ms: event.backing_off_ms,
    resets_at: event.resets_at ? Date.parse(event.resets_at) : null,
  };
  break;
}

function reasonText(r: 'rate_limit' | 'server_error' | 'network'): string {
  switch (r) {
    case 'rate_limit':   return 'rate limit';
    case 'server_error': return 'server error';
    case 'network':      return 'network error';
  }
}
```

The `reasonText` function is the source of truth for the
human-rendered reason; it's referenced by
`render_retry_modifier_for(view)` and `render_frozen_retry_modifier`
in `working-phase-visibility.allium` (which the spec models
abstractly; the implementation is this helper).

### StateBar derivation

No change to the StateBar derivation rules themselves — those live in
`working-phase-visibility.allium` and read from `TurnRetryContext`.
The change here is that `TurnRetryContext` is now populated by a real
rule (this spec's `TurnRetryContextUpdatedOnLlmAttempt`) instead of
the PLACEHOLDER stub.

### Post-hoc badge rendering

`ui/src/components/MessageComponents.tsx` (`AssistantMessage`
component) checks `display_data.retry_count > 0` and renders a small
`(retried Nx)` badge next to the assistant message header. Same DOM
locale as the existing `duration_ms` rendering on tool results.

## Schema Changes

None server-side: `display_data` is a JSON-typed column, and `retry_count`
is one more typed field on the `MessageDisplayData` struct
(ts-rs-exported, same path as `tool_starts`). The runtime writes the
field at persist time; on read, `#[serde(default)]` covers old rows
that predate the field.

The use of `#[serde(default)]` here is a deliberate rollout shim
(old assistant messages never had a `retry_count`), not a permanent
schema-evolution workaround — see AGENTS.md "Schema evolution belongs
in migrations, not serde annotations." For this field there is no
need for a backfill: pre-existing rows are correctly interpreted as
`retry_count = 0` (zero retries) by absence, which matches the
display rule (badge hidden iff `retry_count = 0`).

## ts-rs Codegen

The `LlmAttempt` variant and the `LlmAttemptReason` enum both
require `./dev.py codegen` to regenerate `ui/src/generated/`. The
`retry_count: u32` field on `MessageDisplayData` likewise needs
codegen if that struct is ts-rs-exported (audit at implementation
time — the sibling spec already added `tool_starts` and noted the
codegen path).

The `parity_*` tests in `crates/phoenix-ide/src/api/sse.rs` must be
updated to include the new variant in expected JSON output. The
valibot schema in `ui/src/sseSchemas.ts` adds an `LlmAttemptSchema`
and the SSE-event reducer dispatch picks it up.

## Open Questions

None. Decisions resolved during drafting:

- **`LlmAttempt` vs `RateLimitSnapshot`:** Kept distinct (REQ-LRV-007).
  Different lifecycles and ring-eligibility rules; unifying them would
  degrade reconnect correctness for codex clients.
- **`max_attempts` source:** Global `MAX_RETRY_ATTEMPTS = 3` in
  `transition.rs:183`. Per-provider policies are deferred (the wire
  field is per-event, so future per-provider behaviour is
  wire-compatible without spec change).
- **`retry_count` carrier:** Typed `retry_count: u32` field on the
  assistant message's `display_data` (`MessageDisplayData` struct,
  ts-rs-exported). Mirrors `tool_starts` and the duration_ms-on-
  tool-result convention. Rejected: a new SSE variant (parallel
  representation), or a free-form JSON key (loss of compile-time
  shape).
- **Sub-agent retries:** Parent stays mute (REQ-LRV-004). Each
  conversation surfaces its own retry context only.
- **Cancellation during backoff:** No new event needed — the existing
  `Cancelling*` transitions are the abort signal; stale
  `RetryTimeout`s are already filtered (REQ-LRV-005).
- **Replay-ring eligibility for `LlmAttempt`:** Included
  (`EphemeralEventAppendedToReplayRing` whitelist update). Without
  it, reconnect during retry would lose the retry suffix on the
  StateBar.
- **Plumbing `reason` and `resets_at`:** Widen
  `Effect::ScheduleRetry` and `Event::LlmError` to carry them
  (Option A in design.md). Option B (side-channel lookup at emit
  time) rejected for coupling the executor to codex-specific quota
  state.
- **`backing_off_ms` informational:** V1 does not render a live
  countdown; the field exists for a future "backing off Ns"
  sub-display and for log/debug visibility. Replay-staleness of
  this field is acceptable for V1 because no rule depends on it.
