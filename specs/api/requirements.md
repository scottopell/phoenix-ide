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

WHEN generating slug
THE SYSTEM SHALL use format: `{day-of-week}-{time-of-day}-{word}-{word}`
WHERE day-of-week is from user's local timezone (monday, tuesday, etc.)
AND time-of-day is morning/afternoon/evening/night based on local hour
AND words are random dictionary words

WHEN path validation fails
THE SYSTEM SHALL return error without creating conversation

**Rationale:** Users start new conversations from specific directories. Time-based slugs help users locate recent conversations; random words ensure uniqueness.

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

WHEN client sends chat message while conversation is idle or in error state
THE SYSTEM SHALL forward message to state machine for processing
AND return acknowledgment immediately

WHEN client sends chat message while agent is busy
THE SYSTEM SHALL return error indicating agent is busy
AND inform user they can cancel current operation

WHEN client sends chat message with inline images
THE SYSTEM SHALL accept base64-encoded image data in message payload

WHEN client requests cancellation
THE SYSTEM SHALL forward cancel event to state machine
AND return acknowledgment

**Rationale:** Users interact with agent via messages and can interrupt operations. Rejecting messages while busy simplifies the state machine and makes message ordering explicit.

---

### REQ-API-005: Real-time Streaming

WHEN client connects to conversation SSE stream
THE SYSTEM SHALL send init event with current state, agent_working status, and last_sequence_id
AND stream new messages as they are persisted
AND stream state changes as they occur

WHEN LLM is generating a response
THE SYSTEM SHALL stream token events to connected clients as text is produced
AND include a request identifier so clients can correlate tokens with the in-flight request

WHEN client connects with `after` query parameter
THE SYSTEM SHALL include only messages with sequence_id > after in init event
AND then stream new messages normally

WHEN multiple clients connect to same conversation
THE SYSTEM SHALL broadcast updates to all connected clients

WHEN client reconnects after a connection drop during LLM generation
THE SYSTEM SHALL show one of two consistent states:
- The complete finalized message, if generation completed during the outage
- An accurate in-progress state with activity indication, if generation is still running
AND SHALL NOT show partial or duplicate content from the interrupted stream

WHEN client reconnects during a tool round before its checkpoint persists
THE SYSTEM SHALL include the in-flight assistant message (containing the LLM's text and any pending tool_use blocks) in the init payload
AND SHALL surface the current tool execution state via the breadcrumb / state_change event delivered in init's pending_events
SO THAT the user sees the active tool render in the main message list rather than a blank gap until the tool round completes

**Rationale:** Users expect real-time feedback during agent execution. Token streaming provides immediate evidence that the system is working. The `after` parameter enables seamless reconnection without a separate fetch request, eliminating race conditions. Reconnection correctness ensures dropped connections during long generations never leave users with stale or broken views. The in-flight-assistant-message coverage on reconnect closes the symmetric gap during tool execution: without it, a reconnect between "LLM finished, tool started" and "tool finished, checkpoint persisted" would blank out the assistant's message and the tool card.

---

### REQ-API-011: Granular State Change Events

WHEN conversation transitions to any new state
THE SYSTEM SHALL emit a state_change SSE event
AND include the state name and relevant state_data

WHEN state is `tool_executing`
THE SYSTEM SHALL include in state_data:
- current_tool: {name, id} of the tool being executed
- remaining_count: number of tools queued after current
- completed_count: number of tools already completed this turn

WHEN state is `llm_requesting`
THE SYSTEM SHALL include in state_data:
- attempt: current retry attempt number (1 for first try)

WHEN state is `awaiting_sub_agents`
THE SYSTEM SHALL include in state_data:
- pending_count: number of sub-agents still running
- completed_count: number of sub-agents finished

**Rationale:** Mobile UI displays state machine visualization as primary feedback mechanism. Users need to see exactly which tool is executing and queue depth to have confidence the system is progressing.

---

### REQ-API-006: Conversation Lifecycle

WHEN client requests archive
THE SYSTEM SHALL mark conversation as archived
AND remove from active conversation list
AND run the resource-cleanup cascade (REQ-BED-032) — releasing the
    conversation's bash handles, tmux server (subject to scope-equality
    preservation per REQ-TMUX-WS-002), worktree, and browser session
    (subject to REQ-BROWSER-WS-002 preservation)

THE SYSTEM SHALL NOT expose an `unarchive` operation. Archive is a
terminal lifecycle transition; the row is preserved for retrospection
but the conversation cannot resume in-place. Reviewing the unified
cleanup cascade in PR #135 made it clear that "live resources reclaimed
but row claims it can be resumed" is structurally incoherent — see
REQ-BED-032 rationale.

WHEN client requests delete
THE SYSTEM SHALL permanently remove conversation and all messages
AND run the resource-cleanup cascade (REQ-BED-032)

WHEN client requests rename with new slug
THE SYSTEM SHALL update slug if not already taken

**Rationale:** Users manage conversation lifecycle and organization.
Archive and delete share the same resource-release semantics; they
differ only in whether the DB row (and message history) is preserved.

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

---

### REQ-API-012: Reconnect Replay Buffer

WHEN the server emits a non-Message SSE event (token, state_change, message_updated, agent_done, conversation_update, conversation_became_terminal, error, browser_session_state, steer_message_queued)
THE SYSTEM SHALL retain the event in a per-conversation in-memory ring buffer until the next persisted Message broadcast replaces it (anchor reset)

WHEN the server emits an eager (non-persisted) assistant Message via the runtime's BroadcastAssistantMessage effect
THE SYSTEM SHALL append the Message event to the ring buffer without resetting the anchor
SO THAT a subscriber connecting before the corresponding persist_checkpoint completes still receives the in-flight assistant message

WHEN the ring buffer reaches its capacity (default 512 entries)
THE SYSTEM SHALL discard all entries (clear the ring)
AND set a `pending_truncated` flag on the conversation's ring
AND no-op all subsequent appends until the next anchor reset
WHICH is surfaced in init snapshots as `pending_events: []` and `pending_truncated: true`
SO THAT clients reconnecting in this window perform a clean DB-only resync rather than rendering a partial in-flight view that could mislead the user

WHEN the ring transitions to truncated (overflow detected on append)
THE SYSTEM SHALL emit a warn-level tracing event including the aggregate serialised byte size of the entries discarded at that transition
SO THAT operators get a useful "what was in the ring when it overflowed?" data point without paying a per-event serialisation cost on the hot path

WHEN an operator queries the per-conversation aggregate serialised byte size
THE SYSTEM SHALL compute it on demand by iterating the ring entries
WHERE the metric is observability-only (the cap is enforced by entry count, not bytes)
AND the accessor (`replay_ring_bytes()`) is intended for periodic scraping (gauge collector / dashboard), NOT per-event tracking
SO THAT pathological large-event-dominated rings can be detected before they become a memory issue, without making token streaming pay a `serde_json::to_vec` on every append

WHEN a client subscribes to a conversation's SSE stream
THE SYSTEM SHALL include in the init payload:
- `pending_anchor_sequence_id`: the sequence_id of the last persisted Message (the ring's anchor)
- `pending_events`: the ordered ring entries with sequence_ids strictly greater than the anchor
- `pending_truncated`: whether the ring overflowed since the anchor

WHEN the server process restarts
THE SYSTEM SHALL discard the ring buffer
WHERE clients reconnecting after a restart receive DB-only state in the init payload (empty pending_events), which is correct because no events were in flight across the restart

**Rationale:** Persisted-only state on the SSE wire is sufficient for crash recovery but produces a visible "blank UI" symptom during transient network outages mid-turn. Tokens, state_change events, and the eager in-flight assistant message all live between two persisted Message broadcasts; without a replay buffer, a reconnect in that window resyncs the UI to a state strictly behind what the user saw before disconnect. The ring is bounded, in-memory, and cheap (events are small structured payloads). The 512-entry cap covers ~10 seconds of LLM streaming — comfortable for typical mid-turn outages.

Overflow behaviour is clear-and-truncate rather than evict-oldest because partial replay was deemed misleading: a user reconnecting to a long-running turn would see only the tail of the in-flight stream filling in, with no indication that earlier content was lost. A clean force-resync is honest about the gap and the next persisted message restores authoritative state.

Bytes-based capping is deferred to observability-only: a debug-level metric exposes aggregate ring byte size so pathological large-event-dominated rings can be detected, but the enforcement cap remains entry-count-based for simplicity.
