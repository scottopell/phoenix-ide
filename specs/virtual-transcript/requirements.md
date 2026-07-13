# Virtual Transcript Requirements

Virtual Transcript defines the platform-neutral behavioral contract for rendering and positioning a long conversation. A client may use DOM scrolling, a native collection view, or another physical mechanism, but it must expose the same semantic units and satisfy the same positioning postconditions.

## REQ-VT-001: One Physical Layout Authority

WHEN a transcript view is mounted
THE SYSTEM SHALL have exactly one typed physical executor responsible for the ordered layout model, measured unit extents, rendered window, spacers, target resolution against rendered units, and programmatic viewport writes
AND no other component shall maintain a competing height model or independently compensate the viewport.

In the Phoenix web implementation, `MessageList` owns this typed physical execution surface through its `VirtualTranscript` integration. The layout model is ephemeral to the mounted conversation. It is not persisted across visits.

## REQ-VT-002: Stable Semantic Units

WHEN render units are supplied
THE SYSTEM SHALL preserve their order and stable keys
AND SHALL render a contiguous slice of those units
AND SHALL expose navigation aliases separately from physical unit identity.

A tool-result message identifier aliases its containing agent-turn unit. A non-rendered message has no physical target.

## REQ-VT-003: Bounded Rendering

WHEN transcript content exceeds the viewport
THE SYSTEM SHALL mount only the visible contiguous unit range plus bounded leading and trailing overscan
AND SHALL represent unmounted content using the authoritative layout model
AND SHALL update the rendered range when viewport geometry, scroll position, unit order, or measured extents change.

## REQ-VT-004: Measurement Reconciliation

WHEN a mounted unit's measured block extent changes
THE SYSTEM SHALL update the authoritative layout model from that measurement
AND, if the changed extent precedes an active physical anchor, SHALL preserve that anchor's viewport-start offset
AND SHALL NOT infer user intent from the measurement change.

Reconciliation is triggered by layout measurements. It shall not poll or use elapsed time as evidence of positioning success.

## REQ-VT-005: Geometric Prefix Continuity

WHEN the user requests earlier history while reading
THE SYSTEM SHALL capture a physical anchor consisting of a stable unit key and its signed viewport-start offset
AND SHALL carry both values through history acquisition without loss.

WHEN earlier units are inserted before that anchor
THE SYSTEM SHALL preserve the same unit key at the same viewport-start offset within the platform's conformance tolerance
AND SHALL acknowledge restoration only after the measured geometric postcondition holds.

WHEN the platform issues the physical position write for a command
THE SYSTEM SHALL record the layout revision of that write
AND SHALL accept physical success only from observations at or after the issued layout revision.

The anchor shall be a physically visible unit, not an overscanned range boundary.

## REQ-VT-006: Semantic Navigation

WHEN navigation targets a renderable message identifier
THE SYSTEM SHALL resolve it to its physical render unit and position that unit according to the requested alignment
AND SHALL acknowledge the command only after the target is physically present at the requested position.

WHEN no render unit owns the identifier
THE SYSTEM SHALL report the target as missing.

Target resolution and missing-target detection are evidence supplied to the positioning reducer. The reducer shall not infer target presence from time, retries, or physical layout alone.

## REQ-VT-007: Pure Position Command Ownership

THE SYSTEM SHALL model transcript positioning as a pure reducer over a closed input: either `idle(view)` or `positioning(command)`.

The pure positioning controller SHALL own the programmatic command lifecycle, derive terminal results from target-resolution evidence and physical observations, and emit ordered effects for target resolution, physical positioning, and exact-once finish.

THE SYSTEM SHALL allow at most one active programmatic positioning command for a transcript view
AND every command SHALL be bound to a command key derived from command kind, command token, request token, conversation identity, view generation, transcript generation, and target message identity.

WHEN input changes to a replacement command, null idle input, a different view identity, user interruption, or executor detach
THE SYSTEM SHALL supersede the active command before stale target resolution, viewport writes, or physical observations can acknowledge success.

THE SYSTEM SHALL scope terminal command identities to the current view identity.
WHEN the view identity changes
THE SYSTEM SHALL reset the terminal identity set for that view.

Each command shall finish exactly once within a view as applied, target missing, or superseded.

## REQ-VT-008: Durable Tail Following

WHILE tail-follow ownership belongs to the system
WHEN content or measured layout grows
THE SYSTEM SHALL preserve the tail position and keep unread state clear.

WHILE viewport ownership belongs to the reader
WHEN tail content grows
THE SYSTEM SHALL preserve the reader's physical anchor, show unread-tail state, and not move to the tail.

Layout growth and proximity to the tail shall not independently transfer viewport ownership.

## REQ-VT-009: Initial Placement and Conversation Isolation

WHEN a conversation with content first becomes measurable
THE SYSTEM SHALL converge to the newest content unless user interaction first transfers viewport ownership.

WHEN conversation identity changes
THE SYSTEM SHALL atomically discard the prior conversation's layout measurements, active transaction, geometry baselines, gesture state, and rendered window.

## REQ-VT-010: Dynamic Content

WHEN streaming text, images, expanded tool output, system prompts, viewport resizing, or typography changes alter unit extents
THE SYSTEM SHALL reconcile from measured geometry while preserving the active reader anchor or tail invariant according to viewport ownership.

A streaming-to-finalized transition that retains the same render-unit key shall remain an in-place physical unit transition.

## REQ-VT-011: Cross-Platform Conformance Fixtures

THE SYSTEM SHALL maintain a platform-neutral fixture corpus at `fixtures/virtual-transcript/v1/` with a JSON Schema (`schema.json`) and scenario corpus (`scenarios.json`) describing ordered units, stable keys, navigation aliases, initial viewport state, operations, and geometric expectations.

The root `fixtures/virtual-transcript/v1/` schema and corpus SHALL be the shared contract across Phoenix web and iOS clients. Web and native clients SHALL be able to consume the same semantic scenarios without sharing physical implementation code. The TypeScript web adapter SHALL validate the root corpus before exposing scenarios to fixtures or tests.

Conformance SHALL include prefix insertion within a tall unit, dynamic resize above an anchor, semantic navigation through an alias, missing orphan targets, streaming growth while reading, streaming growth while following, and command supersession.

## REQ-VT-012: Browser Conformance

The web implementation SHALL satisfy the Virtual Transcript postconditions in current stable Chromium, Safari, and Firefox.

Geometric continuity SHALL be evaluated using measured viewport-start offsets. The web tolerance is two CSS pixels. Browser-specific event ordering shall not change command ownership or terminal results.
