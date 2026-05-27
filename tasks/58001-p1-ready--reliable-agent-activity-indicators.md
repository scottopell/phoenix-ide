# Reliable "what is the agent doing" indicators — implementation

## Status

Specs are stable (foundational spec done in task 58002; sibling
spec `llm-retry-visibility` drafting tracked in task 58003 and
required before Stage B can land). This task tracks the
**implementation** of the indicators against the spec set in
`specs/working-phase-visibility/`.

Spec authoring discipline that came out of PR #155 is captured
in task 58004 — read before drafting any extension specs.

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
a **base reason** derived from the phase, plus an optional **retry
modifier** when a retry is unresolved (see REQ-WPV-003). Examples:

    thinking 4s                          (llm_requesting, pre-first-byte, no retry)
    executing bash 12s                   (tool_executing, no retry)
    executing bash 12s (retry 2/5)       (tool_executing, retry from earlier in turn)
    thinking 4s (retry 2/5 after 429)    (llm_requesting, retry from current attempt)

Retry is a **modifier on the base reason**, NOT a replacement for it —
the phase is always the primary signal of "what is the agent doing
right now?"; retry answers the secondary "why is it taking this
long?". Do not layer multiple live timers in the StateBar — exactly
one elapsed counter, derived from the phase's state_updated_at.

## Current state (pointers)

- `ui/src/components/StateBar.tsx:283-297, 408-409` — existing
  `toolExecutingStartedAt` elapsed-seconds pattern (template to copy)
- `ui/src/components/StateBar.tsx:349-373` — connection state currently
  short-circuits and *masks* agent state during reconnect
- `crates/phoenix-ide/src/api/wire.rs` — SseWireEvent variants;
  `StateChange` doesn't carry `state_updated_at` on the wire even though
  `Conversation.state_updated_at` already exists on the row
  (`db/schema.rs:476`, bumped on every transition via `db.rs:676`)
- `crates/phoenix-ide/src/llm/error.rs:81-105` — retry classification
- `crates/phoenix-ide/src/llm/anthropic.rs` — retry loop lives inside
  `complete_streaming`; no event emitted on retry
- `crates/phoenix-ide/src/runtime/executor.rs:1630, 1740-1764` — LLM
  dispatch + token forwarder; natural place to emit first-byte event

## Proposed stages

Each stage stands alone and ships value independently.

### Stage A — foundation (no retry visibility yet)

- Add `state_updated_at: DateTime<Utc>` to `StateChange` wire event
  (sourced from the already-bumped `Conversation.state_updated_at`).
  Serialise as RFC3339 to match the existing Init carrier (which is
  the same `DateTime<Utc>` flattened from `Conversation` and already
  on the wire as an RFC3339 string). Init payload unchanged. Client
  converts to ms once at the SSE-handler boundary via `Date.parse(s)`.
  Regenerate `ui/src/generated/`.
- Frontend: replicate the `toolExecutingStartedAt` pattern for every
  working phase (`llm_requesting`, `awaiting_llm`,
  `seeded_llm_requesting`, `awaiting_sub_agents`,
  `awaiting_continuation`, etc.) keyed off `state_updated_at`.
- Inline status on in-flight artifacts: render a placeholder assistant
  bubble with `"thinking 4s..."` while `llm_requesting` and no tokens
  have arrived; show elapsed time on the running tool widget header
  (not only StateBar).
- Client-side heartbeat watchdog: if `connectionState === 'connected'`
  and `convState` is a working phase and no SSE event observable to
  `EventSource` has arrived for >35s, downgrade indicator to
  `"no signal from server for Ns"`. Prerequisite: switch the server
  keep-alive from an SSE comment (`: ping\n\n`, invisible to standard
  `EventSource`) to a typed `event: ping` so the client can observe it
  via an explicit listener. See `specs/working-phase-visibility/`
  REQ-WPV-004 + design.md "Server keep-alive observation."
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
- A wedged server is detected within ~35s and surfaced (the spec sets
  HEARTBEAT_WATCHDOG_SECONDS=35; ~2.3x the 15s server keep-alive
  interval, so a single missed keep-alive doesn't false-positive).
- A reconnect during a working phase preserves the user's
  understanding of what was happening.
