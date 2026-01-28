# HTTP API

## User Story

As a frontend client, I need a well-defined HTTP API so that I can create conversations, send messages, receive real-time updates, and manage conversation lifecycle.

## Requirements

### REQ-API-001: Conversation Listing

WHEN client requests conversation list
THE SYSTEM SHALL return active conversations ordered by last update
AND include conversation ID, slug, working directory, and timestamps

WHEN client requests archived conversations
THE SYSTEM SHALL return archived conversations separately

**Rationale:** Users need to see and navigate their conversations.

---

### REQ-API-002: Conversation Creation

WHEN client requests new conversation
THE SYSTEM SHALL create conversation with specified working directory
AND generate unique ID and human-readable slug
AND return the new conversation details

WHEN working directory is invalid
THE SYSTEM SHALL return error without creating conversation

**Rationale:** Users start new conversations from specific directories.

---

### REQ-API-003: Message Retrieval

WHEN client requests conversation messages
THE SYSTEM SHALL return all messages in sequence order
AND include message type, content, timestamps, and display data
AND include current conversation state and context window usage

**Rationale:** Clients need full conversation history for display.

---

### REQ-API-004: Message Sending

WHEN client sends chat message
THE SYSTEM SHALL queue message for processing
AND return acknowledgment immediately
AND trigger state machine to process message

WHEN message includes images
THE SYSTEM SHALL accept base64-encoded image data

**Rationale:** Users send messages to interact with the agent.

---

### REQ-API-005: Real-time Streaming

WHEN client connects to conversation stream
THE SYSTEM SHALL establish Server-Sent Events connection
AND send current state immediately
AND stream new messages as they occur
AND stream state changes

WHEN multiple clients connect to same conversation
THE SYSTEM SHALL broadcast updates to all connected clients

**Rationale:** Users expect real-time feedback during agent execution.

---

### REQ-API-006: Cancellation

WHEN client requests cancellation
THE SYSTEM SHALL trigger state machine cancellation
AND return acknowledgment

**Rationale:** Users need to interrupt long-running operations.

---

### REQ-API-007: Conversation Management

WHEN client requests archive
THE SYSTEM SHALL mark conversation as archived
AND remove from active list

WHEN client requests unarchive
THE SYSTEM SHALL restore conversation to active list

WHEN client requests delete
THE SYSTEM SHALL permanently remove conversation and messages

WHEN client requests rename
THE SYSTEM SHALL update conversation slug

**Rationale:** Users manage conversation lifecycle.

---

### REQ-API-008: Slug-based Access

WHEN client requests conversation by slug
THE SYSTEM SHALL resolve slug to conversation ID
AND return conversation details

WHEN slug does not exist
THE SYSTEM SHALL return 404 error

**Rationale:** Human-readable URLs improve usability.

---

### REQ-API-009: File Operations

WHEN client uploads file
THE SYSTEM SHALL store file and return reference path

WHEN client requests file read
THE SYSTEM SHALL return file contents with appropriate content type

**Rationale:** Clients need to upload images and read generated files.

---

### REQ-API-010: Directory Validation

WHEN client requests directory validation
THE SYSTEM SHALL check if path exists and is a directory
AND return validation result

WHEN client requests directory listing
THE SYSTEM SHALL return directory contents for navigation

**Rationale:** Clients need to validate and browse directories for conversation creation.

---

### REQ-API-011: Model Information

WHEN client requests available models
THE SYSTEM SHALL return list of available model IDs
AND include default model

**Rationale:** Clients display model selection to users.

---

### REQ-API-012: Static Asset Serving

WHEN client requests frontend assets
THE SYSTEM SHALL serve embedded UI files
AND apply appropriate caching headers

**Rationale:** Single binary deployment includes frontend.
