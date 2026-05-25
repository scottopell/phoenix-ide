# Reliable "what is the agent doing" indicators

## Problem

When a conversation is in any "working" phase the UI shows a generic
spinner / "thinking..." with no specificity. The user cannot
distinguish:

- A long-running tool (e.g. bash compile)
- A slow LLM call (high TTFT)
- An LLM retry loop (429 / 5xx backoff happening *inside*
  `complete_streaming`, invisible to the executor and therefore to the
  UI)
- A wedged server (TCP still open, no events flowing)
- A reconnect where the previous activity is forgotten by the UI

Priority is reliability: the user should *always* have an accurate idea
of what is going on, and no failure mode should be silent.

## Principle

One inline timer per in-flight artifact (active tool widget, pending
assistant message) + one global StateBar summary. The StateBar shows
the *most specific known reason*, with precedence:

    retry > rate-limit > tool name > "thinking"

Do not layer multiple live timers in the StateBar — pick the most
specific reason and surface that.

## Current state (pointers)

- `ui/src/components/StateBar.tsx:283-297, 408-409` — existing
  `toolExecutingStartedAt` elapsed-seconds pattern (template to copy)
- `ui/src/components/StateBar.tsx:349-373` — connection state currently
  short-circuits and *masks* agent state during reconnect
- `crates/phoenix-ide/src/api/wire.rs` — SseWireEvent variants;
  `StateChange` has no `entered_at` timestamp
- `crates/phoenix-ide/src/llm/error.rs:81-105` — retry classification
- `crates/phoenix-ide/src/llm/anthropic.rs` — retry loop lives inside
  `complete_streaming`; no event emitted on retry
- `crates/phoenix-ide/src/runtime/executor.rs:1630, 1740-1764` — LLM
  dispatch + token forwarder; natural place to emit first-byte event

## Proposed stages

Each stage stands alone and ships value independently.

### Stage A — foundation (no retry visibility yet)

- Add `entered_at: i64` to `StateChange` wire event and to the phase
  carried in `Init` (so timers survive reconnect / page reload).
  Regenerate `ui/src/generated/`.
- Frontend: replicate the `toolExecutingStartedAt` pattern for every
  working phase (`llm_requesting`, `awaiting_llm`,
  `seeded_llm_requesting`, `awaiting_sub_agents`,
  `awaiting_continuation`, etc.) keyed off the new `entered_at`.
- Inline status on in-flight artifacts: render a placeholder assistant
  bubble with `"thinking 4s..."` while `llm_requesting` and no tokens
  have arrived; show elapsed time on the running tool widget header
  (not only StateBar).
- Client-side heartbeat watchdog: if `connectionState === 'connected'`
  and `convState` is a working phase and no SSE event of any kind
  (including SSE comments / keep-alives) has arrived for >N seconds
  (suggest N=20), downgrade indicator to
  `"no signal from server for Ns"`.
- Stop letting connection state mask agent state: when reconnecting,
  show both — `"reconnecting (2) — agent was thinking 12s ago"`.

### Stage B — retry visibility (biggest trust win)

- New SSE event `LlmAttempt { attempt, max, reason, backing_off_ms,
  resets_at? }` emitted from the retry loop inside the LLM clients
  (anthropic.rs, openai.rs, fireworks.rs).
- StateBar consumes it and shows
  `"anthropic retry 2/5, backing off 4s after rate limit"` per the
  precedence rule above.
- Persist nothing — purely ephemeral; on reconnect the next attempt's
  event (or success) supersedes it.

### Stage C — sub-phase split for the LLM call

- Emit `LlmFirstByte` (or set a flag on the first `Token` event) from
  the forwarder task in `executor.rs:1740-1764`.
- StateBar transitions
  `"thinking 4.1s"` → `"streaming"` once first byte arrives.
- Optionally: post-hoc TTFT shown on the assistant message bubble
  (like the existing `duration_ms` on tool results).

## Constraints / non-goals

- Do NOT add multiple live timers in the StateBar simultaneously —
  enforce the precedence rule.
- Wire format changes need ts-rs regeneration + parity tests
  (`api/sse.rs` parity_* tests) updated.
- Heartbeat watchdog must not fire spuriously during legitimate long
  LLM streams that *are* sending tokens — gate on "no events of any
  kind" not "no StateChange".

## Acceptance

- During every working phase the user sees, somewhere on-screen, (a) a
  live elapsed counter and (b) the most specific reason currently
  known.
- LLM retries are visible (not buried inside a 45s "thinking..."
  silence).
- A wedged server is detected within ~20s and surfaced.
- A reconnect during a working phase preserves the user's
  understanding of what was happening.
