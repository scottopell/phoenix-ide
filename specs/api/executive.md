# HTTP API - Executive Summary

## Requirements Summary

The HTTP API enables frontend clients to interact with PhoenixIDE conversations. Users list and create conversations, send messages with optional inline images, and receive real-time updates via Server-Sent Events. Conversations are created with a validated working directory and receive auto-generated slugs for human-readable URLs. Message retrieval supports partial fetch via `after_sequence` parameter for reconnection catch-up after SSE interruption. User actions (chat, cancel) forward to the state machine with immediate acknowledgment. Lifecycle operations include archive, unarchive, delete, and rename. Directory browser endpoints support the conversation creation UI. Model information endpoint returns available models based on configured API keys.

## Technical Summary

RESTful API with JSON request/response bodies. SSE streaming broadcasts `init`, `message`, `state_change`, and `agent_done` events to all connected clients. Reconnection flow: client fetches messages with `after_sequence` param, then reconnects SSE. Endpoint paths match Shelley API for frontend compatibility. Server struct holds database, LLM registry, and active conversation runtimes. Gzip compression for large responses; SSE uncompressed for flush-per-event. CSRF protection via custom header. No authentication in MVP (single-user deployment). Images sent inline as base64 in chat messages.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-API-001:** Conversation Listing | ❌ Not Started | GET /api/conversations |
| **REQ-API-002:** Conversation Creation | ❌ Not Started | POST /api/conversations/new |
| **REQ-API-003:** Message Retrieval | ❌ Not Started | With after_sequence support |
| **REQ-API-004:** User Actions | ❌ Not Started | Chat, cancel |
| **REQ-API-005:** Real-time Streaming | ❌ Not Started | SSE with reconnection flow |
| **REQ-API-006:** Conversation Lifecycle | ❌ Not Started | Archive, delete, rename |
| **REQ-API-007:** Slug Resolution | ❌ Not Started | GET /api/conversation-by-slug/{slug} |
| **REQ-API-008:** Directory Browser | ❌ Not Started | Validate, list for creation UI |
| **REQ-API-009:** Model Information | ❌ Not Started | GET /api/models |
| **REQ-API-010:** Static Assets | ❌ Not Started | Embedded UI serving |

**Progress:** 0 of 10 complete
