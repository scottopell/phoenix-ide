# Federation Requirements

## User need

A Coordinator operating one trusted Phoenix instance needs to use supported coordination tools on another trusted Phoenix instance without losing the destination's ordinary correctness, durable identity, or audit guarantees.

## Scope and related contracts

This specification covers optional `instance` selection on `send_conversation_message`, `read_conversation`, `resolve_reference`, and `read_goal`, durable remote conversation watches, and finite settlement of facts durably staged before a terminal watch gap. Goal mutations (including creation), settings, Continue, conversation creation, and rename are outside this contract.

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

When `send_conversation_message`, federated `read_conversation`, `resolve_reference`, or `read_goal` receives the exact, case-sensitive `instance` string `"local"` or omits the selector, the system shall execute the local operation with the same authority checks, validation, result shape, provenance, pagination, waits, and message retry identity.

When one of these tools receives another string selector, the system shall select the remote destination through saved-peer UUID or unambiguous display-name resolution. Recognizing the local selector shall not trim whitespace or fold case. The literal `"local"` shall be reserved for the instance executing the tool.

An explicit `null` selector shall select local routing on the read tools; `send_conversation_message` shall reject an explicit `null`. Tools without an optional `instance` parameter, including remote-only SQL using a required `instance_id` UUID, shall retain their own input and authority contracts.

**REQ-FED-002 — No destination fallback**

If peer selection, authentication, identity verification, transport, or remote execution fails after a remote selector was supplied, the system shall report that failure and shall not execute the operation locally or against another peer.

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

If a remote watch exceeds a destination-configured unacknowledged event, unacknowledged byte, or continuously disconnected-age bound while unacknowledged data exists, the destination shall terminally fail the watch, preserve a durable gap/close tombstone, and require an explicit current-state read and new subscription rather than retain an unbounded backlog or silently skip events.

When a watching instance durably stages that terminal gap, the same transaction shall define the finite pre-gap prefix as exactly the fact admissions already durable for that subscription before the transaction changes the subscription to `failed_gap`. A concurrent fact-staging transaction that commits first contributes to the prefix; one that runs after the terminal transition may only verify an identical already-staged fact and shall otherwise fail without mutation. The watching instance shall admit or replay no new fact under that subscription identity and shall keep every prefix fact eligible for ordered delivery regardless of whether its authorization request had started before the gap. Actual source authorization remains required before delivering each fact.

The watching instance shall process the frozen prefix in staged source-sequence order. If the earliest unsettled staged fact receives source authorization, the watching instance may admit it before processing the next staged fact. A missing source sequence or an explicitly suppressed staged fact shall not make a later staged fact ineligible, and delivery after such a discontinuity shall not falsely advance the contiguous admitted fact cursor. A fact-authorization request already in flight when the gap is staged shall remain eligible to finish and shall consume the first terminal-settlement attempt. After the gap is staged, each outbound request for the earliest unsettled fact shall durably consume one attempt before crossing the network. The terminal-settlement budget shall be three attempts per staged fact, with each request retaining the Federation request's five-second timeout, so restart cannot reset the budget or create an unbounded authorization wait.

When creating the terminal gap, the source shall assign the frozen original prefix one immutable terminal-evidence expiry exactly 30 days after `failed_at`. This terminal-evidence lifetime is distinct from the continuously disconnected backlog-age clock; reconnect, retry, restart, clock rollback, and duplicate gap replay shall not extend or recompute it. The source shall retain only the minimum immutable evidence required to authorize an identical already-staged fact from that prefix, without creating a second replayable payload backlog or admitting a new event under the failed UUID. The original prefix's event-count and payload-byte bounds shall remain unchanged. Every authorization shall require the current trusted enrollment and the exact subscription, event, cursor, transcript, outcome, and occurrence identity. Already-defined settlement, peer trust revocation or replacement, source closure, and explicit re-establishment under a new subscription UUID shall release retained evidence when the source observes that condition. At or after expiry the source shall fail closed and release expired evidence when observed. The immutable terminal tombstone shall survive evidence release so explicit resynchronization remains possible.

If source authorization is definitively rejected, retained terminal evidence has expired, current peer trust no longer authorizes the request, or the three-attempt budget is exhausted by transport failures, the watching instance shall durably suppress only that staged fact with the corresponding machine-readable reason and then process the next staged fact. An accepted fact shall advance the contiguous admitted fact cursor only when its sequence immediately follows that cursor; settlement after a missing or suppressed sequence shall not skip the cursor. The authenticated replay response that stages the immutable gap tombstone shall authorize presentation of that gap control notification without another remote authorization call, but shall not authorize delivery of any fact. The gap shall become presentable only after every staged prefix fact is accepted or explicitly suppressed, and accepting it shall advance no fact cursor.

The system shall preserve the terminal-settlement attempt count, accepted or suppressed fact dispositions, suppression reasons, and gap disposition across restart. Duplicate fact staging before terminal failure, gap replay, authorization response, settlement, and gap presentation shall be idempotent only when their durable identities and metadata match; conflicting reuse shall fail without mutation. An unavailable peer, sequence hole, denied authorization, revoked trust, or unaccepted staged fact shall not cause unbounded retry or permanently hide the gap.

The terminal-gap prefix exception shall not apply to explicit unsubscribe, credential revocation, or source closure. Those endings retain their no-new-admission and cleanup gates.

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

### Peer naming

**REQ-FED-019 — Reserve local as a peer name**

When a peer enrollment or saved peer connection is created or replaced, the system shall trim the display name and reject a blank name or a name equal to `local` ignoring ASCII case. Rejection shall provide a clear input error before replacing a credential or changing the saved connection.

Tool destination selection shall not rename saved peers, change their trust, or rewrite persisted display names.

## Non-goals

- A universal cross-domain idempotency framework.
- An exclusive remote-control lease.
- A combined enable-automatic-continuation-and-create-Goal operation.
- Implicit creation of reverse enrollment, credentials, or trust.
- Remote Bash or general process control.
- Silent remote-to-local fallback.
- Treating peer notes as authorization or as a substitute for destination validation.
