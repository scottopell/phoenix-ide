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
| POST | `/api/conversation/{id}/archive` | Archive conversation |
| POST | `/api/conversation/{id}/unarchive` | Unarchive conversation |
| POST | `/api/conversation/{id}/delete` | Delete conversation |
| POST | `/api/conversation/{id}/rename` | Rename conversation |
| GET | `/api/conversation-by-slug/{slug}` | Get conversation by slug |
| GET | `/api/validate-cwd` | Validate directory path |
| GET | `/api/list-directory` | List directory contents |
| POST | `/api/upload` | Upload file |
| GET | `/api/read` | Read file |
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

Slug generated from random words (e.g., "autumn-river-meadow").

### Get Conversation (REQ-API-003)

```
GET /api/conversation/{id}

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

data: {"type": "init", "conversation": Conversation, "messages": [Message, ...], "agent_working": true}

data: {"type": "message", "message": Message}

data: {"type": "state_change", "state": "tool_executing", "state_data": {...}}

data: {"type": "agent_done"}
```

#### Event Types

| Type | Description | Payload |
|------|-------------|--------|
| `init` | Initial state on connect | Full conversation + messages |
| `message` | New message added | Single message |
| `state_change` | Conversation state changed | New state + state_data |
| `agent_done` | Agent finished turn | None |
| `error` | Error occurred | Error message |

### Cancel (REQ-API-006)

```
POST /api/conversation/{id}/cancel

Response 200:
{
  "cancelled": true
}
```

### Archive/Unarchive/Delete (REQ-API-007)

```
POST /api/conversation/{id}/archive
POST /api/conversation/{id}/unarchive
POST /api/conversation/{id}/delete

Response 200:
{
  "success": true
}
```

### Rename (REQ-API-007)

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

### Get by Slug (REQ-API-008)

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

### Validate Directory (REQ-API-010)

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

### List Directory (REQ-API-010)

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

### File Upload (REQ-API-009)

```
POST /api/upload
Content-Type: multipart/form-data

file: <binary data>

Response 200:
{
  "path": "/tmp/phoenix-uploads/abc123.png"
}
```

### File Read (REQ-API-009)

```
GET /api/read?path=/tmp/phoenix-uploads/abc123.png

Response 200:
Content-Type: image/png
<binary data>
```

### Available Models (REQ-API-011)

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
- No authentication in MVP (single-user local deployment)

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
- Model switching mid-conversation: Not supported in MVP

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

```rust
struct ConversationBroadcaster {
    subscribers: Vec<Sender<SseEvent>>,
}

impl ConversationBroadcaster {
    fn broadcast(&self, event: SseEvent) {
        for sub in &self.subscribers {
            let _ = sub.send(event.clone());
        }
    }
}
```

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
