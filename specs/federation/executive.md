# Phoenix Federation — Executive Status

## Scope

Federation is the originally approved four-slice program: stable instance identity and protected credential reads; directional enrollment and verified HTTPS transport; a persistent Coordinator WorkScope; five explicit-instance tool adapters with typed provenance, destination navigation, direct API/Python support, opaque peer notes, and isolated two-host verification. Bounded conversation subscriptions extend this program without replacing any of those obligations.

The five tools are `query_database`, `search_conversations`, `read_conversation`, `resolve_reference`, and `send_conversation_message`. Remote Bash/process control, automatic discovery, replication, broadcast, generic RPC, a distributed scheduler, transcript/token streaming, and native instance switching are not part of this scope.

## Coverage

| Requirement | Current status | Remaining verification or implementation |
|---|---|---|
| REQ-FED-001–005 | Identity, query-focused directional enrollment, TLS trust, revocation, and credential denial implemented | Exact deployed two-host acceptance |
| REQ-FED-006–007 | Local provenance and peer query authentication implemented | Thread authenticated instance provenance through all five adapters |
| REQ-FED-008 | Typed structured reference foundation and local resolve consumer implemented | Search/read/send outputs, destination navigation, UI/Python consumers |
| REQ-FED-009–010 | Direct API principles and local durable send authority exist | Saved-peer direct client and authenticated remote send adapter |
| REQ-FED-011–012 | Not implemented | Persistent Coordinator WorkScope, grounding, lifecycle, notes |
| REQ-FED-013 | Typed reference foundation implemented | Optional instance selectors and all five remote adapters |
| REQ-FED-014 | Query-focused tests implemented | Full isolated two-instance matrix and failure paths |
| REQ-FED-015 | Specified | Durable bounded authenticated subscriptions and recovery tests |

## Verification policy

Source qualification requires focused tests for every slice, exact-head hosted checks, and independent review. Live primary→devmbp acceptance is a separate Global-owned host operation. Source implementation does not depend on mutating live peers and must use isolated fixtures for remote mutation and subscription tests.
