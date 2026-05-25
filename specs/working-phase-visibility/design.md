# Working-Phase Visibility — Design

## Overview

Three surfaces, one wire change, one client state-machine extension.

1. **Wire** (`crates/phoenix-ide/src/api/wire.rs`): `StateChange` and the
   phase carried in `Init` gain a server-authoritative `entered_at` (unix
   ms). New optional SSE event `LlmFirstByte` marks the boundary inside
   `llm_requesting`.
2. **Runtime** (`crates/phoenix-ide/src/runtime/executor.rs`): stamp
   `entered_at` whenever a phase is entered; emit `LlmFirstByte` from the
   token forwarder when the first token of an LLM request arrives. Stamp
   per-tool `started_at` when `dispatch_tool_execution` begins.
3. **Client** (`ui/src/conversation/atom.ts`, `ui/src/components/StateBar.tsx`,
   `ui/src/components/MessageComponents.tsx`): derive elapsed times from
   server timestamps; render inline indicators on in-flight artifacts;
   implement the heartbeat watchdog; compose display from `(phase,
   connection_state, retry_context)` per REQ-WPV-005.

## Wire Format Changes

### `StateChange.entered_at: i64` (new field)

```rust
StateChange {
    sequence_id: i64,
    #[ts(type = "unknown")]
    state: Value,
    presentation_mode: String,
    /// Unix milliseconds (server clock) at which the conversation entered
    /// this phase. Lets clients display elapsed time deterministically
    /// across reconnect / reload and across multiple tabs viewing the
    /// same conversation.
    entered_at: i64,
}
```

### `Init.conversation.phase.entered_at: i64` (new nested field)

The `EnrichedConversation` struct embedded in `Init.conversation` already
carries the current phase. Add `entered_at` to that phase struct so a fresh
client reconstructs the same elapsed time. Per the project's "no parallel
representations" principle, the field name and semantics MUST match the one
on `StateChange`.

### `LlmFirstByte { request_id, sequence_id }` (new variant)

```rust
LlmFirstByte {
    sequence_id: i64,
    /// Matches the `request_id` carried on `Token` events for the same
    /// LLM request, so the client can correlate the first-byte transition
    /// to the right pending bubble.
    request_id: String,
}
```

Emitted exactly once per LLM request, from the token forwarder in
`executor.rs:1740-1764`, immediately before the first `Token` event for
that request is forwarded. The two events are emitted atomically (no
client can observe a `Token` for a request without first observing the
`LlmFirstByte`). When an LLM request completes with zero tokens (an error
or early termination), `LlmFirstByte` is NOT emitted.

### Per-tool `started_at` on the tool-use block

Tool execution start time is server-authoritative for the same reasons as
phase entry. Two viable carriers; the implementation chooses one:

**Option A (preferred):** Add `started_at: Option<i64>` to the tool-use
block's `display_data` JSON, set when `dispatch_tool_execution` begins, and
propagate via `MessageUpdated`. This piggybacks on an existing event type
and reuses the display_data convention already used for `duration_ms`.

**Option B:** New `ToolExecutionStarted { sequence_id, tool_use_id,
started_at }` SSE event. More explicit but adds wire surface for a value
the client only needs as an input to a derived display.

Decision: Option A. The tool-use block's `display_data` is already the
home for execution metadata (cf. `duration_ms` injection in
`schema.rs:649-655`); a parallel mechanism would violate the
"no-parallel-representations" rule.

## Runtime Changes

### Phase entry timestamping

`executor.rs` already calls a single helper to transition to a new phase
(broadcasting `StateChange`). Extend that helper to capture
`SystemTime::now()` as unix milliseconds and include it on the wire event.
Persist nothing: a reconnect after the next phase transition gets the new
phase's `entered_at` in the next `StateChange`; a reconnect during the
current phase gets it in `Init` (the executor already knows the current
phase's entry time because it's the one who set it).

### First-byte emission

The forwarder task at `executor.rs:1740-1764` subscribes to the LLM client's
chunk channel and emits `SseEvent::Token` for each text chunk. Wrap that
loop with a "first chunk seen for this request_id" flag; on the first
chunk, emit `LlmFirstByte` immediately before the `Token`. The flag is
per-request and lives only in the forwarder task's stack.

### Per-tool started_at

`dispatch_tool_execution` (in `executor.rs`) is the single point at which a
tool transitions from "block exists" to "actually running." Capture
`SystemTime::now()` there, write it into the tool-use block's `display_data`
via the existing `MessageUpdated` path. No new wire variant.

## Client Changes

### Atom state additions

`ui/src/conversation/atom.ts` already tracks per-phase state via the
`convState` discriminated union. Add:

```ts
// On the phase atom (or alongside convState):
phaseEnteredAt: number | null  // unix ms, from server

// On the inline-artifact atoms (tool-use blocks, pending assistant bubble):
toolStartedAt: Record<ToolUseId, number>  // from display_data.started_at

// On the connection observer:
lastSseEventAt: number  // unix ms, client clock — for the watchdog
```

`phaseEnteredAt` is updated on every `StateChange` event and on `Init`.
`toolStartedAt` is updated when a `MessageUpdated` arrives with
`display_data.started_at` set on a tool-use block. `lastSseEventAt` is
updated on every event observable to the client `EventSource` — including
the typed `ping` keep-alive once the server-side switch described in the
next section ships. Standard `EventSource` does NOT surface SSE comment
lines, so the current `: ping\n\n` keep-alive is invisible to the
watchdog until that switch lands.

### Server keep-alive observation

The SSE keep-alive sent by axum's `KeepAlive` API is an SSE comment line
(`: ping\n\n`). Standard `EventSource` does NOT fire any handler for
comments. To observe them for the watchdog, two options:

**Option A (preferred):** Switch the SSE keep-alive to a typed event with
`event: ping` and `data: ""`. EventSource fires the `ping` event handler;
the client listens and bumps `lastSseEventAt`. Minimal Rust change
(`api/sse.rs:69-73`, `handlers.rs:3277-3281`): replace
`.text("ping")` with `.event("ping").text("")`. Forward-compatible:
clients that don't listen for `ping` simply ignore it.

**Option B:** Use a polyfilled EventSource implementation that surfaces
comments. Larger client dependency for a small need.

Decision: Option A. Document the keep-alive event type in the spec so
downstream consumers (clients other than the Phoenix UI) know it exists.

### Heartbeat watchdog hook

A single React effect in the page-level conversation component (or in a
shared hook) compares `Date.now() - lastSseEventAt` every second; when it
exceeds 35 000 ms AND `connectionState === 'connected'` AND `convState` is
a working phase, set a `degradedSignal: true` flag on the StateBar's input.
Cleared the moment any SSE event arrives.

### StateBar derivation

`StateBar.tsx:341-422` already composes its `stateText` from
`(connectionState, convState)`. Extend the composition:

```ts
function deriveStateBarText({
  connectionState, connectionAttempt,
  convState, phaseEnteredAt,
  lastKnownActivity,   // frozen at disconnect (REQ-WPV-005)
  retryContext,        // from llm-retry-visibility
  degradedSignal,      // from REQ-WPV-004
  hasFirstByte,        // from LlmFirstByte for the current request
}): { dotClass: string; text: string }
```

Output rules (informal, see `working-phase-visibility.allium` for the
formal derivation):

- `connectionState in {reconnecting, offline}` AND `lastKnownActivity != null`
  → `"reconnecting (N) — last: <activity>"` with the activity's elapsed
  frozen at disconnect time.
- `degradedSignal` → `"no signal from server for Ns"` (overrides the working
  phase text; the user needs to know the channel is suspect before they
  trust any further detail).
- Working phase, no retry, no first byte yet, `llm_requesting` → `"thinking
  Ns"`.
- Working phase, no retry, first byte received, `llm_requesting` →
  `"streaming"` (no counter, per REQ-WPV-007).
- Working phase, no retry, `tool_executing` → `"executing <tool_name> Ns"`.
- Working phase + retry → base reason + ` (retry K/N <reason>)`.

### Inline indicators

**Tool widget timer:** `MessageComponents.tsx` `ToolUseBlock` renders the
tool header (lines 1037-1046). When the block has `display_data.started_at`
and no `result` yet, render the elapsed counter inline in the header. Tick
via the same one-second interval pattern used for the StateBar's
`toolElapsedSeconds` (StateBar.tsx:283-297) — extract to a shared
`useElapsedSeconds(startedAt)` hook.

**Pending assistant bubble:** `MessageComponents.tsx:635-638` filters out
empty agent messages. Change the filter to *retain* an empty agent message
when it is the live in-flight target (identified by being the most recent
agent message whose enclosing conversation is in `llm_requesting` AND no
tokens have arrived for the current request_id). The bubble renders the
elapsed counter where the streamed text will appear; on the first token,
the streaming text replaces the counter in-place.

## Connection-State Composition

The Allium spec captures this formally. Informal summary:

| `connectionState` | `convState` working? | Display |
|-------------------|----------------------|---------|
| `connected`       | no (idle/terminal)   | phase text only, no timer |
| `connected`       | yes                  | base + retry-modifier + live timer |
| `reconnecting`    | yes                  | `reconnecting (N) — last: <frozen>` |
| `reconnecting`    | no                   | `reconnecting (N)` |
| `offline`         | (any)                | `offline` (no last-known fallback if we don't have one) |

The frozen `lastKnownActivity` is captured the moment `connectionState`
leaves `connected`, and cleared the moment a fresh `Init` lands.

## Schema Changes

None. All new fields are on wire events; no DB columns affected.

## ts-rs Codegen

The `StateChange.entered_at` and `LlmFirstByte` additions both require
`./dev.py codegen` to regenerate `ui/src/generated/SseWireEvent.ts`. The
`parity_*` tests in `crates/phoenix-ide/src/api/sse.rs` must be updated to
include the new field in expected JSON output. The valibot schemas in
`ui/src/sseSchemas.ts` need a `LlmFirstByteSchema` and an extension of
`StateChangeSchema` to include `entered_at`.

## Open Questions

None. Decisions resolved during elicitation (session of 2026-05-25):

- Elapsed counter under `streaming` base reason: suppressed; the stream
  itself is the progress signal (REQ-WPV-007).
- Server keep-alive observability: switched from SSE comment to typed
  `event: ping` so the client watchdog observes it.
- `entered_at` on `Init`'s phase: required; without it, a fresh client
  cannot reconstruct the displayed elapsed time and REQ-WPV-008 fails.
- Per-tool `started_at` carrier: `display_data` on the tool-use block,
  reusing the `duration_ms` convention.
- Frozen-elapsed during reconnect: counter does not advance through the
  disconnect (honest about what we know).
