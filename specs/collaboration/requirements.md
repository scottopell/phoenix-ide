# Native Collaboration

## Product thesis

Phoenix share mode becomes the live multiplayer surface for humans working with
one LLM agent. A shared Phoenix conversation is a shared cockpit where any
participant with the share link can follow the live transcript, send steering
messages, answer prompts, and approve agent decisions. Phoenix records who did
what and preserves a single authoritative event order.

Native collaboration favors real co-driving over strict turn-taking. Multiple
participants may steer at once; Phoenix serializes accepted actions at the server
boundary and makes queued human intent visible in the transcript.

## User Stories

### Story 1: Co-Drive From the Share Link

As a developer pairing with coworkers, I want the shared Phoenix URL to become
the place where we all co-drive the same agent session so that nobody has to
screen-share one browser tab or copy suggestions through the owner.

### Story 2: Let Collaboration Be Messy

As a collaborator, I want to send my own instruction when I have useful context,
even if another participant is also steering, so that Phoenix supports energetic
pairing and group debugging rather than forcing a meeting protocol.

### Story 3: Preserve Accountability

As a reviewer of a collaborative conversation, I want human messages and
approvals to identify their contributor so that the transcript explains who made
each steering decision.

### Story 4: Support Any Practical Group Size

As a conversation owner, I do not want Phoenix to impose an arbitrary participant
limit. Most sessions involve a pair or trio, but demos, incident rooms, and team
reviews may have more active participants.

## Requirements

### REQ-COLLAB-001: Share Link Grants Live Co-Driving

WHEN a participant opens a valid live share URL
THE SYSTEM SHALL show the conversation history and live updates
AND SHALL expose the co-driving controls that let the participant send human
instructions and answer or approve agent prompts

THE SYSTEM SHALL treat the live share URL as the collaboration entry point rather
than creating a separate co-driver mode beside share mode

**Rationale:** Multiplayer should evolve the feature users already understand:
share this conversation. A separate co-driver mode would split the journey and
make the main collaborative path feel bolted on.

---

### REQ-COLLAB-002: Contributor Identity on Entry

WHEN a participant joins a live shared conversation
THE SYSTEM SHALL establish a contributor identity for that browser session before
accepting conversation-advancing actions

THE SYSTEM SHALL render contributor identity on accepted human instructions,
prompt answers, approval decisions, and other conversation-advancing human
actions

**Rationale:** A shared link can invite many humans. Phoenix needs enough identity
to make the transcript accountable even when the link itself is the access path.

---

### REQ-COLLAB-003: Multiple Active Drivers

WHILE a live shared conversation is open
THE SYSTEM SHALL allow multiple participants to submit conversation-advancing
actions without requiring a single active driver or baton

THE SYSTEM SHALL NOT impose an arbitrary upper limit on active participants

**Rationale:** Real collaboration can be chaotic. Phoenix should support the
natural flow where several people may send useful context, queue a next step, or
respond to the agent without first negotiating turn ownership in the product.

---

### REQ-COLLAB-004: Server-Ordered Human Actions

WHEN multiple participants submit conversation-advancing actions near the same
time
THE SYSTEM SHALL serialize accepted actions at the server boundary into one
authoritative conversation order

THE SYSTEM SHALL display accepted queued human actions in that order to every
connected participant

**Rationale:** Concurrent steering is acceptable, but the conversation still needs
one durable history. Server ordering avoids client-side disagreement about what
the agent saw and when.

---

### REQ-COLLAB-005: State-Aware Acceptance

WHEN a submitted human action no longer applies to the conversation state where
it was composed
THE SYSTEM SHALL either queue it for the next valid human-input point or reject it
with an explanation visible to the submitting participant

THE SYSTEM SHALL NOT silently drop a participant's submitted instruction or
approval decision

**Rationale:** A messy multi-driver model must be honest. If Phoenix cannot apply
an action safely, the contributor should know whether it is queued or rejected.

---

### REQ-COLLAB-006: Shared Approval Semantics

WHEN the agent is waiting for task approval, prompt answers, continuation, or a
similar human decision
THE SYSTEM SHALL allow any live share participant with an established contributor
identity to submit a decision

THE SYSTEM SHALL accept the first valid decision that reaches the server for a
single-choice prompt
AND SHALL reject later conflicting decisions with a visible explanation

**Rationale:** Shared approval is part of co-driving. For decisions that can only
have one answer, Phoenix should use clear first-valid server acceptance rather
than forcing a driver role.

---

### REQ-COLLAB-007: Runtime Start Follows Co-Driving Authority

WHEN a live share participant submits a conversation-advancing action and the
conversation has no live runtime
THE SYSTEM SHALL start or wake the runtime as needed to process that accepted
action

THE SYSTEM SHALL NOT start or wake the runtime merely because a participant opens
a conversation export or other non-live read-only artifact

**Rationale:** In multiplayer share mode, live participants are allowed to steer
the agent. Passive exported artifacts remain side-effect free.

---

### REQ-COLLAB-008: Awareness Signals Support Steering

THE SYSTEM MAY show presence, typing state, reactions, pointers such as "look
here", and help requests

THE SYSTEM SHALL keep awareness signals separate from conversation-advancing
actions unless a participant explicitly converts a signal into an agent-steering
message or approval decision

**Rationale:** Awareness tools help collaborators coordinate on a call, but they
are supporting signals. They must not become a hidden control plane for the
agent.

---

### REQ-COLLAB-009: Collaboration State Replays After Reconnect

WHEN a participant reconnects to a live shared conversation
THE SYSTEM SHALL restore conversation history, accepted queued actions,
contributor attribution, and durable collaboration metadata needed to continue
co-driving

THE SYSTEM SHALL NOT rely on an SSE connection as the only copy of accepted human
actions or contributor attribution

**Rationale:** Collaboration state affects what the agent sees and how the
transcript is interpreted. It must survive refresh, reconnect, and server restart
semantics.
