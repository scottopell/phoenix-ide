# Federation Owner Control - Executive Summary

## Current Reality

Phoenix has an owner-only local Continue endpoint plus a fixed delegated peer HTTPS endpoint. The caller's owner-authenticated forward route uses the enrolled peer credential only to establish caller identity. The destination admits the request only when `PHOENIX_FEDERATION_OWNER_CONTROL_CALLER_INSTANCE_ID` independently names that authenticated peer and `PHOENIX_FEDERATION_OWNER_CREDENTIAL_PLIST` names an absolute, bounded destination-local LaunchAgent plist whose single `PHOENIX_PASSWORD` matches the server's active owner password. The password never crosses the network or enters Phoenix persistence. The destination then reuses the continuation application service and checked database transaction. No Coordinator tool, generic peer mutation authority, or cross-host secret retrieval is implied by this API surface.

## Requirement Map

| Requirement | Surface | Verification |
|---|---|---|
| REQ-FOC-001 | `FederationOwnerContinueRequest`, `ContinuationExpectation` | foreign/stale product and scope tests |
| REQ-FOC-002 | `OwnerAuthenticated`, `DestinationOwnerCredentialAssociation` | owner-vs-peer route regression |
| REQ-FOC-003 | `continue_remote_owner`, `PeerBaseUrl::owner_continue_endpoint` | two-instance private-CA transport and no-local-fallback tests |
| REQ-FOC-004 | `ContinuationApplicationService::continue_owner`, `Database::continue_conversation_with_intent_checked` | checked reservation tests |
| REQ-FOC-005 | `OwnerContinuationIntentAdmission`, `ContinuationDispatchStatus` | exact/changed intent and existing dispatch-intent tests |
| REQ-FOC-006 | local `/approve-task` contract | auth and lifecycle handler tests |
