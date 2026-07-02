# MessageList Render Units — Requirements

## Scope

This spec governs the structural model of the conversation message list:
the render-unit type layer, the virtualized rendering boundary over that
layer, and the streaming-view subscription path that preserves
historical-list render isolation.

It supersedes the raw-message virtualization model that the
`isRenderableHistoricalMessage` patch in `MessageList.tsx` mitigated but
did not structurally replace.

**Virtualization vendor:** As of task 60410, virtualization is delegated
to `react-virtuoso` (REQ-MLRU-015). The hand-rolled bottom-anchored
window, IntersectionObserver-driven boundary expansion, exact-scroll
compensation, and measured-spacer-with-kind-fallback geometry layer
(formerly REQ-MLRU-005, REQ-MLRU-006, REQ-MLRU-007, REQ-MLRU-008) are
all deprecated. The library is now authoritative for windowing,
scroll anchoring, and item-height measurement.

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

**Rationale:** Today's code keeps tool results in a `Map<string, Message>`
built at the MessageList level and looked up during render. Moving the
pairing to construction makes it structurally impossible to render an agent
without its results attached. The agent component renders by `toolUseId`
lookup against its own unit field; no parallel data source.

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

**Rationale:** Today's `inAgentRun` let mutates during the render map call.
The window cannot break grouping in the current code because the mutation
sees the full list, but the invariant is enforced by convention, not
structure. Moving the computation to construction makes it structurally
impossible for the window to affect header rendering.

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

**DEPRECATED:** Replaced by REQ-MLRU-015. Virtuoso's
`initialTopMostItemIndex` (set to `allUnits.length - 1`) plus
`alignToBottom` provide bottom-pinned-on-mount; no per-conversation
window-index state is computed by Phoenix code.

---

### REQ-MLRU-006: IntersectionObserver Boundary Expansion

**DEPRECATED:** Replaced by REQ-MLRU-015. Virtuoso owns boundary
expansion internally via its own viewport-overscan logic. No sentinel
DOM nodes or `IntersectionObserver` instances exist in MessageList.

---

### REQ-MLRU-007: Exact Scroll Compensation

**DEPRECATED:** Replaced by REQ-MLRU-015. Virtuoso applies its own
scroll-anchor compensation in a `useLayoutEffect` between commit and
paint when items above the viewport mount or unmount. Phoenix code
does not capture `scrollHeight` or adjust `scrollTop`.

---

### REQ-MLRU-008: Measured Spacer Height with Kind-Estimate Fallback

**DEPRECATED:** Replaced by REQ-MLRU-015. Virtuoso measures items via
its own internal `ResizeObserver` and maintains an in-memory height
cache for the lifetime of the Virtuoso instance. No `KIND_ESTIMATES`
table, no per-conversation height-cache Map, no spacer DOM elements
exist in MessageList.

---

### REQ-MLRU-009: Unit-Anchor Saved-Scroll Restore

**DEPRECATED:** Removed alongside REQ-CONV-013. The implementation
(saved anchor capture on visibilitychange/unmount, restore on first
paint, RESTORE_OVERSCAN window widening, ack-time DOM snapshot) was
deleted. There is no saved-scroll restore; mount always lands pinned
to the bottom per REQ-MLRU-005.

**Deprecation Reason:** Unit-anchor restore is structurally correct
*in isolation*, but it interacted poorly with pending → sent
acknowledgement: the unit whose key was anchored could change shape
during the pending → sent swap, and the ack-time DOM-snapshot
compensation introduced by PR #152 was a band-aid for that
interaction. The decision was to remove the entire feature rather
than continue patching. The pending → sent transition is now handled
correctly-by-construction (REQ-MLRU-001 single-key invariant).

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
unit-anchor restore (REQ-MLRU-009) landed precisely without a
reflow. With saved-scroll restore gone, persistence is dead weight.

The measured-height cache (REQ-MLRU-008) remains as an *in-memory*
per-conversation Map for the lifetime of the conversation's mount;
it is rebuilt on each mount as `ResizeObserver` callbacks fire. The
first paint uses per-kind estimates; subsequent renders use measured
values as they accumulate. The brief visual settle on first paint
is acceptable.

---

### REQ-MLRU-014: Pinned-to-Bottom Preservation

WHEN a new render unit appears at the tail and the user is currently
pinned to the bottom of the list
THE SYSTEM SHALL keep the bottom of the list visible after the unit is
appended
(implemented via the `totalListHeightChanged` callback, which re-snaps
to the last item when the user's pre-growth distance from the bottom is
within the pin threshold)

THE SYSTEM SHALL compute the pre-growth distance from the bottom in DOM
scroller units — the previously observed `scrollHeight` against the
current `scrollTop` and viewport height — not against the virtualizer's
reported total list height. The virtualizer's total is an
estimate-corrected model value whose divergence from the DOM
`scrollHeight` grows with the number of not-yet-measured rows; on long
conversations that divergence exceeds the pin threshold, and a
model-based check misclassifies a genuinely pinned user as scrolled-up,
silently disabling auto-follow.

WHEN a new render unit appears and the user is NOT pinned to the bottom
(scrolled up)
THE SYSTEM SHALL show the "jump-to-newest" button
AND SHALL NOT auto-scroll the viewport

WHEN the total list height changes while a user scroll gesture is in
progress — an active touch drag (finger down), or an upward scroll
(wheel, scrollbar, keyboard, or touch momentum) within a short rolling
suppression window
THE SYSTEM SHALL NOT auto-scroll the viewport, even if the pre-change
scroll position is within the pin threshold
SO THAT measurement-driven height deltas (rows mounting and being
measured during the user's own scroll-up, late image loads, syntax
highlighters) cannot clobber the gesture and trap the user at the
bottom — a departing user's first pin-threshold's worth of travel is
otherwise re-snapped on every height delta.
The suppression window is refreshed by each upward scroll event, so it
only needs to outlive the gap between momentum scroll events; downward
scrolls never suppress (the auto-follow snap itself scrolls downward,
and a user scrolling down is heading to the bottom).

THE SYSTEM SHALL NOT force-scroll the viewport for any message type.
No "force" override exists for system messages, approval prompts, or
any other unit kind: a user who has scrolled up retains their scroll
position regardless of incoming content type, and the jump-to-newest
button is the sole mechanism for returning to the tail on demand.

THE SYSTEM SHALL determine "pinned to bottom" using Virtuoso's
`atBottomStateChange` callback with `atBottomThreshold` configured to
match the prior 100-pixel threshold.

THE SYSTEM SHALL disable Virtuoso's built-in auto-scroll mechanisms
(`followOutput={false}`) and rely solely on the `totalListHeightChanged`
callback for auto-follow. Virtuoso's built-in `followOutput="auto"`
enables a size-increase handler that misclassifies user scroll-up as
content growth during streaming (its `notAtBottomBecause` priority
order checks `scrollHeight` growth before scroll direction), yanking
the user back to the bottom. The manual `totalListHeightChanged`
callback uses the pre-growth scroll position to distinguish "user was
near the bottom" from "user scrolled up," which is correct during
streaming where Virtuoso's built-in handler is not.

WHILE the user has not yet interacted with the conversation's scroller
(no touch, wheel, or pointer input, and no conversation-nav jump)
THE SYSTEM SHALL re-snap to the bottom on every total-list-height
change regardless of the measured distance from the bottom
SO THAT a stranded initial placement self-heals: the virtualizer's
initial bottom placement is computed against pre-measurement estimates,
and a large estimate correction landing right after mount can leave the
viewport far from the bottom (even at the top of the conversation).
Distance-based pinning cannot recover from stranding — it classifies
the stranded viewport as a scrolled-up user. The mount contract is
"open pinned to the newest message"; only a user interaction releases
the viewport from it.

WHEN the Virtuoso instance mounts with empty data and the first
messages arrive later (fresh conversation, or cached metadata before
messages load)
THE SYSTEM SHALL explicitly scroll to the bottom on the first
non-empty height measurement, because `initialTopMostItemIndex` only
controls the mount position and does not re-apply when data arrives
after mount.

WHEN the viewport height decreases (browser resize, terminal/panel
expansion, composer growth) and the user was pinned to the bottom
before the shrink
THE SYSTEM SHALL re-snap to the bottom using the previous (pre-shrink)
viewport height for the pin-distance calculation, so a pinned user is
not misclassified as scrolled-up by the smaller viewport.

**Rationale:** Force-scroll-on-system-message (the prior implementation
of this requirement) was an over-broad trigger that yanked the viewport
on routine system messages (mode transitions, cancellations) as well as
actionable ones (approval prompts). It is a hostile UX pattern not used
by comparable chat-style products. Pinned-vs-scrolled-up is the only
state that drives auto-scroll; the jump-to-newest button is the only
escape hatch. Disabling `followOutput` eliminates the double-auto-scroll
where Virtuoso's built-in handler and the manual `totalListHeightChanged`
callback both fire and fight each other during streaming.

---

### REQ-MLRU-015: Virtuoso-Owned Virtualization

WHEN the message list renders
THE SYSTEM SHALL pass the concatenation `[...historicalUnits, ...tailUnits]`
as the `data` prop to a single `<Virtuoso>` instance from
`react-virtuoso`
AND render each item via `itemContent={(_, unit) => renderUnit(unit, ...)}`
AND key each item via `computeItemKey={(_, unit) => unit.key}`

THE SYSTEM SHALL configure the Virtuoso instance with:
- `followOutput={false}` — disable ALL of Virtuoso's built-in auto-scroll
  mechanisms (both the totalCount-based followOutput and the size-increase
  handler). Auto-follow is handled solely by the `totalListHeightChanged`
  callback (per REQ-MLRU-014), whose pre-growth `oldFromBottom` logic
  correctly distinguishes "user was near the bottom" from "user scrolled
  up" — unlike Virtuoso's built-in size-increase handler, which
  misclassifies user scroll-up as content growth during streaming.
- `initialTopMostItemIndex={allUnits.length - 1}` (or `0` when empty) —
  bottom-pinned mount (replaces REQ-MLRU-005)
- `alignToBottom` — when total content height is less than viewport
  height, items pin to the bottom of the viewport rather than the top
- `atBottomThreshold={100}` — match the prior pin-detection threshold
- `atBottomStateChange={isAtBottom => …}` — wired to the
  jump-to-newest button visibility state (fires independently of
  `followOutput`)
- `totalListHeightChanged={handleTotalListHeightChanged}` — the sole
  auto-scroll mechanism; re-snaps to the last item when the user's
  pre-growth distance from the bottom is within the pin threshold
- `increaseViewportBy={{ top: 600, bottom: 600 }}` — overscan distance
  matching the prior 600-pixel sentinel rootMargin
- `key={conversationId}` on the React element — force a fresh Virtuoso
  instance per conversation, so no stale scroll/measurement state can
  leak between conversations

WHEN the user is scrolled up and clicks the jump-to-newest button
THE SYSTEM SHALL call the imperative
`virtuosoRef.current.scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' })`
AND clear the button visibility (via `atBottomStateChange` firing
`true` after the scroll completes)

WHEN a `systemPrompt` is provided
THE SYSTEM SHALL render it via Virtuoso's `components={{ Header }}` slot
so it scrolls with the message content and Virtuoso measures it as part
of the scrollable region

THE SYSTEM SHALL NOT capture, persist, or restore any per-conversation
scroll position, height cache, or measurement state outside Virtuoso's
own internal cache. Cross-conversation visits are first-render-fresh by
design (REQ-CONV-013 stays deprecated).

**Rationale:** The hand-rolled spacer + IntersectionObserver-sentinel
+ scroll-compensation stack (PR #161, #162, #163 hotfixes) repeatedly
introduced new scroll-jump regressions because each layer made
assumptions the others had to compensate for. Virtuoso encapsulates
the entire windowing + anchor-compensation contract behind a stable,
library-tested API, replacing four Phoenix requirements (REQ-MLRU-005,
006, 007, 008) with one declarative configuration.

---

## Acceptance Criteria Mapping

The acceptance criteria in `tasks/60410-p1-ready--migrate-messagelist-to-react-virtuoso.md`
map to requirements above:

| Task criterion | Requirement |
|----------------|-------------|
| `MessageList` builds deterministic `RenderUnit[]` | REQ-MLRU-001 |
| Pending and sent user messages share one render-unit key | REQ-MLRU-001 |
| Single `<Virtuoso>` instance renders units | REQ-MLRU-015 |
| Tool-result-heavy tail regression test | REQ-MLRU-012 |
| Switch into large conversation lands pinned to bottom | REQ-MLRU-015 |
| No visible scroll jump as older units come into view | REQ-MLRU-015 (Virtuoso-owned) |
| No visible scroll jump on pending → sent acknowledgement | REQ-MLRU-001 (single-key timeline) |
| Streaming token updates don't re-render historical list | REQ-MLRU-010 |
| System message arrives while scrolled up — no force-scroll | REQ-MLRU-014 |
| System prompt, pending, sub-agent, tool inline, jump-to-newest, context menu preserved | REQ-MLRU-004, REQ-MLRU-002, REQ-MLRU-014, REQ-MLRU-015 |
