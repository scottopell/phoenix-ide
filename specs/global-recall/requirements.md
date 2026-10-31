# Phoenix Coordinator

## User Story

As a Phoenix user, I often have several unrelated streams of work active across projects, continuation chains, and standalone conversations. I want one durable Phoenix-wide conversation where I can survey that work, inspect relevant history, and send useful text guidance to existing conversations without opening and operating each one manually.

The Coordinator is an open-ended cross-conversation console, not a manager for one global objective. It queries current-work facts on demand, selectively reads source conversations, and may communicate through the same message acceptance path used by the ordinary chat composer. Write-capable ordinary ProductConversations may use bounded global evidence and singular cross-conversation messaging on explicit turns, while restricted planning conversations and sub-agents remain scoped. Structurally, Phoenix models Coordinator identity separately from ordinary product-conversation lifecycle rows: ordinary parent transcript rows participate in the Open/History product lifecycle and WorkScope model, while the Coordinator retains normal transcript persistence, continuation, and message runtime without any ProductConversation Open/History lifecycle or ordinary Close/Delete controls. Sub-agents remain a separate execution kind and are not Coordinators.

## Why the User Cares

- **Orientation should be evidence-based.** Current-work briefings should use bounded fresh relational facts and distinguish observation from interpretation.
- **Long-running work should not fragment identity.** A continuation chain represents one work item even though it spans multiple conversations.
- **Intervention should be narrow and trustworthy.** The Coordinator may send text to existing conversations, while the receiving conversation's authoritative state determines whether the message starts immediately, becomes steering, or is rejected.
- **Committed actions should be truthful.** When reporting a delivery, the Coordinator distinguishes each target's acceptance from another agent's understanding, execution, or completion.
- **Global access should be deliberate and bounded.** Write-capable ordinary ProductConversations and the Coordinator may inspect Phoenix-wide evidence and send singular messages; restricted planning conversations and sub-agents remain scoped.

## Transparency Contract

The user must be able to answer:

1. Which ProductConversation and WorkScope own an open work item?
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
THE surface SHALL present the normal Coordinator conversation together with only the bounded subordinate activity surfaces admitted by REQ-GR-010

THE facts SHALL distinguish durable ProductConversation identity, derived root transcript-row identity, and latest execution-row identity and SHALL include current state, state-update time, conversation-update time, available task metadata, attached WorkScope identity, and authoritative active WorkScope cwd and worktree paths without suppressing runtime state when task metadata disagrees

---

### REQ-GR-002: Expose Continuation Identity Without Collapsing Evidence

WHEN transcript rows form a continuation chain
THE SYSTEM SHALL expose the durable ProductConversation, its derived root transcript row, and the current/latest execution row

WHEN the Coordinator requests current transcript evidence
THE SYSTEM SHALL direct it to the current/latest transcript row rather than silently reading only the historical root row

Historical chain members SHALL remain addressable through durable references

---

### REQ-GR-003: Interpret Activity From Explicit Evidence

WHEN the Coordinator describes work as active, idle, blocked, stale, or stalled
THE SYSTEM SHALL instruct it to identify the current relational state, relevant timestamps, and recent message or tool evidence supporting that interpretation

THE SYSTEM SHALL NOT suppress an active runtime merely because associated task metadata is closed, unavailable, or inconsistent

Stored state, task metadata, and transcript content SHALL remain separate facts when they disagree

---

### REQ-GR-004: Provide Bounded Read-Only Relational Queries

WHILE a write-capable ordinary ProductConversation or the Coordinator is answering a user request
THE SYSTEM SHALL allow exactly one bounded read-only SQLite statement per database-query tool call against operational Phoenix data

THE query capability SHALL support relational joins, common table expressions, grouping, ordering, JSON reads, and allowed full-text reads

THE SYSTEM SHALL enforce statement count, read-only authority, allowed objects and functions, result rows, result bytes, and execution work or duration structurally rather than through prompt discipline or SQL keyword filtering

THE SYSTEM SHALL return typed cells, explicit truncation, and stable policy or budget errors without exposing the database filesystem path

---

### REQ-GR-005: Provide Stable References and App-Local Links

WHEN a work identity, ProductConversation, transcript row, or source message is displayed as a source
THE SYSTEM SHALL provide an app-local navigation target or stable typed reference handle that can be copied or cited

THE reference syntax SHALL distinguish ProductConversation identities, transcript-row identities, and open-work views
AND SHALL treat previously issued chain references as compatibility aliases that normalize to one ProductConversation plus derived transcript topology rather than as a second mutable identity

WHEN a previously issued work-item reference is resolved
THE SYSTEM SHALL continue to resolve it to the durable ProductConversation, derived root transcript row, and current execution-row identities and SHALL report raw current state and timestamps without inferring open or closed status

THE navigation targets SHALL be app-relative so deployment hostnames and browser gateways do not determine reference validity

---

### REQ-GR-006: Provide One Durable Coordinator Identity

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL resolve it to exactly one durable Coordinator conversation identity

THE SYSTEM SHALL create that Coordinator conversation on demand when it does not exist

THE Coordinator SHALL use the normal transcript, composer, streaming, continuation, persistence, and user-message runtime

THE SYSTEM SHALL structurally distinguish the Coordinator from ordinary product conversations rather than encoding that difference as omitted lifecycle fields or nullable product-conversation links

THE SYSTEM SHALL NOT present the Coordinator as ordinary repository-backed coding work or as a user-created open-work item
AND SHALL NOT give the Coordinator an ordinary ProductConversation Open/History lifecycle, worker-owned WorkScope attachment, Close control, or Delete control

THE SYSTEM SHALL reject archive and hard-delete operations targeting any member of the Coordinator continuation chain so its transcript and singleton identity remain durable

WHEN a chain lifecycle operation contains a Coordinator conversation
THE SYSTEM SHALL reject the entire operation before mutating any chain member

---

### REQ-GR-007: Bound Phoenix-Wide Agent Capabilities

WHILE a write-capable ordinary ProductConversation or the Coordinator is answering a user request
THE SYSTEM MAY provide host-bound tools for global message search across Phoenix's own ProductConversation/transcript/message corpus, bounded transcript reads, bounded read-only database queries, and singular cross-conversation messaging

WHILE a restricted planning conversation or sub-agent is running
THE SYSTEM SHALL NOT provide Phoenix-wide history search, global conversation reads, database queries, global reference resolution, or cross-conversation messaging tools

THE ordinary-parent predecessor capability defined by
`../conversation-retrieval/requirements.md` REQ-RET-009 SHALL remain separate
from Phoenix-wide capabilities: restricted planning parents may inspect their
own predecessors through that bound capability without receiving global tools

WHILE the singleton Global Coordinator is answering a user request
THE SYSTEM MAY additionally provide host-bound tools for global reference resolution and unsandboxed Bash
AND MAY provide a Coordinator-only built-in skill documenting supported Phoenix HTTP APIs

WHEN the user authorizes a Coordinator action supported by a public Phoenix HTTP API
THE Coordinator MAY invoke that API through explicitly WorkScope-targeted Bash
AND SHALL ground the operation in inspected current deployment/source contracts applicable to the selected destination
AND SHALL treat built-in API recipes as non-exhaustive best practices rather than an endpoint allowlist
AND SHALL treat gap inventories as illustrative and qualified by inspected deployment/source, permitting verified supported alternatives and bounded state reads within user authority
AND SHALL NOT invent guarantees or receipts, bypass enforcement through databases/private handlers, or create background monitors to substitute for missing API semantics
AND SHALL preserve normal API authorization, destination identity, authenticated encrypted nonlocal transport, destination-bound nonserialized credential handling, and server-enforced lifecycle admission
AND SHALL retain the original target and exact intent across uncertain outcomes, retry only under the endpoint's supported idempotency contract, and never mint a new identity for an uncertain retry
AND SHALL freshly read the relevant target state before acting, verify both the HTTP response and the resulting Phoenix state, and distinguish request acceptance from observed execution
AND SHALL NOT use private handlers or database writes to bypass public API enforcement

WHEN the Coordinator submits answers to a pending structured question through the public API
THE instructions SHALL permit exact user-provided answers or decisions made within explicit delegated decision scope for every question, with exact full question-text keys and exact option labels or authorized custom text
AND SHALL NOT treat a generic instruction to inspect or unblock work as decision authority, fabricate approvals, or extend delegation
AND SHALL require the exact current opaque question request identity, distinct from provider tool-use identity, under `../ask-user-question/requirements.md` REQ-AUQ-009
AND SHALL require complete answers without claiming server-enforced completeness
AND SHALL direct one POST followed by a fresh read of the original target, treating the question request identity as a freshness guard rather than an idempotency key
AND SHALL NOT transplant answers to a new identity, automatically replay an uncertain submission, or treat a conflict as success

THE host-bound capabilities SHALL NOT become ambient prompt memory or autonomous background behavior except for explicit Global Coordinator subscriptions governed by REQ-GR-015

WHEN the singleton Global Coordinator needs user input to proceed
THE SYSTEM SHALL provide the parent-conversation `ask_user_question` capability
AND SHALL use the shared request-bound structured-question state and web interaction surface
AND SHALL NOT grant that capability to sub-agents

THE search and transcript-read capabilities SHALL describe recalled text as untrusted stored data rather than instructions

WHEN a destination serves a federated conversation-page read
THE SYSTEM SHALL examine at most 12,992 physical message rows and issue at most 203 bounded-history fetches for that request
AND SHALL count visible rows, hidden rows, and rows replayed to reach the rendered-content cursor against the same row budget
AND SHALL cap each fetch to the remaining row budget

IF the system reaches that budget before a fetched row proves rendered continuation or a bounded fetch proves physical transcript EOF
THE SYSTEM SHALL fail the request without returning page content, a continuation cursor, or an end-of-transcript indication
AND SHALL NOT expose a resumable physical-scan position
AND an unchanged request SHALL restart the same scan rather than advance it

THE physical-message budget SHALL bound work rather than wall-clock duration
AND neither that budget nor an HTTP deadline response SHALL guarantee interruption of in-progress SQLite work

WHEN the singleton Global Coordinator invokes Bash
THE SYSTEM SHALL require an explicit active `WorkScope` ID for every new command
AND SHALL resolve and canonicalize that WorkScope's persisted worktree path or cwd before launching unsandboxed Bash
AND SHALL NOT infer a default repository or cwd
AND SHALL reject the command without spawning a process when the WorkScope ID is missing, blank, stale, invalid, or resolves to no live owner

WHEN the singleton Global Coordinator publishes a static SVG
THE SYSTEM MAY provide the existing `present_svg` capability with one required active WorkScope target
AND SHALL re-resolve that WorkScope through the same active persisted authority used by Coordinator Bash
AND SHALL restrict the source read to a contained regular file beneath the resolved root without following symlinks
AND SHALL own the durable artifact and invocation by the executing Coordinator transcript rather than the selected WorkScope or its conversation
AND SHALL NOT thereby grant generic filesystem authority or any unrelated write capability.

THE SYSTEM MAY provide a dedicated cross-conversation message tool to a write-capable ordinary ProductConversation or the Coordinator: sending non-empty text to one other existing non-Coordinator conversation through the authoritative input acceptance path

THE SYSTEM SHALL additionally provide explicit subscription management only to the Global Coordinator as governed by REQ-GR-015

THE cross-conversation message capability SHALL NOT accept images, files, skills, filesystem references, user-agent metadata, lifecycle commands, or batch targets

THE SYSTEM SHALL NOT provide writable filesystem tools to the Coordinator other than the singleton Coordinator's explicitly WorkScope-targeted Bash capability
AND SHALL NOT provide browser, MCP, task drafting, task approval, project, workspace, source-scoped retrieval for another conversation's private follow-up surface, or dedicated lifecycle mutation tools other than the bounded creation, naming, and Goal metadata capabilities defined below to the Coordinator

WHILE the persisted singleton Global Coordinator is the executing current transcript
THE SYSTEM MAY provide dedicated conversation creation and naming tools through the same application actions used by public conversation operations
AND SHALL recheck current Coordinator authority at the committing persistence boundary, including operation reconciliation reads
AND SHALL NOT grant these tools to ordinary conversations, predecessor transcripts, sub-agents, or restricted planning conversations
AND SHALL preserve durable request identity, exact immutable target identity, source tool-call provenance, and operation-specific replay semantics
AND SHALL expose creation acceptance and historical naming receipts without claiming runtime completion or current naming state
AND SHALL NOT thereby grant continuation, structured-answer, review, task approval, or other lifecycle mutation authority

THE creation capability SHALL follow `../conversation-creation/requirements.md` REQ-CCR-013
AND THE naming capability SHALL follow `../conversation-identity/requirements.md` REQ-CID-007

WHEN the current authenticated Coordinator uses native Goal metadata tools
THE SYSTEM MAY provide `read_goal`, `prepare_goal`, and reference-only `apply_goal` through the shared Goal command service
AND SHALL enforce the same admission, provenance, retained-receipt, and history rules as owner Goal commands
AND SHALL follow the admission, retained-state, safety-control, and no-implicit-execution requirements in [`goal-continuation/requirements.md`](../goal-continuation/requirements.md) REQ-GOAL-AC-001 through REQ-GOAL-AC-004
AND SHALL require explicit persisted owner autoContinue enablement for new Goal creation or replacement without enabling that preference
AND SHALL preserve Goal/history reads, Complete/Suspend safety exits, and retained receipt replay while that preference is OFF
AND SHALL NOT expose these tools to ordinary conversations, predecessor transcripts, or sub-agents, schedule continuation, or dispatch target work through them

---

### REQ-GR-008: Answer With Source Citations

WHEN the Coordinator answers a question using conversation history
THE SYSTEM SHALL instruct the answering agent to cite source ProductConversations and exact transcript messages using app-local links or typed reference handles

THE SYSTEM SHALL expose enough source metadata through global read tools for the agent to cite the stable ProductConversation ID, exact transcript ID, message ID when available, role, timestamp, and excerpt or read content that supports the answer

THE SYSTEM SHALL distinguish current relational facts from transcript evidence and SHALL NOT present either as proof of claims belonging to the other source

---

### REQ-GR-009: Resolve Durable Targets Without Guessing

WHEN a user or the Coordinator provides a supported work reference, typed ProductConversation reference, typed transcript-row reference, or app-local link
THE SYSTEM SHALL resolve it to one durable target kind, target id, app-local navigation target when available, title when available, and concise summary
AND SHALL reject a bare identifier whose identity domain is ambiguous rather than resolving it by equal underlying bytes

THE resolved target SHALL include the WorkScope attached to the selected transcript member, its lifecycle and environment kind, authoritative cwd and worktree path, and an effective path that prefers worktree path over cwd
AND SHALL mark those paths as server-filesystem locations rather than caller-local paths

WHEN an app-local `/c/<reference>` identifies an authoritative ordinary ProductConversation and carries no explicit transcript or message selector
THE SYSTEM SHALL interpret it as conversation navigation that follows the current continuation
AND SHALL emit the percent-encoded `/c/<allocated-root-slug>` as its current canonical ordinary conversation route under `specs/conversation-identity/requirements.md`
AND SHALL preserve permanent ProductConversation IDs as durable reference identities and `/product-conversations/<id>` as a compatibility alias
AND SHALL NOT promise that a former slug remains bound after rename

WHEN an app-local reference contains a `source_transcript` selector
THE SYSTEM SHALL validate that the selected transcript belongs to the referenced conversation and identity domain before selecting it
AND SHALL reject empty, duplicate, malformed, or nonmember selectors without silently following the current member
AND SHALL preserve message anchors and other query parameters through browser canonicalization

WHEN a legacy slug or explicit historical member link is opened
THE SYSTEM SHALL resolve its authoritative binding rather than infer an identity domain from UUID spelling
AND SHALL preserve explicit historical evidence, Global Coordinator routing, and the existing desktop `phoenix://conversation/<id>` handoff

WHEN a stable ProductConversation reference is resolved
THE SYSTEM SHALL resolve the current transcript member and its attached WorkScope from one database point in time

WHEN a stable ProductConversation reference is resolved through a bounded global conversation-read or reference-resolution surface
THE SYSTEM SHALL resolve its eligible ordinary identity and topology from metadata
AND SHALL NOT deserialize message content, continuation handoff content, or continuation opening content solely to resolve that identity

WHEN an exact transcript-row reference is resolved
THE SYSTEM SHALL resolve only that historical member's attached WorkScope and SHALL NOT substitute a current or successor member

IF the selected member has no attached WorkScope, its attached WorkScope record cannot be read, or the scope is retired or has no environment path
THE SYSTEM SHALL represent that fact explicitly without inventing a scope, lifecycle, environment kind, or path

WHEN an open-work reference is used for messaging
THE SYSTEM SHALL target its topology-derived latest parent transcript row without silently retargeting a terminal latest row to a historical member

IF the reference has unsupported or ambiguous syntax
THE SYSTEM SHALL return a clear error instead of guessing

THE typed read and message tools SHALL accept only `@conv:<product_conversation_id>` for a stable ProductConversation or `@transcript:<conversation_id>` for an exact transcript member
AND legacy app-local, chain, and work references SHALL remain confined to the compatibility resolver
AND a WorkScope identifier SHALL NOT be accepted as read or message target syntax

---

### REQ-GR-010: Keep the Coordinator Surface Chat-Only

WHEN a user opens `/global`
THE SYSTEM SHALL present the normal Coordinator transcript, composer, conversation status, and conversation navigation
AND MAY present bounded subordinate activity surfaces containing only:
- still-running Bash commands launched by the Coordinator, their authoritative command metadata, output navigation, and supported exact-stop controls; and
- the current server-backed set of active Coordinator watch subscriptions, with authoritative conversation identity and navigation

THE activity surfaces SHALL remain subordinate to the transcript and composer

THE Bash activity surface SHALL NOT invent durable or cross-restart command state

THE watch activity surface SHALL reflect current subscription state rather than reconstructing state from transcript history

THE SYSTEM SHALL NOT present a separate current-attention pane, open-work list, deterministic work search, cross-scope resource explorer, or Conversation/Work view selector

THE composer SHALL provide a compact action that submits a normal Coordinator message requesting a bounded whole-stack check-in and coordination pass through existing owners within verified standing authority

THE briefing action SHALL preserve the user's draft and SHALL NOT create a separate message, streaming, persistence, or cancellation path

---

### REQ-GR-011: Obtain Current Activity on Demand

WHEN the Coordinator needs current activity facts for a user request
THE SYSTEM SHALL provide the bounded read-only database query capability
AND SHALL instruct the Coordinator to query relevant current transcript rows, timestamps, continuation identities, and authoritative active WorkScope identities and paths before making current-state claims or choosing a Bash target

WHEN the Coordinator dispatches a model request
THE SYSTEM SHALL NOT automatically inject current activity facts into its system instructions or conversation context

WHEN the user requests a check-in through the composer action
THE SYSTEM SHALL submit a normal message requesting a broad fresh check of commissioned work and selective intervention through established owners within verified standing scope and approvals
AND SHALL direct the Coordinator to leave healthy progressing work alone, avoid redundant nudges, replanning or assignments merely to demonstrate activity, and not infer that an idle owner is stalled
AND SHALL direct the Coordinator to investigate blocked or waiting owners before treating them as healthy or stalled, take the smallest recovery step allowed by existing authority through the established owner, and verify resumed work rather than message acceptance; progress elsewhere SHALL NOT discharge a blocked obligation
AND SHALL reconcile later decisions and completed work before reviving questions or replaying earlier answers
AND SHALL direct the Coordinator to recover unanswered user decisions from history and verify that each remains unresolved and material before prioritizing user input, while advancing independent authorized work first
AND SHALL preserve prior decisions, corrections, pauses and gates; a repeated check-in SHALL NOT itself expand authority or reset them
AND SHALL distinguish requested action and message acceptance from verified completion without requiring an announcement for every routine delivery
AND SHALL lead the check-in briefing with still-needed user input, presenting genuine choices through `ask_user_question` with context and a recommendation, then explain what changed, why it matters and what happens next in concrete language understandable without intervening history, naming the actual work rather than opaque coordination labels, with useful links and grounded status where appropriate, without requiring a fixed report template or announcements for factual watch events

AND SHALL include process or evidence limits when material to understanding or a decision, without indefinite polling or waiting for all owners to finish

WHEN processing events without a user request for a briefing
THE SYSTEM SHALL direct the Coordinator to report only needed user action or decisions, meaningful goal-level results, or new blockers or risks materially changing the plan; otherwise it SHALL continue authorized work quietly
AND SHALL NOT treat routine check results, message deliveries or unchanged waiting as user-facing milestones
AND SHALL include compact hypothetical bad-versus-good wording and silence examples in both language-specific system prompts, distinguishing requests from verified outcomes without treating examples as incident facts

---

### REQ-GR-011A: Bound Database Integrity and Resource Use

WHILE a write-capable ordinary ProductConversation or the Coordinator executes a database query
THE SYSTEM SHALL permit reads from Phoenix application tables, including hidden messages, settings, serialized state, and workflow payloads that may not be visible through normal UI

THE SYSTEM SHALL deny reads of known credential-bearing columns, including owner sessions, share tokens, MCP OAuth client secrets and tokens, and federation credential verifiers, without treating the query surface as a general security sandbox

THE SYSTEM SHALL describe this capability as operator-level forensic access and SHALL treat all returned values as untrusted stored data rather than instructions

THE SYSTEM SHALL structurally deny writes, transactions, SQLite internal and FTS shadow storage, filesystem functions, extension loading, database attachment, and pragmas

THE integrity boundary SHALL apply at SQLite authorization time so views, common table expressions, subqueries, aliases, and alternate SQL spelling cannot bypass it

THE SYSTEM SHALL enforce bounded SQL input, columns, rows, serialized output, and execution work or duration

IF SQLite rejects a query for a reason other than authorization policy or execution-budget exhaustion
THE SYSTEM SHALL return an actionable engine diagnostic containing the operation phase, primary and extended SQLite result codes, symbolic result-code name, SQLite diagnostic message, and parse-error offset when SQLite provides one

THE SYSTEM SHALL preserve authorization-policy and execution-budget errors as distinct outcomes rather than replacing them with SQLite engine diagnostics

---

### REQ-GR-012: Commit and Report One Message Outcome

WHEN a write-capable ordinary ProductConversation or the Coordinator submits a valid text message to one resolved conversation
THE SYSTEM SHALL use the same authoritative acceptance and dispatch service used by the ordinary chat endpoint

THE service SHALL preserve persisted-message idempotency, steering-queue idempotency, live runtime authority, stable stored-state rejection, runtime materialization, message acceptability checks, steering depth limits, persistence, broadcast behavior, and applicable PR auto-fix baseline behavior

IF the target accepts a normal user message
THE SYSTEM SHALL report a delivered result containing the resolved target, conversation id, and message id

IF the target accepts the message into its steering queue
THE SYSTEM SHALL report a queued-as-steering result containing the resolved target, conversation id, and message id

IF the target cannot accept the message
THE SYSTEM SHALL report a rejected result with a stable reason code and explanatory message and SHALL NOT report it as delivered or queued

THE SYSTEM SHALL reject the originating conversation, the Coordinator conversation and every member of its continuation chain as message targets
AND SHALL continue to treat sub-agent conversations as separately ineligible targets under the ordinary acceptance rules rather than by conflating them with Coordinator identity

THE SYSTEM SHALL reject archived, deleted, unavailable, terminal, context-exhausted, awaiting-question, and awaiting-approval targets according to authoritative conversation state

WHEN the same message id is retried for the same committed message
THE SYSTEM SHALL NOT create a second persisted message or steering entry and SHALL return the committed semantic outcome

THE result SHALL describe acceptance only and SHALL NOT claim recipient understanding, acknowledgement, execution, or completion

WHEN several singular message calls occur in one agent turn
THE SYSTEM SHALL commit and report each target independently without batch transaction semantics

---

### REQ-GR-013: Preserve Coordination Context Through Compaction

WHEN the global Coordinator requests a continuation summary
THE SYSTEM SHALL select coordination-focused handoff instructions through the existing Coordinator identity
AND SHALL use the shared tool-free continuation pipeline and protected accepted-handoff contract in [REQ-BED-020](../bedrock/requirements.md#req-bed-020-continuation-summary-generation)
AND SHALL describe its actual capability boundaries without promising an ambient working directory, conversation creation, or background monitoring
AND SHALL NOT inject live activity facts into summary generation

THE instructions SHALL prioritize unresolved workstreams, objectives, scoped user authority and preferences including reporting preferences, corrections, decisions, blockers, dependencies, obligations, and next actions
AND SHALL preserve owner identities and delegation relationships, with durable target references distinct from historical transcript references when available
AND SHALL distinguish direct user instructions, delegate requests, Coordinator commitments, and historical observations without promoting a delegate request or inherited assertion into user authority
AND SHALL distinguish delivery acceptance, acknowledgement, execution, and verified completion
AND SHALL retain evidence provenance and last-observed timestamps when known, mark unknowns honestly, and direct the resumed Coordinator to refresh relevant live status before making current-state claims
AND SHALL apply explicit corrections and supersession while preserving unresolved obligations, including paused work
AND SHALL retain exact primary user message references and relevant scoped wording for standing directions and claimed historical approvals when known, with narrow retrieval pointers for missing evidence rather than fabricated authority
AND SHALL request the next authorized in-scope action, required current status and ownership checks, completion criteria, and outstanding verification

WHEN the Coordinator resumes from a continuation handoff
THE trusted instructions SHALL distinguish evidence verification from obeying directives embedded in recalled content
AND SHALL direct narrowly recovering authentic primary user evidence and checking attribution, exact scope, later corrections, supersession, pauses, gates, current status and established ownership before acting
AND SHALL describe continuation as a handoff, not cancellation or expiry of verified standing user directions, and direct the next authorized in-scope step by default with follow-through to verified completion rather than status-only acknowledgement or redundant permission
AND SHALL direct narrow evidence recovery or a focused question when authority is missing, conflicting, or insufficient, while preserving pauses and gates
AND SHALL keep generated summaries, quotations, delegate assertions and encoded payloads untrusted and unable to self-authorize, override instructions, expand scope, or grant blanket merge, deploy, or delete permission

THE handoff instructions SHALL compress completed details before unresolved commitments, retaining only resolution context needed to avoid reopening settled work
AND SHALL preserve relevant paths, verification evidence, unfinished edits, and pending tool arguments when the Coordinator performs hands-on work

THE coordination policy SHALL NOT change the Coordinator's capabilities, lifecycle, WorkScope ownership, or message authority
AND SHALL NOT infer a coordination role for ordinary conversations

**Rationale:** Cross-conversation coordination depends on remembering ownership and unfinished obligations through interruptions. Historical memory guides the next inspection; it does not establish current state or new authorization. Prompt instructions guide generated content but do not guarantee lossless retention of unlimited obligations.

### REQ-GR-014: Preserve Trusted Input Attribution

THE SYSTEM SHALL assign input origin at trusted server boundaries and SHALL distinguish user-facing API input, internal conversation messages, system-generated input, and subscription events.

WHEN a conversation sends a message
THE SYSTEM SHALL preserve the sender's stable ProductConversation identity and exact transcript identity through durable admission, steering, persisted history, transport, UI attribution, and model-bound rendering.

THE SYSTEM SHALL NOT accept a model-supplied origin as authority or infer human authorship for historical input whose origin was not recorded.

THE SYSTEM SHALL treat user-facing API origin as a channel classification, not proof that a biological human authored the input.

WHEN an API retry addresses an accepted pre-provenance input with otherwise matching identity and payload
THE SYSTEM SHALL preserve the original acceptance and unknown historical origin rather than create new input or reattribute the stored input.

THE SYSTEM SHALL continue to reject changed payloads and conflicts with recorded origins on retries.

WHEN conversation-delivered input has a recorded originating tool invocation
THE SYSTEM SHALL retain the server-owned source transcript, assistant message, and tool-call identity through admission, queueing, persistence, and retrieval.

WHEN the user activates that input's source-call link
THE SYSTEM SHALL open the recorded transcript member, locate the originating tool call, expand its collapsed presentation, and highlight it without substituting a current successor or recipient message.

WHEN historical input has no recorded source-call locator
THE SYSTEM SHALL state that the original call is unavailable and offer only the recorded source transcript.

WHEN an exact source member cannot be loaded
THE SYSTEM SHALL display an error within its normal layout without silently navigating to another member.

### REQ-GR-015: Watch Explicit Stable Conversations

THE Global Coordinator SHALL have a trusted capability to enroll, inspect, and remove watches of open ordinary ProductConversations without per-target user approval.

THE SYSTEM SHALL retain a watch across transcript continuation until removal or source Close, SHALL make repeated enrollment idempotent, and SHALL return source current state consistently with enrollment.

THE SYSTEM SHALL deliver only relevant occurrences committed after enrollment and SHALL NOT replay historical events or revive suppressed events on re-enrollment.

WHEN watched execution ends normally, fails, or is explicitly cancelled
THE SYSTEM SHALL durably record a factual notification obligation with its source occurrence.

THE SYSTEM SHALL exclude successful continuation handoff and awaiting-question or awaiting-approval states from stop notifications.

WHEN a watched conversation durably enters an awaiting-question or awaiting-task-approval state
THE SYSTEM SHALL record a distinct wait-entry outcome through the same event outbox without settling the active execution as terminal.

THE SYSTEM SHALL identify a question wait by its authoritative request identity when present. For a wait without a request identity, THE SYSTEM SHALL allocate its occurrence identity atomically with the committed wait entry and its notification obligations. Re-persisting an identical pending wait SHALL NOT create another occurrence; leaving and re-entering that wait SHALL create a new occurrence. Enrollment during an already-persisted wait SHALL NOT replay that wait.

THE SYSTEM SHALL deliver factual packets with source identity, occurrence identity, outcome, time, and supported cause information, without behavioral instructions or recovery recommendations. Natural-language derived facts are permitted.

THE SYSTEM SHALL admit notifications through ordinary durable input admission or steering, with stable replay identity, and SHALL NOT introduce a separate execution scheduler.

WHEN removal or source Close precedes subscription notification acceptance
THE SYSTEM SHALL suppress the unaccepted subscription notification.

WHEN a distinct Close cleanup failure is durably recorded
THE SYSTEM SHALL append a mandatory factual notification to the same event outbox, regardless of watch enrollment or source History membership, and SHALL route it to the current Global Coordinator transcript.

THE SYSTEM SHALL identify the mandatory notification by the persisted failure occurrence, SHALL deduplicate replay of that occurrence, and SHALL emit a new notification for a distinct failure of a newly authorized cleanup run.

THE SYSTEM SHALL preserve the failure and notification atomically, SHALL distinguish confirmed conversation-and-process shutdown from uncertain shutdown in the notification, and SHALL NOT treat notification delivery or failure as authorization to resume cleanup.

WHEN notification acceptance precedes removal or source Close
THE SYSTEM SHALL retain accepted input without retraction.

WHEN the Coordinator's execution is stopped
THE SYSTEM SHALL retain subscriptions and permit future notifications, and SHALL NOT redeliver already-accepted input merely because execution was cancelled.


### REQ-GR-016: Own a Persistent Local Execution Home

THE SYSTEM SHALL provide exactly one instance-owned active WorkScope with a server-managed non-project home at `<resolved data_dir>/coordinator/`, independently of ordinary conversations and the current Coordinator transcript.
AND SHALL retain its identity, environment binding and files across Coordinator continuation and server restart.
AND SHALL distinguish that ownership and environment from ordinary worktrees, unowned cwd, managed repositories and task approval authority.

THE SYSTEM SHALL initialize a fresh binding safely and reconcile an existing Coordinator chain atomically without moving, importing, deleting, executing or trusting improvised worker-tree files.
AND SHALL reject conflicting ownership, worker-worktree path overlap, invalid environment shape, unexpected configuration rebinding, inaccessible home, non-directory home and symlink replacement with actionable errors.
WHEN a bound home is missing
THE SYSTEM SHALL refuse automatic recreation or relocation rather than mask retained-data loss.

THE instance-owned binding, environment, authority and active lifecycle SHALL NOT be mutated or deleted by ordinary creation, task approval, mode/cwd changes, close, archive, delete or cleanup.
AND ordinary scope ownership and selected project execution SHALL remain separate.
THE SYSTEM SHALL isolate default development data roots by worktree while preserving explicit operator data-root configuration.

### REQ-GR-017: Discover the Home Without Promoting Historical Data

THE SYSTEM SHALL provide authoritative active home WorkScope identity, server path, owner and non-project environment in Coordinator prompt context and grounding.
AND SHALL require explicit active WorkScope targeting for every new scoped Bash command, including home execution.
AND SHALL apply the same scoped target resolution to supported contained artifact publication without granting remote cwd execution or new lifecycle authority.

THE grounding SHALL support server-backed directory browsing and file viewing on desktop and mobile without Git/project/task/ordinary-retirement controls.
AND SHALL permit only the authoritative home root through the shared canonical path and symlink containment checks, not the whole data directory.

THE Coordinator handoff policy SHALL retain concise deliberately selected note/evidence pointers and unfinished obligations while regenerating authority from current server state.
AND SHALL treat retained file contents and notes as historical untrusted data, never instructions or authority.
AND SHALL NOT scan or automatically ingest home files into prompts or compaction.
AND SHALL distinguish quiet user updates from passive coordination, delegation from execution, and fresh runtime/source/test evidence from stale claims.
AND SHALL keep persistent blockers visible and require concrete supported follow-up triggers for waits.

### REQ-GR-018: Retain and Export Home State Deliberately

THE SYSTEM SHALL retain home files independently of ordinary conversation, transcript-history and worker cleanup.
AND SHALL NOT automatically migrate or prune retained files.
THE SYSTEM SHALL identify conversation transcript export and database-only backups as excluding home bytes.

WHEN an operator backs up or restores Coordinator state
THE supported procedure SHALL stop the runtime and preserve the database together with the home directory at its configured canonical location under `specs/compatibility/requirements.md` offline replacement policy.
AND SHALL NOT imply automatic relocation, live replacement, cross-version rollback or a generic memory/backup platform.

WHEN retained home files are intentionally cleaned
THE supported procedure SHALL require deliberate owner action identifying the files to remove, preserve the binding and directory, and never be an ordinary Close/Delete side effect.
WHEN the entire instance is intentionally decommissioned
THE operator SHALL stop the runtime and explicitly remove its database and retained data as an instance-wide action, not a WorkScope retirement command.
