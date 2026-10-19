# Implement trusted-device Phoenix federation end to end

Implement the user-authorized four bounded slices without deployment or live credential enrollment:

1. Persist stable instance identity and exclude known credential-bearing relational data from Coordinator SQL reads.
2. Add owner-managed directional enrollment, one active receiver-issued credential per caller UUID, strict authenticated HTTPS peer transport, and revocation/replacement semantics.
3. Give the Coordinator a persistent execution WorkScope that exists with zero ordinary conversations.
4. Add explicit-instance adapters for the five approved remote tools, typed destination-qualified references/provenance, Python/API/web navigation, opaque connection notes, and failure-path coverage.
5. Add bounded authenticated subscriptions to terminal completion, failure, and cancellation facts for a stable remote conversation. Subscriptions follow continuation, recover across restart/reconnect with durable event IDs, deduplication, and cursor consistency, and expose an explicit bounded-retention gap plus current-state reconciliation. Unsubscribe and peer revocation stop future delivery without retracting already accepted facts.

## Coverage ledger

| Surface | Current evidence | Remaining obligation |
|---|---|---|
| Stable instance identity and credential denial | Landed migrations and Coordinator SQL guards | Two-host acceptance on deployed exact source |
| Directional enrollment and verified HTTPS | Landed receiver enrollment, peer import, per-peer CA trust, IPv6 origin support | Authorized primary→devmbp operational acceptance, owned by Global's sole host writer |
| `query_database` | Landed bounded `remote_query_database` subset | Fold into the shared optional `instance` selector and preserve local omission semantics |
| Shared typed references | `QualifiedConversationReference` source slice in progress | Thread typed references through search, read, resolve, send, navigation, and provenance |
| `search_conversations` | Local-only | Authenticated peer adapter, typed stable/evidence locators, failure/no-fallback tests |
| `read_conversation` | Local-only | Stable-current and exact-historical peer adapter with bounded paging |
| `resolve_reference` | Local typed-reference consumer in progress | Destination-authoritative remote route/URL and missing/unreachable peer behavior |
| `send_conversation_message` | Local-only | Authenticated remote admission, idempotency, sender provenance, Coordinator exception, uncertainty tests |
| Coordinator WorkScope | No persistent Coordinator-owned execution scope | Schema/runtime/cwd/grounding lifecycle and zero-ordinary-work tests |
| Direct API/Python client | Enrollment/query subset only | Saved-peer auth resolution, bounded calls, OpenAPI, create/inspect destination work |
| Destination navigation | Local routes only | HTTPS destination navigation without credentials, setup/connectivity guidance |
| Opaque peer notes | Not implemented | One whole-value non-secret note independent of identity/trust/credential fields |
| Subscriptions | Local watches only | Normative extension, authenticated peer transport, durable cursor/event admission, bounded retention/gap reconciliation, unsubscribe/revocation tests |
| Automated acceptance | Query-focused unit/integration coverage | Isolated two-host suite for five tools, subscriptions, TLS/auth/provenance/failure paths |

Reuse existing message idempotency, destination services, and ADR-070 watch admission. Do not add native iOS switching, database synchronization, remote Bash/process management, RBAC, multitenancy, broadcast/global directory, indefinite event streaming, transcript/token streaming, a distributed scheduler, generalized recovery, insecure TLS fallback, or production enrollment/configuration. Global retains publication, integration, deployment, and live pairing.
