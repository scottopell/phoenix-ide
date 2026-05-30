# HTTP API - Executive Summary

## Requirements Summary

The HTTP API enables frontend clients to interact with PhoenixIDE conversations. Users list and create conversations, send messages with optional inline images, and receive real-time updates via Server-Sent Events. Conversations are created with a validated working directory and receive auto-generated slugs in format `{day}-{time}-{word}-{word}` (e.g., "monday-morning-autumn-river"). SSE streaming includes `after` query parameter for seamless reconnection without race conditions. User actions (chat, cancel) forward to the state machine with immediate acknowledgment. Lifecycle operations include archive (terminal — runs cleanup cascade, preserves row), delete (terminal — runs cleanup cascade, removes row), and rename. Directory browser endpoints support the conversation creation UI. Model information endpoint returns available models based on configured API keys.

## Technical Summary

RESTful API with JSON request/response bodies. SSE streaming broadcasts conversation events to all connected clients (`init`, `message`, `state_change`, `token`, `agent_done`, and the full set enumerated in `specs/sse_wire/`). The `init` snapshot includes `last_sequence_id` and the `ReplayRing`'s `pending_events`, so a reconnecting client resyncs durable DB state and resumes any in-flight ephemeral state in one payload. Endpoint paths match Shelley API for frontend compatibility. Server struct holds database, LLM registry, and active conversation runtimes. Gzip compression for large responses; SSE uncompressed for flush-per-event. CSRF protection via custom header. Single-user deployment with no authentication. Images sent inline as base64 in chat messages.

## Requirement Map

| Requirement | Surface |
|-------------|---------|
| **REQ-API-001:** Conversation Listing | GET /api/conversations and /archived |
| **REQ-API-002:** Conversation Creation | Slug: day-time-word-word format |
| **REQ-API-003:** Message Retrieval | GET with `after_sequence` param |
| **REQ-API-004:** User Actions | POST chat, cancel endpoints |
| **REQ-API-005:** Real-time Streaming | SSE with init, token events (`request_id` for correlation), and `ReplayRing`-backed reconnection |
| **REQ-API-006:** Conversation Lifecycle | Archive (terminal), delete (terminal), rename. Both terminal transitions run the REQ-BED-032 cleanup cascade |
| **REQ-API-007:** Slug Resolution | GET /api/conversation-by-slug/{slug} |
| **REQ-API-008:** Directory Browser | validate-cwd and list-directory |
| **REQ-API-009:** Model Information | GET /api/models with default |
| **REQ-API-010:** Static Assets | UI assets served from the binary (embedded), with a filesystem fallback |
