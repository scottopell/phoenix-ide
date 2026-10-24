# Phoenix Federation — Executive Status

## Scope

Federation is the originally approved four-slice program: stable instance identity and protected credential reads; directional enrollment and verified HTTPS transport; a persistent Coordinator WorkScope; five explicit-instance tool adapters with typed provenance, destination navigation, direct API/Python support, opaque peer notes, and isolated two-host verification. Bounded conversation subscriptions extend this program without replacing any of those obligations.

The five tools are `query_database`, `search_conversations`, `read_conversation`, `resolve_reference`, and `send_conversation_message`. Remote Bash/process control, automatic discovery, replication, broadcast, generic RPC, a distributed scheduler, transcript/token streaming, and native instance switching are not part of this scope.

## Coverage

| Requirement | Current status | Remaining verification or implementation |
|---|---|---|
| REQ-FED-001–005 | Receiver-issued bundles own destination identity/name/URL/trust; import authenticates and verifies both instances over bounded pinned TLS before enabling; query-focused enrollment, revocation, and credential denial are exact-head qualified | Deployed two-host enrollment acceptance |
| REQ-FED-006–007 | Direct/owner send derives RemoteApi only; explicit remote tool uses distinct source-server-authored send with persisted membership and exact tool-call provenance; exact-head source qualification is green | Deployed two-host acceptance |
| REQ-FED-008 | Typed identities, qualified peer navigation, visible missing/unreachable connection recovery guidance, Rust-backed TypeScript bindings, owner facades, web/native presentation, and Python are exact-head qualified | Deployed two-host acceptance |
| REQ-FED-009–010 | Five owner facades, Python methods, complete federation lifecycle/peer OpenAPI, and destination-bound saved-peer reference fetch are exact-head qualified | Deployed peer fetch acceptance |
| REQ-FED-011–012 | Peer-notes migration 133, typed 32 KiB UTF-8 bounds, owner GET/PUT, and the Settings Connections editor are exact-head qualified | Coordinator WorkScope remains goals-owned; deployed notes acceptance remains |
| REQ-FED-013 | Five tool adapters accept optional instance selection; owner operation and OpenAPI-fetch facades require explicit saved-peer selection; exact-head qualification is green | Deployed two-host acceptance |
| REQ-FED-015 | Normalized finite replay/authorization, retention/gaps, End-on-Close, durable Coordinator admission, unsubscribe/revocation, presentation, generated bindings, and isolated fixtures are exact-head qualified | Final independent review and deployed two-host acceptance |

## Owner direct API source slice

`GET /api/federation/peers` returns `FederationPeerSummary[]` containing instance ID, display name, canonical base URL, and creation timestamp only. `GET`/`PUT /api/federation/peers/:instance/notes` exchange `PeerNotesDocument` (`{notes: string}`); writes replace the complete text and reject unknown fields. Notes never participate in connection resolution, authentication, or sender attribution.

The five `POST /api/federation/{query-database,search-conversations,read-conversation,resolve-reference,send-conversation-message}` routes require a non-null `instance` string resolving to one saved peer UUID or unambiguous saved name. They do not share the tools' omission-means-local behavior. Their closed request types are `OwnerQueryRequest`, `OwnerSearchRequest`, `OwnerReadRequest`, `OwnerResolveRequest`, and `OwnerSendRequest`. Results reuse the existing typed query/read/resolve/send representations; search wraps qualified hits with the destination instance ID in `OwnerSearchResponse`. Transport failures expose a static error code, not raw peer/TLS/credential diagnostics. Rejected send receipts retain their typed rejection code with a static diagnostic.

Focused DB and router regressions cover exact text, empty notes, byte bounds, reopen/reimport/delete, unknown fields, selector failures, and unchanged connection authority. Isolated HTTPS tests exercise all five owner facades with same-ID local lookalikes, typed destination identities/navigation, RemoteApi admission, Coordinator spoof rejection, and broken TLS/auth/offline failure without fallback. Exact-head CI at `52fd29f1bffadc37e71cd9d560cf755de8650f2f` passes Rust, Clippy, UI/specs, end-to-end, task-validation, and finalization lanes; the Rust lane also passes the focused goal qualification and production-feature musl check.

## Peer send trust boundary source slice

`POST /api/federation/peer/send-conversation-message` accepts the closed `RemoteSendMessageRequest` (`destination_instance_id`, typed `target`, UUID `message_id`, literal `message`) with no sender. Sender/source/caller-instance claims are unknown fields. The destination constructs `InputOrigin::RemoteApi` solely from `PeerAuthenticated`; owner/Python facades and the direct peer transport use this endpoint.

`POST /api/federation/peer/send-authored-conversation-message` is a distinct fixed server-to-server operation using the closed `RemoteAuthoredSendMessageRequest`. Its additional `sender` is only ordinary conversation or Coordinator attestation, never RemoteApi or a caller-instance claim. The explicit-instance `send_conversation_message` tool derives it from the persisted source conversation and `ToolContext` source-call locators, then uses the authored transport. Both endpoints preserve the existing admission/replay path, ordinary/API upward restriction, remote Coordinator exception, bounded verified HTTPS, and no fallback. No new owner-authored facade or response type is introduced.

Both endpoints use the same directional enrolled-peer bearer. Authentication proves only the enrolled source instance; the receiver trusts that server to attest its local membership and tool-call identity under REQ-FED-006. An enrolled source, or a holder of its bearer, can deliberately invoke the authored endpoint and fabricate those source-local claims. Endpoint separation prevents direct API bodies from accidentally acquiring authored provenance; it is not credential isolation, independently signed authorship, or protection against a compromised source/credential. OpenAPI marks the authored operation and states this boundary explicitly.

Closed-body/API-forgery, stored RemoteApi, exact ordinary/Coordinator source provenance, cross-channel replay conflict, original-call retention, Coordinator admission, and two-instance/OpenAPI regressions pass exact-head hosted qualification. Live-host acceptance remains separate.

## Generated federation API reference source slice

`GET /openapi.json` serves OpenAPI 3.1 titled **Federation API reference**, not a complete Phoenix API inventory. `schemars` derives schemas from the actual owner/peer request, response, target, navigation, cursor, receipt, subscription, summary, notes, and error types; a typed path registry assembles the document. The dependency was already present transitively; the manifest/lock update adds direct derive access and UUID/chrono support without changing registry package versions. No checked-in JSON sample or generic RPC dispatcher is used.

The reference covers the five owner operation facades, five corresponding direct peer endpoints plus the distinct server-to-server authored send endpoint, peer summaries and GET/PUT notes, owner subscription list/create/sync-once/unsubscribe/notifications, and peer subscription create/replay/authorize/unsubscribe. Owner authentication uses the existing bearer-password or session channel. Only the exact documentation GET additionally accepts an active directional peer bearer, allowing authenticated Rust peer retrieval without distributing owner passwords. This documentation exception grants no owner operation authority; revoked/invalid peer credentials fail closed without owner fallback. When no owner password is configured, the existing unauthenticated owner-channel policy remains unchanged.

`POST /api/federation/openapi` is owner-only and requires `FetchOpenApiRequest.instance`, resolving one saved UUID or unambiguous saved name. It fetches that connection's `/openapi.json` using the shared verified HTTPS transport and saved directional peer credential, with redirects disabled, a 30-second transport timeout, and a 512 KiB limit checked against both Content-Length and streamed bytes. The target document carries `x-phoenix-instance-id`; mismatches, missing identity, invalid documents, credential echoes, TLS/auth/offline failures, and oversized documents fail with the existing static owner error code, never a local reference or raw transport diagnostic. `DestinationApiReference` binds the verified document to `destination_instance_id` and preserves extra document fields without interpreting them as authority.

Generation/path/schema/reference-resolution/auth/selector/bounds regressions and isolated HTTPS success/TLS/revocation/offline/redirect/stream-size/wrong-destination/credential-echo regressions pass exact-head hosted qualification. Deployment and live-peer acceptance remain separate.

## Verification policy

Source qualification requires focused tests for every slice, exact-head hosted checks, and independent review. Live primary→devmbp acceptance is a separate Global-owned host operation. Source implementation does not depend on mutating live peers and must use isolated fixtures for remote mutation and subscription tests.
