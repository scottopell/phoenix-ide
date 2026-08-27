# Instrument client conversation readiness

Extend the existing content-free conversation-open telemetry so one correlated browser timeline spans ConversationPage route render, authoritative route resolution, EventSource construction and native open, validated SSE init handling, and the first transcript paint opportunity.

## Acceptance criteria

- Initial opens correlate browser milestones with the SSE stream through one redacted open ID.
- Reconnects retain stream-only telemetry without fabricating route milestones.
- Error, cancellation, React StrictMode replay, and pre-paint teardown complete at most once.
- The backend accepts only bounded ordered timelines and emits no conversation identity or content.
- Focused UI/backend regressions and the full development gate pass.
