# Native Collaboration

## Product thesis

Phoenix share mode is the live multiplayer surface for humans working with one
LLM agent. Anyone who joins a live shared conversation can see the same history,
submit messages, and answer agent prompts. Phoenix attributes each human action
and preserves one durable server-defined order.

Collaboration does not create a runtime, tool actor, or work environment per
participant. Participants submit actions to one conversation; that conversation
continues to own agent execution and its existing work environment.

## User Stories

### Story 1: Work Together From the Shared URL

As a developer pairing with coworkers, I want the shared Phoenix URL to be where
we all work with the same agent so that nobody must screen-share one browser or
relay suggestions through its owner.

### Story 2: Contribute Without Turn-Taking Ceremony

As a collaborator, I want to send useful context even when another participant is
also steering so that Phoenix supports natural group work rather than imposing a
baton or active-driver role.

### Story 3: Know Who Did What

As a participant or later reviewer, I want every human message and decision to
identify its contributor so that the conversation remains understandable and
accountable.

## Requirements

### REQ-COLLAB-001: Live Share Is the Multiplayer Entry Point

WHEN a participant opens a valid live share URL
THE SYSTEM SHALL show conversation history and live updates
AND SHALL allow the participant to establish a contributor identity
AND, after identity is established, SHALL expose message and pending-decision
controls supported by the shared conversation

THE SYSTEM SHALL NOT require a separate co-driver mode, driver role, or baton

**Rationale:** Multiplayer should evolve the share journey users already
understand. Collaboration is a property of the live shared conversation, not a
second product beside it.

---

### REQ-COLLAB-002: Durable Contributor Identity

WHEN a participant establishes an identity in a live shared conversation
THE SYSTEM SHALL issue or recognize a stable opaque contributor identifier
AND SHALL associate a display label with that identifier
AND SHALL bind subsequent shared mutations to that contributor

THE SYSTEM SHALL NOT use a display label, browser user-agent string, IP address,
or SSE connection as contributor identity

**Rationale:** Labels can collide or change, network connections are temporary,
and user-agent strings identify software rather than people. Durable attribution
needs a stable identifier distinct from presentation.

---

### REQ-COLLAB-003: Attributed Durable Message Submission

WHEN a live share participant submits a message
THE SYSTEM SHALL durably accept, replay, conflict, queue, cancel, reconcile, and
materialize that message through the same conversation message-submission
contract used by the owner
AND SHALL preserve the contributor identifier with the accepted human turn and
its materialized transcript message

WHEN multiple participants submit messages near the same time
THE SYSTEM SHALL allow each distinct valid message to be accepted
AND SHALL expose the server-defined accepted or queued outcome for each message
AND SHALL NOT require one participant to become the active driver

**Rationale:** Message identity, ordering, retry, queueing, and crash recovery are
conversation concerns. Collaboration adds provenance and another authorized HTTP
entry point; it does not need a second queue or delivery protocol.

---

### REQ-COLLAB-004: Attributed Single-Choice Decisions

WHEN the agent is waiting for a task decision, question response, continuation,
or another single-choice human decision
THE SYSTEM SHALL allow any identified live share participant to submit a valid
decision
AND SHALL atomically accept at most one decision for that pending decision
AND SHALL persist the winning contributor identifier with the decision
AND SHALL reject later conflicting submissions with a visible explanation

WHEN the winning submission is retried
THE SYSTEM SHALL replay the accepted result without applying the decision again

**Rationale:** Messages can queue, but a one-shot decision has one winner. Atomic
acceptance and contributor attribution make simultaneous human decisions
unambiguous and retry-safe.

---

### REQ-COLLAB-005: Share Authority Is Conversation-Scoped

WHERE a request presents a valid live share token and established contributor
identity
THE SYSTEM SHALL authorize only the supported shared-conversation mutations for
the conversation named by that token

THE SYSTEM SHALL NOT turn a share participant into a runtime actor, tool actor,
resource owner, or holder of work-environment authority
AND SHALL NOT expose owner-only lifecycle, settings, resource, filesystem,
terminal, browser, or repository controls through share authority

WHEN the owner revokes the live share token
THE SYSTEM SHALL reject subsequent reads and mutations authorized by that token

**Rationale:** Collaborators steer one existing conversation. The conversation's
runtime retains tool and work-environment authority; possession of a shared URL
does not grant direct control of the underlying resources.

---

### REQ-COLLAB-006: Reconnect Restores Authoritative Outcomes

WHEN a live share participant reconnects after an ambiguous submission outcome
THE SYSTEM SHALL reconcile submitted message identities against durable
acceptance independently of transcript visibility
AND SHALL restore materialized messages, queued-message outcomes, and contributor
attribution from durable state

THE SYSTEM SHALL NOT treat SSE receipt or absence as acceptance authority

**Rationale:** A disconnect can happen after server acceptance but before the
browser sees a response. Reconnect must converge on durable server state without
duplicating or losing a participant's action.
