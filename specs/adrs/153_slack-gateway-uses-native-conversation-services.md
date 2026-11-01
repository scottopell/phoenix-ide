# ADR-153: Slack ingress uses native conversation services

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-SG-004, REQ-SG-008, REQ-SG-009, REQ-CCR-001

## Context

Slack triggers need durable acceptance, ordinary local execution and a return
address. Phoenix already owns conversation creation and direct-turn delivery.

## Options considered

1. A separate process calling public HTTP endpoints: independent lifetime, but
   additional authentication and durable handoff surfaces.
2. An application background service using internal conversation services:
   shared existing authority and persistence, coupled to application lifetime.
3. A new agent runtime behind Slack: duplicates lifecycle and permission policy.

## Decision

Run the gateway as an application background service. Use the MCP manager for
Slack access and shared internal services for native creation and direct turns.
Persist gateway source, request, association and return evidence in SQLite.
Slack owns transport interpretation; native conversations own execution. Keep
the gateway outside the HTTP handler layer without adding a plugin framework.
Create Direct conversations rooted at the server user's home, with no inferred
repository attachment, even when that home is Git-backed.

## Consequences

- Existing permissions, recovery and native UI apply to Slack work.
- Application shutdown pauses discovery; durable state supports resumption.
- Another transport would need its own adapter and equivalent source evidence.
  Supporting additional transports is not an implementation commitment.

## References

- `specs/slack-gateway/requirements.md`
- `specs/conversation-creation/requirements.md`
- `slack_accept_native_request`, `send_native` and `CreationIntent`
