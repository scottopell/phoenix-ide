# Web UI

## User Story

As a user on mobile or desktop, I need a responsive web interface to interact with Phoenix so that I can have conversations with the AI agent, monitor its progress, and manage my conversations—even with unreliable network connectivity.

## Transparency Contract

The user has delegated a complex task to an autonomous agent. The single worst outcome
is not a failed task — it is a user who cannot determine which outcome occurred. Every
UI requirement exists to help users confidently answer these questions.

**During execution:**
1. Is the agent still running, or has it stopped?
2. What is it doing right now — which specific operation is in flight?
3. How long has it been in the current operation?
4. What has it completed so far, in what order?
5. Are sub-agents running? Which ones finished?
6. Has anything gone wrong that I should know about, even if execution continues?

**Immediately after completion:**
7. Did the task succeed or fail?
8. What did the agent actually do? (All steps, in order, with results.)
9. For each tool call: what was the input, what was the output, did it succeed?
10. If it failed: where, and what was the error?

**Days later on review:**
11. What was the original request?
12. What did the agent do in response?
13. What model was used? How many tokens were consumed?
14. Were there timeouts, retries, or partial failures?

This contract is the acceptance test for UI completeness. If a question cannot be
answered confidently from the UI, that is a missing or incomplete requirement.

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
AND display current state description with an explicit label for every possible state
AND NOT use a catch-all or generic label for unrecognized states

WHEN state is `llm_requesting`
THE SYSTEM SHALL show "thinking..." with retry attempt number if retrying

WHEN state is `tool_executing`
THE SYSTEM SHALL show tool name and queue depth: "bash (+2 queued)"

WHEN state is `awaiting_sub_agents`
THE SYSTEM SHALL show sub-agent progress: "sub-agents (2/3 done)"

WHEN state is `cancelling`, `cancelling_tool`, or `cancelling_sub_agents`
THE SYSTEM SHALL show "cancelling..."

WHEN a new backend state variant is added
THE SYSTEM SHALL require an explicit display label before it can be rendered

WHEN agent is idle, in error, or in a terminal state
THE SYSTEM SHALL NOT show the activity indicator

**Rationale:** Users need confidence the system is making progress (transparency questions 1-2). Exhaustive state labels prevent silent degradation when backend states evolve. The "is the agent working?" question must have exactly one unambiguous answer derived from one source.


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

**DEPRECATED:** Replaced by REQ-UI-015 (mobile bottom sheet), REQ-UI-017 (desktop full page), and REQ-UI-018 (desktop inline sidebar).

**Deprecation Reason:** Original requirement was too generic. New conversation flows differ significantly by viewport and context, requiring separate requirements for each mode.

---

### REQ-UI-010: Responsive Layout

WHEN viewport is mobile-sized (< 768px)
THE SYSTEM SHALL use full-width single-column layout
AND ensure touch targets are at least 44px
AND respect safe area insets for notched devices

WHEN viewport is tablet-sized (768px - 1024px)
THE SYSTEM SHALL use mobile layout patterns
AND support keyboard navigation where available

WHEN viewport is desktop-sized (> 1024px)
THE SYSTEM SHALL use sidebar layout per REQ-UI-016
AND support full keyboard navigation

**Rationale:** Phoenix serves both mobile (on-the-go monitoring) and desktop (primary development) use cases. Each viewport size gets optimized layout rather than one-size-fits-all responsive scaling.

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

---

### REQ-UI-012: Conversation State Indicators

WHEN displaying the conversation list
THE SYSTEM SHALL show a visual state indicator for each conversation
AND use distinct colors/icons for idle (green), working (yellow/pulsing), and error (red) states

WHEN user views the conversation list
THE SYSTEM SHALL enable at-a-glance identification of which conversations need attention (error) or are actively running (working)

**Rationale:** Users managing multiple conversations need quick visibility into what's running, what's waiting for input, and what has failed—without opening each conversation individually.

---

### REQ-UI-013: Per-Conversation Scroll Position Memory

WHEN user navigates away from a conversation
THE SYSTEM SHALL persist the current scroll position for that conversation

WHEN user returns to a previously viewed conversation
THE SYSTEM SHALL restore the scroll position to where they left off
AND NOT auto-scroll to the bottom (unless they were already at the bottom)

WHEN conversation receives new messages while user is away
THE SYSTEM SHALL still restore to saved position
AND provide affordance to jump to newest content

**Rationale:** Users reading through long conversations lose their place when switching contexts. Preserving scroll position respects user attention and reduces re-orientation friction.

---

### REQ-UI-014: Desktop Message Readability

WHEN viewport is desktop-sized (> 768px)
THE SYSTEM SHALL constrain message content width to a readable maximum (approximately 800px)
AND center the constrained content within available space

WHEN code blocks or wide content appear
THE SYSTEM SHALL allow horizontal scroll within the block rather than expanding the container

**Rationale:** Unconstrained line lengths on wide displays harm readability. Comfortable reading width (60-80 characters for prose) reduces eye strain during long sessions.

---

### REQ-UI-015: Mobile New Conversation Bottom Sheet

WHEN user initiates new conversation on mobile viewport
THE SYSTEM SHALL present a bottom sheet overlay (not full-page navigation)
AND include directory picker, model selector, and initial message input
AND provide "Send" button to create and navigate to conversation
AND provide "Send in Background" option to create without navigating
AND allow dismissal via swipe-down or backdrop tap

WHEN bottom sheet is open
THE SYSTEM SHALL keep the current view visible behind the sheet (dimmed)
AND NOT lose context of what user was viewing

WHEN user chooses "Send in Background"
THE SYSTEM SHALL create the conversation and start agent processing
AND close the bottom sheet
AND keep user in current conversation
AND show brief confirmation toast

**Rationale:** Full-page navigation for new conversation breaks user's mental context. Bottom sheet maintains awareness of current state. "Send in Background" enables spawning tasks without context-switching, consistent with desktop inline sidebar mode.

---

### REQ-UI-016: Desktop Sidebar Layout

WHEN viewport is desktop-sized (> 1024px)
THE SYSTEM SHALL display conversation list as a persistent sidebar alongside the main content
AND show the active conversation highlighted in the sidebar
AND place "+ New" button at the top of the sidebar
AND allow collapsing the sidebar to a narrow strip via toggle

WHEN sidebar is visible and user clicks a conversation
THE SYSTEM SHALL switch the main content to that conversation without full-page navigation

WHEN sidebar is collapsed
THE SYSTEM SHALL show conversation state indicators (dots) for recent conversations
AND expand on click or hover

**Rationale:** Desktop users have screen real estate to see both conversation list and active conversation simultaneously. This eliminates the multi-step navigation to switch contexts and provides ambient awareness of other conversations' states.

---

### REQ-UI-017: Desktop New Conversation - Full Page Mode

WHEN user navigates to root route (`/`) on desktop with sidebar visible
THE SYSTEM SHALL render the full new-conversation form in the main content area
AND show the conversation list in the sidebar (no active highlight)
AND display Phoenix icon at top of sidebar as click target for this view

WHEN user clicks "+ New" button while on root route
THE SYSTEM SHALL treat click as no-op (already on new conversation view)

WHEN user clicks Phoenix icon in sidebar
THE SYSTEM SHALL navigate to root route (`/`)

WHEN user submits the new conversation form
THE SYSTEM SHALL create the conversation and navigate to it
AND highlight it in the sidebar

WHEN user submits with "Send in Background" option
THE SYSTEM SHALL create the conversation and start agent processing
AND remain on root route for another new conversation
AND show brief confirmation toast

**Rationale:** Direct navigation to `/` indicates intentional "start fresh" flow. Full-page form provides complete settings access without space constraints. Phoenix icon provides visual anchor and alternative navigation path. Background send enables batch-spawning multiple conversations.

---

### REQ-UI-018: Desktop New Conversation - Inline Sidebar Mode

WHEN user clicks "+ New" while viewing an existing conversation (`/c/:slug`)
THE SYSTEM SHALL expand an inline new-conversation form at the top of the sidebar
AND keep the current conversation visible in the main content area
AND NOT navigate away from the current conversation

WHEN inline form is visible
THE SYSTEM SHALL provide directory picker, model selector, and message input
AND provide "Send" button to create and navigate to conversation
AND provide "Send in Background" option to create without navigating
AND allow dismissal via cancel button or Escape key

WHEN user submits with "Send" (default)
THE SYSTEM SHALL create the conversation, navigate to it, and collapse the form

WHEN user submits with "Send in Background"
THE SYSTEM SHALL create the conversation and start agent processing
AND collapse the form
AND keep user in current conversation
AND show brief confirmation toast

**Rationale:** Users monitoring an active conversation need to spawn side tasks without losing context. Inline form enables "quick new conversation" without disrupting the current view. "Send in Background" is the power-user path for spawning work while staying focused.

---

### REQ-UI-020: Navigation Persistence

WHEN user navigates away from a conversation and returns within the same session
THE SYSTEM SHALL restore the conversation state without a full re-fetch
AND reconnect the SSE stream from the last seen sequence ID
AND NOT show a loading flash for recently-visited conversations

WHEN user navigates back to a conversation with an active agent
THE SYSTEM SHALL resume displaying the current state immediately
AND pick up any missed events via sequence-based reconnection

WHEN user navigates back to a conversation where streaming was in progress
THE SYSTEM SHALL show the current agent state (streaming may have completed during absence)
AND NOT attempt to reconstruct missed token events

**Rationale:** Navigation between conversations should feel instantaneous for recently-visited conversations. The reconnection cursor (`lastSequenceId`) must survive navigation — it cannot live in component state that unmounts. Missed streaming tokens during navigation are acceptable; the finalized message will arrive via normal reconnection.

---

### REQ-UI-019: Streaming Text Display

WHEN LLM is generating a text response
THE SYSTEM SHALL display partial text as it arrives, below the conversation history
AND render the text with basic formatting as it accumulates

WHEN LLM response completes and the message is saved
THE SYSTEM SHALL replace the streaming display with the finalized rendered message
AND the transition SHALL NOT produce visible duplication, flickering, or content loss

WHEN user scrolls up during streaming
THE SYSTEM SHALL NOT force auto-scroll back to the streaming text
AND SHALL provide affordance to jump to live output

**Rationale:** Progressive text display confirms the system is working and lets users start reading during generation. Clean transition to the finalized message ensures the streaming view is never a "different version" of the response — the saved message is always authoritative.
