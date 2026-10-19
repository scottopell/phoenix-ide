# Durable Conversation Goal Storage

## User story and scope

As a Phoenix user, I need an ordinary conversation's immutable goal and attributed status history to remain inspectable across transcript changes, without an older execution affecting a replacement goal or a later status.

This contract covers goal storage, mutation fencing, attribution consistency and immutable report bindings. It does not grant execution, resource, deployment or merge authority. Automatic follow-through, admission orchestration, Stop integration, release/resume interpretation, ceilings, renewals, waiting and watch delivery are outside this storage contract.

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

THE SYSTEM SHALL distinguish trusted user-input-channel origin, owning-agent origin, Coordinator origin and generated-host origin.

THE SYSTEM SHALL preserve the requester separately from the applier and record the concrete causative input/action identity.

WHEN an agent is recorded as an actor
THE SYSTEM SHALL record its stable conversation, transcript, workflow/turn, assistant message and tool-use locator.

THE SYSTEM SHALL check ordinary-owner versus Coordinator kind and membership consistency and SHALL check that an agent's message contains the recorded typed ToolUse.

THE SYSTEM SHALL treat exact invocation assignment as trusted runtime context rather than infer it from a matching transcript, a model-supplied actor label or arbitrary retrieved text.

THE SYSTEM SHALL NOT describe user-API channel provenance as proof of biological human identity or actor attribution as commission/resource authorization.

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

The storage primitive's ability to insert an immutable observation does not establish when the runtime captured it. Actual turn admission owns atomic capture; an absent binding is never proof that a late current-goal attachment is valid.

### REQ-GOAL-006 — Immutable objective and explicit status transitions

THE SYSTEM SHALL reject objective changes under an existing goal identity and SHALL provide no objective-edit mutation.

THE SYSTEM SHALL distinguish Underway, Blocked, successful Complete and unfinished Exhausted outcomes.

WHEN a Blocked goal is returned to Underway
THE SYSTEM SHALL retain that goal's identity and objective and advance the owner/status version.

THE SYSTEM SHALL keep Complete and Exhausted terminal on their original identity and SHALL reject further status mutations or reopening of either outcome.

THE SYSTEM SHALL distinguish creating a new current identity from modifying or reopening the prior identity.

THE SYSTEM SHALL NOT treat goal status, supersession or a replacement's Underway marker as permission to release a protected user Stop, override security or admit execution.

The dormant Suspended representation supports historical fencing and has no status-release operation. Durable runtime Stop enforcement and authorized admission are separate prerequisites; storage alone does not establish them.

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
