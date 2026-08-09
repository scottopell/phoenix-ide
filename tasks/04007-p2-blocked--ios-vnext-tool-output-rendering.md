# Complete iOS vNext tool-output rendering

## Outcome

Give every current Phoenix tool invocation and result an intentional, legible native presentation.

## Dependencies

Blocked by ProductConversation migration and the rendering fixture harness.

## Scope

Inventory the shipped tool registry and wire payloads, group tools by renderer family, and create numbered leaf tasks such as `ios-tool-010-...`. Each leaf task owns one narrow renderer or shared family, representative lifecycle/error cases, and focused fixture/test evidence.

Preserve a conspicuous generic fallback for genuinely unknown future tools; do not use it to hide known unimplemented tools.

## Acceptance

- Every shipped tool is cataloged as specialized, intentionally shared, or explicitly queued.
- Pending, running, completed, failed, malformed, empty, long, and unknown cases remain visible.
- Invocation/result pairing and typed payload distinctions are preserved.
- Agents can take the next ready numbered tasks without reopening renderer-wide design.

## Out of scope

Grounding/file browsing and prose-reader interactions.
