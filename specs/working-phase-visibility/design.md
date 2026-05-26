# Working-Phase Visibility — Design

## Overview

Three surfaces, one wire change, one client state-machine extension.

1. **Wire** (`crates/phoenix-ide/src/api/wire.rs`): `StateChange` gains
   a `state_updated_at: DateTime<Utc>` field (RFC3339 on the wire,
   matching the existing flattened Init carrier; sourced from the
   existing `Conversation.state_updated_at` row field). New SSE event
   `LlmFirstByte` marks the boundary inside `llm_requesting`. No new
   field on `Init` — the existing flattened `Conversation.state_updated_at`
   is already at the top level of the Init payload.
2. **Runtime** (`crates/phoenix-ide/src/runtime/executor.rs`): emit
   `LlmFirstByte` from the token forwarder when the first token of an
   LLM request arrives; inject `started_at` into the assistant message's
   `display_data.tool_starts` map when `dispatch_tool_execution` begins.
   No new server-side tracking for state entry — the existing
   `Conversation.state_updated_at` row update is reused.
3. **Client** (`ui/src/conversation/atom.ts`, `ui/src/components/StateBar.tsx`,
   `ui/src/components/MessageComponents.tsx`): derive elapsed times from
   server timestamps; render inline indicators on in-flight artifacts;
   implement the heartbeat watchdog; compose display from `(phase,
   connection_state, retry_context)` per REQ-WPV-005.

## Wire Format Changes

### `StateChange.state_updated_at: DateTime<Utc>` (new field)

```rust
StateChange {
    sequence_id: i64,
    #[ts(type = "unknown")]
    state: Value,
    presentation_mode: String,
    /// The server clock at which the conversation entered this state —
    /// the same `Conversation.state_updated_at: DateTime<Utc>` value the
    /// runtime already bumps on every state transition (`db.rs:676`),
    /// re-emitted on every StateChange for parity with the Init carrier.
    /// Serialises as RFC3339 on the wire (matching the Init flatten);
    /// the client converts to ms once at the SSE-handler boundary.
    state_updated_at: DateTime<Utc>,
}
```

### `Init.conversation.state_updated_at: string` (already present, RFC3339)

`EnrichedConversation` flattens `Conversation` via `#[serde(flatten)]`
(`runtime.rs:618-620`), so the existing `Conversation.state_updated_at:
DateTime<Utc>` field is ALREADY at the top level of the Init payload's
`conversation` object. No new struct is required.

**Wire-format note:** `DateTime<Utc>` serialises to an RFC3339 string by
default (e.g. `"2026-05-25T20:24:36.123456789Z"`), not an integer. So
the wire shapes are asymmetric:

| Carrier | Wire type | Source field |
|---|---|---|
| `Init.conversation.state_updated_at` | RFC3339 string | `Conversation.state_updated_at: DateTime<Utc>` (flatten, unchanged) |
| `StateChange.state_updated_at` | RFC3339 string | same `DateTime<Utc>`, serialised the same way for parity |

Both arrive on the wire as strings. The client converts to a unix-ms
number once, at the SSE handler boundary (immediately when the event is
parsed, before reaching the conversation atom), and stores the
converted value in `phaseStateUpdatedAt: number | null`. From there
every consumer reads an integer — the conversion happens in exactly one
place. `Date.parse(rfc3339)` returns ms-since-epoch directly in JS, so
the converter is one line.

(Earlier drafts of this design typed `StateChange.state_updated_at` as
`i64`. Replaced with the string form so Init and StateChange have the
same wire type for the same value, avoiding two parallel
representations of the same field.)

(The original draft of this design proposed adding a new
`Init.conversation.phase.entered_at` nested field. That would have
required adding a new `phase` object alongside `state` — a parallel
representation of the same data. Replaced with the flattened
`state_updated_at` reuse described above.)

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

### Per-tool `started_at` carrier

Tool execution start time is server-authoritative for the same reasons as
phase entry. The challenge: `ContentBlock::ToolUse` is shaped `{id, name,
input}` only (`llm/types.rs:117-121`), it's persisted as JSON in
`messages.content`, and it crosses every LLM provider. Adding a new field
to it would require schema migration and provider-wide changes for a
purely UI-side metadata value. Three carriers considered:

**Option A — message-level `display_data.tool_starts` map (chosen).**
The parent assistant message's `display_data` (already mutable via the
`MessageUpdated` event path) gains an entry like:

```json
{
  "tool_starts": { "<tool_use_id>": 1716663041234, ... }
}
```

The runtime mutates this map when `dispatch_tool_execution` begins and
emits `MessageUpdated{display_data}` with the updated map. The client
reads `display_data.tool_starts[tool_use_id]` for the inline widget timer.
This is the same display_data side-channel pattern that
`ToolResult.duration_ms` rides on (cf. `schema.rs:649-655`, `wire.rs:259-264`)
— a mutable per-message blob for UI metadata that does not belong on the
LLM content block.

**Option B — typed field on `ContentBlock::ToolUse`.** Cleaner type story
but expensive: requires migrating `messages.content` JSON, threading
the field through all LLM provider response parsing, and reasoning about
its meaning at the LLM-provider boundary (where it has none). Rejected
for a UI-side metadata value.

**Option C — new `ToolExecutionStarted` SSE variant.** Rejected for the
same reason `duration_ms` doesn't have its own variant: parallel
representation with an existing carrier.

Decision: Option A. The new map key (`display_data.tool_starts`) is a
typed addition to the runtime's existing `MessageDisplayData` struct (or
equivalent) — implemented as a typed field on the Rust side, with
ts-rs codegen mirroring it to the client. The `display_data` JSON blob
remains the carrier, but the map itself is typed at compile time, not
free-form.

## Runtime Changes

### Phase entry timestamping

The runtime already updates `Conversation.state_updated_at` on every
state transition (`db.rs:676`, `db.rs:1491`). The only change is to
include that field on the new `StateChange.state_updated_at` wire field
when the executor broadcasts the event. No new client-side tracking, no
new server-side tracking: the value is read from the row that was just
updated, and propagated verbatim. On Init, the existing
`#[serde(flatten)]` on `EnrichedConversation` already exposes
`state_updated_at` at the top level.

### First-byte emission

The forwarder task at `executor.rs:1740-1764` subscribes to the LLM client's
chunk channel and emits `SseEvent::Token` for each text chunk. Wrap that
loop with a "first chunk seen for this request_id" flag; on the first
chunk, emit `LlmFirstByte` immediately before the `Token`. The flag is
per-request and lives only in the forwarder task's stack.

### Per-tool started_at injection

`dispatch_tool_execution` (in `executor.rs`) is the single point at which
a tool transitions from "block exists" to "actually running." When it
begins, the runtime mutates the parent assistant message's display_data
to set `tool_starts[tool_use_id] = now_unix_ms` and emits
`MessageUpdated{display_data}` with the updated map. The
`MessageDisplayData` struct (or its equivalent) gains a typed
`tool_starts: BTreeMap<String, i64>` field; ts-rs codegen mirrors the
shape to the client.

## Client Changes

### Atom state additions

`ui/src/conversation/atom.ts` already tracks per-phase state via the
`convState` discriminated union. Add:

```ts
// On the phase atom (or alongside convState):
phaseStateUpdatedAt: number | null  // unix ms, from server

// On the inline-artifact atoms (tool-use blocks, pending assistant bubble):
toolStartedAt: Record<ToolUseId, number>  // from display_data.tool_starts

// On the connection observer:
lastSseEventAt: number  // unix ms, client clock — for the watchdog
```

`phaseStateUpdatedAt` is updated on every `StateChange` event (from
`state_updated_at`) and on `Init` (from `conversation.state_updated_at`).
`toolStartedAt` is updated when a `MessageUpdated` arrives carrying a
`display_data.tool_starts` map; the reducer merges the map's entries
into the atom's per-tool-id record. `lastSseEventAt` is updated on every
event observable to the client `EventSource` (each per-event listener
bumps it before delegating; see the EventSource listener notes below) —
including the typed `ping` keep-alive once the server-side switch
described in the next section ships. Standard `EventSource` does NOT
surface SSE comment lines, so the current `: ping\n\n` keep-alive is
invisible to the watchdog until that switch lands.

### Server keep-alive observation

The SSE keep-alive sent by axum's `KeepAlive` API is an SSE comment line
(`: ping\n\n`). Standard `EventSource` does NOT fire any handler for
comments. To observe them for the watchdog, two options:

**Option A (preferred):** Switch the SSE keep-alive to a typed event with
`event: ping` and `data: ""`. EventSource fires the `ping` event handler;
the client listens and bumps `lastSseEventAt`. The axum 0.7 API for
KeepAlive carries an `Event`, not a raw string (cf.
<https://docs.rs/axum/latest/axum/response/sse/struct.KeepAlive.html>):

```rust
// Before (api/sse.rs:69-73, handlers.rs:3277-3281):
KeepAlive::new()
    .interval(Duration::from_secs(15))
    .text("ping")

// After:
KeepAlive::new()
    .interval(Duration::from_secs(15))
    .event(Event::default().event("ping").data("ping"))
```

The `data("ping")` payload is intentional and non-empty: per axum's
`Event::data` documentation
(<https://docs.rs/axum/latest/axum/response/sse/struct.Event.html#method.data>)
events with an empty `data` field are ignored by the browser, so
`data("")` would produce a wire frame the client never observes —
defeating the entire watchdog. The exact payload string is irrelevant
to the client (the listener just bumps `lastSseEventAt` on the named
`ping` event and discards the body), but it MUST be non-empty.

Forward-compatible: clients that don't listen for `ping` simply ignore it.

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

#### EventSource listener wiring (required)

Native `EventSource` has no wildcard for named SSE events — named events
only fire listeners registered for their specific event name (the default
`onmessage` only fires for unnamed events). To make
`lastSseEventAt` track *every* server-emitted event, the SSE client
layer (`ui/src/hooks/useConnection.ts` or its delegate) MUST register
an explicit listener for every event type the server emits, each
bumping `lastSseEventAt` before delegating to per-event reducer
handling. The current set, sourced from `SseWireEvent::event_type()`
(`crates/phoenix-ide/src/api/wire.rs`):

```
init, message, message_updated, state_change, token, agent_done,
conversation_became_terminal, conversation_update, error,
browser_session_state, steer_message_queued, rate_limit_snapshot,
conversation_hard_deleted
```

Plus the new variants introduced here and in the sibling spec:

```
llm_first_byte   (working-phase-visibility)
llm_attempt      (llm-retry-visibility, sibling spec)
ping             (server keep-alive, see "Server keep-alive observation")
```

A small `wrapHandler(eventName, fn)` helper in the SSE-client layer
(one place, not duplicated per listener) is the recommended shape so
the bump-then-delegate sequence cannot drift listener-by-listener. The
listener registration list is the authoritative enumeration of
event types the client observes — any new `SseWireEvent` variant MUST
add a matching registration in the same change, since without a catch-
all a forgotten registration silently degrades the watchdog (it will
go stale during a turn that only emits the un-registered event type).

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
tool header (lines 1037-1046). Look up the start time in the parent
assistant message's `display_data.tool_starts[tool_use_id]` map (typed
`BTreeMap<String, i64>` on the Rust side, see the "Per-tool started_at
carrier" decision above). When that entry is present and no `result`
has landed for the tool yet, render the elapsed counter inline in the
header. Tick via the same one-second interval pattern used for the
StateBar's `toolElapsedSeconds` (StateBar.tsx:283-297) — extract to a
shared `useElapsedSeconds(startedAt)` hook.

**Pending assistant bubble:** there is no persisted empty assistant
message to retain — text-only LLM responses are committed to the
messages table only after the `LlmResponse` transition (the
`Effect::persist_agent_message` path in
`crates/phoenix-ide/src/state_machine/transition.rs` ~L711), so during
the pre-first-byte `llm_requesting` window the message list literally
contains no row for the in-flight turn. The live streaming bubble after the first byte is
already materialised by the `renderUnits` machinery as a synthetic
`streaming_agent` tail unit driven by `streamingBuffer`
(`ui/src/conversation/renderUnits.ts:193-197`).

Add a new synthetic tail unit (`pending_agent` or similar) emitted by
`renderUnits.ts` immediately before the existing `streaming_agent` tail
unit's slot. Discriminator (when to emit):

- The atom's `phase: ConversationState` discriminant equals
  `'llm_requesting'` or `'seeded_llm_requesting'`, checked via
  `atom.phase.type === 'llm_requesting'` etc. ConversationState is the
  discriminated union defined in `ui/src/api.ts:215`; its discriminator
  field is `type`, not `phase`.
- `streamingBuffer` is empty (no tokens for the current request yet).
- `PendingAssistantBubble` (the spec-level entity defined in
  `working-phase-visibility.allium`) is in `placeholder` state — the
  reducer mirrors this from the same triggers the spec's rules consume.

**Why not `awaiting_llm`?** That variant is set client-side via
`local_phase_change` (`ConversationPage.tsx:577`) the moment the user
presses send, before any server StateChange has landed; its
`state_updated_at` would be stale (the previous phase's timestamp) or
absent, so the elapsed counter would start from the wrong time. The
spec deliberately gates the timer on server-authoritative timestamps
only, so the `awaiting_llm` window stays uncounted. If a "sending..."
affordance is needed during that brief gap, render it separately
without an elapsed counter (it's not the same artifact as the pending
bubble).

The unit's payload carries `placeholder_since` (sourced from the atom's
`phaseStateUpdatedAt`); the React component renders the elapsed counter
where the streamed text would appear. On the first token, the reducer
clears `placeholder` (per the bubble lifecycle in the Allium spec), the
`pending_agent` tail unit drops out of `renderUnits`, and the existing
`streaming_agent` tail unit takes over the same screen slot — same
visual position, contents transition from counter to streamed text. On
the final assistant `Message` arrival, the streaming unit retires and
the persisted message renders in its place.

The empty-message filter at `MessageComponents.tsx:635-638` is NOT
changed — it correctly hides genuinely empty historical agent rows. The
placeholder is a derived/synthetic unit, not a row in the messages
list.

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

The `StateChange.state_updated_at` and `LlmFirstByte` additions both
require `./dev.py codegen` to regenerate `ui/src/generated/SseWireEvent.ts`.
The `parity_*` tests in `crates/phoenix-ide/src/api/sse.rs` must be
updated to include the new field in expected JSON output. The valibot
schemas in `ui/src/sseSchemas.ts` need a `LlmFirstByteSchema` and an
extension of `StateChangeSchema` to include `state_updated_at`. The
typed `tool_starts: BTreeMap<String, i64>` field on the message
display_data struct also needs codegen if that struct is ts-rs-exported
(or its TS shape updated by hand if hand-typed).

## Open Questions

None. Decisions resolved during elicitation (session of 2026-05-25):

- Elapsed counter under `streaming` base reason: suppressed; the stream
  itself is the progress signal (REQ-WPV-007).
- Server keep-alive observability: switched from SSE comment to typed
  `event: ping` so the client watchdog observes it.
- State-entry timestamp on the wire: reuse the existing
  `Conversation.state_updated_at` (already bumped on every transition)
  rather than introducing a parallel `entered_at` field. On Init the
  field is already at the top of `init.conversation` via flatten; on
  StateChange it's added as `state_updated_at: DateTime<Utc>` (RFC3339
  on the wire to match the Init flatten; client converts to ms once).
- Per-tool `started_at` carrier: a typed `tool_starts: BTreeMap<String,
  i64>` field on the assistant message's display_data — NOT a new field
  on `ContentBlock::ToolUse` (which would require schema migration and
  cross-provider work for a UI-side value) and NOT a new SSE variant
  (parallel representation).
- Frozen-elapsed during reconnect: counter does not advance through the
  disconnect (honest about what we know).
