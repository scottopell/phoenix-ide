# Virtual Transcript

## Purpose

Virtual Transcript is the platform-neutral contract and Phoenix-owned web implementation for long, dynamically measured conversations. It consumes the render units defined by `specs/messagelist-render-units/`, gives the pure positioning controller sole ownership of command lifecycle and ordered effects, and relies on `MessageList` as the typed physical executor for windowing, measurement, target resolution, viewport writes, prefix continuity, and tail preservation.

## Implementation surfaces

- `ui/src/components/MessageList.tsx` — typed physical executor that projects history-expansion state into transcript-positioning input, resolves message targets against render units, and drives `VirtualTranscript` as the sole web physical authority.
- `ui/src/components/VirtualTranscript.tsx` — web physical layout authority for measured extents, rendered windows, spacers, and viewport writes/observations.
- `ui/src/conversation/virtualTranscriptLayout.ts` — pure measured layout model and visible-window calculations.
- `ui/src/conversation/transcriptPositioning.ts` — pure reducer for closed `idle(view)`/`positioning(command)` input, target-resolution evidence, issued-position revisions, physical observations, supersession, and exact-once finish effects.
- `ui/src/conversation/transcriptPositioning.test.ts` — reducer tests for command ownership, supersession, revision gating, and exact-once finish behavior.
- `fixtures/virtual-transcript/v1/schema.json` and `fixtures/virtual-transcript/v1/scenarios.json` — root portable JSON schema and conformance corpus shared by Phoenix web and iOS clients.
- `ui/src/fixtures/virtualTranscript/scenarios.ts` — TypeScript validated adapter for the root corpus.
- `specs/virtual-transcript/virtual_transcript.allium` — reducer-aligned positioning and measurement lifecycle.
- `specs/messagelist-render-units/` — render-unit construction and conversation scroll ownership policy.

## Directional start alignment

`VirtualTranscript` shares native start-position readback and a single bounded conservative overshoot correction between immediate and deferred target execution. `MessageList` explicitly selects continuity positioning for prefix restoration, excluding it from directional correction. Component coverage includes nearest-integer scroll quantization, accepted fractional positions, integer controls, nonzero target/start offsets, zero/upper clamps, final anchor preservation after header resize, own-write echoes, and no-op user scrolling. Existing hidden authority/supersession and end/tail/restore tests remain in the focused validation set. Source regression coverage does not establish actual browser qualification of a corrected artifact. Typed physical anchors retain directional-start intent through same-anchor measurement updates, including a header ref mounting after an initial zero-header jump. The late-header regression exercises that callback path and checks accepted-fraction, continuity, interruption, cancellation, replacement, subsequent fractional resize, and same-key item-change controls. Final applied anchors are returned through physical-change processing rather than overwritten by pre-write anchors.

Successful finish execution calls `completePosition` before parent notification, transferring matching command ownership to a settled geometric anchor scoped by the history-view key. `interruptPosition` handles accepted user takeover and detach even without a native scroll event; replacement/view changes retire settled ownership. The real reducer/executor parent fixture covers applied → idle authority removal → late header mounting, distinct from constant-authority component cases. Command finish and position counts remain exact-once. Post-settlement controls cover scroll, takeover without scroll, cancellation, replacement, stale completion, and same-conversation view reset. ADR-115 records the capability/geometry lifetime distinction. Actual corrected-artifact qualification is not established by these source tests.

## Verification matrix

| Requirement | Verification |
|---|---|
| REQ-VT-001–004 | Pure layout-model tests, `MessageList` executor tests, and `VirtualTranscript` component tests |
| REQ-VT-005 | Tall-row prefix continuity browser scenario with ≤2 CSS px drift |
| REQ-VT-006–007 | `ui/src/conversation/transcriptPositioning.test.ts` reducer tests for target resolution, missing-target evidence, position-issued revision gating, physical observation, replacement/null/view/interruption/detach supersession, terminal identity scoping, and exact-once finish |
| REQ-VT-008–010 | MessageList scroll-policy tests and dynamic measurement browser scenarios |
| REQ-VT-011 | Root JSON Schema/corpus validation and `ui/src/fixtures/virtualTranscript/scenarios.ts` adapter tests |
| REQ-VT-012 | Playwright Chromium, WebKit, and Firefox conformance runs |
