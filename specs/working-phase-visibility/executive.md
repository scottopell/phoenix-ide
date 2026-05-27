# Working-Phase Visibility — Executive Summary

## Requirements Summary

A reliable conversation UI must always answer "what is the agent doing right
now?" with specificity. Today the StateBar conflates every working phase
(`llm_requesting`, `tool_executing`, `awaiting_sub_agents`, ...) into a
generic spinner with a phase-name label; only `tool_executing` has an
elapsed-time counter, and connection-state messaging masks agent state
entirely during reconnects. This spec adds:

- A **server-authoritative state-entry timestamp** — the existing
  `Conversation.state_updated_at: DateTime<Utc>` (already bumped on every
  transition, `db.rs:676`) is added to the `StateChange` wire event and
  already exposed on `Init.conversation` via `#[serde(flatten)]`, so
  elapsed times survive reconnect, page reload, and multi-tab observation
  with no new field on the conversation row.
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

- `SseWireEvent::StateChange` gains `state_updated_at: DateTime<Utc>`,
  serialised as RFC3339 on the wire — the same shape as the existing
  Init carrier so both arrive in the same form. Client converts to ms
  once at the SSE-handler boundary.
- No new field on `Init.conversation`: the existing flattened
  `Conversation.state_updated_at` is already at the top level of the
  Init payload.
- New `SseWireEvent::LlmFirstByte { request_id, sequence_id }` emitted from
  the forwarder task in `executor.rs`, exactly once per LLM
  request, immediately before the first `Token` event for that request.
- The assistant message's `display_data` gains a typed
  `tool_starts: BTreeMap<String, i64>` map (keyed by `tool_use_id`,
  unix-ms values), populated by `dispatch_tool_execution` and propagated
  via `MessageUpdated` — message-level rather than tool-use-block-level
  because `ContentBlock::ToolUse` is `{id, name, input}` with no
  display_data field and is persisted/cross-provider (a typed field
  there would require schema migration for a UI-side value). Mirrors the
  duration_ms-on-tool-result-display_data convention; no new wire
  variant.
- SSE keep-alive switches from a comment line to a typed `event: ping`
  payload (small change in `api/sse.rs:69-73` and `handlers.rs:3355-3359`)
  so the client `EventSource` observes it for the watchdog. Forward-
  compatible: legacy clients that don't listen for `ping` ignore it.

Client-side, a shared `useElapsedSeconds(startedAt)` hook (extracted from
the existing `toolElapsedSeconds` pattern at the `toolElapsedSeconds` pattern in `StateBar.tsx`) feeds
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
| **REQ-WPV-001:** Server-authoritative state-entry timestamp | ❌ New | Adds `state_updated_at` to `StateChange` (sourced from existing `Conversation.state_updated_at`); Init already exposes it via flatten. ts-rs regen + `parity_*` test update required |
| **REQ-WPV-002:** Inline elapsed-time on in-flight artifacts | ❌ New | Tool widget timer reads from the assistant message's `display_data.tool_starts[tool_use_id]` map (typed `BTreeMap<String, i64>`); pending assistant bubble is the synthetic render unit specified by REQ-WPV-006 below |
| **REQ-WPV-003:** StateBar derivation rule | 🔄 Extend | Existing the `stateText` composition block in `StateBar.tsx` composition gains retry-modifier and degraded-signal precedence; existing `tool_executing` timer path becomes a special case of the generalised rule |
| **REQ-WPV-004:** Heartbeat watchdog | ❌ New | Threshold 35s; depends on keep-alive switch from SSE comment to typed `ping` event |
| **REQ-WPV-005:** Connection state does not mask agent state | 🔄 Rewrite | the connection short-circuit in `StateBar.tsx` currently short-circuits; replace with composition that retains last-known activity with frozen elapsed |
| **REQ-WPV-006:** Pending assistant bubble | ❌ New | Synthetic `pending_agent` tail unit added to `ui/src/conversation/renderUnits.ts` (parallel to the existing `streaming_agent` unit), gated on `atom.phase.type === 'llm_requesting'` + empty `streamingBuffer`. The `MessageComponents.tsx` empty-message filter (`hasRenderableContent` guard) (`hasRenderableContent` guard in `MessageComponents.tsx`) is NOT changed — the placeholder is a derived render unit, not a message row |
| **REQ-WPV-007:** First-byte sub-phase distinction | ❌ New | Driven by new `LlmFirstByte` event; `streaming` displays without elapsed counter |
| **REQ-WPV-008:** Display continuity across reload | ❌ New | Acceptance criterion for REQ-WPV-001 + REQ-WPV-005; integration-test target |

**Progress:** 0 of 8 implemented. REQ-WPV-003 and REQ-WPV-005 are
extensions/rewrites of existing logic; the other six are greenfield
additions on existing infrastructure.

## Cross-Spec Dependencies

- **`specs/llm-retry-visibility/`** — sibling spec, drafted in a follow-up
  commit (not yet present in this branch). Will produce the retry-modifier
  context that REQ-WPV-003 composes onto the base reason. Until that spec
  lands, the `RetryContext` value type is inlined in this spec's Allium
  file as a `PLACEHOLDER` block; on landing, the `use
  "../llm-retry-visibility/llm-retry-visibility.allium" as llm_retry`
  import is restored and the placeholder is deleted.

- **`specs/sse_wire/`** — wire-format authority; new variants (`LlmFirstByte`,
  `state_updated_at` on `StateChange`) and the keep-alive typed-event switch are
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
- `PendingAssistantBubble` lifecycle (reusable across turns; idle
  ground state is `not_present`): `not_present → placeholder` (entering
  `llm_requesting`, no tokens) → `streaming` (first token received) →
  `not_present` on the next assistant `Message` event (turn complete).
  The "phase exited llm_requesting without any token" path also returns
  the bubble to `not_present`. The state enum has three values —
  `not_present | placeholder | streaming` — no terminal states, so the
  bubble can re-arm on every subsequent turn.
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
