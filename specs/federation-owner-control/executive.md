# Federation Owner Control - Executive Summary

## Current Reality

Phoenix has a destination owner-only Continue endpoint and a fixed HTTPS client transport. The local forward route resolves an ephemeral owner credential only when `PHOENIX_FEDERATION_OWNER_CREDENTIAL_INSTANCE_ID` independently names the requested destination and `PHOENIX_FEDERATION_OWNER_CREDENTIAL_PLIST` names an absolute, bounded LaunchAgent plist containing exactly one `PHOENIX_PASSWORD`. The value is read at request time and is not stored by Phoenix. The destination reuses the continuation application service and checked database transaction, while peer bearer authentication remains limited to peer routes. No Coordinator tool or cross-host secret retrieval is implied by this API surface; an authorized source must be locally available to the caller-side Phoenix before outbound authentication can occur.

## Requirement Map

| Requirement | Surface | Verification |
|---|---|---|
| REQ-FOC-001 | `FederationOwnerContinueRequest`, `ContinuationExpectation` | foreign/stale product and scope tests |
| REQ-FOC-002 | `OwnerAuthenticated`, `DestinationOwnerCredentialAssociation` | owner-vs-peer route regression |
| REQ-FOC-003 | `continue_remote_owner`, `PeerBaseUrl::owner_continue_endpoint` | two-instance private-CA transport and no-local-fallback tests |
| REQ-FOC-004 | `ContinuationApplicationService::continue_owner`, `Database::continue_conversation_with_intent_checked` | checked reservation tests |
| REQ-FOC-005 | `OwnerContinuationIntentAdmission`, `ContinuationDispatchStatus` | exact/changed intent and existing dispatch-intent tests |
| REQ-FOC-006 | local `/approve-task` contract | auth and lifecycle handler tests |
