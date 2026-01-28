# Bedrock: Core Conversation State Machine

## User Story

As a developer using PhoenixIDE, I need reliable, predictable conversation execution so that my agent interactions never get stuck, lose state, or behave unexpectedly.

## Requirements

### REQ-BED-001: Pure State Transitions

WHEN any event occurs in a conversation
THE SYSTEM SHALL compute the next state and effects using a pure function with no I/O

WHEN the transition function is called with identical inputs
THE SYSTEM SHALL return identical outputs

**Rationale:** Users need predictable agent behavior. Pure functions enable comprehensive testing and eliminate entire classes of state-related bugs.

---

### REQ-BED-002: User Message Handling

WHEN user sends a message while conversation is idle
THE SYSTEM SHALL transition to awaiting LLM response
AND queue the message for LLM processing

WHEN user sends a message while agent is working
THE SYSTEM SHALL queue the message as a follow-up
AND process it after current work completes

**Rationale:** Users expect their messages to be acknowledged and processed in order, even during active agent work.

---

### REQ-BED-003: LLM Response Processing

WHEN LLM responds with text only and end_turn=true
THE SYSTEM SHALL transition to idle
AND persist the response for display

WHEN LLM responds with tool use requests
THE SYSTEM SHALL transition to tool executing state
AND queue all requested tools for execution

WHEN LLM responds with text only and end_turn=false
THE SYSTEM SHALL continue awaiting additional LLM content

**Rationale:** Users need seamless flow between conversation and tool execution without manual intervention.

---

### REQ-BED-004: Tool Execution Coordination

WHEN multiple tools are requested in a single LLM response
THE SYSTEM SHALL execute all tools concurrently
AND collect results preserving request order

WHEN all tools complete successfully
THE SYSTEM SHALL transition to awaiting LLM response
AND send tool results to LLM

WHEN any tool fails
THE SYSTEM SHALL include the error in results sent to LLM
AND allow LLM to handle the error

**Rationale:** Users benefit from faster execution through parallelism while maintaining predictable result ordering.

---

### REQ-BED-005: Cancellation Handling

WHEN user requests cancellation during LLM request
THE SYSTEM SHALL transition to cancelling state
AND complete gracefully when LLM responds

WHEN user requests cancellation during tool execution
THE SYSTEM SHALL attempt to stop running tools
AND transition to idle after cleanup

WHEN cancellation completes
THE SYSTEM SHALL preserve all conversation history
AND allow user to continue the conversation

**Rationale:** Users need the ability to interrupt long-running operations without losing their work.

---

### REQ-BED-006: Error Recovery

WHEN LLM request fails with retryable error
THE SYSTEM SHALL retry up to 3 times with exponential backoff

WHEN LLM request fails after all retries
THE SYSTEM SHALL transition to error state
AND display actionable error message to user

WHEN conversation enters error state
THE SYSTEM SHALL allow user to retry or continue with new message

**Rationale:** Users should not lose their conversation due to transient failures.

---

### REQ-BED-007: State Persistence

WHEN conversation state changes
THE SYSTEM SHALL persist the new state before executing effects

WHEN server restarts with active conversations
THE SYSTEM SHALL restore conversations to their persisted state
AND resume pending operations

**Rationale:** Users expect their conversations to survive server restarts without data loss.

---

### REQ-BED-008: Sub-Agent Spawning

WHEN LLM requests sub-agent spawn
THE SYSTEM SHALL create independent sub-agent conversations
AND execute them in parallel

WHEN all sub-agents complete
THE SYSTEM SHALL aggregate results
AND return them to parent conversation

WHEN any sub-agent fails
THE SYSTEM SHALL include failure information in aggregated results
AND allow parent to handle the failure

**Rationale:** Users benefit from parallel task execution for bootstrapping and complex multi-step operations.

---

### REQ-BED-009: Sub-Agent Isolation

WHEN sub-agent is executing
THE SYSTEM SHALL maintain completely independent state from parent
AND prevent sub-agents from spawning their own sub-agents

WHEN sub-agent conversation exists
THE SYSTEM SHALL track it as non-user-initiated
AND exclude it from normal conversation listings

**Rationale:** Users need isolation guarantees to prevent cascading failures and resource exhaustion.

---

### REQ-BED-010: Immutable Working Directory

WHEN conversation is created
THE SYSTEM SHALL assign a fixed working directory

WHEN tools execute
THE SYSTEM SHALL use the conversation's assigned working directory
AND reject attempts to change it

**Rationale:** Users benefit from simplified mental model where each conversation operates in a predictable location.

---

### REQ-BED-011: Real-time Event Streaming

WHEN conversation state changes
THE SYSTEM SHALL emit event to all connected clients

WHEN new message is persisted
THE SYSTEM SHALL stream it to clients immediately

WHEN client connects to active conversation
THE SYSTEM SHALL send current state and recent messages

**Rationale:** Users expect responsive UI that reflects agent activity in real-time.

---

### REQ-BED-012: Context Window Tracking

WHEN LLM response includes usage data
THE SYSTEM SHALL track context window consumption

WHEN context approaches model limit
THE SYSTEM SHALL notify user of approaching limit

**Rationale:** Users need visibility into context usage to manage long conversations effectively.
