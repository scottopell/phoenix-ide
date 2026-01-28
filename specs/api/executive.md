# HTTP API - Executive Summary

## Requirements Summary

The HTTP API enables frontend clients to interact with PhoenixIDE conversations. Users list and create conversations, send messages with optional inline images, and receive real-time updates via Server-Sent Events. Conversations are created with a validated working directory and receive auto-generated slugs in format `{day}-{time}-{word}-{word}` (e.g., "monday-morning-autumn-river"). SSE streaming includes `after` query parameter for seamless reconnection without race conditions. User actions (chat, cancel) forward to the state machine with immediate acknowledgment. Lifecycle operations include archive, unarchive, delete, and rename. Directory browser endpoints support the conversation creation UI. Model information endpoint returns available models based on configured API keys.

## Technical Summary

RESTful API with JSON request/response bodies. SSE streaming broadcasts `init`, `message`, `state_change`, and `agent_done` events to all connected clients. SSE `init` event includes `last_sequence_id`; reconnection uses `?after=N` to receive only missed messages in init event. Endpoint paths match Shelley API for frontend compatibility. Server struct holds database, LLM registry, and active conversation runtimes. Gzip compression for large responses; SSE uncompressed for flush-per-event. CSRF protection via custom header. No authentication in MVP (single-user deployment). Images sent inline as base64 in chat messages.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-API-001:** Conversation Listing | ✅ Complete | GET /api/conversations and /archived |
| **REQ-API-002:** Conversation Creation | ✅ Complete | Slug: day-time-word-word format |
| **REQ-API-003:** Message Retrieval | ✅ Complete | GET with after_sequence param |
| **REQ-API-004:** User Actions | ✅ Complete | POST chat, cancel endpoints |
| **REQ-API-005:** Real-time Streaming | ✅ Complete | SSE with init event and ?after |
| **REQ-API-006:** Conversation Lifecycle | ✅ Complete | Archive, unarchive, delete, rename |
| **REQ-API-007:** Slug Resolution | ✅ Complete | GET /api/conversation-by-slug/{slug} |
| **REQ-API-008:** Directory Browser | ✅ Complete | validate-cwd and list-directory |
| **REQ-API-009:** Model Information | ✅ Complete | GET /api/models with default |
| **REQ-API-010:** Static Assets | ✅ Complete | Route defined (no embedded assets in MVP) |

**Progress:** 10 of 10 complete
