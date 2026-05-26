# MessageList Render Units — Requirements

## Scope

This spec governs the structural model of the conversation message list:
the render-unit type layer, the bottom-anchored window over that layer,
boundary expansion, measured spacer geometry, and the streaming-view
subscription path that preserves historical-list render isolation.

It supersedes the raw-message virtualization model that the
`isRenderableHistoricalMessage` patch in `MessageList.tsx` mitigated but
did not structurally replace.

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
at construction time by consuming the immediately-following sequence of
`tool`-type messages until a non-tool boundary is reached

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

WHEN the message list mounts
THE SYSTEM SHALL set `firstRenderedUnitIndex = max(0, historicalUnits.length - INITIAL_WINDOW)`
WITH `INITIAL_WINDOW = 12`
AND scroll the container to its bottom such that the last historical unit
and all tail units are visible

WHEN `historicalUnits.length` grows due to async message arrival while the
user has not scrolled up
THE SYSTEM SHALL recompute `firstRenderedUnitIndex` reactively to keep the
tail of the list bottom-pinned

WHEN the user has scrolled up such that the topmost rendered unit is
not at the bottom of the list
THE SYSTEM SHALL preserve the user's expanded `firstRenderedUnitIndex`
and SHALL NOT auto-anchor to the bottom on new message arrival
(the existing jump-to-newest button handles that case)

**Rationale:** Task 65002 established that bottom-pinned-on-mount is the
only correct landing for a conversation switch. With REQ-CONV-013
removed, this is the *sole* mount landing strategy. The window must be
expressed in unit indexes so that tool-result-heavy turns cannot push
the latest agent turn out of the initial window.

---

### REQ-MLRU-006: IntersectionObserver Boundary Expansion

WHEN historical units are collapsed behind the spacer
THE SYSTEM SHALL render a sentinel `<div aria-hidden />` immediately
below the spacer and above the first rendered unit
AND observe that sentinel via `IntersectionObserver` rooted at the scroll
container with `rootMargin: '600px 0px 0px 0px'`

WHEN the sentinel intersects the expanded root (i.e., enters the
600-pixel-buffered viewport)
THE SYSTEM SHALL expand the window upward by `EXPAND_BATCH = 12` (decrement
`firstRenderedUnitIndex`)
AND SHALL keep at most `MAX_RENDERED_UNITS = 48` historical units in the DOM
by moving the newer overflow into a measured bottom spacer
RELYING on the browser's CSS scroll-anchoring per REQ-MLRU-007

WHEN newer historical units are collapsed behind the bottom spacer
THE SYSTEM SHALL render a bottom sentinel above that spacer
AND when it intersects the lower buffered viewport, expand the window
downward by `EXPAND_BATCH = 12`, again preserving the max rendered-unit cap

THE SYSTEM SHALL NOT use any heuristic based on
`scrollTop - estimatedSpacerHeight` to trigger expansion

**Rationale:** The current `scrollTop - spacerHeight > 600px` trigger
depends on spacer-estimate accuracy. Per-unit-kind estimates (REQ-MLRU-008)
improve this, but the only correct trigger is the actual DOM boundary
between collapsed and rendered content. The sentinel is that boundary.

---

### REQ-MLRU-007: Browser-Native Scroll Anchoring

WHEN the virtualization window expands (revealing older or newer units
into the DOM) or when measured heights replace kind-estimate fallbacks
in the spacer
THE SYSTEM SHALL preserve the topmost in-viewport render-unit's
viewport position by relying on the browser's CSS scroll-anchoring
algorithm (`overflow-anchor`)

THE SYSTEM SHALL set `overflow-anchor: auto` (or default) on
`#main-area` (the scroll root) and on every render-unit wrapper

THE SYSTEM SHALL set `overflow-anchor: none` on
`.message-collapsed-spacer` divs so the browser never selects a
spacer as the anchor node (the spacer's height is the thing changing,
so anchoring on it would defeat the compensation)

THE SYSTEM SHALL NOT maintain a hand-rolled scroll-compensation layout
effect that captures `scrollHeight` pre-render and applies a delta
post-commit. This pattern was removed: it required guarding every
spacer/window mutation by its trigger (expansion vs. measurement
settle vs. structural change), and missing a guard produced
arbitrary-position scroll jumps.

**Rationale:** Browser scroll-anchoring is the correct-by-construction
mechanism for "preserve visible content during reflow above it." The
browser handles every case uniformly — window expansion, measured-
height settle, font-loading-driven height changes, image dimensions
landing late — without per-case guards. CSS scroll-anchoring is
universally supported in modern Chrome/Firefox and in Safari 18+
(2024); Phoenix targets latest browsers.

**Out of scope:** auto-scroll-to-bottom on new messages when the user
is pinned — that is a different concern (pulling the user forward,
not preserving an anchor) and lives in `MessageList`'s
`ResizeObserver` effect, not in this requirement.

---

### REQ-MLRU-008: Measured Spacer Height with Kind-Estimate Fallback

WHEN a `HistoricalUnit` is rendered in the DOM
THE SYSTEM SHALL observe it via `ResizeObserver`
AND write its current `offsetHeight` into a measured-height cache keyed
by `unit.key`

WHEN computing spacer height
THE SYSTEM SHALL sum the measured or estimated heights for every collapsed
historical unit in either spacer range:
- top spacer: `historicalUnits.slice(0, firstRenderedUnitIndex)`
- bottom spacer: `historicalUnits.slice(lastRenderedUnitIndex)`
using the measured height if present in the cache, otherwise the per-kind
estimate from `KIND_ESTIMATES`

THE SYSTEM SHALL provide initial `KIND_ESTIMATES`:
- `user`: 100px
- `skill`: 80px
- `agent_turn`: 400px
- `system`: 100px

THE SYSTEM SHALL apply measured heights immediately on each
`ResizeObserver` callback, without waiting for the next state update
(the cache write triggers a re-render of the spacer only)

**Rationale:** A single `360px * count` spacer over-allocates for
tool-message-heavy tails and under-allocates for long agent turns. Measured
geometry corrects both. Per-kind estimates are the right fallback because
they reflect the actual structural diversity of unit kinds rather than a
universal row estimate.

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

WHEN a new message arrives via SSE and the user is currently pinned to
the bottom of the list (`isPinnedToBottom === true`)
THE SYSTEM SHALL keep the bottom of the list visible after the new unit
is appended
(carries forward the existing ResizeObserver-driven scroll-to-bottom)

WHEN a new message arrives and the user is NOT pinned to the bottom
(scrolled up)
THE SYSTEM SHALL show the "jump-to-newest" button (existing behavior)
AND SHALL NOT auto-scroll the viewport

THE SYSTEM SHALL determine "pinned to bottom" using the same
`scrollTop + clientHeight >= scrollHeight - threshold` check as the
prior implementation

**Rationale:** Non-regression of existing REQ-CONV-002 behavior. The
render-unit refactor changes the windowing model; it must not change the
auto-scroll-on-arrival semantics.

---

## Acceptance Criteria Mapping

The acceptance criteria in `tasks/01004-p1-ready--messagelist-render-units-virtualization.md`
map to requirements above:

| Task criterion | Requirement |
|----------------|-------------|
| `MessageList` builds deterministic `RenderUnit[]` | REQ-MLRU-001 |
| Pending and sent user messages share one render-unit key | REQ-MLRU-001 |
| Virtualization hook accepts `renderUnitCount` | REQ-MLRU-005 |
| `MessageListBody` renders units, not raw rows | REQ-MLRU-001 |
| Tool-result-heavy tail regression test | REQ-MLRU-012 |
| Boundary expansion via IntersectionObserver sentinel | REQ-MLRU-006 |
| No `scrollTop - spacerHeight` trigger remains | REQ-MLRU-006 |
| Switch into large conversation lands pinned to bottom | REQ-MLRU-005 |
| No visible scroll jump when expanding older units | REQ-MLRU-007 |
| No visible scroll jump on pending → sent acknowledgement | REQ-MLRU-001 (single-key timeline) |
| Streaming token updates don't re-render historical list | REQ-MLRU-010 |
| System prompt, pending, sub-agent, tool inline, jump-to-newest, context menu preserved | REQ-MLRU-004, REQ-MLRU-002, REQ-MLRU-014; menu carried unchanged |
| Validate with `browser_profile conversation-load` | Implementation step; see design.md |
