# Web UI

## User Story

As a user on mobile or desktop, I need a responsive web interface to interact with Phoenix so that I can have conversations with the AI agent, monitor its progress, and manage my conversations—even with unreliable network connectivity.

## Requirements

### REQ-UI-001: Conversation List

WHEN user opens the app
THE SYSTEM SHALL display a list of active conversations
AND show conversation slug, working directory, and last update time
AND order conversations by most recently updated

WHEN user taps a conversation
THE SYSTEM SHALL navigate to that conversation's chat view
AND preserve the URL for deep linking (`/c/{slug}`)

**Rationale:** Users need to find and resume conversations. Deep links enable sharing and bookmarking.

---

### REQ-UI-002: Chat View

WHEN viewing a conversation
THE SYSTEM SHALL display all messages in chronological order
AND visually distinguish user messages from agent messages
AND group tool calls with their results
AND auto-scroll to newest content

WHEN agent message contains markdown
THE SYSTEM SHALL render basic markdown (code blocks, bold, italic, paragraphs)

**Rationale:** Users need to read the conversation history and understand tool execution.

---

### REQ-UI-003: Message Composition

WHEN user types in the input field
THE SYSTEM SHALL auto-resize the input up to a maximum height
AND persist draft text to localStorage per conversation
AND restore draft text on page load or navigation

WHEN user presses Enter (without Shift)
THE SYSTEM SHALL send the message

WHEN user presses Shift+Enter
THE SYSTEM SHALL insert a newline

**Rationale:** Users expect standard text input behavior. Draft persistence prevents frustrating message loss.

---

### REQ-UI-004: Message Delivery States

WHEN user sends a message
THE SYSTEM SHALL immediately display it with "sending" indicator (optimistic UI)
AND transition to "sent" indicator when server returns `{queued: true}`
AND transition to "failed" state if request fails

WHEN message is in "failed" state
THE SYSTEM SHALL display retry affordance
AND allow user to tap to retry sending

WHEN user sends message while offline
THE SYSTEM SHALL queue the message locally
AND display "sending" state (same as online send)
AND automatically send when connection is restored
AND persist queued messages to localStorage

Message states:
- **sending** (⏳): Not yet confirmed by server (queued offline or request in flight)
- **sent** (✓): Server returned `{queued: true}`
- **failed** (⚠️): Request failed, tap to retry

**Rationale:** Users on unreliable networks need confidence their messages won't be lost. Three simple states (sending/sent/failed) are easy to understand without exposing internal queue mechanics.

---

### REQ-UI-005: Connection Status

WHEN SSE connection is established
THE SYSTEM SHALL show "ready" indicator (green)

WHEN SSE connection is lost
THE SYSTEM SHALL immediately show "reconnecting" indicator (yellow)
AND attempt reconnection with exponential backoff (1s, 2s, 4s, ... max 30s)
AND show attempt count: "Reconnecting (attempt N)..."

WHEN reconnection fails repeatedly (3+ attempts)
THE SYSTEM SHALL show "offline" banner
AND display countdown to next retry attempt
AND continue retrying indefinitely (ceiling at 30s interval)

WHEN `navigator.onLine` transitions to false
THE SYSTEM SHALL immediately show offline state
AND pause reconnection attempts until online

WHEN connection is restored
THE SYSTEM SHALL show brief "reconnected" confirmation
AND resume normal "ready" state

**Rationale:** Users on subway commutes experience frequent, unpredictable disconnections. Clear feedback about connection state and automatic recovery reduces frustration.

---

### REQ-UI-006: Reconnection Data Integrity

WHEN reconnecting to SSE stream
THE SYSTEM SHALL track `last_sequence_id` from all received messages
AND reconnect with `?after={last_sequence_id}` parameter
AND deduplicate any messages by `sequence_id` as safety net

WHEN reconnection succeeds
THE SYSTEM SHALL seamlessly merge missed messages into the view
AND NOT show duplicate messages

**Rationale:** Users should never see duplicate messages or miss messages due to reconnection. The sequence_id mechanism ensures consistency.

---

### REQ-UI-007: Agent Activity Indicators

WHEN agent is working
THE SYSTEM SHALL show activity indicator (yellow pulsing dot)
AND display current state description ("thinking...", "bash", etc.)
AND show breadcrumb trail of completed steps in current turn

WHEN state is `tool_executing`
THE SYSTEM SHALL show tool name and queue depth: "bash (+2 queued)"

WHEN state is `awaiting_sub_agents`
THE SYSTEM SHALL show sub-agent progress: "sub-agents (2/3 done)"

**Rationale:** Users need confidence the system is making progress, especially during long operations.

---

### REQ-UI-008: Cancellation

WHEN agent is working
THE SYSTEM SHALL show Cancel button instead of Send
AND enable user to cancel the current operation

WHEN cancellation is in progress
THE SYSTEM SHALL show "Cancelling..." state
AND disable further cancel attempts

**Rationale:** Users need escape hatch for runaway operations or mistakes.

---

### REQ-UI-009: New Conversation

WHEN user taps "+ New" button
THE SYSTEM SHALL show modal to select working directory
AND validate directory exists before creation
AND navigate to new conversation on success

**Rationale:** Users need to start new conversations in specific project directories.

---

### REQ-UI-010: Responsive Layout

WHEN viewport is mobile-sized
THE SYSTEM SHALL use full-width layout
AND ensure touch targets are at least 44px
AND respect safe area insets for notched devices

WHEN viewport is desktop-sized
THE SYSTEM SHALL remain usable (not require mobile)
AND support keyboard navigation

**Rationale:** Primary use case is mobile, but desktop must work for development and occasional use.

---

### REQ-UI-011: Local Storage Schema

WHEN persisting data to localStorage
THE SYSTEM SHALL use keys namespaced by conversation ID:
- `phoenix:draft:{conversationId}` - draft message text in input
- `phoenix:queue:{conversationId}` - array of unsent messages (sending or failed)
- `phoenix:lastSeq:{conversationId}` - last seen sequence_id for reconnection

WHEN localStorage is unavailable or full
THE SYSTEM SHALL degrade gracefully without crashing
AND log warning to console

**Rationale:** Structured storage enables reliable persistence and cleanup. Namespace prevents conflicts.
