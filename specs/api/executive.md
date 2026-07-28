# HTTP API - Executive Summary

## Requirements Summary

The HTTP API exposes conversation listing, creation, chat submission, SSE streaming, directory/model helpers, and conversation lifecycle endpoints used by the shipped UI.

## Current Reality

The API still reflects the legacy lifecycle and chain model. Current shipped endpoints include archived-vs-active listing (`GET /api/conversations` and `/api/conversations/archived`), ordinary archive (`POST /api/conversations/:id/archive`), legacy terminal cleanup (`POST /api/conversations/:id/abandon-task`, `POST /api/conversations/:id/mark-merged`), chain routes under `/api/chains/:rootId/...`, and work-scope inventory endpoints under `/api/work-scope/:scope_key/...`. The future unified Open/History API shape has not landed yet, so this executive keeps those legacy endpoints visible as current behavior rather than implying the new normative lifecycle is already shipped.

## Technical Summary

REST/JSON plus SSE. The conversation SSE stream is specified to carry root-keyed `conversation_update`, typed `product_conversation_lifecycle`, continuation-boundary projection, and `work_scope_update` alongside persisted message, state-change, token, LLM lifecycle, and deletion events. Aggregate-bound live carriers are now normatively required to materialize as one closed typed envelope over the authoritative carrier set, sharing one RootStreamLedger ordering space across init, replay, and live delivery while carrying the root ProductConversation identity plus member identity where applicable. `work_scope_update` is aggregate-bound in that contract: it allocates a root-stream sequence, carries the root ProductConversation id plus inventory payload identity, omits any member-row owner, and appends to the same root replay/broadcast path. Only `init` and terminal `conversation_hard_deleted` remain outside that aggregate-bound envelope set. The server continues to back both ordinary conversation routes and dedicated chain routes. Conversation creation remains the existing shell-first HTTP flow; the durable creation protocol is only partially cut over.

## Requirement Map

| Requirement | Surface | Current status note |
|-------------|---------|---------------------|
| **REQ-API-001:** Conversation Listing | GET /api/conversations and /archived | Implemented, still active/archived rather than Open/History |
| **REQ-API-002:** Conversation Creation | Slug: day-time-word-word format | Implemented; durable creation cutover incomplete |
| **REQ-API-003:** Message Retrieval | GET with `after_sequence` param | Implemented |
| **REQ-API-004:** User Actions | POST chat, cancel endpoints | Implemented |
| **REQ-API-005:** Real-time Streaming | SSE with init, token events (`request_id` for correlation), replay-backed reconnection | Implemented |
| **REQ-API-006:** Conversation Lifecycle | Archive, delete, rename | Partially implemented against current norms: archive/delete/rename ship, but unified Close/Open/History lifecycle does not |
| **REQ-API-007:** Slug Resolution | GET /api/conversation-by-slug/{slug} | Implemented |
| **REQ-API-008:** Directory Browser | validate-cwd and list-directory | Implemented |
| **REQ-API-009:** Model Information | GET /api/models with default | Implemented |
| **REQ-API-010:** Static Assets | UI assets served from the binary (embedded), with a filesystem fallback | Implemented |
| **REQ-API-013:** One-Shot Command Suggestion | POST /api/suggest | Implemented |

## Verification Notes

Reconciled against `crates/phoenix-ide/src/api/handlers.rs`, `crates/phoenix-ide/src/api/lifecycle_handlers.rs`, `crates/phoenix-ide/src/api/chains.rs`, and `crates/phoenix-ide/src/api/sse.rs`.
