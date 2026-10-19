# Phoenix Federation

## User story and principles

As the owner of a small trusted fleet, I want to reach conversations and commission work on the appropriate machine without changing interfaces or building a distributed scheduler.

- Personal utility earns complexity; conventional APIs, manual enrollment, and Bash/Python are preferred over speculative infrastructure.
- Identity, provenance, destination, and acceptance remain legible across machines.
- Familiar operations stay cheap; documentation is consulted when necessary, not rediscovered on every call.
- Host-specific knowledge is remembered as observations, not promoted into a distributed source of authority.

## Scope

Direct, explicitly enrolled, same-owner instance connections; query/search/read/resolve/message and existing Phoenix API actions. A preferred primary is a user convention, not a privileged hub. Remote process execution and federated process handles are excluded. Local Bash invoking a remote Phoenix HTTP API is not remote Bash.

No automatic transitive trust, forwarding, scheduler, replicas, database synchronization, agent migration, distributed storage, VPN/NAT implementation, identity provider, public-key authentication protocol, RBAC, multitenancy, or per-tool approval framework is required. Existing tool validation, operator-query semantics, and destination WorkScope authority remain in force.

## Requirements

### REQ-FED-001: Persistent instance identity

THE SYSTEM SHALL generate a random instance UUID once when initializing a database without an instance identity and persist that identity in the database.
THE SYSTEM SHALL seed an owner-editable display name from the server hostname once, and maintain an explicitly configured canonical HTTPS URL separately from identity.
Restart, deployment, and name or URL edits SHALL retain the UUID. A same-instance offline move or paired restore SHALL retain the identity present in its database; this grants no new database portability, rollback, credential recovery, or live replacement guarantee beyond [compatibility](../compatibility/requirements.md).
An independently operating clone SHALL be explicitly initialized with a fresh identity and re-enrolled, without carrying usable peer authentication into the new identity. Automatic clone detection is not required.

### REQ-FED-002: Reachability and explicit trust

THE SYSTEM SHALL connect only to explicitly configured enrolled destinations using verified HTTPS, including supported explicit certificate trust, without blanket remote certificate-verification bypass.
Public Internet exposure SHALL NOT be required; private overlay reachability is sufficient.
Connection in one direction SHALL NOT enroll its reverse or authorize a third instance.
The target SHALL retain authoritative admission, API validation, and execution-environment decisions.

### REQ-FED-003: Receiver-first enrollment

WHEN the owner enrolls a caller on a receiving instance
THE SYSTEM SHALL accept the caller's non-secret instance UUID and display name, issue a random bearer credential bound to that caller, and present a one-time secret transfer bundle.
The bundle SHALL identify the destination UUID, display name, canonical URL, caller UUID, and token; this specifies semantic contents, not a wire encoding.
WHEN the caller imports the bundle
THE SYSTEM SHALL reject a caller UUID mismatch and verify the destination UUID over authenticated verified HTTPS before enabling the connection.
The caller SHALL store the usable token; the receiver SHALL persist a non-recoverable verifier and caller association.
Lost transfer material SHALL require a new credential rather than recovery of the original receiver-side secret.
Enrollment SHALL use owner-authenticated settings, not model prompts, a remote owner-login flow, or a pairing service.

### REQ-FED-004: Authenticated caller and revocation

THE receiving instance SHALL derive the caller identity from an active credential enrollment, never from a submitted instance name or UUID alone.
Enrolled peers SHALL have the trusted federation authority without a role matrix; authentication SHALL NOT bypass the target's normal semantic validation or work-admission rules.
The owner SHALL be able to revoke each directional credential independently. Subsequent requests using a revoked credential SHALL fail authentication; revocation SHALL NOT imply rollback of accepted actions or cancellation of already running work.
Owner login credentials MAY coexist with peer credentials. Peer authentication SHALL NOT be reported as owner-channel authorship.

### REQ-FED-005: Secret handling and anti-enumeration

Credentials SHALL NOT appear in conversation/navigation links, prompts, agent notes, routine command arguments, ordinary tool output, or routine logs. Explicit owner enrollment transfer is a secret-handling surface.
THE SYSTEM SHALL authenticate private conversation listings, search, reads, resolution, streams, and mutations even when a caller knows an exact UUID.
Unauthenticated responses SHALL NOT explicitly disclose existence of private conversations through content, status, or routing distinctions. This does not require indistinguishable timing or a constant-time application. Explicit owner-created share-token surfaces remain governed by [auth](../auth/requirements.md); ordinary UUID links are not share grants.
Authentication material SHALL NOT be forwarded to an unrelated origin, including by redirects. Public bootstrap metadata SHALL NOT disclose private conversations or enrolled-peer secrets.

### REQ-FED-006: Structured remote provenance

THE SYSTEM SHALL extend the typed input-provenance path with authenticated source-instance identity and server-attested local sender identity, including stable ProductConversation and exact transcript identity where the sender has them.
The sending server SHALL derive conversation membership from its persisted runtime identity, not model-supplied claims. The receiver SHALL bind source-instance attribution to authenticated enrollment.
Conversation-authored tool-call attribution SHALL survive admission, steering, persistence, retries, retrieval, transport, UI, and model rendering. Arbitrary Bash/Python API invocation and API-created opening objectives SHALL NOT be required to carry conversation authorship. Those calls authenticate the enrolled instance but retain API-channel input classification; they SHALL NOT invent a sending conversation. No UDS/peer-credential attestation subsystem is required.
A Coordinator sender SHALL retain its actual stable Coordinator identity and exact transcript rather than fabricate ordinary ProductConversation membership.
This attribution SHALL NOT be described as independently signed conversation authorship or proof of an uncompromised physical device.

### REQ-FED-007: Peer coordinator messages

THE SYSTEM SHALL preserve rejection of same-instance conversation messages to the local Coordinator chain and preserve existing sub-agent and self-target restrictions.
An enrolled remote Coordinator SHALL be able to message the destination Coordinator through authoritative message admission.
Acceptance SHALL report delivered, queued, or rejected without implying acknowledgement, execution, or completion. This exception SHALL NOT grant arbitrary ordinary remote conversations permission to message upward to a Coordinator.

### REQ-FED-008: Instance-qualified navigation

Conversation references SHALL identify their owning instance and stable ProductConversation, or stable Coordinator identity for Coordinator references; exact historical references SHALL preserve transcript/message locators when available.
Web navigation SHALL use the destination's canonical route and URL. Canonical web navigation SHALL provide the remote journey without requiring native multi-instance support. Native connection switching is a separately scoped client extension under REQ-IOS-022, not a prerequisite for federation.
Missing configuration SHALL offer connection setup with the known URL and normal secure credential handling; an unreachable configured destination SHALL present connectivity guidance.
Neither URL nor display name SHALL substitute for instance identity. Links SHALL carry no credential. This contract does not mandate a new URI scheme or continuous simultaneous streams.

### REQ-FED-009: Bash-first direct commissioning

THE originating Coordinator SHALL be able to invoke destination Phoenix APIs through local unsandboxed Bash/Python, without exposing remote Bash or process handles.
The execution environment SHALL make supported client functionality available using the [simple client](../simple_client/requirements.md) and same-binary companion prior art where appropriate; no separate agent HTTP-tool ecosystem is required.
Normal client use SHALL resolve selected connection authentication without putting secrets in model-visible command arguments.
The originating Coordinator SHALL be able to create and inspect destination work directly, optionally asking the remote Coordinator for conventions, project paths, or models.
Known destination-host conventions and project inventory MAY supply cwd. Destination validation SHALL remain authoritative; a JSON path field SHALL NOT imply shell expansion.

### REQ-FED-010: Cheap API reference and bounded uncertainty

THE target SHALL serve generated OpenAPI at `/openapi.json` describing its supported API. Skills SHALL explain conceptual workflows and remain audience-bound; copied instructions SHALL grant no authority.
Known common API operations MAY be used without fetching OpenAPI for every call. Relevant target documentation SHALL be consulted for unfamiliar operations or concrete mismatches.
Unsupported operations SHALL fail honestly rather than use blind latest-version assumptions or automatic shape adaptation.
Calls SHALL have bounded waits and retries. Mutations SHALL reuse an operation's documented request identity where supported; uncertain delivery SHALL NOT trigger blind retries or claims of completion. Federation SHALL NOT add a general exactly-once or offline delivery guarantee.

### REQ-FED-011: Coordinator execution ownership

THE instance Coordinator SHALL own a persistent execution WorkScope and cwd at `<resolved data_dir>/coordinator/`, respecting Phoenix's configurable data-directory resolution.
The scope SHALL exist without active ordinary work, persist across restart and transcript continuation, and remain outside ordinary ProductConversation Open/History, Close/Delete, repository, and task-approval lifecycle.
Ordinary conversation cleanup SHALL NOT retire this scope or delete its files. Explicit project operations SHALL retain selected WorkScope authority.
The cwd SHALL organize unsandboxed execution, not claim filesystem isolation.
Dev launching SHALL supply a distinct data directory per worktree when the operator has not explicitly supplied one. Multiple production instances MAY use separately configured roots; explicit configuration SHALL NOT be silently rewritten. Separate databases SHALL NOT be represented as proof of separate filesystem roots.

### REQ-FED-012: Discoverable files and peer notes

The Coordinator grounding panel SHALL show its working-directory path and browsable contents through server-backed file viewers without implying a Git project or ordinary work retirement controls.
Connections SHALL offer agent-readable/writable notes for conventions and observations. Agent note edits SHALL NOT mutate destination, identity, credential association, enabled state, or trust. Notes SHALL contain no secrets and SHALL NOT confer authority.
Coordinator files MAY retain local notes and scripts. Connection notes SHALL be one opaque text value read and written whole, with no automatic extraction or cross-peer synchronization. Files MAY repeat observations; they SHALL NOT become a second configuration authority. No general memory engine is required.

### REQ-FED-013: Instance selection and typed references

The five remote tools SHALL use an explicit optional `instance` selector alongside their existing local arguments. Omission SHALL retain local behavior; an explicitly selected unavailable or unknown instance SHALL fail without local fallback.
Saved peer names MAY be ergonomic selectors, but SHALL resolve unambiguously to a saved instance UUID. Ambiguous names SHALL require explicit disambiguation; UUID selection SHALL be available without introducing a globally encoded conversation-reference language.
Structured reference data SHALL associate the owning instance UUID with a typed local target: stable ProductConversation, exact transcript with optional message, or Coordinator. Display names and URLs SHALL NOT replace identity.
Search and read results SHALL retain exact evidence locators and stable conversation locators when available. Read/resolve tools SHALL preserve stable-current versus exact-historical semantics; send SHALL accept conversation targets rather than message citations.
Navigation locators SHALL be generated by the destination using its authoritative routes and combined with its configured canonical base URL through URL handling. Federation SHALL NOT hardcode the ProductConversation route spelling or parse formatted Markdown to reconstruct reference identity.
A stable conversation locator SHALL follow the conversation; an evidence locator SHALL retain the exact transcript/message. If a message has no supported rendered anchor, the UI MAY open the containing transcript without promising message highlighting, while preserving the exact machine reference.
A changed destination URL SHALL NOT change structured reference identity. Previously copied HTTPS links SHALL NOT be promised address-change durability without separately configured redirects.
Existing native URL schemes MAY remain supported locally; federation SHALL NOT require extending them to remote instances.

### REQ-FED-015: Bounded Conversation Subscriptions

WHEN an authenticated enrolled caller subscribes to an open ordinary destination ProductConversation
THE SYSTEM SHALL bind the subscription to the authenticated source instance and destination stable ProductConversation identity
AND SHALL follow that conversation across transcript continuation
AND SHALL emit only destination-committed terminal completion, failure, and cancellation facts
AND SHALL NOT infer task completion, stream transcript text or model tokens, or expose arbitrary destination events.

Each subscription event SHALL carry a destination-owned durable event identity and the typed destination conversation reference
AND destination admission SHALL deduplicate that event identity before advancing a durable per-subscription cursor
AND restart or reconnect SHALL resume strictly after the last durably admitted cursor without retracting accepted facts.

The destination SHALL retain at most 1,024 undelivered events per subscription and SHALL expire undelivered events after 30 days.
WHEN a requested cursor predates retained history
THE SYSTEM SHALL report an explicit gap together with the authoritative current terminal or nonterminal state so the caller can reconcile without fabricated intermediate events.

Unsubscribe or peer revocation SHALL prevent future event delivery
AND SHALL NOT retract facts already durably accepted by the caller.
WHEN the destination ProductConversation closes
THE SYSTEM SHALL end its active subscriptions and suppress facts that the caller has not durably admitted
AND SHALL retain facts already durably admitted by the caller.
Subsequent hard deletion SHALL NOT require a retained subscription tombstone or current-state reconstruction guarantee.
Delivery discovery SHALL reuse existing durable admission and owner-driven wake boundaries rather than add an indefinite broadcast, distributed scheduler, or generic event platform.
