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
AND assign a time limit to each sub-agent

WHEN sub-agent completes its task
THE SYSTEM SHALL require it to call a dedicated result submission tool
AND capture the submitted result

WHEN all sub-agents have submitted results
THE SYSTEM SHALL aggregate results
AND return them to parent conversation

WHEN any sub-agent fails or times out without submitting
THE SYSTEM SHALL include failure information in aggregated results
AND allow parent to handle the failure

**Rationale:** Users benefit from parallel task execution for bootstrapping and complex operations. Explicit result submission provides clean completion semantics. Time limits prevent indefinite resource consumption by stuck sub-agents.

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

**Rationale:** Users need visibility into context usage to manage long conversations effectively.

> **Note:** User notification at approaching limits is handled by REQ-BED-023 (Context Warning Indicator).

---

### REQ-BED-014: Conversation Mode

**DEPRECATED:** Replaced by REQ-BED-027.

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

**Deprecation Reason:** The Restricted/Unrestricted framing placed Landlock as the
primary isolation mechanism. The new model (REQ-BED-027) uses Explore/Work modes
where git worktrees provide primary physical isolation on all platforms and Landlock
becomes defense-in-depth for Explore mode read-only enforcement only.

---

### REQ-BED-015: Mode Upgrade Request

**DEPRECATED:** Replaced by REQ-PROJ-003, REQ-PROJ-004, and REQ-BED-028.

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

**Deprecation Reason:** The `request_mode_upgrade` tool and `AwaitingModeApproval`
state are replaced by the `propose_plan` tool and `AwaitingTaskApproval` state. The
new flow is richer: the agent proposes a full task plan rather than just a reason
string, and the user reviews via the prose reader with line-level annotation support.
The mode transition is now inseparable from task creation.

---

### REQ-BED-016: Mode Downgrade

**DEPRECATED:** Replaced by REQ-PROJ-009 (merge) and REQ-PROJ-010 (abandon).

WHEN user requests mode downgrade (Unrestricted → Restricted)
THE SYSTEM SHALL transition immediately to Restricted mode
AND NOT require agent approval

WHEN mode changes (either direction)
THE SYSTEM SHALL persist the new mode as part of conversation state

**Deprecation Reason:** The downgrade concept (Unrestricted -> Restricted) is replaced
by task completion flows. A Work conversation transitions to Terminal state on task
completion (REQ-PROJ-009) or abandonment (REQ-PROJ-010). There is no standalone mode
downgrade; mode is always tied to worktree lifecycle.

---

### REQ-BED-017: Mode Communication

WHEN conversation mode changes (Explore to Work on task approval)
THE SYSTEM SHALL inject a synthetic system message visible to the agent
WHICH clearly states the new mode and its implications for tool availability

WHEN agent is in Explore mode
THE SYSTEM SHALL NOT modify tool descriptions based on mode

WHEN a tool is unavailable due to mode restrictions
THE SYSTEM SHALL return a clear, actionable error message
AND for write tools blocked in Explore mode, SHALL suggest using `propose_plan` to
propose work that requires write access

**Rationale:** Tool descriptions must remain static throughout a conversation to avoid
confusing the LLM. Mode awareness comes through synthetic messages on transitions and
clear error responses when tools are blocked. Updated from REQ-BED-014/015 framing to
reflect Explore/Work mode names and `propose_plan` as the path to write access.

---

### REQ-BED-018: Sub-Agent Mode Enforcement

WHEN sub-agent is spawned by an Explore conversation
THE SYSTEM SHALL always create the sub-agent in Explore mode
AND configure its working directory as the parent's main branch checkout

WHEN sub-agent is spawned by a Work conversation with Explore mode requested
THE SYSTEM SHALL create the sub-agent in Explore mode (read-only)
AND configure its working directory as the parent's worktree path

WHEN sub-agent is spawned by a Work conversation with Work mode requested
THE SYSTEM SHALL create the sub-agent in Work mode (read-write)
AND configure its working directory as the parent's worktree path
AND enforce that only one Work sub-agent exists per parent at a time

WHEN sub-agent is running
THE SYSTEM SHALL NOT provide `propose_plan` tool to sub-agents
AND sub-agents SHALL NOT be able to change their own mode

**Rationale:** Sub-agents operate under the parent's direction with a constrained
tool set. Explore sub-agents are safe to run in parallel — they cannot write.
Work sub-agents inherit the parent's worktree so they operate on the same codebase
state; the one-at-a-time constraint maintains a single writer per worktree.

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
AND report failure to parent conversation as "context exhausted before result submission"

**Rationale:** Sub-agents are short-lived workers that shouldn't run long enough to exhaust context. If they do, failing fast surfaces the failure to the parent agent which can naturally decide how to proceed (retry with refined task, work around it, etc.).

---

### REQ-BED-025: Token-by-Token LLM Output

WHEN LLM is generating a text response to a user message
THE SYSTEM SHALL display the response text to the user progressively as it is generated
AND NOT wait for the full response before showing any text

WHEN LLM generates a response that contains only tool invocations and no prose text
THE SYSTEM SHALL NOT display streaming text
AND SHALL continue to indicate work is in progress via the existing activity indicator

WHEN text is actively streaming to the user
THE SYSTEM SHALL update the displayed content frequently enough that the user perceives continuous output

WHEN streaming stops due to completion or error
THE SYSTEM SHALL immediately reflect the stopped state

**Rationale:** Long responses on large conversation contexts can take many seconds to generate. Without progressive display, users cannot distinguish active generation from a silent hang or network failure. Seeing words appear confirms the system is working and allows the user to begin reading early.

---

### REQ-BED-026: Sub-Agent Timeout Enforcement

WHEN sub-agent is spawned
THE SYSTEM SHALL assign a mandatory time limit

WHEN sub-agent exceeds its time limit without submitting a result
THE SYSTEM SHALL terminate the sub-agent
AND report timeout failure to parent conversation

WHEN sub-agent timeout fires
THE SYSTEM SHALL NOT wait for the sub-agent to finish its current operation

**Rationale:** Without enforced time limits, a stuck or slow sub-agent can hold the parent conversation indefinitely. Users need assurance that sub-agent work will complete or fail within a bounded time.

---

### REQ-BED-027: Explore, Work, and Standalone Conversation Modes

WHEN a conversation is created for a project (git-backed directory)
THE SYSTEM SHALL initialize the conversation in Explore mode
AND store the mode as a field on the conversation record (not inside state machine state)

WHEN a conversation is created for a non-git directory
THE SYSTEM SHALL initialize the conversation in Standalone mode

WHILE a conversation is in Explore mode
THE SYSTEM SHALL configure the tool registry with read-only settings
AND reject any state machine outcomes that would write files to the project

WHILE a conversation is in Work mode
THE SYSTEM SHALL configure the tool registry with write access scoped to the worktree path
AND record the worktree path and associated task ID in the mode field

WHILE a conversation is in Standalone mode
THE SYSTEM SHALL configure the tool registry with full write access (equivalent to Work)
AND SHALL NOT provide `propose_plan` tool
AND the mode SHALL NOT change for the lifetime of the conversation

WHEN conversation mode changes (Explore to Work on task approval)
THE SYSTEM SHALL persist the updated mode before resuming execution

**Rationale:** Mode is conversation-level identity — it persists across all state machine
transitions and survives server restarts. Keeping it as a separate field (not embedded
in every ConvState variant) prevents combinatorial explosion of state variants and
makes crash recovery straightforward: the executor reads mode and state independently
and configures the tool registry accordingly. Standalone mode exists for directories
without git — it provides full tool access but without the safety features (worktrees,
task tracking, branch isolation) that require a git repository.

**Dependencies:** REQ-PROJ-002, REQ-PROJ-007, REQ-PROJ-016

---

### REQ-BED-028: Task Approval State

WHEN the LLM response contains a `propose_plan` tool call
THE SYSTEM SHALL intercept it at the LlmResponse handler (same pattern as submit_result)
AND NOT route it through the tool executor
AND persist the assistant message and a synthetic tool result as a CheckpointData::ToolRound
AND transition the conversation to AwaitingTaskApproval state
AND emit a `task_approval_requested` SSE event with the plan content

THE AwaitingTaskApproval state SHALL carry: title, priority, and plan text
  (all serializable data — no file paths, no oneshot channels, no git references)

WHEN the user approves the task while in AwaitingTaskApproval
THE SYSTEM SHALL write a task file to `tasks/` and commit to main
AND create branch `task-{NNNN}-{slug}` from main HEAD and checkout it
AND transition the conversation to Idle in Work mode

WHEN the user provides annotation feedback while in AwaitingTaskApproval
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND transition the conversation to Idle in Explore mode
  (the agent may revise and call `propose_plan` again, re-entering AwaitingTaskApproval)

WHEN the user discards the task while in AwaitingTaskApproval
THE SYSTEM SHALL transition the conversation to Idle in Explore mode
AND NOT perform any git operations (nothing was written to disk)

**Persistence and restart:**

WHEN the server persists AwaitingTaskApproval to the database
THE SYSTEM SHALL store the title, priority, and plan text as part of the serialized ConvState

WHEN the server restarts and loads a conversation in AwaitingTaskApproval
THE SYSTEM SHALL reconstruct the state from the serialized data (all data is in the DB)
AND re-emit the `task_approval_requested` SSE event when the UI reconnects

**Rationale:** AwaitingTaskApproval is a first-class state because it has a distinct
set of valid incoming events (approve, discard, feedback) and a distinct UI
representation (prose reader with plan content). `propose_plan` follows the
submit_result interception pattern — pure data carrier, no side effects, no tool
execution. All git operations are deferred to the approval moment.

**Dependencies:** REQ-PROJ-003, REQ-PROJ-004

---

### REQ-BED-029: Conversation Terminal State on Task Resolution

WHEN a Work conversation's task is completed (squash merged to base_branch)
THE SYSTEM SHALL transition the conversation to Terminal state
AND the conversation SHALL NOT accept new user messages

WHEN a Work conversation's task is abandoned
THE SYSTEM SHALL transition the conversation to Terminal state
AND the conversation SHALL NOT accept new user messages

WHEN a conversation enters Terminal state after task resolution
THE SYSTEM SHALL inject a synthetic system message indicating the outcome
  (completed with commit hash, or abandoned)
AND the conversation SHALL remain visible in the sidebar for reference
AND the user SHALL be able to start a new Explore conversation on the same project

**Rationale:** Work conversations are single-purpose: one task, one worktree, one
lifecycle. When the task concludes (successfully or not), the conversation is done.
Returning to Explore mode would create confusion about what the conversation's
context represents (the old worktree is gone, the pinned commit is arbitrary).
Terminal state is clean and explicit. The user creates a new Explore conversation
to continue working on the project.
