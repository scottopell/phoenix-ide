# MessageList Render Units — Requirements

## Scope

This spec governs the structural model of the conversation message list:
the render-unit type layer, the virtualized rendering boundary over that
layer, and the streaming-view subscription path that preserves
historical-list render isolation.

It supersedes raw-message virtualization models that let persisted rows
and rendered conversation units diverge.

**Virtualization authority:** Phoenix VirtualTranscript is the sole
physical layout authority for windowing, scroll-anchor compensation,
item-height measurement, and programmatic viewport positioning
(REQ-MLRU-015; see `specs/virtual-transcript/`). The hand-rolled
bottom-anchored window, IntersectionObserver-driven boundary expansion,
exact-scroll compensation, and measured-spacer-with-kind-fallback
geometry layer (formerly REQ-MLRU-005, REQ-MLRU-006, REQ-MLRU-007,
REQ-MLRU-008) are deprecated.

**No saved-scroll restore:** REQ-CONV-013 (per-conversation scroll
memory) was deprecated. This spec accordingly does not specify, and
implementations do not provide, any unit-anchor capture, restore, or
DOM-snapshot machinery. Mount always lands pinned to the bottom.

## Transparency Contract (carried from conversation-ui)

The user is looking at a conversation that may contain thousands of messages
spanning multiple turns. The single worst outcome is **not** poor scrolling
performance — it is a user who sees content that disagrees with what is
actually in the conversation. Every virtualization requirement exists to
preserve these answers:

1. The newest activity is visible without the user having to scroll.
2. Every tool result is paired with the tool call that produced it.
3. Revealing older messages on scroll-up does not change the appearance of
   already-visible messages (no header re-numbering, no scroll jump).
4. Token-streaming updates do not cause the conversation history to re-render
   or change identity.
5. Acknowledgement of a pending user message (server echo) does not
   move the viewport away from what the user is reading; the pending
   bubble and its acknowledged form share a single timeline identity.

This contract is the acceptance test for completeness. If a question cannot
be answered confidently from the rendered UI, the requirement is incomplete.

---

## Requirements

### REQ-MLRU-001: Render Unit Layer

WHEN the message list renders historical (incl. pending user), sub-agent-
status, and streaming content
THE SYSTEM SHALL derive a single ordered pair of typed lists
`(historicalUnits: HistoricalUnit[], tailUnits: TailUnit[])` from
`messages`, `pendingMessages`, `convState`, and the streaming-active flag
AND render exactly the bounded slice
`historicalUnits.slice(firstRenderedUnitIndex, lastRenderedUnitIndex)`
followed by all `tailUnits`, with no filtering inside the render loop

WHEN a pending user message exists in `pendingMessages`
THE SYSTEM SHALL emit it as a `HistoricalUnit` of kind `pending_user`
appended at the tail of `historicalUnits`, keyed by `localId`
AND share that key with the acknowledged `user` HistoricalUnit that
replaces it on server echo (the server populates `message_id = localId`,
so the same render-unit key persists through the pending → sent
transition)

THE SYSTEM SHALL NOT emit pending user messages as `TailUnit`. The
type-level membership in `HistoricalUnit` guarantees that the
pending → sent transition is an in-place payload swap on a single
render unit, not a cross-region promotion that would require scroll
compensation.

WHEN a `tool`-type message is encountered during render-unit construction
THE SYSTEM SHALL NOT emit a standalone unit for it
AND SHALL attach it to the preceding `agent_turn` unit's
`toolResultsByUseId` map keyed by the result's `tool_use_id`

WHEN a `system`-type message has empty or absent `content.text`
THE SYSTEM SHALL skip it (emit no unit)
AND log a `console.debug` recording the skipped `message_id`

WHEN a message has a `message_type` not recognized by the construction
function
THE SYSTEM SHALL skip it
AND log a `console.debug` recording the unrecognized type and `message_id`

**Rationale:** The rendered DOM list cardinality must equal the unit-list
cardinality minus the collapsed prefix and suffix. Filtering at render time
(`collapsedRenderableIds`, `if type === 'tool' return null`) creates parallel
representations of "what renders" and is the structural drift this spec
eliminates.

---

### REQ-MLRU-002: Tool-Result Structural Ownership

WHEN `buildRenderUnits` emits an `agent_turn` unit
THE SYSTEM SHALL populate its `toolResultsByUseId: ReadonlyMap<string, Message>`
at construction time with subsequent `tool`-type messages owned by that
agent turn, keyed by each result's `tool_use_id`, until a `user`, `skill`,
or subsequent `agent` message starts a new ownership scope

WHEN a `system` message appears after an `agent_turn` and before that
agent turn's tool result
THE SYSTEM SHALL preserve the active agent ownership scope
SO THAT live status/system events cannot make the result appear orphaned
or leave the originating tool card in an in-flight state

WHEN a `tool`-type message has a `tool_use_id` that does not match any
`tool_use` block id in the preceding `agent_turn`'s content blocks
THE SYSTEM SHALL include it in the map anyway (the render layer is
responsible for displaying it as "orphan result" or similar)
AND log a `console.debug` recording the orphan pairing

WHEN no preceding `agent_turn` exists for a `tool` message (e.g. first
message in the conversation is a tool result, which would indicate a
backend invariant violation)
THE SYSTEM SHALL skip the tool message
AND log a `console.debug` recording the orphan with
`reason: 'orphan_tool'` (per REQ-MLRU-011, all capability-gap skips
log at `debug` level — never higher; a backend invariant violation
manifesting here is still a UI-side recoverable skip, not a render-
time error)

**Rationale:** Keeping tool results in a `Map<string, Message>` built at
the MessageList level and looked up during render creates a parallel data
source. Moving the pairing to construction makes it structurally impossible
to render an agent without its results attached. The agent component renders
by `toolUseId` lookup against its own unit field; no parallel data source.

---

### REQ-MLRU-003: Header Suppression by Construction

WHEN `buildRenderUnits` walks the message list to emit `agent_turn` units
THE SYSTEM SHALL set each unit's `isFirstInTurn: boolean` field based on
whether the immediately-preceding rendered unit (regardless of window
position) is also an `agent_turn` from the same turn-run

WHEN a `user`, `skill`, or `pending_user` historical unit precedes an
`agent_turn`
THE SYSTEM SHALL set `isFirstInTurn = true` on that `agent_turn`

WHEN another `agent_turn` precedes an `agent_turn` with no intervening
`user` or `skill`
THE SYSTEM SHALL set `isFirstInTurn = false`

THE SYSTEM SHALL compute `isFirstInTurn` before the window applies
SO THAT revealing older units on scroll-up cannot change the rendered
header state of any already-visible unit

**Rationale:** Computing turn grouping during a render map call enforces
header suppression by convention rather than structure. Moving the
computation to construction makes it structurally impossible for the window
to affect header rendering.

---

### REQ-MLRU-004: Tail-Pinned Unit Typing

THE SYSTEM SHALL type `sub_agent_status` and `streaming_agent` units as
`TailUnit`, distinct from `HistoricalUnit`

THE SYSTEM SHALL accept only `HistoricalUnit[]` (not `RenderUnit[]`) as
input to the virtualized-window hook
SO THAT a `TailUnit` cannot be collapsed into the spacer by the window

WHEN tail units exist
THE SYSTEM SHALL render them in declaration order
(`sub_agent_status` → `streaming_agent`) after the entire historical
slice
AND ensure they are always present in the rendered DOM, never represented
by spacer height

**Rationale:** Sub-agent status and streaming-agent are ephemeral
display-only views with no acknowledgement lifecycle — they appear,
update, and disappear in response to phase changes rather than message
arrival. Keeping them as `TailUnit` reflects that. Pending user
messages, by contrast, are ackable and must share render-unit identity
with their acknowledged form; they live in `HistoricalUnit` per
REQ-MLRU-001.

---

### REQ-MLRU-005: Bottom-Anchored Initial Window

**DEPRECATED:** Replaced by REQ-MLRU-015. VirtualTranscript provides
bottom-pinned-on-mount; no per-conversation window-index state is computed
outside the physical layout authority.

---

### REQ-MLRU-006: IntersectionObserver Boundary Expansion

**DEPRECATED:** Replaced by REQ-MLRU-015. VirtualTranscript owns boundary
expansion through bounded overscan. No sentinel DOM nodes or
`IntersectionObserver` instances exist in MessageList.

---

### REQ-MLRU-007: Exact Scroll Compensation

**DEPRECATED:** Replaced by REQ-MLRU-015. VirtualTranscript owns
scroll-anchor compensation when measured items above the viewport mount,
unmount, or change extent. MessageList does not maintain a parallel height
model or independently compensate `scrollTop`.

---

### REQ-MLRU-008: Measured Spacer Height with Kind-Estimate Fallback

**DEPRECATED:** Replaced by REQ-MLRU-015. VirtualTranscript measures items
and maintains an in-memory extent model for the lifetime of the mounted
conversation. No `KIND_ESTIMATES` table, no per-conversation height-cache
Map, and no MessageList-owned spacer DOM elements exist.

---

### REQ-MLRU-009: Unit-Anchor Saved-Scroll Restore

**DEPRECATED:** Removed alongside REQ-CONV-013. The implementation
(saved anchor capture on visibilitychange/unmount, restore on first
paint, RESTORE_OVERSCAN window widening, ack-time DOM snapshot) was
deleted. There is no saved-scroll restore; mount always lands pinned
to the bottom per REQ-MLRU-005.

**Deprecation Reason:** Unit-anchor restore interacted poorly with
pending → sent acknowledgement because the anchored unit could change
shape during the swap. The pending → sent transition is instead handled
correctly by construction through the REQ-MLRU-001 single-key invariant.

---

### REQ-MLRU-010: Streaming Subscription Isolation

WHEN the conversation is in a streaming phase
THE SYSTEM SHALL emit a `{ kind: 'streaming_agent'; key: string }`
`TailUnit` from `buildRenderUnits`
(the tag only — no buffer in the unit)

WHEN rendering a `streaming_agent` tail unit
THE SYSTEM SHALL render a `<StreamingMessage />` component that
subscribes to the streaming-buffer atom internally via `useAtomValue`
(or `useSyncExternalStore` on the underlying store)

THE SYSTEM SHALL NOT pass the streaming buffer as a prop into
`MessageList` or any of its parents on the path from the router atom
context to `MessageList`
SO THAT per-token updates do not cause `MessageList` or
`MessageListBody` to re-render

WHEN streaming completes (the `sse_message` reducer rule fires)
THE SYSTEM SHALL transition atomically: `streaming_agent` tail unit
disappears, a new `agent_turn` historical unit appears at the tail,
in a single render commit
(this is the existing REQ-CONV-019 atomic-swap behavior, preserved)

**Rationale:** The unified-list invariant ("the unit list is everything
that renders") holds without sacrificing render isolation. The unit struct
stays pure data; ephemeral high-frequency state lives behind a subscription
keyed by the unit. Per-token re-renders are limited to the
`<StreamingMessage />` leaf.

---

### REQ-MLRU-011: Capability-Gap Logging

WHEN `buildRenderUnits` skips a message for any reason (empty system,
orphan tool, unknown type, unhandled variant)
THE SYSTEM SHALL emit a `console.debug` call recording:
- the message_id (or local identifier)
- the message_type
- a structured reason string ('empty_system' | 'orphan_tool' |
  'unknown_type' | etc.)

THE SYSTEM SHALL NOT emit at `console.warn` or higher for routine skips
(only at `debug`, so production console stays clean)

**Rationale:** Per CLAUDE.md "capability gaps are logged, not silenced":
silent skip is indistinguishable from a bug. Debug-level keeps the noise
out of production while making the next renderer extension diagnosable.

---

### REQ-MLRU-012: Tool-Result-Heavy Tail Regression Test

THE SYSTEM SHALL include a test in `MessageList.test.tsx` (or a sibling)
that:
- constructs a conversation with one `user` message, one `agent` message
  containing 20 `tool_use` blocks, and 20 matching `tool` messages
- mounts the MessageList component
- asserts that the `agent_turn` unit for that turn appears in the
  initial rendered DOM (not collapsed behind the spacer)
- asserts that all 20 tool results are rendered inline within that
  `agent_turn`

THE SYSTEM SHALL fail this test on a build that uses raw-message
windowing (i.e., the test must be exercising the structural property,
not an incidental rendering artifact)

**Rationale:** The exact failure mode the task names as the trigger for
this work. Encoding it as a structural assertion guards against any future
regression of the unit model.

---

### REQ-MLRU-013: SessionStorage Height Cache

**DEPRECATED:** Removed alongside REQ-CONV-013. Persisting measured
heights to `sessionStorage` existed solely to make the first paint
after navigation produce exact spacer geometry so that the
unit-anchor restore (REQ-MLRU-009) could avoid reflow. Without saved-scroll restore, persistence has no consumer.

VirtualTranscript owns its in-memory measurement cache for the lifetime of
the mounted conversation. Phoenix does not persist a parallel height cache.

---

### REQ-MLRU-014: Durable Tail-Follow Policy

WHEN a conversation with content opens or becomes active
THE SYSTEM SHALL establish tail-follow intent and converge to the newest
content through a bounded mount-rescue lifecycle
UNLESS user interaction transfers viewport ownership before convergence

WHILE tail-follow intent belongs to the system
WHEN the total list height changes for any reason, including streaming
growth, delayed measurement, late layout, or viewport shrink
THE SYSTEM SHALL preserve the tail by issuing exactly one VirtualTranscript
scroll-to-tail command
AND SHALL keep unread-tail state clear

WHEN upward wheel or viewport movement, a moved touch, or a conversation-
navigation jump indicates that the user is reading earlier content
THE SYSTEM SHALL transfer viewport ownership to the user immediately
AND SHALL retain that ownership without any time-based expiry

WHEN tail content advances while viewport ownership belongs to the user
THE SYSTEM SHALL show the jump-to-newest unread affordance
AND SHALL NOT move the viewport

WHEN unrelated layout height changes while viewport ownership belongs
to the user
THE SYSTEM SHALL neither move the viewport nor create unread state

WHEN an idle viewport is confirmed at the bottom
THE SYSTEM SHALL restore tail-follow intent and clear unread state

WHEN the user requests jump-to-newest
THE SYSTEM SHALL enter a returning-to-tail mode and issue exactly one
VirtualTranscript scroll-to-tail command
AND SHALL remain in returning-to-tail mode until bottom geometry is
confirmed

WHEN a touch begins
THE SYSTEM SHALL remember the pre-gesture follow mode
AND a touch that ends without movement SHALL restore that mode

WHEN a touch moves
THE SYSTEM SHALL transfer viewport ownership to the user even if no
scroll event is emitted
AND a bottom callback received during that moved touch SHALL NOT release
user ownership
AND touch end or cancellation SHALL preserve user ownership

THE SYSTEM SHALL use VirtualTranscript pinned-state notification only as
bottom geometry and explicit return-to-tail confirmation
AND SHALL use VirtualTranscript total-extent notification only as
notification that layout height changed and a follow action may be required
AND SHALL NOT infer viewport ownership from height growth or proximity

WHILE bounded mount rescue is active
THE SYSTEM SHALL periodically verify bottom placement and may assign the
DOM scroller's `scrollTop` to its current `scrollHeight`
SO THAT silent virtualizer placement stranding converges to the newest
content

WHEN any user interaction occurs or the mount-rescue deadline elapses
THE SYSTEM SHALL stop mount rescue synchronously for that mounted
conversation
AND SHALL NOT restart it until conversation identity changes
AND normal live follow SHALL NOT write the DOM scroll position directly

WHEN VirtualTranscript is first measured empty and content later arrives
THE SYSTEM SHALL issue one VirtualTranscript scroll-to-tail command and
begin bounded mount rescue

WHEN conversation identity changes
THE SYSTEM SHALL atomically reset follow intent, geometry baselines,
gesture state, unread state, and mount-rescue eligibility

THE SYSTEM SHALL NOT force-scroll the viewport for any message type
AND SHALL NOT model momentum duration or native scroll physics
AND each policy transition SHALL emit at most one visible scroll command
AND SHALL NOT both show and clear unread state.

---

### REQ-MLRU-015: VirtualTranscript-Owned Virtualization

WHEN the message list renders
THE SYSTEM SHALL pass the concatenation `[...historicalUnits, ...tailUnits]`
as `items` to a single Phoenix `<VirtualTranscript>` instance
AND render each item via `renderItem={(unit, index) => renderUnit(unit, ...)}`
AND key each item via `getKey={(unit) => unit.key}`

THE SYSTEM SHALL configure the VirtualTranscript instance with:
- `initialTail={allUnits.length > 0}` — bottom-pinned mount (replaces
  REQ-MLRU-005)
- `estimatedExtent={120}` — bounded pre-measurement extent estimate
- `overscan={600}` — bounded leading and trailing overscan distance
- `onPinnedChange={handlePinnedStateChange}` — reports bottom geometry and
  confirms an explicit return to tail; it does not independently grant
  permission to move the viewport
- `onTotalExtentChange={handleTotalListHeightChanged}` — reports layout
  extent changes to the policy; durable follow mode, rather than a
  proximity calculation, decides whether to scroll
- `key={conversationId}` on the React element — force a fresh
  VirtualTranscript instance per conversation, so no stale scroll,
  measurement, or positioning state can leak between conversations

WHEN the user is scrolled up and clicks the jump-to-newest button
THE SYSTEM SHALL call the imperative `transcriptRef.current.scrollToTail()`
AND clear the button visibility after pinned geometry is confirmed

WHEN a `systemPrompt` is provided
THE SYSTEM SHALL render it via VirtualTranscript's `header` slot so it
scrolls with the message content and is measured as part of the scrollable
region

THE SYSTEM SHALL NOT capture, persist, or restore any per-conversation
scroll position, height cache, or measurement state outside
VirtualTranscript's internal layout model. Cross-conversation visits are
first-render-fresh by design (REQ-CONV-013 stays deprecated).

**Rationale:** A single Phoenix-owned physical layout authority avoids
scroll-jump bugs caused by independent windowing, spacer, measurement, and
compensation layers making incompatible geometry assumptions. The
platform-neutral authority and web conformance requirements live in
`specs/virtual-transcript/`.
