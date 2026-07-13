# Virtual Transcript

## Purpose

Virtual Transcript is the platform-neutral contract and Phoenix-owned web implementation for long, dynamically measured conversations. It consumes the render units defined by `specs/messagelist-render-units/` and provides one physical authority for windowing, measurement, positioning, prefix continuity, and tail preservation.

## Implementation surfaces

- `ui/src/components/VirtualTranscript.tsx` — web physical layout authority and effect executor for viewport writes/observations.
- `ui/src/conversation/virtualTranscriptLayout.ts` — pure measured layout model and visible-window calculations.
- `ui/src/conversation/transcriptPositioning.ts` — pure reducer for closed `idle(view)`/`positioning(command)` input, target-resolution evidence, issued-position revisions, physical observations, supersession, and exact-once finish effects.
- `fixtures/virtual-transcript/v1/schema.json` and `fixtures/virtual-transcript/v1/scenarios.json` — root portable JSON schema and conformance corpus.
- `ui/src/fixtures/virtualTranscript/scenarios.ts` — TypeScript validated adapter for the root corpus.
- `specs/virtual-transcript/virtual_transcript.allium` — reducer-aligned positioning and measurement lifecycle.
- `specs/messagelist-render-units/` — render-unit construction and conversation scroll ownership policy.

## Verification matrix

| Requirement | Verification |
|---|---|
| REQ-VT-001–004 | Pure layout-model tests and VirtualTranscript component tests |
| REQ-VT-005 | Tall-row prefix continuity browser scenario with ≤2 CSS px drift |
| REQ-VT-006–007 | `transcriptPositioning` reducer tests for target resolution, missing-target evidence, position-issued revision gating, physical observation, replacement/null/view/interruption/detach supersession, terminal identity scoping, and exact-once finish |
| REQ-VT-008–010 | MessageList scroll-policy tests and dynamic measurement browser scenarios |
| REQ-VT-011 | Root JSON Schema/corpus validation and TypeScript adapter tests |
| REQ-VT-012 | Playwright Chromium, WebKit, and Firefox conformance runs |
