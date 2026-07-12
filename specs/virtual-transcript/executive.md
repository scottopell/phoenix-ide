# Virtual Transcript

## Purpose

Virtual Transcript is the platform-neutral contract and Phoenix-owned web implementation for long, dynamically measured conversations. It consumes the render units defined by `specs/messagelist-render-units/` and provides one physical authority for windowing, measurement, positioning, prefix continuity, and tail preservation.

## Implementation surfaces

- `ui/src/components/VirtualTranscript.tsx` — web physical layout authority.
- `ui/src/conversation/virtualTranscriptLayout.ts` — pure measured layout model and visible-window calculations.
- `ui/src/fixtures/virtualTranscript/` — cross-platform semantic fixture corpus and web conformance renderer.
- `specs/virtual-transcript/virtual_transcript.allium` — positioning and measurement lifecycle.
- `specs/messagelist-render-units/` — render-unit construction and conversation scroll ownership policy.

## Verification matrix

| Requirement | Verification |
|---|---|
| REQ-VT-001–004 | Pure layout-model tests and VirtualTranscript component tests |
| REQ-VT-005 | Tall-row prefix continuity browser scenario with ≤2 CSS px drift |
| REQ-VT-006–007 | Navigation, missing-target, acknowledgement, and supersession tests |
| REQ-VT-008–010 | MessageList scroll-policy tests and dynamic measurement browser scenarios |
| REQ-VT-011 | Typed shared fixture schema and corpus tests |
| REQ-VT-012 | Playwright Chromium, WebKit, and Firefox conformance runs |
