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
THE SYSTEM SHALL reject the message with "agent is busy" error
AND inform user they can cancel current operation

**Rationale:** Users can cancel and send a new message if needed. Rejecting during busy state simplifies the state machine and avoids hidden message queues.

---

### REQ-BED-013: Image Handling

WHEN user sends message with attached images
THE SYSTEM SHALL include images in the message content sent to LLM
AND persist image references in message history

WHEN preparing LLM request with images
THE SYSTEM SHALL encode images appropriately for the provider
AND respect provider image size limits by resizing if necessary

**Rationale:** Users need to share screenshots, diagrams, and other visual context with the agent. Image handling must flow cleanly through the state machine to the LLM provider.

---

### REQ-BED-003: LLM Response Processing

WHEN LLM responds with text only and end_turn=true
THE SYSTEM SHALL transition to idle
AND persist the response for display

WHEN LLM responds with tool use requests
THE SYSTEM SHALL transition to tool executing state
AND queue tools for serial execution in request order

WHEN LLM responds with text only and end_turn=false
THE SYSTEM SHALL continue awaiting additional LLM content

**Rationale:** Users need seamless flow between conversation and tool execution without manual intervention.

---

### REQ-BED-004: Tool Execution Coordination

WHEN multiple tools are requested in a single LLM response
THE SYSTEM SHALL execute tools serially in the order requested
AND complete each tool before starting the next

WHEN all tools complete
THE SYSTEM SHALL transition to awaiting LLM response
AND send all tool results to LLM

WHEN any tool fails
THE SYSTEM SHALL include the error in results sent to LLM
AND allow LLM to handle the error

**Rationale:** Serial execution respects LLM's intended order and prevents resource conflicts between tools.

---

### REQ-BED-005: Cancellation Handling

WHEN user requests cancellation during LLM request
THE SYSTEM SHALL abort the in-flight HTTP request immediately
AND transition to idle state
AND NOT persist any partial LLM response

WHEN user requests cancellation during tool execution
THE SYSTEM SHALL interrupt the running tool immediately (within 100ms)
AND terminate any spawned subprocesses
AND record a synthetic tool result indicating cancellation
AND skip remaining queued tools with synthetic cancelled results
AND transition to idle state

WHEN cancellation is requested
THE SYSTEM SHALL NOT queue the cancel behind completion of current operation
AND SHALL process cancel with higher priority than operation completion

WHEN cancellation completes
THE SYSTEM SHALL preserve all conversation history including synthetic results

**Rationale:** Users need the ability to interrupt long-running operations immediately, not after they complete. CPU-intensive tools or stuck processes must be killable. Synthetic tool results maintain message chain integrity required by LLM APIs.

---

### REQ-BED-006: Error Recovery

WHEN LLM request fails with retryable error (network, rate limit, 5xx)
THE SYSTEM SHALL retry automatically up to 3 times with exponential backoff
AND remain in LLM requesting state during retries
AND display retry status to user

WHEN LLM request fails after all retries exhausted
THE SYSTEM SHALL transition to error state
AND display actionable error message indicating retry failure

WHEN LLM request fails with non-retryable error (auth, 4xx)
THE SYSTEM SHALL transition to error state immediately
AND display specific error message

WHEN user sends message while in error state
THE SYSTEM SHALL transition to awaiting LLM
AND attempt to continue the conversation

**Rationale:** Users should not lose their conversation due to transient failures. Clear error states with specific messages enable recovery.

---

### REQ-BED-007: State Persistence

WHEN conversation state changes
THE SYSTEM SHALL persist the new state before executing effects

WHEN server restarts
THE SYSTEM SHALL restore all conversations to idle state
AND preserve complete message history

**Rationale:** Users expect their conversation history to survive server restarts. Resuming from idle is simple and predictable; users can re-send their last message if interrupted.

---

### REQ-BED-008: Sub-Agent Spawning

WHEN LLM requests sub-agent spawn
THE SYSTEM SHALL create independent sub-agent conversations
AND execute them in parallel

WHEN sub-agent completes its task
THE SYSTEM SHALL require it to call a dedicated result submission tool
AND capture the submitted result

WHEN all sub-agents have submitted results
THE SYSTEM SHALL aggregate results
AND return them to parent conversation

WHEN any sub-agent fails or times out without submitting
THE SYSTEM SHALL include failure information in aggregated results
AND allow parent to handle the failure

**Rationale:** Users benefit from parallel task execution for bootstrapping and complex operations. Explicit result submission provides clean completion semantics.

---

### REQ-BED-009: Sub-Agent Isolation

WHEN sub-agent is executing
THE SYSTEM SHALL maintain completely independent state from parent
AND prevent sub-agents from spawning their own sub-agents
AND provide only the result submission tool plus standard tools

WHEN sub-agent conversation exists
THE SYSTEM SHALL track it as non-user-initiated
AND exclude it from normal conversation listings

**Rationale:** Users need isolation guarantees to prevent cascading failures and resource exhaustion.

---

### REQ-BED-010: Fixed Working Directory

WHEN conversation is created
THE SYSTEM SHALL assign a fixed working directory

WHEN tools execute
THE SYSTEM SHALL use the conversation's assigned working directory as the starting point

**Rationale:** Users benefit from simplified mental model where each conversation operates from a predictable location. Shell cd commands within tool execution follow normal semantics but do not persist across tool calls.

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

---

### REQ-BED-014: Conversation Mode

WHEN conversation is created
THE SYSTEM SHALL initialize in Restricted mode (if Landlock is available)

WHEN Landlock is unavailable (non-Linux OS or kernel < 5.13)
THE SYSTEM SHALL operate with only Unrestricted mode available
AND indicate Landlock unavailability to the user

WHEN conversation is in Restricted mode
THE SYSTEM SHALL enforce read-only semantics on all tools
AND execute bash commands under Landlock restrictions
AND block outbound network connections (no TCP connect/bind)
AND disable write-capable tools (patch)

WHEN conversation is in Unrestricted mode
THE SYSTEM SHALL allow full tool capabilities

**Rationale:** Users need a safe exploration mode for understanding codebases and triaging issues before committing to changes. Landlock provides kernel-level enforcement that cannot be bypassed by clever commands or prompt injection.

> **What is Landlock?** Landlock is a Linux Security Module (LSM) introduced in kernel 5.13 that enables unprivileged processes to restrict their own capabilities. Unlike traditional sandboxing (containers, VMs), Landlock runs in the same process and enforces allowlist-based restrictions on filesystem access (read-only everywhere, write only to specific paths) and network operations (block TCP connect/bind). It's defense-in-depth: even if an attacker achieves prompt injection and the model complies, the kernel blocks exfiltration and mutation.

---

### REQ-BED-015: Mode Upgrade Request

WHEN LLM needs write capabilities in Restricted mode
THE SYSTEM SHALL provide a `request_mode_upgrade` tool
WHICH accepts a reason string explaining why upgrade is needed

WHEN upgrade is requested
THE SYSTEM SHALL transition to AwaitingModeApproval state
AND notify user of the upgrade request with reason
AND pause agent execution until user responds

WHEN user approves upgrade
THE SYSTEM SHALL transition to Unrestricted mode
AND resume agent execution

WHEN user denies upgrade
THE SYSTEM SHALL remain in Restricted mode
AND return denial to agent via tool result
AND resume agent execution

WHEN user does not respond within reasonable time
THE SYSTEM SHALL remain paused (no automatic timeout to Unrestricted)

**Rationale:** Agents should be able to request capabilities when needed, with human approval as the gate. This enables "planning mode" → "implementation mode" workflow where agents explore and understand before making changes.

---

### REQ-BED-016: Mode Downgrade

WHEN user requests mode downgrade (Unrestricted → Restricted)
THE SYSTEM SHALL transition immediately to Restricted mode
AND NOT require agent approval

WHEN mode changes (either direction)
THE SYSTEM SHALL persist the new mode as part of conversation state

**Rationale:** Users can always tighten permissions. The asymmetry (user approval to upgrade, immediate downgrade) reflects the trust model: escalation requires consent, de-escalation does not.

---

### REQ-BED-017: Mode Communication

WHEN mode changes (upgrade or downgrade)
THE SYSTEM SHALL inject a synthetic system message visible to the agent
WHICH clearly states the new mode and its implications

WHEN agent is in Restricted mode
THE SYSTEM SHALL NOT modify tool descriptions based on mode

WHEN tool is unavailable due to mode restrictions
THE SYSTEM SHALL return clear, actionable error message
WHICH suggests using request_mode_upgrade if write access is needed

**Rationale:** Tool descriptions must remain static throughout conversation to avoid confusing the LLM. Mode awareness comes through synthetic messages (on transitions) and clear error responses (when tools are blocked).

---

### REQ-BED-018: Sub-Agent Mode Enforcement

WHEN sub-agent is spawned AND Landlock is available
THE SYSTEM SHALL always create sub-agent in Restricted mode
REGARDLESS of parent conversation's mode

WHEN sub-agent is spawned AND Landlock is unavailable
THE SYSTEM SHALL create sub-agent in Unrestricted mode (only option)

WHEN sub-agent is running
THE SYSTEM SHALL NOT provide request_mode_upgrade tool to sub-agents
AND sub-agents cannot change their mode

**Rationale:** Sub-agents are autonomous and less supervised than the parent conversation. Forcing Restricted mode (when available) limits blast radius. Only the parent conversation, with direct user oversight, can operate in Unrestricted mode.

---

### REQ-BED-019: Context Continuation Threshold

WHEN LLM response indicates context usage >= 90% of model's context window
AND conversation uses threshold-based continuation behavior
THE SYSTEM SHALL trigger continuation flow
AND NOT execute any tools requested in that response

WHEN calculating context usage
THE SYSTEM SHALL use total tokens from LLM response usage data
AND compare against model-specific context window size

**Rationale:** Users need graceful handling when conversations grow long. Triggering at 90% leaves room (~20k tokens on 200k models) for the continuation summary while avoiding hard failures. Rejecting tools at the threshold boundary prevents context overflow.

---

### REQ-BED-020: Continuation Summary Generation

WHEN continuation flow is triggered
THE SYSTEM SHALL request a session summary from the LLM
AND the request SHALL NOT include any tool capabilities
AND the request SHALL mention any tools that were requested but not executed

WHEN continuation summary is received
THE SYSTEM SHALL store it as a continuation message
AND transition to context exhausted state

WHEN continuation request fails after standard retries
THE SYSTEM SHALL transition to context exhausted state
AND use a fallback summary indicating the failure

**Rationale:** The summary preserves session context for users to seed a new conversation. Mentioning rejected tools acknowledges what the agent intended. Failures shouldn't block users from moving on.

---

### REQ-BED-021: Context Exhausted State

WHEN conversation enters context exhausted state
THE SYSTEM SHALL reject new user messages with explanatory error
AND display the continuation summary prominently
AND offer action to start new conversation

WHEN user starts new conversation from exhausted conversation
THE SYSTEM SHALL optionally pre-populate with continuation summary
AND preserve link to original conversation for reference

**Rationale:** Clear terminal state prevents confusion. Optional summary seeding enables continuity without forcing it.

---

### REQ-BED-022: Model-Specific Context Limits

WHEN determining context threshold
THE SYSTEM SHALL use the context window size for the conversation's model
AND support models with different limits

WHEN model context window is unknown
THE SYSTEM SHALL use the smallest known model limit as default

**Rationale:** Models have varying context capacities. Conservative defaults ensure safe behavior with unknown models.

---

### REQ-BED-023: Context Warning Indicator

WHEN context usage exceeds 80% of model's context window
THE SYSTEM SHALL display a warning indicator to the user
AND offer option to trigger continuation manually

WHEN user manually triggers continuation
THE SYSTEM SHALL behave identically to automatic continuation at threshold

**Rationale:** Users may want to wrap up conversations naturally before hitting the hard limit. Early warning with manual trigger gives control.

---

### REQ-BED-024: Sub-Agent Context Exhaustion

WHEN sub-agent context usage reaches threshold
THE SYSTEM SHALL fail the sub-agent immediately
AND NOT trigger continuation flow for sub-agents
AND report failure to parent conversation

WHEN parent receives sub-agent context exhaustion failure
THE SYSTEM SHALL allow parent to spawn replacement sub-agent with refined task

**Rationale:** Sub-agents are short-lived workers that shouldn't run long enough to exhaust context. If they do, failing fast lets the parent adapt rather than generating summaries nobody will read.
