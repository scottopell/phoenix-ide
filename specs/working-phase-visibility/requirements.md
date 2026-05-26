# Working-Phase Visibility

## User Story

As a user watching a Phoenix conversation, I need to always have an accurate,
specific idea of what the agent is doing right now — distinguishing a
long-running tool from a slow LLM call from a wedged server — so that I can
decide whether to wait, cancel, or intervene. The current spinner-with-no-
information UI fails this test: a 45-second silence could mean any of those
things, and I have no way to tell.

## Background

The conversation runtime exposes "phases" via `SseWireEvent::StateChange`:
`llm_requesting`, `tool_executing`, `awaiting_sub_agents`, and others. Today
the UI's `StateBar` collapses every working phase into a generic spinner with
a phase-name label. There is no elapsed-time indicator on most phases (only
`tool_executing` has one, added ad-hoc in `StateBar.tsx:283-297`), no
indication when streaming has begun versus when we're still waiting for the
first byte, no indication when the SSE stream has gone silent, and the
StateBar masks the agent's last-known activity entirely whenever the
connection state is anything other than `connected`.

This spec covers the UI-side observability of working phases. The retry/
backoff sub-system that generates additional context to display is specified
separately in `specs/llm-retry-visibility/` and consumed here as a modifier
on the displayed activity (REQ-WPV-003).

## Requirements

### REQ-WPV-001: Server-Authoritative Phase Entry Timestamp

WHEN the conversation transitions to a new state
THE SYSTEM SHALL include `state_updated_at` (unix milliseconds, server
clock) in the `StateChange` SSE event — sourced from the existing
`Conversation.state_updated_at: DateTime<Utc>` field the runtime already
bumps on every transition

WHEN an SSE stream is opened (or reconnects)
THE SYSTEM SHALL include `state_updated_at` in the `Init` snapshot,
already carried at the top level of `init.conversation` via the
`#[serde(flatten)]` on `EnrichedConversation`

**Rationale:** Elapsed-time displays must survive reconnect, page reload,
and multi-tab observation. A client-derived timestamp at event-arrival
time fails all three (timer resets on reconnect, drifts under network
jitter, and diverges across tabs viewing the same conversation).
Server-authoritative `state_updated_at` makes the elapsed time a pure
function of `now() - state_updated_at`, deterministic and consistent.
Reusing the existing `Conversation.state_updated_at` (rather than
introducing a parallel `entered_at` field) avoids parallel representation
of the same semantic value.

---

### REQ-WPV-002: Inline Elapsed-Time Indicator on In-Flight Artifacts

WHEN a tool's execution is in flight (the tool-use block exists but no
tool-result block has been persisted)
THE SYSTEM SHALL render an elapsed-time indicator on the tool widget itself,
ticking at one-second resolution

WHEN the agent is in `llm_requesting` and no tokens have arrived yet
THE SYSTEM SHALL render a placeholder assistant message bubble with the
current elapsed time

WHEN tokens begin arriving for that bubble
THE SYSTEM SHALL replace the elapsed-time placeholder with the streaming text
without unmounting/remounting the bubble (continuous visual identity)

**Rationale:** The StateBar is a global summary that can scroll out of the
user's field of view. The activity the user is watching for — "is this
specific tool/turn making progress?" — should be visible at the point of
expectation, not only in a header. Information at the point of expectation is
the project's "information density, not minimalism" UI principle applied
here.

---

### REQ-WPV-003: StateBar Activity Derivation Rule

WHEN deriving the StateBar's working-phase text
THE SYSTEM SHALL combine a **base reason** (derived from the conversation
phase) with an optional **retry modifier** (derived from the most recent
unresolved `LlmAttempt` event for this turn; see
`specs/llm-retry-visibility/`)

WHEN multiple potential live timers exist for a single phase
THE SYSTEM SHALL display exactly one elapsed-time counter in the StateBar,
selected by the precedence:
1. The base reason's own timer (`now() - phase.state_updated_at`)
2. NOT layered with any other timer (the inline-artifact timer in REQ-WPV-002
   covers per-artifact granularity)

**Rationale:** A reliable indicator is a *specific* indicator. Layering
multiple counters compounds into noise; the user reads "one number,
explained" faster than three numbers in different units. Retry is a modifier
on the base state, not a replacement, because the question "what's it doing
right now?" still has its primary answer in the phase (thinking, waiting on
a tool, etc.) and the question "why is it taking this long?" is the
secondary answer that the modifier addresses.

**Format examples (illustrative, not normative wire syntax):**
- `thinking 4s`
- `thinking 4s (retry 2/5 after rate limit)`
- `executing bash 12s`
- `executing bash 12s (retry 2/5)` *(when a tool itself didn't retry but the
  surrounding turn did before reaching this tool — display the most recent
  unresolved retry context)*
- `backing off 4s (retry 2/5 after rate limit)`

---

### REQ-WPV-004: Heartbeat Watchdog

GIVEN the SSE connection state is `connected` AND the conversation phase is
any working phase
WHEN no SSE event observable to the client `EventSource` has arrived for
`HEARTBEAT_WATCHDOG_SECONDS` (default 35)
THE SYSTEM SHALL surface a degraded-signal indicator in the StateBar:
"no signal from server for Ns"

WHEN any SSE event subsequently arrives
THE SYSTEM SHALL clear the degraded-signal indicator immediately

**Prerequisite:** The server keep-alive is currently emitted as an SSE
comment line (`: ping\n\n`, see `api/sse.rs:71` / `handlers.rs:3279`), and
standard `EventSource` does NOT fire any handler for comments. For this
requirement to be implementable as written, the keep-alive MUST be switched
to a typed event (`event: ping\ndata:\n\n`) so the client `EventSource`
observes it via an explicit `ping` listener. This switch is owned by
design.md ("Server keep-alive observation") and is forward-compatible:
clients that don't listen for `ping` simply ignore it.

**Rationale:** TCP-level connection health is not the same as application-
level liveness; a wedged server can hold the SSE socket open indefinitely.
The keep-alive interval is 15s; `HEARTBEAT_WATCHDOG_SECONDS = 35` gives
~2.3x headroom so a single missed keep-alive does not trigger a false
positive. The threshold is for "no client-observable event of any kind"
(typed `ping` events count) — a long LLM stream that is in fact sending
tokens does not trigger it.

---

### REQ-WPV-005: Connection State Does Not Mask Agent State

WHEN the SSE connection state is `reconnecting` or `offline` during a working
phase
THE SYSTEM SHALL display BOTH the connection state AND the last-known agent
activity:
- Connection chip: `reconnecting (N)`
- Last-known agent activity, with elapsed time **frozen at disconnect**:
  `last: thinking 12s`

WHEN the connection re-establishes
THE SYSTEM SHALL resume live derivation from the (possibly new) phase carried
in the next `Init` snapshot

**Rationale:** A user whose connection blips mid-LLM-call needs to know that
the long wait they're observing is a connection problem *and* what the agent
was doing when it started. Freezing the elapsed counter (rather than
continuing to count forward through the disconnect) is honest: we *do not
know* whether the server is still working, so an active counter would be
misleading. The "last:" prefix communicates that this is stale data.

---

### REQ-WPV-006: Pending Assistant Bubble During llm_requesting

WHEN the conversation phase is `llm_requesting` (or its variants:
`awaiting_llm`, `seeded_llm_requesting`) AND no tokens have been received
for the current LLM request
THE SYSTEM SHALL render an empty assistant message bubble at the bottom of
the conversation containing the elapsed-time indicator

WHEN the first token arrives for that request
THE SYSTEM SHALL transition the bubble's contents from the elapsed-time
placeholder to the streaming text (REQ-WPV-002)

WHEN the phase exits `llm_requesting` without any tokens having arrived (an
error or cancellation path)
THE SYSTEM SHALL remove the placeholder bubble

**Rationale:** Today the empty-message filter at `MessageComponents.tsx:635-638`
hides agent messages with no content. That is correct for genuinely empty
historical messages but wrong for the live in-flight case where the absence
*is* the information ("we're waiting"). The placeholder is anchored to the
spot where the text will appear, giving the user spatial continuity.

---

### REQ-WPV-007: First-Byte Sub-Phase Distinction

WHEN the first token of an LLM response arrives over the SSE stream
THE SYSTEM SHALL transition the displayed base reason from `thinking Ns`
(pre-first-byte) to `streaming` (post-first-byte), without resetting the
phase elapsed timer

WHEN displaying `streaming`
THE SYSTEM SHALL NOT show an elapsed counter (the stream itself is visible
progress; an additional counter is redundant)

**Rationale:** "Thinking" and "streaming" feel identical to the user today
because they share a phase (`llm_requesting`). The user-meaningful boundary
is the first byte: pre-first-byte the user has no signal of life, post-
first-byte the text itself is the signal. Splitting the display reduces the
window in which "thinking..." can hide a real problem.

---

### REQ-WPV-008: Display Continuity Across Reload

GIVEN a user reloads the page mid-working-phase
AND the `Init` payload carries `pending_truncated = false` (the SSE
replay ring did not overflow since the last anchor — see
`specs/sse_wire/sse_wire.allium`)
THE SYSTEM SHALL reconstruct the inline and StateBar indicators from `Init`
such that elapsed times match (within one second) what they would have
shown on a continuously-connected client

GIVEN a user reloads the page mid-working-phase
AND the `Init` payload carries `pending_truncated = true`
THE SYSTEM SHALL display the phase-level StateBar indicator (which
reconstructs cleanly from the always-present `conversation.state` +
`conversation.state_updated_at` fields) AND a degraded notice in place
of the per-artifact inline indicators: "reload truncated — in-flight
detail will reappear at the next checkpoint"

**Rationale:** Acceptance-criterion expression of REQ-WPV-001 + REQ-WPV-005.
The truncated-replay exception is necessary because pending_truncated=true
means Phoenix intentionally sent an empty pending-event list and the DB
snapshot lacks the eager assistant/tool-use blocks until the tool round
checkpoint completes (see `specs/sse_wire/sse_wire.allium`'s
`pending_truncated` semantics). Reconstructing the inline indicators
within one second is not possible in that case; the spec is explicit
about the degradation so implementers don't silently render stale data
or empty timers. Listed separately because it is the integration-test
target.

---

## Out of Scope

- Persisting the elapsed time *after* a phase exits (we display live elapsed
  while in-phase; on exit, the existing duration metadata on tool results
  and the post-hoc retry badge from `llm-retry-visibility` cover the audit
  trail).
- A retry-attempt event log queryable from the UI ("show all retries for
  this turn"). The `(retried 2x)` badge from `llm-retry-visibility` and the
  server-side logs cover diagnostics needs at v1; richer surfacing is
  deferred.
- Multiple parallel tool executions in a single assistant message. The
  existing model is sequential tool dispatch within a turn. If parallel
  execution arrives, the inline per-tool timers (REQ-WPV-002) already key
  off per-tool started_at and will continue to work.
- Cross-conversation aggregate dashboards ("which conversations have stuck
  agents").
