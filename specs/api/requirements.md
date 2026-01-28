# HTTP API

## User Story

As a frontend client, I need a well-defined HTTP API so that I can create conversations, send messages, receive real-time updates, and manage conversation lifecycle.

## Requirements

### REQ-API-001: Conversation Listing

WHEN client requests conversation list
THE SYSTEM SHALL return active conversations ordered by last update
AND include conversation ID, slug, working directory, state, and timestamps

WHEN client requests archived conversations
THE SYSTEM SHALL return archived conversations separately

**Rationale:** Users need to see and navigate their conversations.

---

### REQ-API-002: Conversation Creation

WHEN client requests new conversation with working directory path
THE SYSTEM SHALL validate path exists and is a directory
AND create conversation with unique ID and human-readable slug
AND return the new conversation details

WHEN path validation fails
THE SYSTEM SHALL return error without creating conversation

**Rationale:** Users start new conversations from specific directories.

---

### REQ-API-003: Message Retrieval

WHEN client requests conversation messages
THE SYSTEM SHALL return all messages in sequence order
AND include message type, content, timestamps, and display data
AND include current conversation state and context window usage

WHEN client specifies after_sequence parameter
THE SYSTEM SHALL return only messages with sequence_id greater than specified
AND include current state for reconnection sync

**Rationale:** Full retrieval for initial load; partial retrieval for reconnection after SSE interruption.

---

### REQ-API-004: User Actions

WHEN client sends chat message
THE SYSTEM SHALL queue message for state machine processing
AND return acknowledgment immediately

WHEN client sends chat message with inline images
THE SYSTEM SHALL accept base64-encoded image data in message payload

WHEN client requests cancellation
THE SYSTEM SHALL forward cancel event to state machine
AND return acknowledgment

**Rationale:** Users interact with agent via messages and can interrupt operations.

---

### REQ-API-005: Real-time Streaming

WHEN client connects to conversation SSE stream
THE SYSTEM SHALL send current state and agent_working status immediately
AND stream new messages as they are persisted
AND stream state changes as they occur

WHEN multiple clients connect to same conversation
THE SYSTEM SHALL broadcast updates to all connected clients

WHEN client reconnects after disconnection
THE SYSTEM SHALL allow catch-up via REQ-API-003 partial retrieval before reconnecting stream

**Rationale:** Users expect real-time feedback during agent execution. Reconnection flow handles network interruptions.

---

### REQ-API-006: Conversation Lifecycle

WHEN client requests archive
THE SYSTEM SHALL mark conversation as archived
AND remove from active conversation list

WHEN client requests unarchive
THE SYSTEM SHALL restore conversation to active list

WHEN client requests delete
THE SYSTEM SHALL permanently remove conversation and all messages

WHEN client requests rename with new slug
THE SYSTEM SHALL update slug if not already taken

**Rationale:** Users manage conversation lifecycle and organization.

---

### REQ-API-007: Slug Resolution

WHEN client requests conversation by slug
THE SYSTEM SHALL resolve slug to conversation ID
AND return conversation details with messages

WHEN slug does not exist
THE SYSTEM SHALL return 404 error

**Rationale:** Human-readable URLs in browser improve usability over opaque IDs.

---

### REQ-API-008: Directory Browser

WHEN client requests directory validation for conversation creation
THE SYSTEM SHALL check if path exists and is a directory
AND return validation result with error message if invalid

WHEN client requests directory listing for path browser UI
THE SYSTEM SHALL return entries with name and is_directory flag
AND handle permission errors gracefully

**Rationale:** Conversation creation UI needs to validate and browse filesystem to select working directory.

---

### REQ-API-009: Model Information

WHEN client requests available models
THE SYSTEM SHALL return list of model IDs that are currently usable
AND indicate which model is the default

**Rationale:** UI displays model selection; only shows models with valid API keys configured.

---

### REQ-API-010: Static Assets

WHEN client requests path not matching API routes
THE SYSTEM SHALL serve embedded frontend assets
AND apply appropriate cache headers

**Rationale:** Single binary deployment includes frontend; no separate static file server needed.
