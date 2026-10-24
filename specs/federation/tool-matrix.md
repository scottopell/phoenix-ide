# Remote tool and owner API matrix

All five tools have explicit-instance HTTPS adapters. The owner-facing direct API resolves saved peer connections server-side and reuses those adapters.

## Directional enrollment setup

The receiver-issued transfer bundle owns the destination instance ID, display name, canonical HTTPS base URL, caller instance ID, directional bearer, and TLS trust. Import accepts that single bundle rather than a second destination representation. Before a connection row is saved or enabled, the importer performs a bounded, no-redirect request using the bundle's pinned TLS trust and bearer, then requires the peer to attest both the destination and authenticated caller instance IDs. Offline, untrusted, unauthorized, or mismatched destinations fail without persisting the connection.

## Owner operations

All operation requests are closed JSON objects and require `instance: string` (saved UUID or unambiguous saved name). Missing, null, blank, unknown, and ambiguous selectors fail; none invoke the local operation. The API does not share the tools' omission-means-local convention.

| Owner route (`POST`) | Request type and remaining fields | Response type | Existing adapter |
|---|---|---|---|
| `/api/federation/query-database` | `OwnerQueryRequest`: `sql` | `RemoteQueryDatabaseResponse` | `query_remote_database` |
| `/api/federation/search-conversations` | `OwnerSearchRequest`: `query` | `OwnerSearchResponse`: destination instance and qualified hits | `search_remote_conversations` |
| `/api/federation/read-conversation` | `OwnerReadRequest`: typed `target`, optional `cursor` (zero by default) | `QualifiedConversationRead` | `read_remote_conversation` |
| `/api/federation/resolve-reference` | `OwnerResolveRequest`: `reference` | `ResolveGlobalReferenceResponse` | `resolve_remote_reference` |
| `/api/federation/send-conversation-message` | `OwnerSendRequest`: typed `target`, `message`, explicit UUID `message_id` | `RemoteSendMessageResponse` | `send_remote_conversation_message` |

Requests are bounded to 256 KiB. Existing HTTPS-only, verified trust, no-redirect, 30-second transport and bounded-response behavior is reused. No automatic retry, fallback, broadcast, merged index, schema translation, remote executable-code dispatch, or delivery queue is added. Transport errors expose static codes rather than raw peer diagnostics. Send retains destination receipt identity and rejection codes; owner calls derive only `RemoteApi` attribution. Request bodies cannot supply a conversation or Coordinator sender.

## Authored send adapter

The explicit-instance `send_conversation_message` tool uses `send_remote_authored_conversation_message` and the fixed peer-only `/api/federation/peer/send-authored-conversation-message` endpoint. Its closed request permits only ordinary conversation or Coordinator attestation with a complete source tool call, derived from persisted source membership and the tool context. Owner/direct API sends instead use the sender-free `/api/federation/peer/send-conversation-message` endpoint and always retain `RemoteApi` provenance. Both bind caller identity solely to peer enrollment; ordinary/API sends cannot message upward to the Coordinator.

The authored endpoint trusts the enrolled source server. Both send endpoints accept the same directional bearer, so a holder can deliberately submit source-local attestations on the authored endpoint. This is not independent authorship proof or a credential-isolation boundary. OpenAPI labels the authored operation as server-to-server, not an owner/Python operation.

## Connections and opaque notes

- `GET /api/federation/peers`: `FederationPeerSummary[]` (`peer_instance_id`, `peer_display_name`, `base_url`, `created_at`). No bearer, verifier, or CA field.
- `GET /api/federation/peers/:instance/notes`: `PeerNotesDocument` (`notes: string`).
- `PUT /api/federation/peers/:instance/notes`: closed `PeerNotesDocument`, whole-value replacement, same response shape.

Each saved connection has owner-only opaque notes in one `TEXT NOT NULL DEFAULT ''` column on `federation_peer_connections`, with a 32 KiB UTF-8 byte limit in the schema and typed API boundary. Import/reimport leaves notes unchanged; connection deletion removes notes. No JSON interpretation, authority semantics, peer foreign key, history, or synchronization is added. Clients load notes lazily and save whole-text edits without interpreting the text as authority.
