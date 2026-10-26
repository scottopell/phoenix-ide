# Slack gateway

## User need

As Phoenix's owner, I need to flag Slack discussions for preparation and explicitly ask my local agent for work without copying links into a new chat. I need the resulting work to remain an ordinary Phoenix conversation, with trustworthy source attribution, predictable reply destinations, and no duplicate execution when discovery or delivery is retried.

## Scope

This feature serves one configured owner through that owner's authenticated Slack connection. It covers discovery, acceptance, conversation association, and return delivery. Phoenix's conversation creation, execution, questions, permissions, and lifecycle retain their own contracts. Slack search observes current state; it is not a complete event history.

## Requirements

### REQ-SG-001 — Accept requests only from the owner

THE SYSTEM SHALL bind gateway authority to an explicit Slack workspace and owner user identity, verify that the connected Slack identity matches, and refuse discovery admission when identity cannot be established.

Before acknowledgments or replies, actor and workspace evidence SHALL refer to one fixed serving connection. Replacing or reauthorizing that connection SHALL require fresh verification rather than permit the write on another account.

WHEN a message or reaction is considered as a trigger, THE SYSTEM SHALL confirm the source and triggering actor through Slack and accept only the configured owner's action. A message from another user, an app-authored message, or a gateway-authored message SHALL NOT count as the owner's instruction merely because its words name Phoenix or its displayed author resembles the owner. For reactions, the triggering actor is the reacting user, independently of the source message's author; an app-authored source MAY supply briefing context but SHALL NOT become an owner-authored explicit instruction.

An incomplete or ambiguous reactor list SHALL remain unverified rather than establish the owner's absence. A reaction-dependent candidate SHALL retain its discovery page for verified retry. A separately verified owner-authored fire prefix SHALL remain sufficient without reaction evidence, independently of the query that found the source.

**Rationale:** An unrelated Slack participant must not gain execution authority over the owner's machine and credentials.

### REQ-SG-002 — Recognize three explicit gestures

WHEN the owner reacts with `:fire:` to another person's message, THE SYSTEM SHALL prepare a private briefing about that message and its discussion.

WHEN the owner posts a message whose first non-whitespace content is the fire emoji or its Slack `:fire:` representation, THE SYSTEM SHALL treat the remaining message as an explicit request. The word `phoenix` SHALL NOT be required for recognition.

WHEN the owner reacts with `:fire:` to their own eligible message, THE SYSTEM SHALL treat that message as an explicit request without requiring a prefix.

THE SYSTEM SHALL NOT treat an untriggered follow-up, an ordinary mention of Phoenix, quoted trigger text, or another person's reaction as an invocation. An empty explicit request SHALL receive an explanation rather than inferred work.

**Rationale:** The owner can flag context or submit an instruction using deliberate, recognizable actions without automatic participation in channel chatter.

### REQ-SG-003 — Discover within a four-calendar-day window

THE SYSTEM SHALL discover eligible messages in channels, private channels, direct messages, and threads accessible through the configured connection. The source-message date SHALL be today or one of the previous three calendar dates in the configured owner's timezone; a date outside this window SHALL NOT become eligible because its reaction is recent.

WHEN service resumes after downtime, THE SYSTEM SHALL discover still-present, unhandled eligible triggers within that window. A newly added reaction on an eligible older message SHALL remain discoverable independently of the last discovery time. Discovery SHALL follow available result pages without treating an incomplete search as proof that no further triggers exist.

THE SYSTEM SHALL NOT promise discovery of removed-before-observation reactions, deleted or inaccessible messages, or source messages outside the window. Window expiry SHALL NOT cancel accepted work.

Definitive source unavailability SHALL require structured rejection evidence for the relevant source-read operation. An opaque error token, unqualified envelope or unrelated lookup failure SHALL retain the candidate and page for retry.

**Rationale:** Friday's messages remain eligible on Monday without requiring an unbounded history scan or a false event-replay guarantee.

### REQ-SG-004 — Establish a baseline without swallowing catch-up

WHEN the gateway is first enabled for an owner and workspace, THE SYSTEM SHALL record matching pre-existing triggers as a baseline without executing them. THE SYSTEM SHALL show initialization as inactive and baseline all matches found during that scan, including gestures made during initialization. Admission SHALL begin only after the baseline is complete and the gateway reports ready; a failed or partial baseline SHALL remain visibly incomplete. Gestures made before ready MAY be absorbed by the baseline.

WHEN the gateway restarts with retained discovery state, THE SYSTEM SHALL preserve the baseline and handled identities rather than establish a new baseline that discards missed requests.

WHEN initialization has retained a search boundary but is not complete, THE SYSTEM SHALL refuse timezone changes until it completes. Pausing discovery or repairing the connection without changing that timezone SHALL remain available.

**Rationale:** Social reactions from before enablement must not unexpectedly start agents, while routine restarts must not lose requested work.

### REQ-SG-005 — Accept one immutable request per source message

WHEN a verified trigger is accepted, THE SYSTEM SHALL durably retain its workspace, owner, source channel, exact message identity, thread root, trigger classification, source instruction snapshot, return destination, stable admission identity, and obligation to report a result or failure before acknowledging it.

WHEN discovery repeats the same source message, including both a prefix and an owner reaction, THE SYSTEM SHALL reuse the accepted request rather than create another conversation, turn, or reply obligation. Removing and re-adding a reaction SHALL NOT constitute a new request for an accepted source. Later source edits SHALL NOT silently change an accepted instruction; a different instruction requires a new source message.

WHEN an owner or workspace change would leave another account's accepted work or delivery obligations outstanding, THE SYSTEM SHALL refuse that change until those obligations settle. Disabling discovery or repairing the same account's connection SHALL preserve and continue accepted work.

A briefing's confirmed needs-input notice SHALL NOT establish terminal lifecycle evidence. An account change SHALL remain blocked until creation/admission rejection or a captured terminal briefing occurrence has its corresponding return settled.

**Rationale:** Polling, overlapping gestures, edits, and restarts must not execute the same request twice or change what was accepted.

### REQ-SG-006 — Acknowledge durable acceptance with eyes only

WHEN request acceptance is durable, THE SYSTEM SHALL attempt to add `:eyes:` to the triggering source message. THE SYSTEM SHALL NOT add it before acceptance is durable or use it to claim that work is running, complete, or delivered.

WHEN adding eyes fails ambiguously or transiently, THE SYSTEM SHALL retain the accepted request and retry the acknowledgment independently of execution. A structured add-reaction rejection establishing source deletion or inaccessibility SHALL instead persist a visible terminal acknowledgment failure and stop those retries. This settles only the acknowledgment for account-switch checks; native work and return obligations SHALL remain intact. Removing fire after acceptance SHALL NOT cancel the request. Gateway progress SHALL NOT add completion, waiting, or failure reactions.

An already-present acknowledgment SHALL require structured rejection evidence identifying the add-reaction operation. An unqualified error token SHALL NOT confirm the acknowledgment.

**Rationale:** One small acknowledgment tells the owner when they can remove their discovery gesture safely.

### REQ-SG-007 — Separate private preparation from Slack conversations

WHEN a briefing request is accepted, THE SYSTEM SHALL associate it with a new private ordinary Phoenix ProductConversation distinct from explicit Slack conversations, even when their source Slack thread is the same.

WHEN an explicit request is accepted, THE SYSTEM SHALL associate it with the ordinary ProductConversation bound to that workspace, owner, channel, and root Slack thread, creating one when no binding exists. Concurrent first requests and retries SHALL converge on one such binding. A top-level message SHALL serve as its own thread root.

THE SYSTEM SHALL retain the stable product association across transcript continuation without redirecting exact source evidence to a different source message. Private Phoenix follow-ups in an explicit Slack-bound conversation SHALL remain part of the context reused by its next explicit Slack request; those follow-ups SHALL NOT acquire automatic Slack publication obligations.

**Rationale:** A private briefing must not expose the owner's exploration by silently becoming the context for a publicly answered request.

### REQ-SG-008 — Start ordinary conversations at home

WHEN a gateway request requires a new conversation, THE SYSTEM SHALL use an explicit Direct/no-repository creation intent at the Phoenix server user's home directory, even if that directory is a Git checkout, and use ordinary conversation model and tool defaults under the conversation-creation contract. No repository selection SHALL be required to flag a discussion.

THE SYSTEM SHALL NOT select the Global Coordinator, a restricted planning conversation, or a distinct Slack permission mode as the target. Subsequent repository attachment and permission decisions SHALL follow their normal Phoenix contracts.

**Rationale:** Capturing a request must not require choosing its eventual codebase or learning another execution mode.

### REQ-SG-009 — Preserve instructions and context as different facts

THE SYSTEM SHALL preserve gateway-assigned Slack provenance through admission, steering, persisted input, history, model context, and native source presentation. It SHALL NOT relabel the input as unqualified user-facing HTTP input, accept model-provided provenance as authority, or reduce source identity to a prompt prefix alone.

THE SYSTEM SHALL provide an investigation-and-report assignment for briefing and investigative requests. An explicit, unambiguous owner instruction SHALL authorize the requested change under ordinary Phoenix permissions without redundant confirmation. WHEN authorization or scope is ambiguous, THE SYSTEM SHALL instruct the agent to explain the proposed action and seek clarification before the change.

Other messages SHALL supply context rather than independent authorization. The owner's instruction MAY explicitly adopt another message's proposal. A reaction requesting a briefing SHALL NOT authorize executing instructions found inside that briefing's source. These task instructions SHALL NOT be represented as technically enforced read-only tools or a new permission system.

For explicit requests, the agent MAY investigate through the owner's broader configured Slack, code, and telemetry access under ordinary permissions. It SHALL compose an intentional answer suitable for the source-thread audience rather than automatically copy private evidence into that thread. This assignment SHALL NOT introduce a separate permission mode.

**Rationale:** Referenced text can explain what the owner wants without becoming a second source of machine authority.

### REQ-SG-010 — Prepare bounded, inspectable context

WHEN preparing a request, THE SYSTEM SHALL retain the source reference and read relevant source discussion through the owner's connection. It SHALL distinguish the owner's instruction, quoted discussion, attachments, and inaccessible or truncated context, and expose source links and coverage limits in Phoenix. Context gathering SHALL be bounded and SHALL NOT fail silently when access or tool capabilities prevent a complete read.

WHEN a briefing is ready, THE SYSTEM SHALL provide a concise summary, relevant evidence, and likely next steps, then await the owner's direction. It SHALL NOT keep monitoring the source for briefing updates. WHEN the owner explicitly asks to read latest Slack updates, THE SYSTEM SHALL make the retained source usable for a fresh read subject to access.

**Rationale:** Preparation should save the owner effort while remaining honest about what the agent actually read.

### REQ-SG-011 — Reuse durable Phoenix admission

WHEN a request is associated with a target, THE SYSTEM SHALL create or resolve it through shared conversation services and submit the input through ordinary durable turn admission with its stable identity. Accepted, already-admitted, queued-as-steering, and rejected outcomes SHALL remain distinguishable.

WHEN interrupted after creation or turn acceptance, THE SYSTEM SHALL reconcile the same creation and turn identities rather than submit replacement work. An acknowledgment, context-read, or return-delivery failure SHALL NOT rerun an accepted turn. Only same-version restart with retained feature state is guaranteed; no mixed-version writer, downgrade, database replacement, or cross-host recovery guarantee is added beyond the compatibility contract.

WHEN the gateway schema is introduced, THE SYSTEM SHALL preserve existing conversation, message, steering, and turn data and attribution, add gateway state without enabling discovery, and retain the existing bounded production-deployment qualification gate. A new feature migration SHALL NOT silently qualify a newer production activation target.

**Rationale:** Slack must not become a second conversation runtime or create duplicate work at crash boundaries.

### REQ-SG-012 — Reply to each explicit request

WHEN an explicit Slack-origin request is accepted, THE SYSTEM SHALL durably owe an intentional response in the source root thread. An answer, clarification, admission rejection, cancellation, terminal runtime failure, or completion without an answer SHALL produce a request-bound answer or explanation, or an explicitly visible delivery failure. The response SHALL be correlated with that request's accepted identity and, when admission occurred, its exact native input and outcome rather than inferred from whichever conversation message is newest. No inferred decision about whether a reply is deserved SHALL suppress this obligation.

WHEN several explicit requests in the same bound thread overlap, THE SYSTEM MAY provide one intentional reply covering multiple requests. It SHALL durably record every covered accepted request and its exact admitted input/outcome evidence; message order or thread membership alone SHALL NOT imply coverage. A confirmed shared reply SHALL satisfy the corresponding response obligations; failed or uncertain delivery SHALL remain visible for every covered request. Requests omitted from the coverage set SHALL retain their own obligations.

WHEN one native execution consumes a queued batch containing several Slack inputs, completion, failure and question evidence SHALL account for every consumed Slack input rather than only the first batch message. Unconsumed queued inputs SHALL retain separate obligations.

WHEN clarification is required, THE SYSTEM SHALL post it in that thread, durably associate that clarification with the exact native question identity, and direct the owner to answer through a deep link to the ordinary Phoenix conversation. The confirmed clarification handoff SHALL satisfy the request's automatic Slack response obligation; subsequent native discussion SHALL remain private unless separately authorized for publication by a new explicit request. Slack triggers SHALL remain ordinary requests and SHALL NOT answer native questions. WHEN the native conversation cannot accept a new request while awaiting an answer, THE SYSTEM SHALL explain that the owner must answer in Phoenix without changing the pending question or treating the new request as an answer. Persisted question associations SHALL NOT authorize unsubmitted Slack answers.

THE SYSTEM SHALL include a Phoenix deep link in gateway-generated clarifications and explanations: the stable conversation route when one exists, otherwise the specific return's status route. For source-thread returns, the gateway SHALL durably freeze that complete outgoing text, including the link, when creating its return obligation and reuse it for a known-unsent retry. The assignment SHALL encourage the agent to link its Phoenix conversation when useful in an intentional answer.

Owner-facing status SHALL distinguish a reserved product identity from a native-route-eligible published conversation and SHALL offer a conversation link only while native routing can resolve it. Pending, failed or deleted targets SHALL remain inspectable through gateway status.

WHEN a native question has been answered, replaced or become unavailable before its message-post attempt, THE SYSTEM SHALL retain the request's response duty and create a generic linked handoff explanation rather than publish the stale question or private follow-up content. A materialized pending or known-unsent clarification SHALL be retired and replaced by a distinct return carrying the same source, coverage and destination; the original frozen text SHALL remain intact. This replacement SHALL NOT resend or replace an attempting, confirmed or uncertain external operation.

**Rationale:** An explicit Slack invocation should not disappear into an agent that silently decides to say nothing.

### REQ-SG-013 — Publish only request-bound output

THE SYSTEM SHALL publish the answer, clarification, or failure selected for the Slack-origin request and SHALL NOT mirror tool transcripts, reasoning, private briefing conversations, or unrelated native Phoenix turns. A native Phoenix follow-up SHALL NOT automatically acquire the prior Slack request's return obligation. Thread association alone SHALL NOT authorize publishing the entire conversation.

THE SYSTEM SHALL instruct the agent to return the invocation's automatic answer and clarifications through the gateway-owned return path rather than independently post them with Slack tools. A separately and explicitly requested Slack side effect SHALL remain governed by ordinary task authorization and permissions; this assignment-level division of reply ownership SHALL NOT introduce tool restrictions or a new permission mode.

THE SYSTEM SHALL retain each output's exact native source and Slack destination, distinguish queued, confirmed, failed, and uncertain delivery, and preserve output obligations across same-version restart. A known-unsent operation MAY be retried within a bounded retry policy. An operation whose external outcome is unknown SHALL be reconciled or exposed as uncertain rather than blindly resent or declared delivered. An unavailable destination SHALL NOT be replaced by a different public channel.

Thread-delivery evidence SHALL identify exactly one source root. Duplicate or conflicting root parameters SHALL NOT confirm delivery.

Terminal destination rejection SHALL require structured rejection evidence for the message-post operation with no accepted-message identity. An opaque destination-error token or a failure from a later lookup SHALL remain uncertain. An external attempt whose final persistence write fails SHALL be reconciled on a later serialized service pass without requiring a restart or repeating the external operation.

An explicit structured send rejection proving rate-limit refusal MAY establish known-unsent status and SHALL retain its retry delay. The evidence SHALL identify rejection of the message-post operation itself. A transport-level rate error, opaque tool error, unqualified operation failure, or failure after a possible accepted post SHALL NOT establish that proof.

**Rationale:** The owner needs intentional replies and truthful delivery state, not accidental transcript publication or duplicate posts after a lost network response.

### REQ-SG-014 — Respect unavailable conversations and sources

WHEN an accepted request cannot be fulfilled because its source or target is unavailable, THE SYSTEM SHALL preserve the request and report the concrete reason through its available native and private notification surfaces. It SHALL NOT silently resurrect a closed conversation, redirect a binding to an unrelated conversation, or discard accepted work as if it had never been requested.

Deleting the last live primary transcript of a bound conversation SHALL atomically capture a terminal cancellation for unfinished admitted requests. Recoverable native completion evidence SHALL be materialized before removing its transcript or execution checkpoint. A remaining live successor SHALL retain its work; deleting already-completed work SHALL NOT generate another terminal notice. Required notices and uncertain external attempts SHALL remain outstanding until settled before an account change.

WHEN the configured Slack transport forbids replying to an external shared channel, THE SYSTEM SHALL expose that limitation and SHALL NOT claim to have replied there. A transport failure SHALL NOT be reported as a completed investigation or successful delivery.

**Rationale:** Lifecycle boundaries and access failures must remain visible rather than producing misleading success or hidden replacement work.

### REQ-SG-015 — Deliver private notices through an owner-configured command

WHEN a briefing is ready, requires owner input, or fails during creation, admission, or execution, THE SYSTEM SHALL submit a private notification with its Phoenix conversation link or, before publication, its exact request-status link through an owner-configured local command. The command SHALL deliver only to the configured owner. Notifications SHALL contain a short readiness, needs-input, or failure message and the relevant link rather than private investigation content or credentials.

Each captured briefing occurrence SHALL retain its own notification obligation; a prior needs-input notice SHALL NOT suppress a later captured ready or failure occurrence.

THE SYSTEM SHALL pass notification content through a versioned input protocol rather than interpolate it into command arguments. Provider authentication SHALL remain the local command's responsibility; supplied credentials SHALL NOT be persisted in gateway settings. Before submission, the command and destination identity SHALL be frozen with the attempt. Later inspection SHALL use that frozen command and verify the exact notification, owner, and submission receipt. The system SHALL distinguish submission from confirmed delivery and SHALL NOT claim that confirmation proves a desktop or mobile notification was displayed. Missing configuration, authentication failure, throttling, and uncertain notification outcomes SHALL remain visible in Phoenix without rerunning the agent.

The first private notification attempt SHALL atomically persist its exact program, arguments and Phoenix target URL before launch. Known-unsent retries and same-version restart SHALL reuse that operation despite later settings or publication changes, including removal of the current command. Historical definitely-unsent notices lacking their original URL SHALL fail visibly rather than reconstruct a destination from current settings; existing submitted receipts SHALL remain inspectable with their retained command.

Submitted-notification inspection SHALL reserve a persisted next-inspection deadline before invoking the local adapter. The initial inspection MAY occur on the next service opportunity; pending results and inspection errors SHALL defer the next inspection by at least thirty seconds after completion. The deadline SHALL survive same-version restart and remain scoped to the exact return and submission receipt. Frequent local recovery passes SHALL NOT bypass it. Terminal notification outcomes SHALL stop inspection.

The pre-call reservation SHALL cover the adapter's maximum call duration plus its completion cooldown, including when the completion write fails. Ready thread replies SHALL receive their dispatch opportunity before private notification commands. A serialized reconciliation opportunity SHALL start at most one private command, whether submission or inspection. Definitely-unsent private submissions SHALL defer retry by at least thirty seconds after completion, retained across restart. Deferred attempts SHALL NOT prevent other due notices from progressing. A stream of fresh pending notices SHALL NOT indefinitely outrank overdue retries or receipt inspections; fresh notices SHALL enter due ordering at their retained creation time.

**Rationale:** A provider-neutral receipt can notify the owner without exposing private findings or coupling the gateway to a particular notification service.

### REQ-SG-016 — Keep the native experience inspectable

THE SYSTEM SHALL expose gateway-created work as ordinary Phoenix conversations with source Slack links, the accepted request, and a private-briefing or source-thread reply destination. It SHALL show request acceptance, admission, output, and delivery failures without requiring a separate Slack inbox or dedicated agent runtime.

Conversation links SHALL use the native ordinary-route eligibility boundary, including completed publication. A reserved identity or physical product row during pending or failed initial delivery SHALL NOT imply a routable conversation. Until eligible, generated handoff links SHALL target the exact return status.

THE SYSTEM SHALL use shared internal application services for conversation creation, target resolution, attributed admission, and outcome reads, extracting shared operations from HTTP handlers where necessary. It SHALL reuse the configured MCP client for Slack operations and SHALL NOT duplicate permission, execution, or conversation lifecycle ownership in the gateway.

Owner-facing setup SHALL use Phoenix's light/dark themes and remain usable at narrow widths. Private notification configuration SHALL be summarized separately from its launcher's name; advanced editing SHALL expose the complete program-and-arguments array, preserve exact argument values, and explain which existing retries retain their original operation.

**Rationale:** The owner can understand and continue the work through the native conversation interface while adapters remain replaceable.

### REQ-SG-017 — Bound discovery load and expose its limits

THE SYSTEM SHALL expose whether discovery is enabled, baselining, healthy, awaiting authentication, backing off, or failed, and the last completed scan time, calls, pagination, and throttling. It SHALL distinguish prefix-message age at first observation from first-observation-to-acceptance delay, and SHALL report reaction-add-to-discovery delay as unavailable when the source has no reaction occurrence timestamp rather than use the source message's age as that measurement. Discovery SHALL use a finite call budget, honor server-directed retry delays, and avoid overlapping scans that multiply load.

THE SYSTEM SHALL NOT promise a fixed discovery latency or complete event capture from search. Partial scans and throttling SHALL NOT advance completion evidence or handled identities for unseen triggers. Disabling discovery SHALL stop new acceptance without cancelling ordinary Phoenix work already accepted.

Ready responses SHALL be reconciled independently of waiting for the next discovery interval. Their Slack operations SHALL share the gateway's finite call allowance and persisted retry deadline with discovery. Previously recorded source identities, including baseline identities, SHALL be skipped before fetching additional source context. Pagination SHALL preserve progress for both trigger searches without allowing one search to starve the other.

The gateway's owner-facing read-access inspection SHALL share that allowance, serialization and persisted retry deadline, without accepting triggers or advancing discovery. Direct agent or manual MCP tool calls are outside this gateway allowance.

Inspection SHALL report sampled hits and verified sources for each query, and distinguish successful search from demonstrated source and reaction reads. Empty or unverified samples SHALL be inconclusive about read access. Accepted sources SHALL retain verified prefix/reaction provenance independently of request kind; historical unknown provenance SHALL NOT be inferred from message age or used for prefix-age metrics.

Ready return delivery SHALL receive an opportunity before acknowledgment retries can exhaust allowance or introduce a new provider delay. Retry deadlines SHALL apply to the configured owner/workspace, rather than another retained account. Non-throttling discovery failures SHALL be reported as failed rather than provider backoff. A startup recovery failure SHALL remain visible and be retried without requiring a Phoenix restart; no external work SHALL begin before that recovery succeeds.

Candidate first-observation time SHALL be persisted by workspace, owner, channel and message identity before source verification. A transient verification failure SHALL preserve that time across retries and same-version restart; acceptance SHALL transfer it into the immutable request. This metadata SHALL NOT authorize a request or mark the source handled. Acceptance or a handled ineligible observation SHALL retire the candidate metadata. Metadata older than seven days MAY expire because its source is outside the four-calendar-date eligibility window.

WHEN initialization is complete and the saved prefix search has advanced, THE SYSTEM SHALL offer a bounded newest-prefix-page check independently of that saved position, subject to the shared allowance, backoff and discovery time bounds. It SHALL preserve capacity for catch-up and accepted work, alternate which class receives the first opportunity, and use the same source-verification and immutable acceptance boundary for both paths. A freshness observation SHALL NOT advance saved positions, establish full-window completion, or bypass initialization. This check SHALL NOT promise that all newly indexed messages are observed on that opportunity; full catch-up remains responsible for the rest of the window.

**Rationale:** The owner needs evidence that the gateway is watching without exhausting the same Slack access used for investigations.

## Related contracts

- [Conversation creation](../conversation-creation/requirements.md): ordinary creation, defaults, and reconciliation.
- [Global recall](../global-recall/requirements.md), REQ-GR-014: trusted input attribution.
- [MCP](../mcp/requirements.md): connection ownership and tool execution.
- [Ask user question](../ask-user-question/requirements.md), REQ-AUQ-009: exact pending-question authority.
- [Compatibility](../compatibility/requirements.md): feature-scoped recovery promises.
