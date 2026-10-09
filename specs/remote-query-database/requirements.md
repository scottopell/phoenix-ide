# Remote Query Database — Requirements

## User Need

As an owner coordinating work across enrolled Phoenix instances, I need the
Coordinator to inspect one explicitly selected instance's operational database
without changing it, leaking peer credentials, confusing remote and local
results, or allowing an expensive query to exhaust the destination.

## Scope and Related Contracts

`remote_query_database` is a Coordinator-only, operator-level forensic read
capability. It is not a general security sandbox: application data can include
hidden messages, settings, serialized state, and workflow payloads that normal
UI does not expose. Enrollment is directional; permission to query a receiver
does not confer reciprocal access or owner authority.

- [Auth requirements](../auth/requirements.md), REQ-AUTH-009, own receiver-issued
  enrollment, replacement, revocation, and separation of peer and owner authority.
- [Global recall requirements](../global-recall/requirements.md), REQ-GR-011A,
  own the shared SQLite integrity policy and actionable engine diagnostics.
- [Stale tool results requirements](../stale-tool-results/requirements.md) own
  clearing model-visible output without removing persisted transcript evidence.

Remote writes, arbitrary HTTP endpoints, ambient peer discovery or enrollment,
reciprocal authorization, and automatic local fallback are outside this tool's
scope.

## Requirements

### REQ-RQD-001: Explicit Enrolled Destination

WHEN the Coordinator invokes `remote_query_database`
THE SYSTEM SHALL require a separate `instance_id` UUID and `sql` string
AND SHALL resolve the destination through the caller's persisted directional peer connection keyed by that identity
AND SHALL use the enrolled HTTPS origin and receiver-issued bearer credential, not a model-supplied URL or credential.

IF the identity is malformed, the peer connection is absent, or connection lookup fails
THE SYSTEM SHALL return a tool error without executing SQL.

THE SYSTEM SHALL expose this tool only in the Coordinator tool registry, not to ordinary ProductConversations, restricted planning conversations, or sub-agents.

### REQ-RQD-002: Peer Admission and Enrollment Authority

WHEN a request reaches `POST /api/federation/peer/query-database`
THE SYSTEM SHALL require an active receiver-issued peer bearer enrollment under REQ-AUTH-009
AND SHALL derive caller identity from that enrollment rather than from request JSON, display names, owner passwords, or session cookies.

THE SYSTEM SHALL enforce peer admission even when owner password authentication is disabled
AND SHALL reject missing, invalid, replaced, or revoked peer credentials and credential-lookup failures before SQL execution.

THE SYSTEM SHALL keep issuance, replacement, and revocation on the owner-management channel
AND SHALL NOT grant those operations through peer credentials.

### REQ-RQD-003: Closed HTTPS Transport Without Redirects

THE SYSTEM SHALL construct only the fixed `/api/federation/peer/query-database` endpoint beneath the enrolled bare HTTPS origin
AND SHALL support domain, IPv4, and bracketed IPv6 origins
AND SHALL reject origin values containing URL credentials, a non-root path, query, or fragment.

THE SYSTEM SHALL send one authenticated POST with a 30-second request timeout
AND SHALL disable redirect following, including redirects within the same origin
AND SHALL return a remote rejection for a redirect response rather than reissuing the credential or SQL to another endpoint.

THE SYSTEM SHALL NOT downgrade to HTTP or retry through an alternate destination to bypass a transport failure.

### REQ-RQD-004: Per-Peer TLS Trust

THE SYSTEM SHALL validate the enrolled destination's TLS certificate and hostname before sending its bearer credential or SQL.

THE SYSTEM SHALL support owner-configured certificate trust for an individual peer, including a private or self-signed certificate
AND SHALL bind that trust to the selected peer connection and its enrolled origin
AND SHALL NOT apply it to another peer, disable certificate or hostname verification, or broaden process-wide trust as a side effect.

IF the selected peer's TLS trust cannot be established
THE SYSTEM SHALL fail the operation without an insecure transport fallback.

THE owner-authorized enrollment transfer SHALL identify platform roots or carry exactly one bounded public private-CA certificate whose Basic Constraints identify it as a certificate authority and whose Key Usage permits certificate signing when that extension is present
AND SHALL NOT carry a private key, infer trust on first use, or replace an enrolled trust anchor without an explicit owner-authorized import.

THE owner SHALL supply the peer origin during import
AND THE SYSTEM SHALL persist that owner assertion atomically with the transferred receiver identity, credential, and TLS trust.

WHEN migration adds typed TLS trust to a persisted peer connection
THE SYSTEM SHALL preserve a row without a private-CA certificate as platform-root trust
AND SHALL NOT infer or fabricate a private trust anchor for that row.

### REQ-RQD-005: Authoritative Caller and Destination Provenance

WHEN the receiver accepts a remote query request
THE SYSTEM SHALL compare `destination_instance_id` against its own persisted instance identity before SQL execution
AND SHALL reject a mismatch with HTTP 409.

WHEN a query succeeds
THE SYSTEM SHALL return the receiver's authoritative `destination_instance_id` and the enrollment-authenticated `caller_instance_id` alongside the result.

BEFORE reporting a remote result as successful
THE caller SHALL verify that the returned destination equals the explicitly selected enrolled peer and that the returned caller equals its own persisted instance identity
AND SHALL reject either mismatch rather than rewrite the returned provenance or attribute the result to another instance.

### REQ-RQD-006: Shared Read-Only SQLite Integrity Boundary

THE receiver SHALL execute exactly one read-only SQLite statement through the shared `GlobalReadService` query policy on a separate read-only database connection.

THE receiver SHALL enforce REQ-GR-011A at SQLite authorization time, including through views, aliases, common table expressions, and subqueries
AND SHALL deny writes, transactions, pragmas, database attachment, extension loading, filesystem functions, SQLite internals, and FTS shadow storage.

THE receiver SHALL deny known credential-bearing columns, including owner sessions, share tokens, MCP OAuth secrets and tokens, receiver-side federation verifiers, and caller-side federation bearer credentials
AND SHALL NOT present the policy as a guarantee that all application data is nonsensitive.

### REQ-RQD-007: Bounded Values, Work, and Admission

THE receiver SHALL apply a bounded HTTP request-body envelope before JSON extraction.

THE receiver SHALL limit SQL input to 16 KiB, result columns to 64, returned rows to 200, and the serialized query result to 64 KiB
AND SHALL report `truncated` when rows are omitted to fit row or output limits rather than return partial rows.

THE receiver SHALL configure an explicit SQLite engine-level value-length limit tied to the bounded query resource budget before large text, blob, or expression values can be materialized
AND SHALL bound cell copying before allocating a returned text value; checking serialized output only after allocation is not sufficient.

THE receiver SHALL apply a 750-millisecond execution budget with SQLite progress checks every 1,000 virtual-machine operations
AND SHALL return budget exhaustion distinctly from authorization denial and ordinary engine errors; this progress-based budget is not a hard wall-clock deadline for an individual SQLite function call.

THE receiver SHALL admit no more than four remote queries simultaneously and SHALL reject excess admission with HTTP 429 before SQL execution
AND SHALL hold admission for the lifetime of the underlying work even if the requesting task or connection is cancelled.

THE caller SHALL enforce a 65 KiB response-body limit across streamed chunks before decoding, for successful and unsuccessful HTTP responses alike.

### REQ-RQD-008: Total Typed Query Wire

THE request SHALL contain `destination_instance_id` and `sql`.

THE successful response SHALL contain `destination_instance_id`, `caller_instance_id`, and `result`
AND `result` SHALL contain `columns`, `rows`, `truncated`, `row_limit`, `byte_limit`, and `elapsed_ms` without silently omitted fields.

THE cell wire SHALL distinguish null, signed 64-bit integer, real, text, and blob-length summary values with a `type` discriminator
AND real values SHALL distinguish finite numbers, positive infinity, negative infinity, and NaN with a `kind` discriminator rather than lose non-finite values as JSON null or fail serialization.

THE blob summary SHALL carry its byte count, not raw blob contents.

THE caller SHALL decode the shared typed response and SHALL return an error for malformed JSON, missing required fields, or invalid variants rather than fabricate a successful empty result.

### REQ-RQD-009: Failure Honesty and No Local Fallback

IF input validation, enrollment, transport, TLS, HTTP status, body bounds, decoding, identity validation, authorization, admission, or execution fails
THE SYSTEM SHALL return a tool error
AND SHALL NOT execute the SQL against the caller's database, select another peer, or report the failure as a successful query.

WHEN the receiver rejects SQL
THE SYSTEM SHALL preserve the receiver's diagnostic detail where available, including the SQLite operation phase, primary and extended result codes, symbolic code, diagnostic message, and parse-error offset when supplied by SQLite.

### REQ-RQD-010: Untrusted and Clearable Output

THE tool description SHALL identify operator-level forensic access, explicit enrolled destination selection, no local fallback, and remote stored values as untrusted data rather than instructions.

THE SYSTEM SHALL return successful remote query data as ordinary tool output, never as trusted instructions
AND SHALL mark the tool clearable under the shared stale-tool-result policy without deleting transcript evidence.

THE SYSTEM SHALL retain authoritative destination and caller provenance in the successful model-visible result.
