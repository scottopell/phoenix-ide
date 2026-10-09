# Remote Query Database — Executive Summary

## Overview

`remote_query_database` lets the Coordinator issue one bounded, read-only SQLite
query to an explicitly selected enrolled Phoenix instance. The destination
reuses the local `GlobalReadService` policy; successful output carries the
receiver's instance identity and the caller identity established by peer
admission. This is operator-level forensic access to application data, not a
general security sandbox or a remote write surface.

The implementation supplies the query path and its directional identity and
enrollment foundation, but specification validation does not establish runtime
conformance.

## Status and Traceability

Paths in the table are repository-relative; symbols identify the implementation
and existing regression coverage without line-number citations.

| Requirement | Status | Implementation and coverage |
|---|---|---|
| REQ-RQD-001: Explicit Enrolled Destination | Implemented | `crates/phoenix-ide/src/coordinator_tools.rs`: `tools`, `RemoteQueryDatabase::input_schema`, `RemoteQueryDatabase::run`; `crates/phoenix-db/src/federation_peers.rs`: `federation_peer_connection`. Registry regressions `coordinator_question_registry_preserves_exact_authority` and `coordinator_question_registration_preserves_non_coordinator_boundaries` cover tool-set boundaries. |
| REQ-RQD-002: Peer Admission and Enrollment Authority | Implemented | `crates/phoenix-ide/src/api/auth.rs`: `peer_auth_middleware`; `crates/phoenix-ide/src/api/federation.rs`: `issue_enrollment`, `revoke_enrollment`; `crates/phoenix-ide/src/api/handlers.rs`: `peer_query_database_requires_peer_auth_and_destination_identity`, `federation_enrollment_requires_owner_and_replaces_peer_credential`. Enrollment is governed by REQ-AUTH-009. |
| REQ-RQD-003: Closed HTTPS Transport Without Redirects | Implemented | `PeerBaseUrl` and `FederationQueryDatabaseEndpoint` construct one fixed HTTPS endpoint from domain, IPv4, or IPv6 origins. `peer_base_url_requires_a_bare_https_origin`, `peer_connection_round_trips_and_replaces_by_instance`, and `migration_120_preserves_peers_and_admits_only_typed_ipv6_hosts` cover bracket-safe IPv6 parsing, typed persistence, and forward migration under ADR-089. `remote_query_client` disables redirects and applies the complete 30-second timeout; `authenticated_remote_query_does_not_follow_307_or_308` covers same-origin 307 and cross-origin 308 without weakening TLS verification. |
| REQ-RQD-004: Per-Peer TLS Trust | Implemented | `PeerTlsTrust` is an exhaustive platform-roots/private-CA choice. Enrollment transfers the receiver's configured trust, migration 119 persists it with the peer connection, and `remote_query_client` replaces platform roots with the selected peer's private CA while retaining ordinary hostname, validity, and chain verification. ADR-089 requires CA capability checks; `peer_private_ca_requires_one_bounded_certificate` rejects leaf certificates and CA certificates whose Key Usage forbids certificate signing; `peer_connection_round_trips_and_replaces_by_instance` and `private_ca_trust_is_isolated_to_the_selected_peer` cover atomic replacement and trust isolation. |
| REQ-RQD-005: Authoritative Caller and Destination Provenance | Implemented | `query_database_with_admission` checks the receiver's persisted identity before execution and returns enrollment-derived caller identity. `decode_remote_query_response` checks both the selected destination and the caller's persisted local identity. `peer_query_database_requires_peer_auth_and_destination_identity` and `remote_response_rejects_wrong_caller_identity` cover both sides. |
| REQ-RQD-006: Shared Read-Only SQLite Integrity Boundary | Implemented | `crates/phoenix-ide/src/api/federation.rs`: `query_database_with_admission`; `crates/phoenix-db/src/coordinator_query.rs`: `execute_coordinator_query`, `authorize`, `read_allowed`, `function_allowed`. Tests `credential_guard_is_table_and_column_specific`, `denies_writes_attach_pragmas_and_multiple_statements`, `denies_schema_and_shadow_table_bypasses`, and `denies_fts_index_and_shadow_storage` cover the shared policy. |
| REQ-RQD-007: Bounded Values, Work, and Admission | Implemented | The route's `DefaultBodyLimit` bounds the HTTP request before JSON extraction; `peer_query_database_bounds_body_before_json_extraction` covers rejection. `execute_coordinator_query` sets SQLite column and value-length limits before preparation and retains SQL/row/result/time budgets. The streamed caller response body is bounded before decoding. `QUERY_ADMISSION` and `spawn_with_admission_permit` retain admission through work completion. `bounds_sqlite_value_allocation_before_materialization` covers text/blob boundaries, and `denies_concat_functions_that_allocate_before_sqlite_length_enforcement` removes built-ins that amplify before SQLite applies its length limit, in addition to the existing output, work, response, exhaustion, and cancellation regressions. |
| REQ-RQD-008: Total Typed Query Wire | Implemented | `RemoteQueryDatabaseRequest`, `RemoteQueryDatabaseResponse` share serialization/deserialization; `crates/phoenix-db/src/coordinator_query.rs`: `CoordinatorQueryResult`, `CoordinatorCell`, `CoordinatorReal`. Tests `remote_query_request_uses_shared_wire_shape` and `non_finite_real_cells_have_total_json_round_trip` cover request parity and non-finite real encoding. |
| REQ-RQD-009: Failure Honesty and No Local Fallback | Implemented | `RemoteQueryDatabase::run` returns `ToolOutput::error` on client failure, with no local-query branch. `decode_remote_query_response` preserves receiver error detail; `remote_rejection_preserves_server_error_detail` covers SQL rejection detail. |
| REQ-RQD-010: Untrusted and Clearable Output | Implemented | `RemoteQueryDatabase::description`, `clearable`, and `run` return ordinary successful JSON text, not `TrustedInstructions`. `remote_query_results_are_clearable_and_marked_untrusted` checks clearability and forensic/untrusted wording. |

## Wire and Resource Inventory

The model supplies `{ "instance_id": "<UUID>", "sql": "<statement>" }`. The client
resolves the stored peer connection, then posts to
`/api/federation/peer/query-database` with a peer bearer and this JSON request:

```json
{
  "destination_instance_id": "<UUID>",
  "sql": "<statement>"
}
```

A successful response has three required top-level fields:
`destination_instance_id` (UUID string), `caller_instance_id` (UUID string), and
`result` (object). `result` has required `columns` (string array), `rows` (arrays
of typed cells), `truncated` (boolean), `row_limit` and `byte_limit` (unsigned
integer limits), and `elapsed_ms` (unsigned integer milliseconds). These fields
have no omission/default serialization attributes.

| SQLite cell | JSON encoding |
|---|---|
| Null | `{ "type": "null" }` |
| Integer | `{ "type": "integer", "value": 42 }` (signed 64-bit value) |
| Finite real | `{ "type": "real", "value": { "kind": "finite", "value": 1.5 } }` |
| Positive/negative infinity or NaN | `{ "type": "real", "value": { "kind": "positive_infinity" } }`, with `negative_infinity` or `nan` for the other variants |
| Text | `{ "type": "text", "value": "stored text" }` |
| Blob summary | `{ "type": "blob", "value": { "bytes": 12 } }` (no blob contents) |

The HTTP route rejects request bodies above 100 KiB before JSON extraction. The
shared engine limits SQL to 16 KiB, columns to 64, rows to 200, and the
serialized `CoordinatorQueryResult` to 64 KiB. Output fitting removes whole
rows and sets `truncated`; metadata alone exceeding the budget returns a budget
error. A progress handler checks the 750-millisecond deadline every 1,000 VM
operations. That handler is not a hard deadline inside a long-running SQLite
function. SQLite also limits each string, BLOB, or encoded row to 64 KiB before
query preparation and value materialization.

The receiver's process-wide remote-query semaphore has four permits and rejects
exhaustion before executing SQL. The spawned work owns the permit, so dropping
the HTTP caller does not admit replacement work prematurely. The client has a
30-second timeout and a 65 KiB streamed response-body cap; the 65 KiB envelope
cap is separate from the 64 KiB query-result cap. Destination mismatch is HTTP
409, exhausted admission is 429, SQL rejection is 422, peer authentication
failure is 401, and internal failures are 500. Error bodies are not claimed to
share the successful-response shape.

## Verification and Remaining Gates

The coverage cited above is existing repository coverage identified by source
inspection, not a claim that Rust tests were rerun for this documentation change.
The documentation validation is the `spec-shape` and `spec-anchors` lanes of
`./dev.py check`, plus the applicable `specs/AUTHORING.md` checks for symbol
anchors, wire shapes, cross-artifact names, and timeless requirements.

Full conformance still requires the cited focused and repository-wide gates at
the exact implementation head. No deployment or live pairing evidence is
claimed here.
