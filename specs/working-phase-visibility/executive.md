# Working-Phase Visibility — Executive Summary

## Requirements Summary

A reliable conversation UI must always answer "what is the agent doing right
now?" with specificity. Today the StateBar conflates every working phase
(`llm_requesting`, `tool_executing`, `awaiting_sub_agents`, ...) into a
generic spinner with a phase-name label; only `tool_executing` has an
elapsed-time counter, and connection-state messaging masks agent state
entirely during reconnects. This spec adds:

- A **server-authoritative phase entry timestamp** (`entered_at`) on every
  `StateChange` and on the phase carried in `Init`, so elapsed times survive
  reconnect, page reload, and multi-tab observation.
- **Inline elapsed-time indicators on in-flight artifacts** (tool widgets;
  a placeholder assistant bubble during `llm_requesting`), so the activity
  signal is at the point of expectation rather than only in a header.
- A **StateBar derivation rule**: one base reason from the phase + optional
  retry-modifier from `specs/llm-retry-visibility/`, with exactly one live
  timer on screen at a time (no layered counters).
- A **heartbeat watchdog**: if no SSE event of any kind (including
  server-typed `ping` events) arrives for 35 seconds during a working phase,
  surface `"no signal from server for Ns"` so wedged-server hangs become
  visible.
- **Connection-state does not mask agent state**: during reconnect, display
  both the connection chip AND the last-known agent activity with elapsed
  frozen at disconnect (`reconnecting (2) — last: thinking 12s`).
- A **first-byte sub-phase split**: `llm_requesting` displays as `thinking
  Ns` until the first token arrives, then transitions to `streaming` (no
  counter, since the stream itself is the progress signal).

## Technical Summary

Wire format changes are additive:

- `SseWireEvent::StateChange` gains `entered_at: i64` (unix milliseconds,
  server clock).
- The phase carried in `Init.conversation` gains `entered_at: i64` so a
  fresh client matches the live-connected display.
- New `SseWireEvent::LlmFirstByte { request_id, sequence_id }` emitted from
  the token forwarder in `executor.rs:1740-1764`, exactly once per LLM
  request, immediately before the first `Token` event for that request.
- Tool-use block `display_data` gains `started_at: i64`, set by
  `dispatch_tool_execution` and propagated via `MessageUpdated` (reuses the
  same convention as `duration_ms`, no new wire variant).
- SSE keep-alive switches from a comment line to a typed `event: ping`
  payload (small change in `api/sse.rs:69-73` and `handlers.rs:3277-3281`)
  so the client `EventSource` observes it for the watchdog. Forward-
  compatible: legacy clients that don't listen for `ping` ignore it.

Client-side, a shared `useElapsedSeconds(startedAt)` hook (extracted from
the existing `toolElapsedSeconds` pattern at `StateBar.tsx:283-297`) feeds
both the StateBar and the inline indicators. The conversation atom gains
`phaseEnteredAt`, `toolStartedAt` (per tool-use-id), and `lastSseEventAt`
fields. The StateBar's display string is derived from a single function
that takes `(connectionState, convState, phaseEnteredAt, lastKnownActivity,
retryContext, degradedSignal, hasFirstByte)`.

No database schema changes. All persistence concerns for retry context live
in the sibling spec `specs/llm-retry-visibility/`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-WPV-001:** Server-authoritative phase entry timestamp | ❌ New | Adds `entered_at` to `StateChange` and to `Init.conversation.phase`; ts-rs regen + `parity_*` test update required |
| **REQ-WPV-002:** Inline elapsed-time on in-flight artifacts | ❌ New | Tool widget timer via `display_data.started_at`; pending assistant bubble retains empty agent message during `llm_requesting` |
| **REQ-WPV-003:** StateBar derivation rule | 🔄 Extend | Existing `StateBar.tsx:341-422` composition gains retry-modifier and degraded-signal precedence; existing `tool_executing` timer path becomes a special case of the generalised rule |
| **REQ-WPV-004:** Heartbeat watchdog | ❌ New | Threshold 35s; depends on keep-alive switch from SSE comment to typed `ping` event |
| **REQ-WPV-005:** Connection state does not mask agent state | 🔄 Rewrite | `StateBar.tsx:349-373` currently short-circuits; replace with composition that retains last-known activity with frozen elapsed |
| **REQ-WPV-006:** Pending assistant bubble | 🔄 Extend | Change `MessageComponents.tsx:635-638` empty-message filter to retain the live in-flight target |
| **REQ-WPV-007:** First-byte sub-phase distinction | ❌ New | Driven by new `LlmFirstByte` event; `streaming` displays without elapsed counter |
| **REQ-WPV-008:** Display continuity across reload | ❌ New | Acceptance criterion for REQ-WPV-001 + REQ-WPV-005; integration-test target |

**Progress:** 0 of 8 implemented. REQ-WPV-003, REQ-WPV-005, REQ-WPV-006 are
extensions/rewrites of existing logic; REQ-WPV-001/002/004/007/008 are
greenfield additions on existing infrastructure.

## Cross-Spec Dependencies

- **`specs/llm-retry-visibility/`** — sibling spec, drafted in a follow-up
  commit (not yet present in this branch). Will produce the retry-modifier
  context that REQ-WPV-003 composes onto the base reason. Until that spec
  lands, the `RetryContext` value type is inlined in this spec's Allium
  file as a `PLACEHOLDER` block; on landing, the `use
  "../llm-retry-visibility/llm-retry-visibility.allium" as llm_retry`
  import is restored and the placeholder is deleted.

- **`specs/sse_wire/`** — wire-format authority; new variants (`LlmFirstByte`,
  `entered_at` on `StateChange`) and the keep-alive typed-event switch are
  changes that the sse_wire invariants must continue to hold (replay ring
  semantics, sequence_id monotonicity, etc.). The `parity_*` tests in
  `api/sse.rs` enforce byte-for-byte parity.

- **`specs/connection_machine/`** — owns `connectionState` transitions.
  REQ-WPV-005 reads `connectionState` but does not modify it. The Allium
  spec imports `connection_machine.allium`.

## Behavioural Specification

The corresponding Allium spec is
`specs/working-phase-visibility/working-phase-visibility.allium`. It models:

- `WorkingPhase` enum (the subset of conversation phases that are
  "working"), and the implicit `IdlePhase` complement.
- `DisplayedActivity` entity: the composition of `(phase, connection_state,
  retry_context, degraded_signal, first_byte_received)` into a single
  rendered text + dot-class. Transitions track the StateBar's derivation
  rules.
- `LastKnownActivity` value: captured on the `connected → reconnecting`
  edge, frozen until the next `Init` lands; carries phase, base reason
  text, and the elapsed value at the moment of disconnect.
- `HeartbeatWatchdog` state: `fresh → stale` after
  `HEARTBEAT_WATCHDOG_SECONDS` without an SSE event; transitions back to
  `fresh` on any event. Invariant: `stale` is reachable only from
  `connected` (a reconnecting/offline socket already conveys the same
  information).
- `PendingAssistantBubble` lifecycle: `not_present → placeholder`
  (entering `llm_requesting`, no tokens) → `streaming` (first token
  received) → `complete` (response finalised) | `removed` (phase exited
  without tokens).
- Invariants: exactly one live timer in the StateBar at a time; the
  inline-artifact timer (on a tool or pending bubble) is independent and
  may coexist; `LastKnownActivity` is set iff `connectionState !=
  connected` and we transitioned from a working phase.
- Surface `ConversationActivityFeed` facing the UI, exposing the derived
  `DisplayedActivity` (and the per-artifact elapsed values) without
  exposing the underlying composition machinery.

The deferred entry `MultipleParallelToolsInOneAssistantMessage` documents
the assumption that tool execution is sequential within a turn. If parallel
dispatch arrives, the per-tool `started_at` carrier (REQ-WPV-002) already
supports it; only the StateBar's tool-name rendering needs revisiting.

Open questions: none. Decisions resolved during the elicitation session
dated 2026-05-25 — see the `Open questions` block at the bottom of
`working-phase-visibility.allium` for the full list.
