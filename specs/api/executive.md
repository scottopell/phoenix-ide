# HTTP API - Executive Summary

## Requirements Summary

The HTTP API enables frontend clients to interact with PhoenixIDE conversations. Users list and create conversations, send messages, and receive real-time updates via Server-Sent Events. Conversations are created with a specified working directory and receive auto-generated slugs for human-readable URLs. Message retrieval returns full history with state machine status and context window usage. Cancellation triggers state machine interruption. Lifecycle operations include archive, unarchive, delete, and rename. File operations support image upload and file reading. Directory validation and listing enable path browsing for conversation creation. Model information endpoint returns available models based on configured API keys.

## Technical Summary

RESTful API with JSON request/response bodies. SSE streaming for real-time updates broadcasts `init`, `message`, `state_change`, and `agent_done` events to all connected clients. Endpoint paths match Shelley API for frontend compatibility. Server struct holds database, LLM registry, and active conversation runtimes. Routes registered via Axum router with embedded UI fallback. Gzip compression for large responses; SSE uncompressed for flush-per-event. CSRF protection via custom header. No authentication in MVP (single-user deployment).

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-API-001:** Conversation Listing | ❌ Not Started | GET /api/conversations |
| **REQ-API-002:** Conversation Creation | ❌ Not Started | POST /api/conversations/new |
| **REQ-API-003:** Message Retrieval | ❌ Not Started | GET /api/conversation/{id} |
| **REQ-API-004:** Message Sending | ❌ Not Started | POST /api/conversation/{id}/chat |
| **REQ-API-005:** Real-time Streaming | ❌ Not Started | SSE endpoint |
| **REQ-API-006:** Cancellation | ❌ Not Started | POST /api/conversation/{id}/cancel |
| **REQ-API-007:** Conversation Management | ❌ Not Started | Archive, delete, rename |
| **REQ-API-008:** Slug-based Access | ❌ Not Started | GET /api/conversation-by-slug/{slug} |
| **REQ-API-009:** File Operations | ❌ Not Started | Upload, read |
| **REQ-API-010:** Directory Validation | ❌ Not Started | Validate, list |
| **REQ-API-011:** Model Information | ❌ Not Started | GET /api/models |
| **REQ-API-012:** Static Asset Serving | ❌ Not Started | Embedded UI |

**Progress:** 0 of 12 complete
