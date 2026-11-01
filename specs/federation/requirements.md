# Federation Requirements

## User need

A Coordinator operating one trusted Phoenix instance needs to use supported coordination tools on another trusted Phoenix instance without losing the destination's ordinary correctness, durable identity, or audit guarantees.

## Scope and related contracts

This specification covers the existing `send_conversation_message` and `read_goal` tools, each extended with an optional `instance` selector, and durable remote conversation watches. Goal mutations (including creation), settings, Continue, conversation creation, and rename are outside this contract.

Related contracts remain authoritative for their domains:

- `specs/conversation-identity/requirements.md` owns stable ProductConversation and exact transcript identity.
- `specs/goal-continuation/requirements.md` owns Goal admission and automatic-continuation prerequisites.
- `specs/global-recall/requirements.md` owns local Coordinator Goal commands.
- `specs/compatibility/requirements.md` owns compatibility and recovery guarantees.
- `specs/remote-query-database/requirements.md` owns remote read-only SQL behavior.
- `specs/tool-contracts/requirements.md` owns shared tool result behavior.

## Requirements

### Peer selection and authority

**REQ-FED-001 — Optional destination selection**

When `send_conversation_message` or `read_goal` receives an `instance` selector, the system shall execute that existing tool operation against the uniquely selected saved peer; when the selector is absent, the system shall preserve the tool's local behavior.

**REQ-FED-002 — No destination fallback**

If peer selection, authentication, identity verification, transport, or remote execution fails after an instance was selected, the system shall report that failure and shall not execute the operation locally or against another peer.

**REQ-FED-003 — Destination-owned admission**

When a trusted peer requests an operation, the destination shall authenticate the peer and apply the same domain validation, lifecycle, and concurrency rules used for the equivalent local operation.

**REQ-FED-004 — Authenticated remote attribution**

When the destination admits a remote message, its durable message record and returned result shall identify the authenticated source instance as origin, distinct from the destination's local Coordinator and human user, so that the attribution is checkable after restart and on replay.

### Remote messages and Goal reads

**REQ-FED-005 — Payload-bound message retry**

When a remote conversation message is retried with the same source instance, message identity, target, and content, the destination shall return the prior durable result without creating another message; if the same identity is reused with changed target or content, the destination shall reject the request without mutation.

**REQ-FED-006 — Restart-safe message reconciliation**

If a remote message result is lost or either instance restarts, retrying the same request shall reconcile the original attempt and shall not execute a duplicate attempt in parallel.

**REQ-FED-007 — Goal read parity**

When a Goal read selects a peer, the destination shall apply the same stable-conversation resolution, version wait, timeout, and result semantics as a local Goal read.

**REQ-FED-008 — Stable and exact conversation identity**

When a remote operation or watch result refers to a conversation, the result shall identify both the stable ProductConversation and the exact transcript in which the reported fact or mutation occurred.

### Durable watches

**REQ-FED-009 — Durable remote watch**

When a trusted peer watches a ProductConversation, the system shall preserve the watch, its ordered event cursor, and unacknowledged delivery state across restarts of either instance.

**REQ-FED-010 — Continuation-following watch**

When a watched ProductConversation continues into a successor transcript, the existing watch shall remain attached to the stable ProductConversation and each subsequent event shall identify its exact transcript.

**REQ-FED-011 — Admission-before-acknowledgement**

When a watching instance receives a remote event, it shall acknowledge that event only after the event has been durably deduplicated and its Coordinator notification has been durably admitted locally.

**REQ-FED-012 — Replay and deduplication**

When an event is redelivered after a lost response or restart, the watching instance shall deduplicate it by durable subscription event identity and shall not admit a second Coordinator notification.

**REQ-FED-013 — Bounded backlog and explicit gap**

If a remote watch exceeds a destination-configured unacknowledged event, unacknowledged byte, or continuously disconnected-age bound, the destination shall terminally fail the watch, preserve a durable gap/close tombstone, and require an explicit current-state read and new subscription rather than retain an unbounded backlog or silently skip events.

**REQ-FED-014 — Ordered unsubscribe**

When a watcher unsubscribes, the system shall durably order the close after already acknowledged events, suppress later delivery, and make repeated unsubscribe requests return the same terminal outcome.

**REQ-FED-015 — Revocation admission boundary**

Once peer revocation commits, the destination shall admit no new operation, watch, pull, or acknowledgement authenticated as that peer; operations admitted before the commit may finish and shall remain auditable.

**REQ-FED-016 — Revocation is not unsubscribe**

When peer trust is revoked, the system shall terminally mark that peer's watches as revoked without requiring a successful unsubscribe exchange or acknowledgement from the revoked peer.

### Complexity boundaries

**REQ-FED-017 — Domain retry ownership**

The system shall use each domain operation's sufficient durable identity and journal for payload-bound retry rather than route all remote mutations through a universal idempotency framework.

**REQ-FED-018 — No global remote-control lease**

The system shall resolve concurrent local and remote operations through destination-owned domain transactions and preconditions rather than an exclusive global remote-control lock.

## Non-goals

- A universal cross-domain idempotency framework.
- An exclusive remote-control lease.
- A combined enable-automatic-continuation-and-create-Goal operation.
- Implicit creation of reverse enrollment, credentials, or trust.
- Remote Bash or general process control.
- Silent remote-to-local fallback.
- Treating peer notes as authorization or as a substitute for destination validation.
