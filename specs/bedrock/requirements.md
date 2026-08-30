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

### REQ-PROJ-003: Propose a Task for Blocking Review

WHILE a Git-backed conversation is in a planning/read-only phase
THE SYSTEM SHALL allow the agent to draft a markdown task file using the `patch` tool, scoped to the discovered tasks directory when task drafting rules require it

WHEN the agent calls the `propose_task` tool with a `task_file` path to a markdown file inside the conversation's allowed workspace
THE SYSTEM SHALL intercept it at the LlmResponse handler (like submit_result)
AND require it to be the only tool call in the response
AND NOT execute any immediate Git side effects
AND read the file and persist the assistant message and a synthetic tool result atomically
AND transition the conversation to AwaitingTaskApproval state
AND pause ordinary agent execution until the user responds

WHEN the task file's name parses as a taskmd filename but the path is **not** under the conversation repository's discovered tasks directory
THE SYSTEM SHALL reject the call

WHEN the task file's name parses as taskmd but its status is not `ready` / `in-progress` / `brainstorming`
THE SYSTEM SHALL reject the call

THE AwaitingTaskApproval state SHALL carry the `task_file` path plus a display copy of the title, priority, and body
AND SHALL persist one reviewed proposal snapshot identity containing the intended repo-relative path, task kind, and content digest bound to that reviewed body
AND, on approval, the executor SHALL re-read the file from disk only for that same reviewed path and task kind
AND SHALL compare the re-read body, path, and task kind against the reviewed snapshot identity before approving
AND SHALL require the user to request changes and capture a new reviewed snapshot before a changed path, changed task kind, or changed body can be approved

WHEN `propose_task` is called in a non-Git Direct conversation
THE SYSTEM SHALL NOT provide the tool at all

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call because task-management authority belongs to the parent conversation

**Rationale:** The task file is a real file the agent edits with `patch`, so revisions are normal file edits rather than plan text hidden in tool arguments. Blocking review preserves user approval without making task approval synonymous with lifecycle mode changes.

---

### REQ-PROJ-004: Review and Place an Approved Task

WHEN a conversation enters AwaitingTaskApproval state
THE SYSTEM SHALL open the prose reader with the plan content from the state
AND present **Continue here**, **Start in new conversation**, and **Request changes / discard** actions alongside the standard annotation feedback

WHEN the user sends annotation feedback
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND return the conversation to its prior Open planning state
AND the agent MAY revise the plan and call `propose_task` again

WHEN the user approves the task and chooses **Continue here**
THE SYSTEM SHALL commit the approved task artifact according to REQ-PROJ-006
AND SHALL approve only the same reviewed snapshot identity rather than whatever file currently exists at approval time
AND SHALL persist one typed approved-task objective that references the normalized approved-task source record
AND SHALL persist Git-backed write authority by referencing that approved-task objective on the same Open conversation and the same attached `WorkScope`
AND keep the same Open conversation and the same `WorkScope`
AND resume execution in that conversation without changing its mode
AND SHALL NOT create, rename, select, or delete a branch as an approval side effect

WHEN the user approves the task and chooses **Start in new conversation**
THE SYSTEM SHALL create a separate Open conversation derived from the source conversation
AND SHALL approve only the same reviewed snapshot identity rather than whatever file currently exists at approval time
AND provision a fresh detached-default-branch disposable worktree for the spawned conversation
AND SHALL treat that spawned conversation as an independent ProductConversation with its own fresh `WorkScope`, not as a sub-agent attachment to the source conversation's worktree
AND seed only the exact approved task as the spawned conversation's starting context
AND preserve the approved task artifact independently of the source worktree's eventual closure by storing one normalized approved-task source record and by materializing the approved artifact in the spawned worktree
AND SHALL complete that materialization before dispatching the spawned conversation's first LLM request
AND record exactly one source relation of kind `approved_task` on the spawned conversation that points to the source conversation
AND dispatch execution in the spawned conversation only after successful durable typed provisioning completion for that spawned ProductConversation, concrete spawned transcript conversation, attached `WorkScope`/worktree, and approved-artifact materialization
AND leave the source conversation Open and no longer blocked in task-approval state
AND SHALL NOT create, rename, select, or delete a branch as an approval side effect
AND SHALL NOT copy, summarize, or inject the source conversation transcript into the spawned conversation as part of approval placement
AND SHALL NOT dispatch the spawned conversation when provisioning finishes in an unresolved failure result

WHEN the user rejects or discards the task
THE SYSTEM SHALL return a rejection result to the agent
AND SHALL NOT perform any Git side effects

**Rationale:** Approval is a user decision about placement and authority, not a hidden transition into branch-backed lifecycle modes. The system offers two deliberate placements — continue in the same conversation or start in a new conversation — while keeping branch ownership out of the approval contract.

---

### REQ-PROJ-006: Task Files as Versioned Living Contracts

WHEN the agent drafts a task file for `propose_task`
THE SYSTEM SHALL place taskmd-named drafts in the conversation repository's discovered tasks directory: Phoenix scans immediate children of the repository root for taskmd sentinel files and prefers `tasks/`, otherwise the lexically-first discovered taskmd directory, otherwise literal `tasks/`
AND the filename SHALL follow the taskmd 1.0 convention `{ID}-{priority}-{status}--{slug}.md` when the repository uses taskmd naming
AND the **filename** SHALL remain the sole authoritative source of taskmd metadata
AND the body SHALL be free-form markdown

WHEN the user approves a taskmd task
THE SYSTEM SHALL parse the task ID, priority, status, and slug from the filename
AND rename the file to `...-in-progress--{slug}.md` if its status is not already `in-progress`
AND persist the approved content so the chosen conversation placement can continue to use the exact approved task
AND SHALL persist the exact approved body together with the reviewed snapshot identity that justified that approval

WHEN the agent later updates the task file during active work
THE SYSTEM SHALL allow edits to it like any other workspace file
AND the agent MAY rename it to `...-done--{slug}.md` or `...-wont-do--{slug}.md` when the work is complete

WHEN the task file is not taskmd-named
THE SYSTEM SHALL treat it as a plain task brief: the display title is the body's first `# H1` (falling back to a title-cased file stem), the display priority defaults to `p2`, and there is no structured id/status/slug contract

**Rationale:** The task artifact is the durable handoff, regardless of whether the user continues in the same conversation or starts a new one. taskmd metadata remains filename-based, but task approval no longer depends on creating or owning a dedicated Phoenix branch lifecycle.

---

### REQ-PROJ-007: Git-Backed Write Authority Is Scoped to the Conversation Worktree

WHILE a Git-backed conversation has write authority
THE SYSTEM SHALL configure tools to operate within the conversation's worktree directory
AND enable file-write tools within that worktree
AND allow bash commands that read and write files within that worktree

WHEN a tool with write authority attempts to write outside the worktree directory
THE SYSTEM SHALL block the write
AND return a descriptive error

**Rationale:** Write authority is scoped to the disposable worktree, not to the whole filesystem and not to a lifecycle mode name. This preserves isolation without requiring a separate writing lifecycle label as a product concept.

---

### REQ-PROJ-012: Provide propose_task Tool to Parent Conversations

WHEN a parent conversation is allowed to request task review or derived follow-up work
THE SYSTEM SHALL provide the `propose_task` tool
WHICH accepts `task_file` (required string): a path, relative to the agent's working directory, to an existing markdown (`.md`) file inside the allowed workspace

WHEN `propose_task` is called from a planning/read-only phase
THE SYSTEM SHALL treat it as the blocking review path defined by REQ-PROJ-003 and REQ-PROJ-004

WHEN `propose_task` is called from a Git-backed conversation that already has write authority
THE SYSTEM SHALL treat it as the same blocking review path used for planning/read-only conversations
AND SHALL NOT reinterpret that call as a nonblocking derived-conversation proposal

WHEN `propose_task` is called in a chat-only Direct conversation
THE SYSTEM SHALL NOT provide the tool, even if its working directory happens to be inside a Git repository

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call
AND explain that task-management authority belongs to the parent conversation

WHEN `propose_task` is not the only tool call in the response
THE SYSTEM SHALL reject it

`propose_task` is a pure data carrier intercepted at the LlmResponse handler. It never performs Git side effects directly.

**Rationale:** `propose_task` is the agent's way of saying “here's a task artifact for human review.” The key distinction is whether the parent is asking for blocking approval in place or proposing a separate derived conversation — not whether the product is in one named lifecycle mode or another.

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

WHEN a cancellation request is routed
THE SYSTEM SHALL use live runtime state when a runtime exists and SHALL use the persisted conversation state only when no live runtime owns transient state

WHEN user-requested cancellation is accepted while work is active but delivered after that work has already reached idle
THE SYSTEM SHALL treat the delivered request as an idempotent boundary no-op
AND SHALL NOT emit a user-facing internal error

WHEN cancellation completes
THE SYSTEM SHALL preserve all conversation history including synthetic results

**Rationale:** Users need the ability to interrupt long-running operations immediately, not after they complete. CPU-intensive tools or stuck processes must be killable. Synthetic tool results maintain message chain integrity required by LLM APIs.

---

### REQ-BED-005a: Bounded Cancellation Liveness

WHEN cancellation is requested during tool execution
THE SYSTEM SHALL abort the tool's executing task rather than relying solely on a cooperative interrupt token
AND SHALL reap any subprocesses spawned by that tool (process-group kill)

WHEN a tool observes its cancellation and returns within the cooperative window (REQ-BED-005, within 100ms)
THE SYSTEM SHALL record synthetic results and transition to idle by the cooperative path
AND SHALL NOT surface any warning or degraded-cancellation message

WHEN a tool neither observes its cancellation token nor returns within a bounded cancellation deadline
THE SYSTEM SHALL force the cancellation to completion without waiting for the tool task
AND SHALL record synthetic cancelled results for the current and remaining tools
AND SHALL transition the conversation to idle (parent) — the same terminal-direction state a cooperative cancel reaches
AND SHALL log a warning recording that the deadline backstop fired
AND SHALL inject a user-visible system message noting that cancellation completed and that aborted work may still be reclaiming resources in the background

THE SYSTEM SHALL bound the time from a cancellation request to a terminal-direction state, and this bound SHALL NOT depend on tool cooperation

WHEN forced teardown reaches idle via the deadline backstop
THE SYSTEM SHALL NOT transition to error state — a cancellation the user requested and that succeeded from their view must not be presented as a failure

**Rationale:** `CancellingTool` and `CancellingSubAgents` are otherwise states whose only exit is the running task returning; a tool that ignores its cancellation token and blocks would wedge the conversation in cancellation forever — a liveness hole. Cooperative interrupt (REQ-BED-005) is the happy path and is effectively immediate; the bounded deadline backstop covers only the pathological case where a tool never observes its token or never returns. Reaching idle (not error) preserves the user's mental model: they asked to cancel, cancellation happened. The OS child process is reaped regardless of task cooperation; the deadline backstop covers the in-process "task never returns" case. The user-visible message keeps the user oriented — work may still be releasing resources after the conversation has already returned to idle. This is the same liveness family as the sub-agent deadline in `specs/subagents/` REQ-SA-006: a wedged worker must not be able to hold its supervising conversation indefinitely.

---

### REQ-BED-006: Error Recovery

WHEN an in-flight conversation-turn LLM request fails with retryable error (network, rate limit, 5xx)
THE SYSTEM SHALL retry automatically up to 3 times with exponential backoff
AND remain in LLM requesting state during retries
AND display retry status to user

WHEN an in-flight conversation-turn LLM request fails after all retries are exhausted
THE SYSTEM SHALL transition to error state
AND display actionable error message indicating retry failure

WHEN a recoverable LLM-backed operation fails with an auth error while credential recovery is in progress
THE SYSTEM SHALL transition to awaiting recovery
AND SHALL carry a typed resume target for the suspended operation

WHEN credential recovery succeeds from awaiting recovery
THE SYSTEM SHALL resume the operation identified by the typed resume target
AND SHALL NOT infer the operation from display text, UI state, or the error message

WHEN credential recovery fails from awaiting recovery for a conversation-turn request
THE SYSTEM SHALL transition to error state
AND SHALL preserve the failure as turn error state rather than as continuation content

WHEN credential recovery fails from awaiting recovery for continuation-summary generation
THE SYSTEM SHALL transition to a user-retryable continuation failure state
AND SHALL preserve the continuation operation identity for an explicit retry
AND SHALL NOT fabricate continuation summaries or persist auth failure copy as continuation content

WHEN continuation-summary generation fails without a durable summary commit
THE SYSTEM SHALL transition to a user-retryable continuation failure state
AND SHALL preserve the continuation operation identity and retry context for an explicit retry

WHEN an in-flight conversation-turn LLM request fails with non-retryable error (4xx other than recoverable auth)
THE SYSTEM SHALL transition to error state immediately
AND display specific error message

WHEN user sends a message while in an error state whose typed policy is user-resumable
THE SYSTEM SHALL transition to awaiting LLM
AND attempt to continue the conversation

WHEN user sends a message while in an error state whose typed policy is not user-resumable
THE SYSTEM SHALL reject the message through the same typed transition policy used by the conversation runtime
AND SHALL NOT accept a chat request that the state machine will later discard

**Rationale:** Users should not lose their conversation due to transient failures. Clear error states with specific messages enable recovery.

---

### REQ-BED-007: State Persistence

WHEN conversation state changes
THE SYSTEM SHALL persist the new state before executing effects

WHEN server restarts
THE SYSTEM SHALL restore ordinary interrupted conversations to idle state
AND preserve complete message history

WHEN server restarts after an accepted steering batch has committed its user or skill messages and awaiting-LLM state but before the first response settles
THE SYSTEM SHALL preserve the awaiting-LLM state
AND resume exactly one LLM request from the committed transcript
AND SHALL settle a synchronous failure to start that request as a persisted typed error
AND if that error cannot be persisted, SHALL retire the executor so reconnect reconstructs from unchanged database truth
AND SHALL derive this bounded ownership from the immutable accepted steering identity, the latest transcript user or skill message, and absence of that exact pending queue entry
AND SHALL NOT introduce a second steering lifecycle or queue authority

WHEN server restarts with a conversation in `awaiting_continuation`, `recoverable_continuation_failure`, or continuation-summary `awaiting_recovery`
THE SYSTEM SHALL preserve the durable continuation operation identity and recovery state
AND materialize the pending continuation operation at startup

**Rationale:** Users expect their conversation history to survive server restarts. Ordinary interrupted turns resume from idle so users can re-send their last message. An already-accepted steering turn cannot safely be resent after its queue row has been atomically consumed, so its immutable acceptance and transcript evidence provide a narrow restart owner until the first response settles. Durable continuation operations retain their identity and explicit recovery path so restart cannot duplicate or strand compaction.

---

### REQ-BED-033: Unclassified Local Persistence Authority Fails Stop

WHEN the owning execution boundary determines under REQ-DWF-043 that the durable
fact needed to continue cannot be established
THE SYSTEM SHALL stop admission and semantic publication
AND SHALL NOT perform database-backed cleanup that depends on the suspect
persistence path
AND SHALL attempt only bounded best-effort shutdown work
AND SHALL terminate the process nonzero, aborting when that bound expires.

WHEN, while serving, a task that owns this local SQLite authority boundary panics,
exits unexpectedly, or is cancelled before delivering a typed result
THE SYSTEM SHALL apply the same fail-stop behavior unless the one exact authority
query against the owning authoritative rows establishes the durable fact needed
to continue.

A task termination covered by a typed coordinated-shutdown disposition SHALL NOT
select this fail-stop behavior.

Failure of a task that does not own this local SQLite authority boundary SHALL
remain governed by its feature's ordinary failure contract.

THE SYSTEM SHALL NOT represent inability to establish local persistence authority
as conversation or workflow semantic state.

WHEN the failed process has stopped and another Phoenix process opens the same
authoritative SQLite database
THE SYSTEM SHALL reconstruct conversation and workflow authority and admit work
only after that process successfully establishes the authoritative durable facts
required for admission
AND SHALL otherwise remain unavailable under this fail-stop requirement
AND SHALL NOT require continuity of process-local runtime, SSE, replay-buffer,
task, timer, or connection identity.

**Rationale:** If Phoenix cannot read SQLite well enough to establish the durable
facts needed to continue safely, it stops. Another process reconstructs from
SQLite rather than coordinating repair across independent in-memory owners. This
local authority rule does not replace feature-owned recovery contracts for genuine
ambiguous external outcomes.

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
primary isolation mechanism. The new model (REQ-BED-027) uses read-only planning versus write-capable execution authority
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
state are replaced by the `propose_task` tool and `AwaitingTaskApproval` state. The
new flow is richer: the agent proposes a full task plan rather than just a reason
string, and the user reviews via the prose reader with line-level annotation support.
The mode transition is now inseparable from task creation.

---

### REQ-BED-016: Mode Downgrade

**DEPRECATED:** Replaced by work-lifecycle REQ-WL-002 (mark merged) and REQ-WL-001 (abandon).

WHEN user requests mode downgrade (Unrestricted → Restricted)
THE SYSTEM SHALL transition immediately to Restricted mode
AND NOT require agent approval

WHEN mode changes (either direction)
THE SYSTEM SHALL persist the new mode as part of conversation state

**Deprecation Reason:** The downgrade concept (Unrestricted -> Restricted) is replaced
by task completion flows. A Work conversation transitions to Terminal state on task
completion (work-lifecycle REQ-WL-002) or abandonment (work-lifecycle REQ-WL-001). There is no standalone mode
downgrade; mode is always tied to worktree lifecycle.

---

### REQ-BED-017: Mode Communication

WHEN conversation execution authority changes after an approval or spawn decision
THE SYSTEM SHALL inject a synthetic system message visible to the agent
WHICH clearly states the new mode and its implications for tool availability

WHEN agent is in Explore mode
THE SYSTEM SHALL NOT modify tool descriptions based on mode

WHEN a tool is unavailable due to mode restrictions
THE SYSTEM SHALL return a clear, actionable error message
AND for write tools blocked in Explore mode, SHALL suggest using `propose_task` to
propose work that requires write access

**Rationale:** Tool descriptions must remain static throughout a conversation to avoid
confusing the LLM. Mode awareness comes through synthetic messages on transitions and
clear error responses when tools are blocked. Updated from REQ-BED-014/015 framing to
reflect read-only planning versus write-capable execution authority and `propose_task` as the path to write access.

---

### REQ-PROJ-018: Direct Mode

Direct mode is the chat-only / non-worktree conversation shape.

WHEN a conversation is created in Direct mode
THE SYSTEM SHALL provide full tool access (bash, patch, all tools)
AND set the working directory to the target directory (not a Phoenix-owned worktree)
AND SHALL NOT include `propose_task`
AND NOT create worktrees, branches, or task files for the Direct conversation itself

THE SYSTEM SHALL visually distinguish Direct mode from Git-backed worktree conversations in the UI

WHEN a Direct conversation targets a Git repository
THE SYSTEM MAY attach its `WorkScope` to the hidden `GitRepository` for discovery and configuration context
AND SHALL derive the ProductConversation's repository context through that `WorkScope`
AND SHALL NOT treat that association as ownership of a Phoenix worktree lifecycle

**Rationale:** Direct mode remains useful for chat-only and ad hoc workflows, while Git-backed conversations use the disposable-worktree model. The important distinction is worktree ownership, not a branching lifecycle picker.

---

### REQ-BED-018: Sub-Agent Mode Enforcement

WHEN sub-agent is spawned by an Explore conversation
THE SYSTEM SHALL always create the sub-agent in Explore mode
AND configure its working directory as the parent's main branch checkout

WHEN sub-agent is spawned by a Work conversation with Explore mode requested
THE SYSTEM SHALL create the sub-agent in Explore mode (read-only)
AND configure its working directory as the parent's worktree path

WHEN sub-agent is spawned by a write-capable parent conversation with write capability requested
THE SYSTEM SHALL create the sub-agent with write capability against the parent's attached `WorkScope`
AND configure its working directory as the parent's worktree path
AND enforce that only one Work sub-agent exists per parent at a time

WHEN sub-agent is running
THE SYSTEM SHALL NOT provide `propose_task` tool to sub-agents
AND sub-agents SHALL NOT be able to change their own mode

**Rationale:** Sub-agents operate under the parent's direction with a constrained
tool set. Explore sub-agents are safe to run in parallel — they cannot write.
Work sub-agents inherit the parent's worktree so they operate on the same codebase
state; the one-at-a-time constraint maintains a single writer per worktree.

---

### REQ-BED-018A: Persisted Prompt Projection Authority

WHEN THE SYSTEM prepares an LLM request for an ordinary conversation turn
THE SYSTEM SHALL derive provider-visible transcript history only from committed message rows and their committed attachment children
AND SHALL hydrate one transactionally consistent snapshot of transcript generation, ordered parent rows, and attachment children when the runtime first needs that history
AND subsequent requests owned by that runtime SHALL read only generation-fenced rows whose conversation-local sequence is greater than the projection cursor
AND SHALL hydrate attachment children with a constant number of set-based queries independent of transcript length
AND ordinary append persistence SHALL preserve strictly increasing conversation-local message sequence values

WHEN a prompt-visible row is changed in place, removed, adopted, or transferred between conversations
THE SYSTEM SHALL atomically advance the transcript generation of every affected source and destination conversation in the same transaction
AND the next provider request SHALL rebuild from committed authority rather than extend the invalid projection

IF authoritative transcript decoding, snapshot hydration, tail hydration, generation validation, or rebuild fails
THEN THE SYSTEM SHALL NOT dispatch an LLM request from partial, substituted, or process-local history
AND SHALL settle any entered requesting transition through its typed LLM error path

**Rationale:** Provider prompts are durable decisions. A bounded projection avoids repeated full-history reads without letting runtime caches become a second transcript authority, and generation fencing makes non-append mutation fail closed.

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
AND the request SHALL frame the summary as an operational handoff to a fresh agent that resumes in the same working directory with no memory of the session
AND the request SHALL describe any tools that were requested but not executed, including their intended arguments
AND the request SHALL preserve the prior tool history as text rather than discarding it
AND the request SHALL be bounded to fit the context window and any request-shape limits declared by the selected provider route
AND bounded history SHALL retain a contiguous newest suffix and begin with a user-role message when non-empty
AND the continuation request SHALL freeze the current member conversation's persisted prompt projection before provider work begins
AND SHALL compact that current member's history rather than flattening all ProductConversation transcript members into one aggregate prompt
AND the system SHALL persist a stable operation identity and the retry inputs before requesting the summary

IF continuation summary generation is interrupted by process restart
THEN THE SYSTEM SHALL resume the persisted operation

WHEN continuation summary is received for the active operation
THE SYSTEM SHALL atomically store one continuation message and transition to context exhausted state
AND duplicate or stale results SHALL NOT create another summary or overwrite newer state

WHEN continuation request fails after standard retries
THE SYSTEM SHALL retain the operation and its retry inputs in a recoverable state
AND SHALL display the failure and offer an explicit retry action

WHEN the continuation summary is empty or whitespace-only
THE SYSTEM SHALL treat it as a recoverable generation failure

WHEN user requests cancellation during continuation summary generation
THE SYSTEM SHALL reject the request as an invalid cancellation state
AND SHALL NOT abort the in-flight continuation request
AND SHALL remain awaiting the continuation summary

**Rationale:** The summary's consumer is a fresh agent that restarts cold in the same worktree, so it is framed as an operational handoff — exact paths, repo state, and an honest verified-vs-assumed split — rather than a human-facing recap, and completeness is favored over brevity. Describing rejected tool calls with their arguments tells the next agent what was about to run, not merely which tool type. The current member's prior tool history is rendered as text rather than deleted so the summary can draw on the actual work record, while generation fencing prevents later appends from leaking into an already-frozen request and bounded suffix selection prevents overflow. An empty summary would silently seed a blank continuation, so it is treated as a recoverable failure. Stable operation identity permits provider calls to be retried while summary commit and continuation remain exactly once.

---

### REQ-BED-021: Context Exhausted State

WHEN conversation enters context exhausted state
THE SYSTEM SHALL reject new user messages with explanatory error
AND display the continuation summary prominently
AND offer action to start new conversation

WHEN the user reviews a context-exhausted conversation with no successor
THE SYSTEM SHALL preserve the generated continuation summary as an immutable handoff
AND SHALL offer distinct actions to:
  - create a successor and submit the generated handoff unchanged as its first user message
  - edit a separate browser-local handoff draft before creating and starting the successor
  - copy the generated handoff without creating a successor
AND SHALL NOT present worktree cleanup or abandon controls on the handoff surface

WHEN the user edits a handoff
THE SYSTEM SHALL NOT modify the generated continuation summary
AND SHALL allow the local edit draft to be reverted to the generated handoff
AND SHALL reject an empty or whitespace-only edited handoff before successor creation

WHEN a continuation successor is created
THE SYSTEM SHALL atomically persist the exact selected handoff and client message identifier as a pending dispatch intent with the successor ownership transfer

WHEN a pending opening-handoff intent is retried after interruption or failed dispatch
THE SYSTEM SHALL dispatch the original persisted handoff with its original message identifier
AND SHALL return the successor identity and dispatch outcome
AND SHALL keep the user on the handoff surface when dispatch remains unsuccessful
AND SHALL NOT replace the original intent with later request text
AND SHALL NOT create a second manual-send path for the same handoff
AND SHALL NOT duplicate an opening message when an idempotent dispatch is retried
AND SHALL require Start in new conversation provisioning to derive any Git-backed starting pin under `specs/conversation-creation/` rather than from approval state alone
AND SHALL NOT let approval metadata publish a target conversation, target `WorkScope`, or target starting pin before creation publication commits atomically

WHEN the opening handoff is persisted as the successor's message or steering entry
THE SYSTEM SHALL atomically consume the pending dispatch intent
AND SHALL NOT retain the handoff payload in a second accepted representation

WHEN a request loses a continuation-creation race to an earlier handoff
THE SYSTEM SHALL report that the existing successor won
AND SHALL NOT claim that the losing request's handoff was accepted

**Rationale:** Context exhaustion is a focused transfer point. Explicit unchanged, edit-first, and copy paths make the consequence of each action visible, while keeping destructive workspace actions away from the continuation controls. Separating generated content from local edits ensures the operational handoff always remains recoverable. A successor creation and message dispatch cross a failure boundary, so the ownership transfer must durably record dispatch intent before asynchronous acceptance begins.

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

WHEN an idle conversation has recorded context usage
THE SYSTEM SHALL offer an option to trigger continuation manually regardless of warning threshold

WHEN user manually triggers continuation
THE SYSTEM SHALL behave identically to automatic continuation at threshold

**Rationale:** Users may want to wrap up conversations naturally before quality degrades or before hitting the hard limit. Always-available manual continuation gives control, while the warning threshold still draws attention when context is objectively high.

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

### REQ-BED-026: Sub-Agent Turn Limit and Timeout Enforcement

WHEN sub-agent is spawned
THE SYSTEM SHALL assign a mandatory time limit
AND assign a mandatory turn limit (maximum LLM turns before termination)

WHEN sub-agent exceeds its time limit without submitting a result
THE SYSTEM SHALL terminate the sub-agent
AND report timeout failure to parent conversation

WHEN sub-agent reaches its turn limit without submitting a result
THE SYSTEM SHALL grant one final "grace turn"
AND SHALL inject a user-role meta message visible to the LLM with mode-specific terminal guidance
AND SHALL state that only `submit_result` or `submit_error` can produce a useful terminal outcome from that grace turn
AND if the grace turn produces neither a submitted terminal action nor fresh admitted text-only implicit completion, terminate the sub-agent
AND report turn-limit failure to parent conversation

WHEN sub-agent timeout or turn limit fires
THE SYSTEM SHALL NOT wait for the sub-agent to finish its current operation

**Rationale:** Without enforced limits, a stuck or verbose sub-agent can hold the parent conversation indefinitely — either by wall-clock time (time limit) or by open-ended tool use (turn limit). Users need assurance that sub-agent work will complete or fail within bounded resources. The grace turn on turn-limit exit gives the sub-agent one last chance to synthesize a result from the work it already did, preserving useful output that would otherwise be discarded.

---

### REQ-BED-027: Conversation Execution Authority

WHEN a conversation is created for a Git-backed directory
THE SYSTEM SHALL initialize the conversation with read-only planning authority
until a later approval or creation path attaches write capability to a
`WorkScope`

WHEN a conversation is created for a non-git directory
THE SYSTEM SHALL initialize the conversation as chat-only with full local
write capability in its chosen working directory
AND SHALL NOT provide `propose_task`

WHILE a conversation has read-only planning authority
THE SYSTEM SHALL configure the tool registry with read-only settings
AND reject any state machine outcomes that would write source files in the
project

WHILE a conversation has write capability against an attached `WorkScope`
THE SYSTEM SHALL configure the tool registry with write access scoped to the worktree path
AND SHALL persist the attached `WorkScope` identity separately from volatile
state-machine execution state

WHILE a conversation is chat-only
THE SYSTEM SHALL configure the tool registry with full local write access
AND that chat-only authority SHALL NOT change into Git-backed lifecycle
ownership for the lifetime of the conversation
AND SHALL NOT provide `propose_task`, even if the working directory happens to be inside a git
  repository

WHEN conversation execution authority changes after an approval or spawn decision
THE SYSTEM SHALL persist the updated authority before resuming execution

**Rationale:** Execution authority is conversation-level identity — it persists across all
state machine transitions and survives server restarts. Keeping authority separate from
`ConvState` prevents combinatorial explosion of state variants and makes crash recovery
straightforward: the executor reads authority and state independently and configures the
tool registry accordingly. Chat-only conversations cover non-git directories without
inventing a parallel Git-backed lifecycle taxonomy.

**Dependencies:** REQ-PROJ-002, REQ-PROJ-007, REQ-PROJ-018

---

### REQ-BED-028: Task Approval State

WHEN the LLM response contains a `propose_task` tool call (which must be the only tool
  call in the response, and references a markdown file the agent already wrote inside the
  worktree — a taskmd file under the project's tasks directory, or any other `.md` accepted
  as a plain brief at its own path, classified by `TaskSource` per REQ-PROJ-006)
  WHILE the conversation has read-only planning authority
THE SYSTEM SHALL intercept it at the LlmResponse handler (same pattern as submit_result)
AND NOT route it through the tool executor
AND read the referenced file and persist the assistant message and a synthetic tool
  result as a CheckpointData::ToolRound
AND transition the conversation to AwaitingTaskApproval state (the blocking planning-to-approved-work checkpoint)

WHEN the same `propose_task` call occurs while the conversation already has write capability
  via an attached `WorkScope` in a Git-backed environment
THE SYSTEM SHALL treat it as the same blocking approval path rather than as a nonblocking derived-conversation review
AND SHALL intercept it at the LlmResponse handler
AND SHALL NOT route it through the tool executor
AND SHALL read the referenced file and persist the assistant message and a synthetic tool
  result as a CheckpointData::ToolRound
AND SHALL transition the conversation to AwaitingTaskApproval state
AND SHALL scope that behavior only to Git-backed write authority; chat-only execution remains ineligible for `propose_task`

THE AwaitingTaskApproval state SHALL carry: `task_file` (the path), plus a display copy
  of the title, priority, and body — all serializable; on approval the executor re-reads
  `task_file` from disk as the source of truth. (`task_file` carries `#[serde(default)]`
  as a rollout shim; a row with an empty `task_file` is surfaced as a "reject and
  re-propose" error rather than silently resetting to Idle.)

THE HandedOff state SHALL reject further user messages;
  it represents a read-only predecessor row whose live execution target is derived solely through
  `continued_in_conv_id`.

WHEN the user approves the task while in AwaitingTaskApproval with the
Continue here policy
THE SYSTEM SHALL record the approved task as the conversation's next objective through a typed approved-task objective value
AND SHALL promote the task file's status to `in-progress` if needed
AND SHALL commit only the approved task artifact if that task-tracking contract requires it
AND SHALL durably persist typed Git-backed write authority referencing that approved-task objective against the existing attached `WorkScope` before resuming execution
AND SHALL continue the same product conversation as the current live writable conversation
AND SHALL treat the approval as a checkpoint only
AND SHALL NOT create a fresh conversation row
AND SHALL NOT encode the approved task as `mode=work`-only conversation shape data
AND SHALL leave the conversation's mode unchanged, including when the pre-approval mode is Explore
AND SHALL NOT change `continued_in_conv_id`, `work_scope_id`, lifecycle, mode, repository state beyond the
  approved task commit, `WorkScope` attachment, source/provenance records beyond the approval itself,
  or branch/worktree provenance

WHEN the user approves the task while in AwaitingTaskApproval with the
Start in new conversation policy
THE SYSTEM SHALL perform the same task-approval artifact persistence
AND create a fresh Open conversation with a fresh attached `WorkScope`
  and worktree
AND record exactly one visible typed source relation of kind `approved_task` on the new conversation that points to the source conversation
AND SHALL treat that typed source relation as the sole current authority for visible provenance, deleted-source status, and source breadcrumb derivation for the spawned conversation
AND SHALL NOT treat any legacy raw source-conversation-id field as current writer or reader authority for approved-task provenance
AND keep the source conversation Open rather than linking it through `continued_in_conv_id`
AND SHALL exit the source conversation from AwaitingTaskApproval once the start-fresh approval is accepted
AND dispatch the next LLM request only in the spawned conversation
AND SHALL gate that dispatch on successful durable typed provisioning completion for the spawned ProductConversation and its attached `WorkScope`/worktree
AND SHALL materialize the exact approved artifact in the spawned worktree before that first dispatch
AND seed that conversation's first LLM-visible context from the approved task brief
  and approval metadata, excluding the source transcript
AND SHALL treat the spawned conversation as a separate Open conversation derived from the source conversation rather than as an in-place continuation of the predecessor's runtime environment
AND SHALL carry only the exact approved task as the derived starting context
AND SHALL NOT dispatch the spawned conversation when provisioning ends in an unresolved failure result

WHEN the user provides annotation feedback while in AwaitingTaskApproval
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND transition the conversation back to its ordinary read-only planning authority
  (the agent may revise the task file and call `propose_task` again, re-entering AwaitingTaskApproval)

WHEN the user discards the task while in AwaitingTaskApproval
THE SYSTEM SHALL transition the conversation to Idle in its prior read-only planning authority
AND NOT perform any repository lifecycle mutations (the task file stays on disk where the agent left it)

**Persistence and restart:**

WHEN the server persists AwaitingTaskApproval to the database
THE SYSTEM SHALL store `task_file` and the display copy of title/priority/body as part
  of the serialized ConvState

WHEN the server restarts and loads a conversation in AwaitingTaskApproval
THE SYSTEM SHALL reconstruct the state from the serialized data
AND the UI SHALL re-open the task-approval reader on reconnect from the conversation
  state payload

**Rationale:** AwaitingTaskApproval is a first-class state because it has a distinct
set of valid incoming events (approve, discard, feedback) and a distinct UI
representation (the task-approval reader). `propose_task` follows the submit_result
interception pattern — pure data carrier (its `run()` is an unreachable fallback), no
side effects, no tool execution. The plan is a real file the agent edits with `patch`,
so revisions are file edits; all git operations are deferred to the approval moment and
happen when the approved task artifact is committed; approval itself does not imply a task-branch lifecycle.

**Dependencies:** REQ-PROJ-003, REQ-PROJ-004

---

### REQ-BED-029: Close Conversation Finalizes Open Work into History

WHEN the latest transcript row of an ordinary Open ProductConversation is running work
THE SYSTEM SHALL expose the initial Close conversation action
AND SHALL create the durable Close attempt without stopping work immediately
AND SHALL require explicit stop-work confirmation before settlement begins

WHEN Close conversation completes for an ordinary Open ProductConversation with an attached `WorkScope`
THE SYSTEM SHALL transition that ProductConversation aggregate to History state
AND the latest transcript row in that aggregate SHALL NOT accept new user messages
AND SHALL durably persist one Close-outcome system message in the finalization transaction before the aggregate lifecycle announcement is emitted

WHEN a ProductConversation enters History after successful Close completion
THE SYSTEM SHALL emit one aggregate lifecycle announcement for downstream consumers
AND that ProductConversation SHALL remain visible in the sidebar for reference
AND the user SHALL be able to start a new ProductConversation on the same project

EACH Close operation SHALL carry one durable Close-attempt identity bound to that exact ProductConversation identity
AND THE SYSTEM SHALL permit at most one non-completed Close attempt for a given ProductConversation at a time
AND SHALL bind every Close phase transition, confirmation, cancellation, settlement, inspection, retirement, retry, and finalization event to that exact Close-attempt identity
AND SHALL retain completed Close attempts as historical records until permanent Delete removes their ProductConversation aggregate
AND SHALL create a new Close-attempt identity after cancellation only when no earlier Close attempt for that ProductConversation remains active
AND SHALL allocate every Close-attempt identity uniquely across active and historical attempts
AND SHALL retain a typed terminal outcome of `archived` or `cancelled` whenever an attempt becomes completed
AND SHALL retain the last bound inspection generation and fingerprint on a completed `archived` attempt
AND SHALL omit that aggregate inspection pair from a completed `cancelled` attempt while preserving any normalized exact-attempt inspection and loss evidence already recorded before cancellation
AND SHALL snapshot the exact ordered parent transcript-row continuation topology of that ProductConversation when the attempt is admitted
AND SHALL bind every snapshot member to both that exact Close attempt and the parent transcript row's explicit ProductConversation membership
AND SHALL treat subordinate execution conversations as aggregate participants rather than continuation-topology members
AND SHALL preserve that admitted snapshot unchanged when later topology changes are permitted

WHILE a non-completed Close attempt exists for a ProductConversation
THE SYSTEM SHALL NOT permit a continuation successor to be created for that ProductConversation

**Rationale:** Closing is the one product-facing way to retire active work. The
conversation moves to History because its owned live environment is gone; there is no
separate in-Phoenix merged-versus-abandoned lifecycle.

---

### REQ-BED-030: Context Continuation Inherits Parent Environment

WHEN user initiates continuation from a context-exhausted conversation
THE SYSTEM SHALL create a new transcript row that explicitly belongs to the same ProductConversation and inherits:
  - the parent's execution authority, preserving whether the latest live row is read-only planning, write-capable against an attached `WorkScope`, or chat-only
  - the parent's working directory
  - the parent's worktree, if any, without destroy-and-recreate
  - any uncommitted changes in that worktree
  - any parent task/proposal context that remains part of the same live conversation authority

WHEN the current execution row has a worktree
THE SYSTEM SHALL keep the same attached `WorkScope` atomically in a single database transaction while keeping the durable ProductConversation identity unchanged:
  - the parent retains its `worktree_path` field as a read-only reference for history navigation
  - the continuation's `worktree_path` is set to the same value
  - the continuation inherits the same `work_scope_id`
  - no `git worktree add` or `git worktree remove` command is executed (the filesystem state is unchanged)

WHEN a continuation successor row is newly created
THE SYSTEM SHALL atomically create that successor row, link it through `continued_in_conv_id`,
  persist the exact continuation boundary summary on the predecessor, and accept the user-selected
  handoff text as the successor row's first user message
AND SHALL use a client-generated message identifier for idempotent acceptance of that first user message
AND SHALL keep the exact persisted continuation boundary summary distinct from the successor's first user message

WHEN a parent conversation already has a continuation
THE SYSTEM SHALL present the Continue action as a navigation link to the existing continuation
AND NOT permit creation of a second continuation from the same parent

**Rationale:** When a user hits context exhaustion mid-task, the most common
next action is to keep working on the same task with a fresh context window.
Preserving the full environment — execution authority, branch state, worktree, uncommitted changes —
matches that intent directly and eliminates the need for `git stash`/restore
ceremony or a separate auto-stash feature. Keeping the same attached `WorkScope`
and filesystem path across continuation, rather than destroying and recreating,
is the only shape that preserves uncommitted work structurally without a separate
auto-stash mechanism. Single-continuation policy keeps execution topology
unambiguous: at any moment, continuation identifies the latest execution row for
one product conversation while that conversation keeps one attached `WorkScope`
and therefore one inherited worktree.

**Dependencies:** REQ-BED-021, REQ-PROJ-025, work-lifecycle REQ-WL-001/REQ-WL-002, REQ-PROJ-028

---

### REQ-BED-030A: ProductConversation Owns Aggregate Identity and Lifecycle

EACH ProductConversation SHALL have a typed durable identity whose domain is independent of every transcript-row identity
AND THE SYSTEM SHALL allocate that identity without deriving it from a root or latest transcript-row identity during runtime creation
AND SHALL NOT substitute a ProductConversation identity for a transcript-row identity, or the reverse, based on equal underlying bytes
AND SHALL require every durable conversation row to explicitly belong to exactly one ProductConversation
AND SHALL distinguish parent transcript rows from subordinate execution participants within that aggregate
AND SHALL derive root and latest parent transcript rows from the continuation topology within the ProductConversation rather than storing either as a second mutable aggregate authority
AND SHALL keep provider prompt projection authority scoped to each current parent transcript row rather than deriving one flattened aggregate transcript

WHEN a parent transcript row has a continuation successor
THE SYSTEM SHALL require that successor to be a parent transcript row in the same ProductConversation
AND SHALL permit at most one predecessor and one successor for each parent transcript row
AND SHALL reject cyclic continuation topology

EACH ProductConversation SHALL have exactly one typed kind: ordinary or coordinator
AND THE SYSTEM SHALL model Open/History lifecycle for ordinary ProductConversations only
AND SHALL make Open/History lifecycle structurally inapplicable to coordinator ProductConversations rather than representing coordinator lifecycle as an optionally absent ordinary value
AND SHALL expose ProductConversation lifecycle through one writable authority rather than parallel writable aggregate and transcript-row values

**Rationale:** Aggregate identity, transcript membership, topology, and lifecycle are distinct facts. Keeping one authority for each prevents a transcript segment from becoming a second ProductConversation or lifecycle owner.

---

### REQ-BED-031: Exhausted Parent Post-Handoff Behavior

WHILE a conversation is in context-exhausted state
THE SYSTEM SHALL preserve the conversation record for history navigation
AND preserve its worktree, if any, across server restarts
AND preserve its branch in git

WHEN a context-exhausted conversation has no continuation
THE SYSTEM SHALL permit user-initiated Close on that latest row of the same ordinary Open ProductConversation
AND SHALL apply the same close contract and worktree/resource disposition as Close from a non-terminal state
AND SHALL route any direct-turn-owned runtime work, wake deliveries, tool execution, sub-agent execution, and other child work through the durable workflow profile's typed effects and reconciliation machinery rather than through parent-row callbacks that mutate conversation state directly

WHEN a context-exhausted conversation has an existing continuation
THE SYSTEM SHALL NOT permit Close conversation on the predecessor row
(the continuation is the latest execution row — any close decision belongs on that latest row)

WHEN the server restarts and encounters a context-exhausted conversation with a worktree
THE SYSTEM SHALL preserve the worktree unchanged
AND NOT demote the conversation's execution authority
AND NOT remove it from the worktree registry

**Rationale:** Context exhaustion is a pause, not an end. The user may
return hours or days later to continue the work, and a surprise cleanup
on server restart would be data loss. Close conversation must stay available on the
parent for the case where the user decides the work isn't worth
continuing — without it, the only cleanup path is to create an unwanted
continuation and then abandon that, which is clunky and produces a
stranded continuation record. When a continuation exists, the live
conversation is the continuation; operating on the parent would be
ambiguous about which conversation the action affects. Parent lifecycle
settlement uses typed durable profile boundaries rather than ad hoc callbacks
that mutate the parent row directly after child completion; wake, child, tool,
sub-agent, and other direct-turn-owned effects reconcile through those
boundaries so restart/replay and Close settlement share one authority.

**Dependencies:** REQ-BED-021, REQ-BED-030, REQ-PROJ-015, work-lifecycle REQ-WL-001/REQ-WL-002

---

### REQ-BED-031A: Start Follow-up Creates a Fresh Open Conversation from History

WHEN the user starts a follow-up from a History ProductConversation
THE SYSTEM SHALL create a separate Open ProductConversation rather than reopening or continuing the historical one
AND SHALL attach a fresh `WorkScope`
AND SHALL provision a fresh detached-default-branch disposable worktree when the follow-up is Git-backed
AND SHALL treat that follow-up as an independent ProductConversation rather than as a sub-agent or continuation attachment to the source `WorkScope`
AND SHALL set the new conversation's user objective from the follow-up request rather than from the historical transcript
AND SHALL NOT inject, copy, or summarize the source transcript into the new conversation's starting transcript
AND SHALL record exactly one visible typed source relation of kind `follow_up` on the new conversation that points to the source conversation
AND SHALL derive any visible source breadcrumb or deleted-source state for that follow-up from the typed source relation rather than from continuation topology or any raw source-conversation-id field
AND SHALL render that Follow-up relationship visibly from the source conversation

**Rationale:** A follow-up from History is new work that benefits from a clean objective and a clean environment, while retaining explicit navigable provenance back to the source conversation.

---

### REQ-BED-031B: Permanent Delete Removes Only the Conversation Aggregate and Is Idempotent

WHEN the user permanently deletes a History conversation
THE SYSTEM SHALL remove the complete ProductConversation aggregate that is solely owned by that History conversation, including every transcript row in that aggregate plus every solely-owned normalized child row required by that aggregate
AND SHALL explicitly delete solely-owned Close-obligation, Close-attempt-member, inspection, inventory, loss, retirement-evidence, lineage-Q&A, message, tool-call, approval, message/file-attachment, retrieval-chunk, retrieval-locator, and FTS rows for that aggregate rather than relying on an unspecified catch-all such as "messages deleted"
AND SHALL preserve typed tombstone-grade source identity needed by surviving provenance consumers
AND SHALL NOT cascade the deletion into related but separately-owned conversations, branches, or pull requests

WHEN source relations on surviving conversations still point to the deleted conversation
THE SYSTEM SHALL preserve tombstone-grade source root identity and deleted status sufficient for later UI and retrieval surfaces to distinguish **Deleted source** from an absent or never-recorded source

WHEN conversation content has been removed by permanent Delete
THE SYSTEM SHALL remove the corresponding FTS/index rows in the same delete path or a deterministic reconciliation path before deleted content can surface again

WHEN the same permanent Delete request is retried after the aggregate has already been removed
THE SYSTEM SHALL treat the operation as idempotent success rather than recreating work, failing because rows are already gone, or cascading into unrelated records

**Rationale:** Permanent Delete is about one conversation aggregate and its solely-owned data, not about erasing every related object in the repository or navigation graph. Tombstone-grade source identity preserves truthful surviving provenance.

---

### REQ-PROJ-033: Propose a Derived Task from an Already-Active Conversation

WHILE a Git-backed conversation already has write authority
THE SYSTEM SHALL provide `propose_task` as the same blocking approval tool used in planning/read-only conversations

WHEN the agent calls `propose_task` with a markdown file inside its allowed workspace
THE SYSTEM SHALL intercept it at the LlmResponse handler
AND require it to be the only tool call in the response
AND validate the file by the same taskmd/plain-brief rules as REQ-PROJ-003
AND read and persist the proposal as the blocking approval artifact defined by REQ-PROJ-003/004
AND transition the conversation to AwaitingTaskApproval rather than leaving it running

WHEN `propose_task` is called from a planning/read-only phase
THE SYSTEM SHALL keep the same blocking review behavior

WHEN `propose_task` is called in Direct mode
THE SYSTEM SHALL NOT provide the tool

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call

**Rationale:** Approved product behavior uses one blocking human-review checkpoint for all Git-backed `propose_task` calls. Write authority changes what approval can grant or preserve; it does not create a second nonblocking proposal meaning.

---

### REQ-PROJ-036: propose_task Availability by Conversation Capability

THE `propose_task` tool SHALL be available as follows:

| Conversation capability shape | `propose_task` behavior |
|-------------------------------|-------------------------|
| Git-backed planning/read-only conversation | Blocking review path (REQ-PROJ-003 / REQ-PROJ-004) |
| Git-backed conversation with write authority | Blocking review path (REQ-PROJ-003 / REQ-PROJ-004) |
| Any Direct conversation | Not provided |
| Any sub-agent | Not provided |

**Rationale:** Availability depends on whether the host conversation itself is Git-backed and therefore eligible for the one blocking approval flow. Chat-only/direct conversations and sub-agents remain excluded, even if a chat-only working directory happens to sit inside a repository.

---

### REQ-BED-032: Conversation Terminal-Transition Cascade


Permanent Delete is the only terminal transition this requirement owns directly. Close-to-History resource retirement is specified by REQ-BED-029 plus `specs/work-lifecycle/requirements.md`; it is not a parallel hard-delete path.

WHEN a permanent Delete request targets a History conversation
THE SYSTEM SHALL run the hard-delete resource-cleanup cascade, which performs the
following sequence of direct calls in order:
1. Reject if the conversation is still busy or otherwise not yet eligible for permanent Delete
2. `cascade_bash_on_delete(conversation)` — kills live bash handles for
   the conversation and drops in-memory tombstones (REQ-BASH-006)
3. `cascade_tmux_on_delete(work_scope, inheritor_scope)` — runs
   `tmux kill-server` against the scope's socket, unlinks the socket
   file, removes the registry entry, unless another still-live owner retains
   the same `WorkScope` (REQ-TMUX-007, REQ-TMUX-WS-002)
4. `cascade_projects_on_delete(conversation, inheritor_scope)` —
   worktree cleanup only when no other live conversation still has the same
   attached `WorkScope`; hard delete does not create, move, or delete branches
   as part of cleanup (REQ-PROJ-015)
5. `cascade_browser_on_delete(work_scope, inheritor_scope)` — drops the
   Chrome session for the scope unless another still-live owner retains the
   same `WorkScope` (REQ-BROWSER-WS-003)
6. `db.delete_product_conversation(product_conversation_id)` — permanent Delete removes the complete History ProductConversation aggregate, including every transcript row plus solely-owned Close-obligation, Close-attempt-member, inspection, inventory, loss, retirement-evidence, lineage-Q&A, message, tool-call, approval, message/file-attachment, retrieval-chunk, retrieval-locator, and FTS rows, while retaining typed tombstones for surviving provenance links and leaving related but
   separately-owned conversations, branches, and pull requests untouched
7. Broadcast `ConversationHardDeleted` for UI consumers

THE handler SHALL invoke each cascade step on its own; there is no
event bus, no subscriber registration, and no dynamic dispatch. The
"subscribers first, row last" ordering is a property of the call sequence
in the handler. Each cascade function reads whatever conversation state
it needs before the row delete in step 6.

WHEN a cascade step fails (subprocess error, file system error,
registry-state inconsistency)
THE SYSTEM SHALL log the failure at WARN level with structured fields
sufficient for an operator to manually clean up — at minimum the
process group ids of any surviving processes and the absolute paths
of any surviving sockets or worktrees
AND continue to the next cascade step — failing steps SHALL NOT block
the conversation row deletion
AND THE SYSTEM SHALL NOT attempt automatic recovery on a subsequent
Phoenix startup. Orphans created by failed cleanup are the operator's
problem; reconciliation machinery for hard-delete orphans is
intentionally out of scope here.

THE SYSTEM SHALL distinguish permanent Delete from UI-only state changes (close-tab, blur, parent window closed — which
do NOT run the cascade). The latter category exists purely to support
"close the tab, come back later" — for those, long-lived per-scope
resources (tmux servers, browser sessions) survive (REQ-TMUX-008
makes this explicit on the tmux side; REQ-BROWSER-WS-003 on the
browser side).

History after Close is NOT reversible in place. Close moves the conversation to History through the separate Close lifecycle; permanent Delete later removes that retained aggregate entirely. There is
no `unarchive` operation because Archive is not a normative lifecycle concept.

THE `ConversationHardDeleted` SSE wire event (step 7) SHALL be emitted
exactly once per hard-delete operation, after all cascade steps have
completed. UI consumers use it to refresh sidebar, navigation, and
related views. It is NOT a subscriber-dispatch hook for cleanup logic;
cleanup runs synchronously inside the handler.

**Rationale:** Bash handles live in-process; tmux servers and project
worktrees live outside Phoenix. Without an explicit hard-delete cascade, deleting a
conversation aggregate leaves these resources orphaned: the OS-visible processes
keep running, socket files accumulate in `~/.phoenix-ide/tmux-sockets/`,
worktrees stay on disk. A direct-call orchestrator in the hard-delete
handler is the simplest shape that satisfies the cleanup contract — a
lifecycle-event/subscriber-bus pattern was considered and rejected as
overengineering for three known callsites in one binary, with no third-
party subscribers and no plugin system planned. Spec elegance traded
for an event-bus abstraction Phoenix doesn't otherwise have.

The not-attempting-recovery decision is deliberate. Cascade failures
should be rare on healthy systems (kill-tree is well-behaved when the
kernel is responsive; `tmux kill-server` rarely hangs). When they do
occur, the failure cause (frozen mount, hung tmux server, file system
error) is also what would prevent automatic recovery from succeeding —
adding a startup orphan-walker would carry complexity without
materially improving reliability. Operators who do encounter orphans
have the WARN-level structured logs to act on.

The not-busy precondition mirrors the Close confirmation contract in
`specs/work-lifecycle/`. Hard-delete during a live tool execution would race
the tool's own cleanup code; canceling first is the deterministic order.
The permissive choice between (a) reject and (b) cancel-first leaves
that policy to the API layer; the bedrock contract is the not-busy
postcondition before cascade fires.

**Dependencies:** REQ-BASH-006 (`specs/bash/`), REQ-TMUX-007
(`specs/tmux-integration/`), REQ-PROJ-WS-001
(`specs/work-lifecycle/`), and REQ-GITREP-007 (`specs/git-repository/`).
