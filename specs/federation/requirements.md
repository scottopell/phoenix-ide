# Federation Requirements

## User need

A Coordinator operating one trusted Phoenix instance needs to use supported coordination tools on another trusted Phoenix instance without losing the destination's ordinary correctness, durable identity, or audit guarantees.

## Scope and related contracts

This specification covers optional `instance` selection on `send_conversation_message`, `read_conversation`, `resolve_reference`, `read_goal`, `query_database`, `read_automatic_continuation`, `set_automatic_continuation`, `watch_conversation`, `unwatch_conversation`, and `list_watched_conversations`; the typed remote automatic-continuation preference operations; durable remote conversation watches; finite settlement of facts durably staged before a terminal watch gap; and native federated ordinary-conversation creation and rename. Goal mutations (including creation), settings other than the typed automatic-continuation preference, and Continue remain outside this contract.

Related contracts remain authoritative for their domains:

- `specs/conversation-identity/requirements.md` owns stable ProductConversation and exact transcript identity.
- `specs/goal-continuation/requirements.md` owns Goal admission and automatic-continuation prerequisites.
- `specs/global-recall/requirements.md` owns local Coordinator Goal commands and local query behavior.
- `specs/compatibility/requirements.md` owns compatibility and recovery guarantees.
- `specs/remote-query-database/requirements.md` owns remote read-only SQL behavior.

## Requirements

### Peer selection and authority

**REQ-FED-001 — Optional destination selection**

When `send_conversation_message`, federated `read_conversation`, `resolve_reference`, `read_goal`, Coordinator `query_database`, `read_automatic_continuation`, or `set_automatic_continuation` receives the exact, case-sensitive `instance` string `"local"` or omits the selector, the system shall execute the local operation through its existing local service with the same authority checks, validation, result shape, provenance, pagination, waits, message retry identity, SQL policy, budgets, and error semantics applicable to that tool.

When one of these tools receives another string selector, the system shall select the remote destination through saved-peer UUID or unambiguous display-name resolution. Local recognition and remote peer matching shall use the selector exactly as supplied, without trimming whitespace or folding case. The literal `"local"` shall be reserved for the instance executing the tool.

An explicit `null` selector shall select local routing on the read tools, including Coordinator `query_database`, except that `read_automatic_continuation` shall reject explicit `null`; `send_conversation_message` and `set_automatic_continuation` shall reject explicit `null`. Tools without an optional `instance` parameter shall retain their own input and authority contracts. In particular, non-Coordinator `query_database` registrations shall remain local-only and shall reject an `instance` field.

**REQ-FED-002 — No destination fallback**

If peer selection, authentication, identity verification, transport, or remote execution fails after a remote selector was supplied, the system shall report that failure and shall not execute the operation locally or against another peer.

**REQ-FED-003 — Destination-owned admission**

When a trusted peer requests an operation, the destination shall authenticate the peer and apply the same domain validation, lifecycle, concurrency, and local-authority rules used for the equivalent local operation. For a remote automatic-continuation preference operation, the destination shall bind the authenticated caller and requested destination identities, transactionally recheck current Federation send authority with exact target resolution and preference access, and close local authority without publishing an ordinary result if write durability becomes unclassified.

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

When a remote operation or watch result reports a transcript-scoped conversation fact or mutation, the result shall identify both the stable ProductConversation and the exact transcript in which that fact or mutation occurred. ProductConversation-scoped settings that do not report a transcript fact shall identify the stable ProductConversation without fabricating an exact transcript.

### Durable watches

**REQ-FED-009 — Durable remote watch**

When `watch_conversation`, `unwatch_conversation`, or `list_watched_conversations` omits `instance` or receives the exact, case-sensitive string `"local"`, the system shall use the existing local watch, unwatch, or list operation respectively, preserving that operation's authority checks, validation, result shape, and error semantics. An explicit `null` `instance` shall be rejected by all three tools rather than treated as omission or as `"local"`.

When one of these tools receives another string selector, the system shall resolve exactly the saved peer UUID or unambiguous saved display name supplied, without trimming whitespace or folding case. A canonical UUID selector shall designate that peer identity; if it is not saved, the watch operation shall not select a different peer whose display name equals that UUID. `watch_conversation` shall create a durable remote subscription for the selected peer's ProductConversation; `unwatch_conversation` shall end the selected peer's durable remote subscription, not a local watch; and `list_watched_conversations` shall list locally stored durable remote subscriptions for that selected peer, not local watches or an assumed live snapshot from the peer. Local watches and remote subscriptions shall remain distinct even when their ProductConversation identifiers match. Failure of peer resolution, authentication, transport, or remote operation shall be reported without falling back to another peer or creating, ending, or listing a local watch in place of the requested remote operation.

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

### Remote automatic-continuation preference

**REQ-FED-020 — Typed remote automatic-continuation control**

When the current source Coordinator invokes `read_automatic_continuation` or `set_automatic_continuation` with a remote selector, the source shall revalidate current-Coordinator authority at the remote-dispatch boundary and hold that source aggregate's admission boundary through the request. The request shall select exactly an ordinary ProductConversation by stable ID or the destination's Coordinator singleton. Read and set shall use distinct closed request shapes; set shall require a Boolean preference value. Unknown fields, unknown target kinds, explicit `null` selectors, missing or `null` set values, and operation-mismatched payloads shall be rejected.

The destination shall authenticate the peer, bind the exact caller and destination instance identities, transactionally recheck the current Federation send authority, and resolve the exact requested aggregate in the same transaction as the preference read or write. The source shall accept a response only when its caller identity and destination identity match the request, its target kind matches the request, an ordinary target's stable ID matches exactly, a Coordinator target carries a syntactically valid stable ProductConversation identity returned by that authenticated exact destination for its Coordinator singleton, and, for a set, the returned Boolean matches the request. A saved-peer selector that resolves to the executing instance shall fail as self-selection rather than issue a remote request.

A remote read shall be side-effect free. A remote set shall change only the prospective automatic-continuation preference and shall preserve the local operation's durability classification. It shall not admit, wake, dispatch, retry, rearm, create, rename, continue, or update a Goal, and shall not alter trust or enrollment. Unknown, ambiguous, self, authentication, identity, target, transport, remote-execution, response-provenance, and unclassified-durability failures shall not execute the operation locally or against another peer. Source-side transport uncertainty shall not be reported as an acknowledged rollback or authorize an automatic retry.

### Native conversation creation and rename

**REQ-FED-021 — Native create and rename use shared application actions**

The current Global Coordinator SHALL expose native ordinary-conversation creation and rename capabilities as thin adapters over the same destination-owned application actions used by supported local creation and naming operations. Native invocation SHALL NOT become an HTTP client, duplicate creation or naming policy, or establish a parallel persistence authority. The local destination SHALL call the same application action directly; selecting local SHALL NOT make an HTTP round trip.

Each capability MAY accept an optional `instance` selector. Omission or the exact case-sensitive string `"local"` selects the local application action. A non-null saved-peer UUID or unambiguous saved display name selects exactly that peer. Explicit `null`, unknown or ambiguous peers, and self-selection SHALL fail. Once a remote selector is supplied, peer resolution, authentication, transport, remote execution, or response-validation failure SHALL be returned as a failure and SHALL NOT invoke the local action or another peer.

Before dispatch, the source SHALL revalidate current-Coordinator authority and derive the exact source ProductConversation, transcript, assistant message, and tool-use identities from trusted execution context. The destination SHALL derive source instance identity from the authenticated peer credential, authorize that peer for the operation, and persist the source-provided call locator as peer-qualified provenance. Tool arguments SHALL NOT override source identity or provenance, and the destination SHALL NOT require foreign source transcript identities to resolve as destination-local rows.

Trusted peer-qualified source-call provenance SHALL be stored as one constrained shape: source instance identity and the exact source ProductConversation, transcript, assistant message, and tool-use identities are all present. Live internal-native creation jobs and qualified rename receipts SHALL require that complete shape; non-native and unknown-historical creation origins SHALL carry none of it. Foreign source identifiers SHALL NOT carry destination-local foreign keys. When upgrading provenance that predates source-instance qualification, the migration SHALL preserve each of the four existing call-locator TEXT values byte-for-byte, including invalid UTF-8. It SHALL backfill the database's durable local instance identity only when all four values are valid UTF-8, nonblank after trimming, and free of NUL; the creation source ProductConversation ID SHALL additionally parse as a ProductConversation ID. Internal creation provenance that cannot be qualified SHALL remain explicit, untrusted `historical_internal` provenance without a source instance; it SHALL NOT authorize a new creation request, and its already-stored agent-authored objective SHALL be processed literally without reconstruction or expansion. Rename provenance that cannot be qualified SHALL retain its locator values without a source instance, SHALL convey no peer authority, and SHALL conflict rather than replay when a new request reuses its operation UUID. The migration SHALL NOT fabricate native provenance for provenance-absent rows. SQLite structural CHECK constraints SHALL enforce representable origin and provenance shapes; because SQLite CHECK cannot validate UTF-8, trusted application write constructors SHALL enforce UTF-8 and the remaining live locator validity conditions before writing peer-qualified provenance.

**REQ-FED-022 — Native creation preserves destination-scoped intent and durable acceptance**

Native creation SHALL use the existing creation request UUID as its durable operation identity and SHALL carry the same closed intent as local native creation: a literal nonblank objective, an existing absolute directory in the selected destination's filesystem namespace, validated model, effort, and language selections, and the explicit submit-objective start intent. Remote option discovery SHALL use the selected destination's directory and model capabilities. The destination SHALL revalidate every supplied choice and apply its own acceptance-time defaults where the native request omits an optional selection. Attachments, caller-authored origin metadata, filesystem references, and skill expansion remain unsupported.

A successful create response SHALL carry an explicit `accepted` status, report durable creation-request acceptance, and identify the exact request UUID and destination-issued stable ProductConversation. It SHALL NOT imply publication, objective delivery, runtime execution, or completion. The response SHALL be bound to the authenticated caller and exact destination instance and SHALL close over the request, ProductConversation, and durable acceptance identities. Reuse of a request UUID with changed intent or source owner SHALL conflict; an identical retry SHALL reconcile the original durable acceptance and preserve its first accepted peer-qualified source-call provenance.

After an uncertain remote create response, the source SHALL query the exact authenticated destination by the same request UUID and SHALL receive either `unknown` or one complete destination snapshot containing the accepted ProductConversation identity, provisioning status, publication and canonical-route state, and initial-objective execution evidence. A known snapshot SHALL be available only to the authenticated peer instance recorded as the durable acceptance owner; another peer presenting the same request UUID SHALL be rejected. This readback SHALL use the same destination-scoped reconciliation semantics as local native creation and SHALL NOT fall back to the source instance or another peer.

**REQ-FED-023 — Native rename is fenced, replayable, and destination-owned**

A native rename request SHALL target an ordinary ProductConversation and SHALL carry a fresh operation UUID, the exact stable ProductConversation ID, its expected canonical root transcript ID, its nonnegative expected naming revision, and the requested title. The destination SHALL use the same naming application action and admission rules as local rename, and SHALL commit the title/projection mutation and durable operation receipt atomically. The destination owns target-kind validation, title normalization, route projection allocation, collision handling, and current lifecycle/admission state.

The operation result SHALL preserve the local native result classification: `applied` means the requested rename and receipt committed; `replayed` means the exact operation UUID and immutable intent already has a durable receipt; `stale` means the expected root or naming revision no longer matches; `conflict` means an operation UUID was reused with changed intent or incompatible source owner; `not_found` means the stable ProductConversation does not resolve; and `unavailable` means the resolved target is not admissible for rename. Replay SHALL return the historical receipt without applying the rename again, even after a later naming change or target deletion. Transport failure, unsupported endpoint, malformed response, and unclassified local durability are protocol failures rather than additional rename results.

Retries of an uncertain rename SHALL retain the same operation UUID and immutable target, root, expected revision, title, destination, and source owner. A caller SHALL NOT generate a replacement operation identity or mutate the intent to resolve uncertainty. Response identity closure SHALL verify authenticated caller and destination instance identities, operation UUID, result status, and exact product/root identity; a response for any other request or peer SHALL be rejected.

**REQ-FED-024 — Closed, bounded protocol and explicit version failure**

Create-option discovery, rename-option discovery, create, create-status, and rename requests and responses SHALL use distinct strict wire shapes and protocol-specific byte limits. Each option-discovery request SHALL be limited to 4,096 bytes and each option-discovery response SHALL be limited to 262,144 bytes. The native-create request limit SHALL be 65,536 bytes; create responses and create-status requests SHALL be limited to 4,096 bytes; create-status responses and rename requests and responses SHALL be limited to 16,384 bytes. Unknown fields, missing required fields, malformed identifiers, invalid enum values, null where not admitted, operation-mismatched fields, and over-limit request or response bodies SHALL be rejected before mutation, acceptance, or reconciliation. Responses SHALL carry enough authenticated caller, destination, operation, and target identity to prove they answer the exact request. A known create-status response SHALL be one complete closed snapshot rather than independently optional fragments, and `unknown` SHALL carry no fabricated target, publication, route, or execution evidence.

If the selected peer does not implement the required option-discovery, native-action, or create-status endpoint, the operation SHALL fail explicitly. It SHALL NOT fall back to a legacy API, another native tool, local execution, or another peer. This contract adds only create and rename; it grants no generic CRUD, settings, trust/enrollment, or lifecycle authority.

## Non-goals

- A universal cross-domain idempotency framework.
- An exclusive remote-control lease.
- A combined enable-automatic-continuation-and-create-Goal operation.
- Implicit creation of reverse enrollment, credentials, or trust.
- Remote Bash or general process control.
- Silent remote-to-local fallback.
- Treating peer notes as authorization or as a substitute for destination validation.
