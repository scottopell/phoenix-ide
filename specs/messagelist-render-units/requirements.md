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

WHEN one or more adjacent `agent_turn` units contain only non-`think` tool calls
THE SYSTEM SHALL partition that run into `tool_only_agent_turn_group` historical units of at most eight members, each keyed by its first member
AND SHALL use the group variant for a one-member run so appending an adjacent tool turn preserves the renderer's identity and interaction state
AND SHALL retain the ordered member units with each member's source identity, `toolResultsByUseId`, and `isFirstInTurn` intact

WHEN older history is prepended
THE SYSTEM SHALL preserve a grouping boundary before the previously loaded first historical unit
SO THAT acquisition cannot merge that existing physical row into an older group or change its stable key, measured extent, or mounted interaction state

WHEN user, skill, pending-user, visible system, assistant prose, or `think` content occurs
THE SYSTEM SHALL terminate any tool-only grouping boundary before that content
SO THAT one group corresponds to one independently measured virtual transcript row without changing visible conversation order

WHEN a `system`-type message has empty or absent `content.text`
THE SYSTEM SHALL skip it (emit no unit)
AND log a `console.debug` recording the skipped `message_id`

WHEN a `system`-type message has `display_data.hidden = true`
THE SYSTEM SHALL skip it (emit no unit)
AND SHALL preserve the active agent run and tool-only grouping adjacency
AND log a `console.debug` recording the skipped `message_id` with `reason: 'hidden_system'`

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

WHEN lookup targets an agent message or owned tool-result message inside a `tool_only_agent_turn_group`
THE SYSTEM SHALL resolve the containing group's historical-unit index and the matched member's agent-message identity
AND for a tool result SHALL also resolve the owning `tool_use_id`
AND SHALL position and highlight the exact matched member or tool card rather than the group's first member or first tool card
AND SHALL retain the member's canonical tool-result ownership rather than copying results into a group-level parallel map

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

WHEN adjacent tool-only agent turns become one `tool_only_agent_turn_group`
THE SYSTEM SHALL preserve each member's precomputed `isFirstInTurn` value

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

WHEN a key whose default action moves the transcript in either direction is
pressed at a target within the transcript
THE SYSTEM SHALL treat it as the user taking the viewport over
AND SHALL NOT treat a key the focused element consumes for itself — text
entry, or an activation key on a control that activates on it — as viewport
movement
SO THAT the test is what the key actually does from that element, not merely
where focus happens to sit: a link inside the transcript activates on Enter
and pages on Space, and pages the transcript accordingly, while a key
delivered outside the transcript moves a box that is not this one
SO THAT a positioning command in flight yields to a reader who keys their own
way to the tail, in either direction

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

WHEN an idle viewport is confirmed at the bottom — by the physical
pinned-state notification, or by downward movement whose observed
position lands inside the pin-to-bottom threshold
THE SYSTEM SHALL restore tail-follow intent and clear unread state
SO THAT a viewport coasting to rest at the tail returns to following
without depending on an exact-bottom edge the viewport may never cross

WHEN the user requests jump-to-newest
THE SYSTEM SHALL enter a returning-to-tail mode and issue exactly one
VirtualTranscript scroll-to-tail command
AND SHALL remain in returning-to-tail mode until bottom geometry is
confirmed

WHEN a touch begins
THE SYSTEM SHALL remember the pre-gesture follow mode
AND a touch that ends without movement SHALL restore that mode

WHEN the rendered range reaches the start of loaded history
THE SYSTEM SHALL acquire earlier history only if the reader moved the
viewport there — while reading, or while a navigation the reader has taken
over is returning under their control
AND SHALL apply that condition whatever noticed the boundary, since a
positioning command that scrolls a target into view directly is
indistinguishable from reader movement at the scroll event alone
SO THAT a jump that lands near the start of loaded history does not
recursively acquire more of it, while a reader who drags there after such a
jump is not left at a boundary that never expands

WHEN a gesture the platform never reports as ended is discovered — at the
next interaction, when the platform recycles an identifier the system still
holds, or when a finger reported down outlasts the gesture-staleness bound
without an event of its own, which SHALL expire on its own rather than only
when some later input happens to look
AND a touch beginning under an identifier already held SHALL end the prior
interaction unconditionally, that being proof rather than inference: a touch
already down cannot begin again
THE SYSTEM SHALL end that gesture without confirming a tail return
AND SHALL discard its travel evidence rather than carrying it into the next
gesture
AND SHALL leave viewport ownership where the interaction placed it
SO THAT an interaction whose end position was never observed can neither
confirm from geometry belonging to a later moment nor defer every subsequent
confirmation to a lift that is not coming

WHEN a touch moves
THE SYSTEM SHALL measure each owned touch against where that same touch began
AND SHALL treat any one of them travelling toward earlier content as upward
intent
SO THAT a viewport clamped at the start of loaded history, which emits no
scroll event to reason from, still hears the finger that is actually dragging
rather than whichever one the platform happens to list first

WHEN a touch moves
THE SYSTEM SHALL transfer viewport ownership to the user even if no
scroll event is emitted
AND a bottom callback received during that moved touch SHALL NOT release
user ownership while the gesture is active
SO THAT a callback describing where the viewport is cannot decide who owns it

WHEN a gesture ends or is cancelled
THE SYSTEM SHALL take its own measurement of where the viewport is at that
moment rather than relying on the last one observed
SO THAT a platform that reports the lift ahead of the scroll frames placing
it cannot have the gesture judged against a position it has already left

WHEN a gesture ends or is cancelled inside the pin-to-bottom threshold,
having moved the viewport toward the tail during that gesture
THE SYSTEM SHALL confirm the tail return
AND SHALL otherwise preserve user ownership
SO THAT a confirmation blocked mid-gesture is honoured at the lift rather
than lost with the callback that produced it, while a drag that stops short
of the tail — or one that never moved the viewport at all, which iOS
produces whenever touch movement outruns scroll events — keeps the viewport

THE SYSTEM SHALL derive that condition at the lift from whether observed
movement carried the viewport toward the tail during the gesture
AND SHALL NOT maintain revocable evidence of having arrived
SO THAT no event is responsible for invalidating state it did not create:
the evidence is only ever set within a gesture, and an update missed
anywhere can withhold a confirmation but never manufacture one

THE SYSTEM SHALL record that evidence only from observed movement toward the
tail
AND SHALL NOT record it from layout changes, settle probes, or echoes of its
own position writes
AND SHALL NOT infer it from distance to the tail measured at two moments
SO THAT content moving beneath a stationary finger — growing away from the
reader, or collapsing until the tail is close — cannot supply the travel the
lift derivation treats as proof of a return, since the tail moves
independently of the viewport and a change in distance is therefore not
evidence of anything the reader did

WHEN a viewport measurement is taken
THE SYSTEM SHALL clamp the observed scroll position into the scrollable range
in force at that moment
SO THAT no recorded measurement carries a position the scroller cannot
actually hold, at either edge

WHEN scroll movement is classified as upward or downward intent
THE SYSTEM SHALL re-clamp the previously recorded position into the range in
force now before comparing the two
SO THAT a range that moved between the two measurements cannot make a
stationary viewport look as though it travelled
AND SHALL classify a clamped position equal to the previous one as neither
direction, recording its geometry without inferring intent
SO THAT overscroll rubber-band bounce-back at either edge is never
classified as user reading intent, and the standstill that clamping
produces at an edge is never mistaken for movement toward the tail

WHEN a scroll event is the echo of a position write VirtualTranscript
itself made — anchor compensation, drift reconciliation, or a tail snap
THE SYSTEM SHALL update geometry baselines without classifying the
movement as user intent
SO THAT physical compensation inside the pin-to-bottom zone cannot be
mistaken for a user tail return

THE SYSTEM SHALL keep the physical tail edge and pin-to-bottom zone
membership as separate observations with distinct owners: the edge is
reported by the virtual transcript's pinned-state notification, and zone
membership is derived from the scroll position carried by each event
AND SHALL NOT record either as the other

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
