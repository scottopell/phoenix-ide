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
7. Which current relational facts and timestamps support each status interpretation, and was the raw result truncated?
8. Which active WorkScope path authorized a Coordinator filesystem inspection?

## Requirements

### REQ-GR-001: Provide Transparent Current Activity Facts

WHEN the Coordinator evaluates current Phoenix activity
THE SYSTEM SHALL provide bounded relational facts rather than application-inferred open, stalled, or attention classifications

WHEN a user opens the Coordinator surface
THE surface SHALL present only the normal Coordinator conversation

THE facts SHALL distinguish continuation-root identity from current-conversation identity and SHALL include current state, state-update time, conversation-update time, available task metadata, WorkScope identity, and authoritative active WorkScope cwd and worktree paths without suppressing runtime state when task metadata disagrees

---

### REQ-GR-002: Expose Continuation Identity Without Collapsing Evidence

WHEN conversations form a continuation chain
THE SYSTEM SHALL expose both the durable chain root and the current/latest conversation

WHEN the Coordinator requests current transcript evidence
THE SYSTEM SHALL direct it to the current/latest conversation rather than silently reading only the historical root

Historical chain members SHALL remain addressable through durable references

---

### REQ-GR-003: Interpret Activity From Explicit Evidence

WHEN the Coordinator describes work as active, idle, blocked, stale, or stalled
THE SYSTEM SHALL instruct it to identify the current relational state, relevant timestamps, and recent message or tool evidence supporting that interpretation

THE SYSTEM SHALL NOT suppress an active runtime merely because associated task metadata is closed, unavailable, or inconsistent

Stored state, task metadata, and transcript content SHALL remain separate facts when they disagree

---

### REQ-GR-004: Provide Bounded Read-Only Relational Queries

WHILE the Coordinator is answering a user request
THE SYSTEM SHALL allow exactly one bounded read-only SQLite statement per database-query tool call against operational Phoenix data

THE query capability SHALL support relational joins, common table expressions, grouping, ordering, JSON reads, and allowed full-text reads

THE SYSTEM SHALL enforce statement count, read-only authority, allowed objects and functions, result rows, result bytes, and execution work or duration structurally rather than through prompt discipline or SQL keyword filtering

THE SYSTEM SHALL return typed cells, explicit truncation, and stable policy or budget errors without exposing the database filesystem path

---

### REQ-GR-005: Provide Stable References and App-Local Links

WHEN a work identity, chain, conversation, or source message is displayed as a source
THE SYSTEM SHALL provide an app-local navigation target or stable reference handle that can be copied or cited

THE reference syntax SHALL distinguish chains, conversations, and open-work items

WHEN a previously issued work-item reference is resolved
THE SYSTEM SHALL continue to resolve it to the durable root and current conversation identities and SHALL report raw current state and timestamps without inferring open or closed status

THE navigation targets SHALL be app-relative so deployment hostnames and browser gateways do not determine reference validity

---

### REQ-GR-006: Provide One Durable Coordinator Identity

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL resolve it to exactly one durable Coordinator conversation identity

THE SYSTEM SHALL create that Coordinator conversation on demand when it does not exist

THE Coordinator SHALL use the normal transcript, composer, streaming, continuation, persistence, and user-message runtime

THE SYSTEM SHALL NOT present the Coordinator as ordinary project coding work or as a user-created open-work item

THE SYSTEM SHALL reject archive and hard-delete operations targeting any member of the Coordinator continuation chain so its transcript and singleton identity remain durable

WHEN a chain lifecycle operation contains a Coordinator conversation
THE SYSTEM SHALL reject the entire operation before mutating any chain member

---

### REQ-GR-007: Bound Phoenix-Wide Coordinator Capabilities

WHILE a normal coding conversation is running
THE SYSTEM SHALL NOT provide Phoenix-wide history search, global conversation reads, database queries, global reference resolution, or cross-conversation messaging tools

WHILE the Coordinator is answering a user request
THE SYSTEM MAY provide host-bound tools for global message search, bounded conversation reads, bounded read-only database queries, reference resolution, and OS-sandboxed filesystem inspection

WHEN the Coordinator invokes sandboxed filesystem inspection
THE SYSTEM SHALL require an explicit active `WorkScope` ID for every new command
AND SHALL resolve and canonicalize that WorkScope's persisted worktree path or cwd before launching the existing Explore-mode `nono` sandbox
AND SHALL NOT infer a default repository or cwd
AND SHALL withhold the tool when the Explore sandbox is unavailable

THE SYSTEM MAY provide exactly one cross-conversation mutation capability to the Coordinator: sending non-empty text to one existing non-Coordinator conversation through the authoritative user-message acceptance path

THE cross-conversation message capability SHALL NOT accept images, files, skills, filesystem references, user-agent metadata, lifecycle commands, or batch targets

THE SYSTEM SHALL NOT provide writable filesystem, browser, MCP, task drafting, task approval, project, workspace, conversation creation, or other lifecycle mutation tools to the Coordinator

---

### REQ-GR-008: Answer With Source Citations

WHEN the Coordinator answers a question using conversation history
THE SYSTEM SHALL instruct the answering agent to cite source conversations or messages using app-local links or stable reference handles

THE SYSTEM SHALL expose enough source metadata through global read tools for the agent to cite the conversation id, message id when available, role, timestamp, and excerpt or read content that supports the answer

THE SYSTEM SHALL distinguish current relational facts from transcript evidence and SHALL NOT present either as proof of claims belonging to the other source

---

### REQ-GR-009: Resolve Durable Targets Without Guessing

WHEN a user or the Coordinator provides a supported work reference, conversation reference, app-local conversation link, or conversation id
THE SYSTEM SHALL resolve it to one durable target kind, target id, app-local navigation target when available, title when available, and concise summary

WHEN an open-work reference is used for messaging
THE SYSTEM SHALL target its current/latest conversation without silently retargeting a terminal current conversation to a historical member

IF the reference has unsupported or ambiguous syntax
THE SYSTEM SHALL return a clear error instead of guessing

---

### REQ-GR-010: Keep the Coordinator Surface Chat-Only

WHEN a user opens `/global`
THE SYSTEM SHALL present only the normal Coordinator transcript, composer, conversation status, and conversation navigation

THE SYSTEM SHALL NOT present a separate current-attention pane, open-work list, deterministic work search, or Conversation/Work view selector

THE composer SHALL provide a compact action that submits a normal read-only Coordinator message requesting a current-work briefing

THE briefing action SHALL preserve the user's draft and SHALL NOT create a separate message, streaming, persistence, or cancellation path

---

### REQ-GR-011: Inject a Bounded Relational Snapshot

WHEN the Coordinator dispatches an LLM turn
THE SYSTEM SHALL attach a bounded current-activity snapshot after the stable cached Coordinator prompt

THE snapshot SHALL expose raw current continuation leaves with root and current identifiers, state, state-update time, conversation-update time, available task metadata, WorkScope identity, and authoritative active WorkScope cwd and worktree paths

THE snapshot SHALL order active runtime states first and then by conversation update time, SHALL state its row limit and selection rule, and SHALL explicitly report result truncation

THE snapshot SHALL state that it contains raw facts rather than open-work, stalled, attention, history, or exact-delta classifications

THE SYSTEM SHALL keep the bounded read-only database query tool available when the snapshot is insufficient

---

### REQ-GR-011A: Bound Database Integrity and Resource Use

WHILE the Coordinator executes a database query
THE SYSTEM SHALL permit reads from Phoenix application tables, including hidden messages, credentials, tokens, settings, serialized state, and workflow payloads that may not be visible through normal UI

THE SYSTEM SHALL describe this capability as operator-level forensic access and SHALL treat all returned values as untrusted stored data rather than instructions

THE SYSTEM SHALL structurally deny writes, transactions, SQLite internal and FTS shadow storage, filesystem functions, extension loading, database attachment, and pragmas

THE integrity boundary SHALL apply at SQLite authorization time so views, common table expressions, subqueries, aliases, and alternate SQL spelling cannot bypass it

THE SYSTEM SHALL enforce bounded SQL input, columns, rows, serialized output, and execution work or duration

IF SQLite rejects a query for a reason other than authorization policy or execution-budget exhaustion
THE SYSTEM SHALL return an actionable engine diagnostic containing the operation phase, primary and extended SQLite result codes, symbolic result-code name, SQLite diagnostic message, and parse-error offset when SQLite provides one

THE SYSTEM SHALL preserve authorization-policy and execution-budget errors as distinct outcomes rather than replacing them with SQLite engine diagnostics

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
