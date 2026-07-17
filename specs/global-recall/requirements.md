# Phoenix Coordinator

## User Story

As a Phoenix user, I often have several unrelated streams of work active across projects, continuation chains, and standalone conversations. I want one durable Phoenix-wide conversation where I can survey that work, inspect relevant history, and send useful text guidance to existing conversations without opening and operating each one manually.

The Coordinator is an open-ended cross-conversation console, not ambient memory for ordinary coding conversations and not a manager for one global objective. It receives deterministic current-work orientation from Phoenix, selectively reads source conversations, and may communicate through the same message acceptance path used by the ordinary chat composer.

## Why the User Cares

- **Orientation should be deterministic.** The user should not spend model tokens or trust an inference step just to discover current work.
- **Long-running work should not fragment identity.** A continuation chain represents one work item even though it spans multiple conversations.
- **Intervention should be narrow and trustworthy.** The Coordinator may send text to existing conversations, while the receiving conversation's authoritative state determines whether the message starts immediately, becomes steering, or is rejected.
- **Committed actions should be transparent.** The Coordinator reports acceptance per target without implying that another agent understood, acknowledged, or completed the instruction.
- **Global access should be deliberate and bounded.** Ordinary conversations remain scoped; Phoenix-wide reads and cross-conversation messaging are reserved for the Coordinator.

## Transparency Contract

The user must be able to answer:

1. Which project owns an open work item?
2. Which conversation is the current source of truth for the item?
3. Why does the item's current state need attention or qualify as open work?
4. Which source conversations or messages support a Coordinator claim about history?
5. Which durable conversation received each attempted message?
6. Was each attempted message delivered, queued as steering, or rejected, and why?
7. Is a displayed projection current and potentially truncated rather than an event history or exact change set?

## Requirements

### REQ-GR-001: Present Deterministic Current Work

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL present a deterministic projection of current Phoenix work without requiring an LLM request

THE surface SHALL prioritize the normal Coordinator conversation and SHALL provide a smaller utility for current attention and deterministic work finding

THE projection SHALL describe current state and SHALL NOT represent an event history, unread inbox, acknowledgement ledger, resolution ledger, or exact delta from a prior observation

---

### REQ-GR-002: Collapse Continuation Chains Into One Work Item

WHEN conversations form a linear continuation chain
THE SYSTEM SHALL represent the chain as one open-work item identified by its chain root

THE SYSTEM SHALL expose the current/latest conversation for the chain

THE SYSTEM SHALL NOT show non-leaf chain members as separate open-work items when they are already represented by the chain item

WHEN historical chain members are archived
THE SYSTEM SHALL preserve the original chain root as the work-item identity if the latest conversation still has positive open-work evidence

---

### REQ-GR-003: Explain Current Inclusion and Attention

WHEN an open-work item is visible
THE SYSTEM SHALL expose deterministic signals that explain its inclusion and current attention priority

THE signals SHALL cover recent activity, work mode, active or attention-needing runtime state, task status when available, and error or recovery state when applicable

THE SYSTEM SHALL include a Work-mode item only when its task status is `in-progress`, `ready`, or `blocked`, or when its runtime state is active, recovery-like, or attention-needing

THE SYSTEM SHALL include an otherwise-idle Direct, Explore, or Branch item only when it was updated within 14 days

THE SYSTEM SHALL suppress completed, failed, handed-off, terminal, archived, and non-user-initiated current conversations

IF a Work-mode task status is unavailable
THE SYSTEM SHALL NOT treat recency alone as evidence that the work remains open

WHEN an item's current state no longer needs attention
THE SYSTEM SHALL remove it from the attention projection without retaining historical inbox state

---

### REQ-GR-004: Provide Bounded Work Selection Data

WHEN the Coordinator or current-work utility presents an item for selection
THE SYSTEM SHALL provide its stable work reference, project, concise title, current conversation, current state, mode, recency, strongest inclusion or attention signals, and app-local navigation target

WHEN the complete deterministic projection is queried
THE SYSTEM MAY additionally provide task, branch, worktree, continuation-root, and chain-membership metadata

THE SYSTEM SHALL tolerate missing metadata without fabricating substitute values

WHEN a deterministic work query includes a text filter
THE SYSTEM SHALL apply the filter before bounded pagination

---

### REQ-GR-005: Provide Stable References and App-Local Links

WHEN an open-work item, chain, conversation, or source message is displayed as a source
THE SYSTEM SHALL provide an app-local navigation target or stable reference handle that can be copied or cited

THE reference syntax SHALL distinguish chains, conversations, and open-work items

WHEN an open-work item later closes or becomes archived
THE SYSTEM SHALL continue to resolve its previously issued work-item reference to the durable source identity and SHALL report its current open, closed, or archived status

THE navigation targets SHALL be app-relative so deployment hostnames and browser gateways do not determine reference validity

---

### REQ-GR-006: Provide One Durable Coordinator Identity

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL resolve it to exactly one durable Coordinator conversation identity

THE SYSTEM SHALL create that Coordinator conversation on demand when it does not exist

THE Coordinator SHALL use the normal transcript, composer, streaming, continuation, persistence, and user-message runtime

THE SYSTEM SHALL NOT present the Coordinator as ordinary project coding work or as a user-created open-work item

---

### REQ-GR-007: Bound Phoenix-Wide Coordinator Capabilities

WHILE a normal coding conversation is running
THE SYSTEM SHALL NOT provide Phoenix-wide history search, global conversation reads, complete open-work reads, global reference resolution, or cross-conversation messaging tools

WHILE the Coordinator is answering a user request
THE SYSTEM MAY provide host-bound tools for global message search, bounded conversation reads, deterministic open-work reads, and reference resolution

THE SYSTEM MAY provide exactly one cross-conversation mutation capability to the Coordinator: sending non-empty text to one existing non-Coordinator conversation through the authoritative user-message acceptance path

THE cross-conversation message capability SHALL NOT accept images, files, skills, filesystem references, user-agent metadata, lifecycle commands, or batch targets

THE SYSTEM SHALL NOT provide filesystem, repository, browser, MCP, task drafting, task approval, project, workspace, conversation creation, or other lifecycle mutation tools to the Coordinator

---

### REQ-GR-008: Answer With Source Citations

WHEN the Coordinator answers a question using conversation history
THE SYSTEM SHALL instruct the answering agent to cite source conversations or messages using app-local links or stable reference handles

THE SYSTEM SHALL expose enough source metadata through global read tools for the agent to cite the conversation id, message id when available, role, timestamp, and excerpt or read content that supports the answer

THE SYSTEM SHALL distinguish deterministic current-work orientation from transcript evidence and SHALL NOT present the current-work projection as proof of historical claims

---

### REQ-GR-009: Resolve Durable Targets Without Guessing

WHEN a user or the Coordinator provides a supported work reference, conversation reference, app-local conversation link, or conversation id
THE SYSTEM SHALL resolve it to one durable target kind, target id, app-local navigation target when available, title when available, and concise summary

WHEN an open-work reference is used for messaging
THE SYSTEM SHALL target its current/latest conversation without silently retargeting a terminal current conversation to a historical member

IF the reference has unsupported or ambiguous syntax
THE SYSTEM SHALL return a clear error instead of guessing

---

### REQ-GR-010: Provide Current Attention and Find Work

WHEN a user opens `/global`
THE SYSTEM SHALL keep the Coordinator transcript and composer visually dominant

THE SYSTEM SHALL present a smaller current-attention utility whose items derive solely from current open-work states and signals for questions, approvals, errors, recovery, or other attention needs

THE utility SHALL provide deterministic text filtering over current open work and compact results containing title, project, state, recency, and stable navigation to the owning current conversation

THE utility SHALL expose freshness and explicit refresh controls and SHALL refresh on page entry, window focus, and completion of a Coordinator turn without timer polling

WHEN a user selects an attention or find-work result
THE SYSTEM SHALL navigate to the owning conversation where native question, approval, retry, cancellation, and error controls remain authoritative

THE utility SHALL NOT reproduce those controls or persist read, seen, acknowledgement, dismissal, resolution, or outcome history

---

### REQ-GR-011: Inject Bounded Current-Work Context

WHEN the Coordinator dispatches an LLM turn
THE SYSTEM SHALL attach a bounded deterministic current-work capsule after the stable cached Coordinator prompt

THE capsule SHALL prioritize attention or recovery work, then active work, then recently idle open work, using deterministic ordering

THE capsule SHALL include aggregate counts and SHALL explicitly report truncation when bounded output omits items

THE capsule SHALL state that it represents current deterministic state rather than complete history, transcript evidence, or an exact delta

THE SYSTEM SHALL keep a complete or paginated deterministic open-work tool available when the bounded capsule is insufficient

---

### REQ-GR-012: Commit and Report One Message Outcome

WHEN the Coordinator submits a valid text message to one resolved conversation
THE SYSTEM SHALL use the same authoritative acceptance and dispatch service used by the ordinary chat endpoint

THE service SHALL preserve persisted-message idempotency, steering-queue idempotency, live runtime authority, stable stored-state rejection, runtime materialization, message acceptability checks, steering depth limits, persistence, broadcast behavior, and applicable PR auto-fix baseline behavior

IF the target accepts a normal user message
THE SYSTEM SHALL report a delivered result containing the resolved target, conversation id, and message id

IF the target accepts the message into its steering queue
THE SYSTEM SHALL report a queued-as-steering result containing the resolved target, conversation id, and message id

IF the target cannot accept the message
THE SYSTEM SHALL report a rejected result with a stable reason code and explanatory message and SHALL NOT report it as delivered or queued

THE SYSTEM SHALL reject the Coordinator conversation and every member of its continuation chain as message targets

THE SYSTEM SHALL reject archived, deleted, unavailable, terminal, context-exhausted, awaiting-question, and awaiting-approval targets according to authoritative conversation state

WHEN the same message id is retried for the same committed message
THE SYSTEM SHALL NOT create a second persisted message or steering entry and SHALL return the committed semantic outcome

THE result SHALL describe acceptance only and SHALL NOT claim recipient understanding, acknowledgement, execution, or completion

WHEN several singular message calls occur in one Coordinator turn
THE SYSTEM SHALL commit and report each target independently without batch transaction semantics
