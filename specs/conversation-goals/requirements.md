# Durable Conversation Goal Storage

## User story and scope

As a Phoenix user, I need an ordinary conversation's immutable goal and attributed status history to remain inspectable across transcript changes, without an older execution affecting a replacement goal or a later status.

This contract covers goal storage, mutation fencing, attribution consistency, immutable report bindings, settled-boundary application admission and bounded local watch facts. It does not grant execution, resource, deployment or merge authority. Automatic follow-through, changes to independent Stop/release semantics, ceilings, renewals, standalone nonboundary-report notifications and remote goal-change fanout are outside this contract. Structured waiting remains an execution fact, not a goal disposition.

## Requirements

### REQ-GOAL-001 — Stable ordinary ownership

THE SYSTEM SHALL associate a goal with an ordinary stable ProductConversation, independently of transcript, model, provider or WorkScope identity.

THE SYSTEM SHALL reject a Coordinator ProductConversation as a goal owner.

THE SYSTEM SHALL expose at most one current goal identity for an ordinary owner and retain ordered status and supersession history while that owner exists.

THE SYSTEM SHALL store one immutable objective per goal identity.

WHEN a new objective is set
THE SYSTEM SHALL create a new goal identity and atomically make it current without requiring a clear or edit step.

THE SYSTEM SHALL retain prior goal identities and outcomes without representing supersession as successful completion or reopening a terminal identity.

WHEN the owner has no goal history
THE SYSTEM SHALL represent that absence without creating a goal or changing ordinary execution behavior.

### REQ-GOAL-002 — All-writer observed-version fencing

WHEN any actor requests a goal mutation
THE SYSTEM SHALL compare the supplied observed owner version with durable current state before accepting the mutation.

THE SYSTEM SHALL apply the same fence to user-facing, owning-agent, Coordinator and generated-host writes without a privileged last-writer-wins path.

WHEN competing mutations name the same observed version
THE SYSTEM SHALL accept at most one distinct mutation and return an explicit conflict for another.

THE SYSTEM SHALL advance one owner version for every accepted status change or new-identity supersession, and SHALL preserve its monotonicity across goal identities.

THE SYSTEM SHALL reject reuse of a historical goal identity for a new goal.

### REQ-GOAL-003 — Atomic immutable history and retry identity

THE SYSTEM SHALL persist a mutation's resulting state, exact attribution, cause/action identity and timestamp atomically.

THE SYSTEM SHALL retain a lossless ordered history from which both preceding and resulting goal identities/statuses are inspectable, with objective text stored only on the immutable identity row and without an independently writable duplicate current-state representation.

WHEN an accepted action identity is retried with the identical owner, expected version, actors and change payload
THE SYSTEM SHALL return the original accepted result without appending another change or replacing the original timestamp.

WHEN the same action identity is reused with different mutation data
THE SYSTEM SHALL reject the retry.

WHEN any mutation write fails
THE SYSTEM SHALL retain neither a partial state change nor orphaned accepted-attribution rows from that mutation.

### REQ-GOAL-004 — Concrete attribution without authority fabrication

THE SYSTEM SHALL distinguish trusted user-input-channel origin, authenticated owner-control-request origin, owning-agent origin, Coordinator origin and generated-host origin.

THE SYSTEM SHALL preserve the requester separately from the applier and record the concrete causative input/action identity.

WHEN an agent is recorded as an actor
THE SYSTEM SHALL record its stable conversation, transcript, workflow/turn, assistant message and tool-use locator.

THE SYSTEM SHALL check ordinary-owner versus Coordinator kind and membership consistency and SHALL check that an agent's message contains the recorded typed ToolUse.

THE SYSTEM SHALL treat exact invocation assignment as trusted runtime context rather than infer it from a matching transcript, a model-supplied actor label or arbitrary retrieved text.

THE SYSTEM SHALL NOT describe user-API channel provenance as proof of biological human identity or actor attribution as commission/resource authorization.

#### REQ-GOAL-004.1 — Request-backed owner controls

WHEN a goal mutation originates from an authenticated owner control without a chat message
THE SYSTEM SHALL attribute it to the actual accepted goal command request, not a fabricated message or a principal claimed by the caller.

THE SYSTEM SHALL authenticate and authorize the current ordinary, open owner at the application boundary before looking up retained request results, without treating attribution evidence as authorization.

THE SYSTEM SHALL require agreement between the accepted request and mutation on owner, request identity, command, expected/result versions and goal identity, and SHALL commit them atomically.

THE SYSTEM SHALL NOT use an accepted Stop or release request as evidence of an accepted goal command.

WHEN a control request is retried with identical command input
THE SYSTEM SHALL retain its original result, and SHALL reject changed input at the same request identity.

THE SYSTEM SHALL apply settled-boundary admission to a new owner goal command without clearing the independent Stop latch.

### REQ-GOAL-005 — Immutable execution observation and stale reports

THE SYSTEM SHALL represent an execution's goal observation as either an explicit no-goal observation at an owner version or a binding to the exact stable owner, immutable goal identity and observed owner/status version.

THE SYSTEM SHALL retain the execution's workflow/turn identity and immutable observation without replacing it from a later current-goal lookup.

WHEN a report requests a state change
THE SYSTEM SHALL require an identical persisted goal-bearing execution binding, matching report source execution and matching current goal identity and owner/status version.

THE SYSTEM SHALL reject an old report after supersession even when the replacement has the same objective text.

THE SYSTEM SHALL reject stale, cross-owner, unrelated-execution, unbound and conflicting-binding reports without applying their state changes.

THE SYSTEM SHALL reject a goal-bearing report binding to an inactive goal state.

WHEN a stored observation is no-goal
THE SYSTEM SHALL NOT convert that observation into a later goal-bearing binding.

WHEN an ordinary Runtime turn is accepted
THE SYSTEM SHALL atomically capture its immutable observation with the accepted turn and workflow, before message materialization or any model request.

THE SYSTEM SHALL bind an Underway goal by its exact fence and SHALL record an explicit no-goal observation at the observed owner version when no goal is Underway.

THE SYSTEM SHALL NOT mint an execution report binding for accepted steering alone or retrofit an absent binding from a current-goal lookup.

WHEN exact accepted-turn replay is requested
THE SYSTEM SHALL reuse the retained observation without recapture.

### REQ-GOAL-006 — Immutable objective and explicit status transitions

THE SYSTEM SHALL reject objective changes under an existing goal identity and SHALL provide no objective-edit mutation.

THE SYSTEM SHALL distinguish Underway, Blocked, successful Complete and unfinished Exhausted outcomes.

WHEN a Blocked goal is returned to Underway
THE SYSTEM SHALL retain that goal's identity and objective and advance the owner/status version.

THE SYSTEM SHALL keep Complete and Exhausted terminal on their original identity and SHALL reject further status mutations or reopening of either outcome.

THE SYSTEM SHALL distinguish creating a new current identity from modifying or reopening the prior identity.

THE SYSTEM SHALL NOT treat goal status, supersession or a replacement's Underway marker as permission to release a protected user Stop, override security or admit execution.

The Suspended representation supports historical fencing and has no status-release operation. Durable runtime Stop enforcement and authorized admission are separate prerequisites; storage alone does not establish them.

THE SYSTEM SHALL NOT infer completion from Idle, final prose, a WorkScope attachment, command dispatch or unrelated successful checks.

THE SYSTEM SHALL store concise objective-specific report reasons without requiring a universal artifact/proof form.

#### Execution-suspension composition boundary

THE SYSTEM SHALL represent pending structured questions and task approvals independently of goal status; awaiting either SHALL NOT by itself change an Underway goal to Blocked.

WHEN a goal consumer encounters pending question or approval suspension
THE SYSTEM SHALL preserve that suspension without admitting a competing goal follow-up.

For the same impediment, the agent either asks and awaits a structured response, or explicitly reports the goal Blocked and finishes; it does not combine both paths. An explicit environmental Blocked report identifies the reason and needed action without secrets. Goal storage does not infer such a report from execution suspension.

A Blocked attention notification uses the existing subscribed watch path and does not grant the Coordinator authority to answer a question or approve a task. Such action requires independently verified standing user scope. These composition constraints do not supply runtime wiring, new watch machinery or a replacement-at-idle policy.

### REQ-GOAL-007 — Retention follows owner deletion

WHEN a source transcript, message or execution is deleted independently of a goal's ordinary owner
THE SYSTEM SHALL preserve recorded goal attribution and execution identities as historical locators without claiming those sources remain live or readable.

WHEN the ordinary ProductConversation is hard-deleted through supported aggregate deletion
THE SYSTEM SHALL remove its goal identities, history, actor rows and execution observations with that owner.

THE SYSTEM SHALL NOT let a terminal or superseded goal or a no-goal-only observation pin ordinary conversation deletion.

THE SYSTEM SHALL reject standalone rewriting or deletion of goal history, actors and observations while their owner exists.

### REQ-GOAL-008 — Relational shape and compatibility boundary

THE SYSTEM SHALL store addressable goal state/history, actor identities and execution bindings in constrained relational rows rather than child collections inside JSON blobs.

THE SYSTEM SHALL store new internal timestamps as checked nonnegative integer Unix microseconds under `specs/compatibility/requirements.md`.

WHEN goal storage is introduced by a forward migration
THE SYSTEM SHALL leave existing conversation, message and execution identities and contents unchanged and SHALL fabricate no historical goals or actors.

THE SYSTEM SHALL NOT infer a downgrade, cross-version replay or general recovery guarantee from this storage contract.

### REQ-GOAL-009 — Settled-boundary mutation admission

WHEN a trusted caller creates or replaces a goal, or requests an unbound disposition change
THE SYSTEM SHALL require the ordinary owner's latest transcript at a settled-turn boundary, rather than infer settlement from an Idle projection alone.

THE SYSTEM SHALL serialize those mutations with existing product-message admission followed by latest-transcript message acceptance, and SHALL recheck the owner, latest transcript, lifecycle admission and durable pending work in the write transaction.

WHILE an admission or acceptance gate is occupied, or an owner has an active or accepted nonterminal turn, queued input, pending structured question or task approval, pending continuation reservation or opening, unfinished automatic continuation admission, unfinished creation delivery, or terminal settlement obligation
THE SYSTEM SHALL refuse a new mutation as busy or unavailable without creating synthetic chat, queuing that mutation or admitting execution.

THE SYSTEM SHALL permit retained terminal automatic-continuation records without treating them as pending work.

THE SYSTEM SHALL apply the same settled-boundary rule to owning-agent create/replace calls; execution does not grant an exemption.

WHEN an owning agent reports against its exact persisted execution binding
THE SYSTEM SHALL recheck that the bound Runtime turn and workflow still own the current transcript without acquiring the message wait gates held by that execution.

THE SYSTEM SHALL preserve exact accepted retries as historical reads without requiring settlement again or repeating the mutation.

THE SYSTEM SHALL expose persisted goal reads without adding ordinary chat or granting execution permission.

### REQ-GOAL-010 — Goal reports finalize with the genuine tool round

WHEN an executing agent prepares a goal report
THE SYSTEM SHALL retain its exact admitted fence, execution, assistant-message and tool-use identity as typed pending data in the existing tool-round checkpoint, not as successful tool output or synthetic source chat.

THE SYSTEM SHALL derive the pending report's owner, expected version and owning-agent execution from one immutable admitted binding, without a separate caller-supplied owner or execution representation.

WHEN the owning tool-round transaction persists the genuine assistant call and its paired results
THE SYSTEM SHALL apply a valid report and finalize its truthful result in that same transaction, including corresponding runtime state and provider replay progress.

WHEN an expected semantic report refusal occurs
THE SYSTEM SHALL roll back that report's mutation, retain unrelated legitimate tool results, and persist a finalized rejection rather than publish success or indefinitely abort the round.

WHEN an unclassified SQL, constraint, I/O or commit failure occurs
THE SYSTEM SHALL preserve the existing checkpoint failure/reconciliation behavior rather than mislabel that failure as an expected semantic refusal.

THE SYSTEM SHALL publish report success or rejection to history, SSE or model context only after its owning checkpoint commits.

WHEN checkpoint recovery or exact retry encounters an already finalized report result
THE SYSTEM SHALL verify the exact pending payload and assistant/tool-use/execution identity and retain the committed outcome, including a rejection whose preconditions have since changed.

THE SYSTEM SHALL NOT treat an existing result row alone as proof of an identical report, or reinterpret a previously committed rejection as success.

### REQ-GOAL-011 — Saved current facts at a local watch occurrence

WHEN explicitly installed goal consumers record a newly inserted eligible local subscription execution-terminal or wait-entry occurrence
THE SYSTEM SHALL capture the ordinary owner's current persisted goal facts in that occurrence's owning transaction, independently of the execution outcome and admission-time report binding.

THE SYSTEM SHALL distinguish four persisted event-time cases: a captured current goal, captured absence of a goal with its observed owner version, capture disabled by explicit installation policy, and legacy historical facts not captured. Disabled and legacy cases SHALL NOT mean NoGoal, and later installation SHALL NOT enrich or reclassify them.

THE SYSTEM SHALL require the existing `GoalControlInstallation` policy from the owning Database or WorkflowRepository at each capture/fanout boundary. Normal constructors SHALL remain Disabled. Schema presence, migration application, current goals, control rows and environment guesses SHALL NOT install a consumer.

WHILE goal consumers are Disabled
THE SYSTEM SHALL retain existing execution/wait notifications with an immutable disabled marker, SHALL NOT read goal SQL or create goal-capture child rows, and SHALL NOT append standalone goal-change events. Standalone fanout SHALL return before querying.

THE SYSTEM SHALL capture only newly inserted eligible local subscription events when Installed, not deduplication conflicts, mandatory Close routes or federation terminal facts.

WHEN a current goal is captured
THE SYSTEM SHALL retain its ordinary owner identity, immutable goal identity and objective, monotonic owner/status version, factual state and exact state reason where present, including inactive states.

THE SYSTEM SHALL NOT restrict saved facts to an Underway execution binding, infer goal completion from execution completion, introduce an objective-revision counter, or reinterpret an old event using a later current goal.

THE SYSTEM SHALL identify usage as explicitly unavailable because it was not captured, rather than fabricate zero, aggregate transcript usage as goal usage, infer budget consumption, or choose a numeric ceiling.

### REQ-GOAL-012 — One causal notification owner

WHEN explicitly installed goal consumers accept a settled Create/State mutation that changes a watched ordinary owner without a target execution boundary
THE SYSTEM SHALL append a distinct local goal-change obligation in the same owning transaction as the accepted mutation and its retained result.

THE SYSTEM SHALL identify this standalone occurrence by ordinary owner, accepted status version and retained watch enrollment identity, SHALL retain the existing subscription delivery identity, and SHALL create no additional occurrence for a rejected mutation, exact accepted retry or re-enrollment replay.

THE SYSTEM SHALL distinguish accepted changes at different owner versions even when their states or objective text match, and SHALL NOT deduplicate distinct genuine execution or wait occurrences by goal version.

THE SYSTEM SHALL assign notification ownership as follows:

| Source sequence | Notification owner | Goal-change obligation |
|---|---|---|
| Report followed by an actual terminal occurrence | The actual execution-terminal transaction, using goal facts saved there | No standalone report obligation |
| Report followed by an actual question or approval wait | The wait-entry transaction, after resolving the report in its owning checkpoint | No standalone report obligation |
| Report with no actual terminal or wait boundary | No additional report-notification guarantee | No standalone report obligation; a later actual boundary captures facts then |
| Accepted settled Create/State on the watched target | The accepted target-mutation transaction | One standalone obligation per accepted owner version and retained enrollment identity |
| Stop followed by an actual Cancelled occurrence | Stop retains its independent control semantics; the actual cancellation transaction owns the execution occurrence | No Stop-induced goal-change or fabricated Suspended occurrence |
| Continuation or context handoff | The existing occurrence and suppression rules under `specs/global-recall/requirements.md` | No synthetic terminal or report obligation |

THE SYSTEM SHALL NOT treat a Coordinator caller's own execution completion as the watched target's execution boundary, or promise a terminal boundary merely because a report was accepted.

THE SYSTEM SHALL preserve mandatory Close routing and federation's terminal-fact protocol, provenance, subscriptions, cursors and delivery semantics. A local goal-change occurrence SHALL NOT masquerade as a federated terminal execution fact.

### REQ-GOAL-013 — Storage failure follows the owning transaction

WHEN saving watch facts or appending a required goal-change obligation fails because of an unclassified SQL, persisted-fact decoding, constraint, I/O or commit error
THE SYSTEM SHALL propagate a storage/checkpoint failure through the owning operation, SHALL NOT publish success or recast the error as semantic report refusal, and SHALL NOT commit a partial owning transaction.

WHEN report resolution, wait entry and notification share a checkpoint transaction
THE SYSTEM SHALL roll back their report changes, tool evidence, state and notification together on such a failure.

WHEN a report checkpoint has committed before a later terminal transaction
THE SYSTEM SHALL roll back the failed terminal transaction while retaining the already committed report and terminal obligation under the existing settlement protocol; this contract SHALL NOT imply atomicity across those transactions.

WHEN a standalone settled mutation's notification append fails
THE SYSTEM SHALL roll back that transaction's mutation, history, actors, request result and notification.

THE SYSTEM SHALL add no watch append or capture dependency to the independent Stop control transaction, SHALL NOT fabricate a Stop recovery or cancellation guarantee, and SHALL NOT undo a committed Stop latch because a later cancellation-event transaction fails.

THE SYSTEM SHALL capture only the relevant source owner's facts without decoding unrelated retained watch payloads. Post-commit delivery failure SHALL retain the existing pending-notification retry behavior rather than roll back committed source facts.

### REQ-GOAL-014 — Event-owned retention and forward upgrade

THE SYSTEM SHALL retain captured goal facts as normalized immutable event-owned data, distinct from the live goal authority, without foreign keys that pin source goal, goal-history, ProductConversation or transcript deletion.

WHEN a captured event remains retained after a source goal change, replacement, continuation or supported source deletion
THE SYSTEM SHALL preserve its saved facts rather than reconstruct them from a current owner or admission binding. Event-owned capture SHALL follow the event's existing retention and deletion lifecycle.

WHEN the local watch schema is extended by a forward migration
THE SYSTEM SHALL preserve existing event identities, payloads, mandatory and subscription routes, wait occurrences, enrollment and delivery identities, pending/accepted/suppressed state, accepted transcript references and continuation state, together with their acceptance, immutability and source-retention constraints and inbound references.

THE SYSTEM SHALL classify pre-capture events as historical facts not captured, SHALL NOT backfill them from current goals, and SHALL leave existing goal, actor, history, binding and control data and already embedded migration identities unchanged.

THE SYSTEM SHALL reuse the existing watch outbox and durable input admission without a second worker, scheduler or startup installation. Saved goal facts, unavailable usage and notification acceptance SHALL NOT grant authority to execute or automatically follow through.
