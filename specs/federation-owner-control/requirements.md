# Federation Owner Control

## User Story

As the owner of multiple enrolled Phoenix instances, I need one narrowly typed way to continue an exhausted conversation on a selected destination so that cross-instance coordination does not become a generic remote mutation capability.

## Requirements

### REQ-FOC-001: Destination-qualified owner Continue

WHEN an authorized control-plane caller requests Continue on another Phoenix instance
THE SYSTEM SHALL require the selected destination `InstanceId`, the exact `ProductConversationId`, the exact current predecessor transcript, the expected destination WorkScope association, one client message identity, and one literal handoff
AND SHALL reject a destination, product, predecessor, membership, state, or WorkScope mismatch before creating a successor
AND SHALL NOT fall back to a local conversation when the selected destination is absent or unreachable.

### REQ-FOC-002: Owner and peer authority remain distinct

WHEN the control plane calls the destination's delegated owner Continue endpoint
THE SYSTEM SHALL authenticate the caller with the enrolled peer credential
AND SHALL separately require an owner-configured allowed caller `InstanceId` and a destination-local owner credential source that matches the destination server's active owner credential
AND SHALL NOT treat the federation peer bearer alone as owner authority
AND SHALL keep the narrow Continue delegation distinct from peer enrollment and from request-controlled fields
AND SHALL report missing delegation, owner credential capability, or authentication rejection truthfully.

The destination owner credential SHALL be borrowed only from an operator-authorized source on the destination host and SHALL NOT traverse the network. Phoenix SHALL NOT discover unrelated secret sources, return the credential, write it to its database or files, or include it in logs, errors, traces, tool input, or tool output.

### REQ-FOC-003: Verified bounded transport

WHEN sending destination owner Continue
THE SYSTEM SHALL use the enrolled destination's typed HTTPS origin, selected TLS trust, and receiver-issued peer credential to establish caller identity
AND SHALL use only the fixed delegated owner Continue path
AND SHALL disable redirects and bound request and response sizes
AND SHALL verify that a successful response names the selected destination, product, and predecessor
AND SHALL NOT use SSH, remote filesystem access, plain HTTP, disabled certificate verification, or a generic mutation proxy.

### REQ-FOC-004: Shared Continue authority

WHEN the destination admits an owner Continue request
THE SYSTEM SHALL execute the same durable continuation reservation and opening-intent transaction used by local owner Continue
AND SHALL preserve mode-specific successor construction, product membership, Close admission, one-successor arbitration, and durable dispatch-intent recovery
AND SHALL revalidate the expected destination facts under the destination's product admission and database transaction rather than trusting a transport precheck.

### REQ-FOC-005: Immutable intent and truthful outcomes

WHEN an exact retry races with or follows a successful reservation
THE SYSTEM SHALL return the existing successor only when the message identity, literal handoff, caller metadata, and opening authority match the persisted winning intent
AND SHALL reject changed intent without replacing or reinterpreting the winner
AND SHALL preserve the successor and durable intent when opening dispatch fails.

A response SHALL distinguish newly admitted intent, exact retry, accepted dispatch, already-settled dispatch, and dispatch failure. Dispatch acceptance SHALL NOT be represented as agent execution or task completion.

### REQ-FOC-006: No remote approval expansion

THE SYSTEM SHALL NOT use destination owner Continue, peer enrollment, or a federation peer bearer to approve a task on another instance. Task approval remains a separate local owner operation governed by the destination's task state, WorkScope authority, lifecycle admission, and handoff rules.
