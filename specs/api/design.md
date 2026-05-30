# HTTP API - Design Document

## Overview

RESTful HTTP API for frontend clients to interact with PhoenixIDE. Designed for compatibility with existing Shelley React UI while supporting the new state machine architecture.

## Endpoint Summary

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/conversations` | List active conversations |
| GET | `/api/conversations/archived` | List archived conversations |
| POST | `/api/conversations/new` | Create new conversation |
| GET | `/api/conversation/{id}` | Get conversation with messages |
| GET | `/api/conversation/{id}/stream` | SSE stream for real-time updates |
| POST | `/api/conversation/{id}/chat` | Send user message |
| POST | `/api/conversation/{id}/cancel` | Cancel current operation |
| POST | `/api/conversation/{id}/archive` | Archive conversation (terminal; runs cleanup cascade) |
| POST | `/api/conversation/{id}/delete` | Delete conversation (terminal; runs cleanup cascade, removes row) |
| POST | `/api/conversation/{id}/rename` | Rename conversation |
| GET | `/api/conversation-by-slug/{slug}` | Get conversation by slug |
| GET | `/api/validate-cwd` | Validate directory path |
| GET | `/api/list-directory` | List directory contents |
| GET | `/api/models` | Get available models |
| GET | `/version` | Get server version |

## Data Types

### Conversation

```typescript
interface Conversation {
  id: string;
  slug: string | null;
  cwd: string;
  state: ConversationState;
  state_data: object | null;  // State-specific data (retry count, pending tools)
  created_at: string;  // ISO 8601
  updated_at: string;
  archived: boolean;
}

type ConversationState = 
  | "idle"
  | "awaiting_llm"
  | "llm_requesting"
  | "tool_executing"
  | "cancelling"
  | "awaiting_sub_agents"
  | "error"
  | "restart_pending";
```

### Message

```typescript
interface Message {
  message_id: string;
  conversation_id: string;
  sequence_id: number;
  type: MessageType;
  content: object;      // JSON structure varies by type
  display_data?: object; // UI-specific rendering data
  usage_data?: UsageData;
  created_at: string;
  end_of_turn?: boolean; // For agent messages
}

type MessageType = "user" | "agent" | "tool" | "system" | "error";

interface UsageData {
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
  cost_usd?: number;
}
```

## Endpoint Details

### List Conversations (REQ-API-001)

```
GET /api/conversations

Response 200:
{
  "conversations": [Conversation, ...]
}
```

Returns non-archived conversations ordered by `updated_at` descending.

### Create Conversation (REQ-API-002)

```
POST /api/conversations/new
Content-Type: application/json

{
  "cwd": "/home/user/project",
  "model": "claude-opus-4.5"  // optional, uses default if omitted
}

Response 200:
{
  "conversation": Conversation
}

Response 400:
{
  "error": "directory does not exist"
}
```

Slug generated as `{day}-{time}-{word}-{word}` (e.g., "monday-morning-autumn-river").

### Get Conversation (REQ-API-003)

```
GET /api/conversation/{id}
GET /api/conversation/{id}?after_sequence=42

Response 200:
{
  "conversation": Conversation,
  "messages": [Message, ...],
  "agent_working": boolean,
  "context_window_size": number
}
```

`agent_working` derived from state machine state.
`context_window_size` from most recent usage data.

With `after_sequence` param, returns only messages with `sequence_id > N`. Useful for debugging or manual inspection; SSE reconnection should use the `?after` param on the stream endpoint instead.

### Send Message (REQ-API-004)

```
POST /api/conversation/{id}/chat
Content-Type: application/json

{
  "text": "Please create a hello world function",
  "images": [  // optional
    {
      "data": "base64...",
      "media_type": "image/png"
    }
  ]
}

Response 200:
{
  "queued": true
}
```

Message queued for state machine processing. Updates arrive via SSE stream.

### SSE Stream (REQ-API-005)

```
GET /api/conversation/{id}/stream
Accept: text/event-stream

Response 200:
Content-Type: text/event-stream

data: {"type": "init", "conversation": Conversation, "messages": [Message, ...], "last_sequence_id": 57, "pending_events": [...]}

data: {"type": "message", "message": Message}

data: {"type": "state_change", "state": "tool_executing", "state_data": {...}}

data: {"type": "agent_done"}
```

Every broadcast event except `init` carries a `sequence_id` from a single per-conversation monotonic counter. The `init` snapshot reports `last_sequence_id` (the highest sequence the server has emitted) plus the ring's pending events, and the client uses these as the floor for replay-suppression. The authoritative wire shape and the full event-type set live in `specs/sse_wire/` (`SseWireEvent` in `crates/phoenix-ide/src/api/wire.rs`); this section covers only the API-surface essentials.

#### Event Types

The core event types a client must handle:

| Type | Description | Payload |
|------|-------------|--------|
| `init` | Initial snapshot on connect | Conversation + messages + `last_sequence_id` + `pending_events` (ring replay) |
| `message` | A newly persisted message | Single message (carries `sequence_id`) |
| `state_change` | Conversation phase transition | New state + state_data |
| `token` | Streaming text chunk | `{ sequence_id, text, request_id }` |
| `agent_done` | Agent finished turn | `{ sequence_id }` |
| `error` | User-facing error | `{ sequence_id, message, error }` |

The complete set additionally includes `message_updated`, `llm_first_byte`, `llm_attempt`, `conversation_became_terminal`, `conversation_hard_deleted`, `conversation_update`, `browser_session_state`, `steer_message_queued`, and `rate_limit_snapshot` — enumerated authoritatively in `specs/sse_wire/`.

#### Token Streaming Events

Token events are *ephemeral*: they are never persisted as DB rows and are never reconstructed from the DB on `init`. They do carry a `sequence_id` from the same counter as every other event, and they are buffered in the per-conversation `ReplayRing` so a reconnect mid-stream replays them.

```
event: token
data: {"sequence_id": 58, "text": "Let me ", "request_id": "req_abc123"}

event: token
data: {"sequence_id": 59, "text": "search for ", "request_id": "req_abc123"}
```

`request_id` lets the UI correlate chunks to the correct in-flight LLM request.

#### Reconnection During Streaming

Reconnection is handled by the `init` snapshot, which carries both the durable DB state and the `ReplayRing`'s `pending_events` (the ephemeral events broadcast since the last persisted message). On reconnect:

- **If the LLM call completed:** the finalized `message` is in the DB snapshot. The client renders it directly.
- **If the LLM call is still running:** `pending_events` carries the in-flight streaming tokens, current state, and any eager-broadcast assistant message, so the client resumes the in-progress view rather than blanking out. When the ring has overflowed (`pending_truncated`), the client falls back to a DB-only resync.

The replay-buffer contract is specified in REQ-API-012 and `specs/sse_wire/`.

### Cancel (REQ-API-004)

```
POST /api/conversation/{id}/cancel

Response 200:
{
  "ok": true
}
```

Forwards cancel event to state machine. State machine handles cancellation logic (REQ-BED-005).

### Archive/Delete (REQ-API-006)

```
POST /api/conversation/{id}/archive
POST /api/conversation/{id}/delete

Response 200:
{
  "success": true
}
```

Both are terminal lifecycle transitions and both run the
resource-cleanup cascade (REQ-BED-032). Archive preserves the DB row
and message history for retrospection; delete removes them via SQLite
`ON DELETE CASCADE`. There is no `unarchive` — archive is not
reversible (see REQ-API-006 / REQ-BED-032 rationale).

### Rename (REQ-API-006)

```
POST /api/conversation/{id}/rename
Content-Type: application/json

{
  "slug": "my-project-chat"
}

Response 200:
{
  "conversation": Conversation
}

Response 400:
{
  "error": "slug already exists"
}
```

### Get by Slug (REQ-API-007)

```
GET /api/conversation-by-slug/{slug}

Response 200:
{
  "conversation": Conversation,
  "messages": [Message, ...],
  "agent_working": boolean
}

Response 404:
{
  "error": "conversation not found"
}
```

### Validate Directory (REQ-API-008)

```
GET /api/validate-cwd?path=/home/user/project

Response 200:
{
  "valid": true
}

Response 200:
{
  "valid": false,
  "error": "directory does not exist"
}
```

### List Directory (REQ-API-008)

```
GET /api/list-directory?path=/home/user

Response 200:
{
  "entries": [
    {"name": "project", "is_dir": true},
    {"name": "file.txt", "is_dir": false}
  ]
}
```

### Available Models (REQ-API-009)

```
GET /api/models

Response 200:
{
  "models": ["claude-opus-4.5", "claude-sonnet-4.5", "gpt-5"],
  "default": "claude-opus-4.5"
}
```

## Error Handling

All errors return JSON with `error` field:

```typescript
interface ErrorResponse {
  error: string;
  details?: object;
}
```

HTTP status codes:
- 400: Bad request (invalid input)
- 404: Not found
- 500: Internal server error

## CORS and Security

- CORS headers for local development
- CSRF protection via custom header requirement
- No authentication (single-user local deployment)

## Compression

- Gzip compression for large responses (conversation messages)
- SSE streams not compressed (need per-event flushing)

## Shelley UI Compatibility

API designed to match Shelley's API surface for frontend compatibility:
- Same endpoint paths
- Same response shapes
- Same SSE event format

Features not implemented return appropriate errors:
- Browser tools: Tool not available
- Model switching mid-conversation: Not supported

## Implementation Notes

### Server Structure

```rust
pub struct Server {
    db: Database,
    llm_registry: ModelRegistry,
    conversations: HashMap<String, ConversationRuntime>,
    logger: slog::Logger,
}

impl Server {
    pub fn routes(&self) -> Router {
        Router::new()
            .route("/api/conversations", get(Self::list_conversations))
            .route("/api/conversations/new", post(Self::create_conversation))
            .route("/api/conversation/:id", get(Self::get_conversation))
            .route("/api/conversation/:id/stream", get(Self::stream_conversation))
            .route("/api/conversation/:id/chat", post(Self::send_message))
            .route("/api/conversation/:id/cancel", post(Self::cancel))
            // ... more routes
            .fallback_service(ServeDir::new("ui"))
    }
}
```

### SSE Broadcasting

Each conversation owns a `SseBroadcaster` (`crates/phoenix-ide/src/runtime.rs`) wrapping a tokio broadcast channel, a monotonic `AtomicI64` sequence counter, and the per-conversation `ReplayRing`. Events are allocated a sequence and emitted atomically with their ring operation via `send_seq` (ephemeral events, appended to the ring) or `send_persisted_message` (persisted messages, which reset the ring anchor). Per-client `sse_stream` handlers subscribe to the channel. The full broadcasting contract — ordering invariants, ring lifecycle, and replay semantics — is specified in `specs/sse_wire/`.

## File Organization

```
src/api/
├── mod.rs
├── server.rs         # Server struct, route registration
├── handlers/
│   ├── mod.rs
│   ├── conversations.rs  # List, create, get
│   ├── messages.rs       # Chat, stream
│   ├── lifecycle.rs      # Archive, delete, rename
│   └── files.rs          # Upload, read, validate
├── sse.rs            # SSE event types, broadcasting
└── types.rs          # API request/response types
```
