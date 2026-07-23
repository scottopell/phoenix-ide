# ADR-023: Native collaboration extends share mode and reuses durable conversation acceptance

- **Status:** Accepted
- **Date:** 2026-07-22
- **Affects:** REQ-COLLAB-001 through REQ-COLLAB-006

## Context

Phoenix already has the user journey for collaboration entry: share a
conversation URL and receive its history and live updates. Durable direct-chat
work defines target-bound message identity, acceptance before runtime delivery,
exact retry and reconciliation, queued steering, and crash recovery. Durable
WorkScope work separately defines the conversation's environment and tool
resource authority.

The design question is therefore smaller than a new collaboration system. Phoenix
must decide how shared participants enter the existing conversation contracts,
how human contributions remain attributable, and whether collaboration changes
runtime or resource ownership.

## Options considered

1. **Separate co-driver mode** — preserve live share as read-only and add another
   invitation, role, and surface for writers. This keeps old semantics isolated
   but splits one simple user journey into two products.
2. **Baton-based collaboration** — allow shared mutation but designate one active
   driver. This reduces human concurrency but imposes coordination that the
   durable message acceptance and steering queue already handle.
3. **A collaboration-specific runtime and queue** — give participants a separate
   control plane that later feeds the conversation. This duplicates message
   identity, ordering, recovery, and resource authority.
4. **Extend share mode over existing conversation contracts** — authorize
   identified share participants to submit through the existing message and
   decision boundaries. Add contributor provenance, but retain one conversation
   runtime, durable acceptance ledger, steering queue, and WorkScope.

## Decision

Phoenix extends live share mode over the existing conversation contracts. A
shared participant establishes a durable contributor identity, then submits
messages through the same durable target-bound acceptance service as the owner.
Multiple participants may submit concurrently; the existing runtime-slot and
steering contracts determine accepted and queued outcomes.

Contributor identity is new durable provenance. It is attached to accepted human
turns, materialized human messages, and accepted human decisions. It is not
inferred from a display name, user-agent string, network address, or SSE
connection.

A share participant is not a runtime or resource actor. The existing conversation
continues to own execution, tools, and its WorkScope. Share authority is a narrow
HTTP capability for one conversation's supported human actions and never grants
direct filesystem, terminal, browser, repository, settings, or lifecycle access.

Messages reuse the durable direct-chat contract. Single-choice decisions use a
separate atomic, idempotent first-valid-winner contract because they cannot queue
as independent future instructions.

## Consequences

- **Positive:** Multiplayer reuses share entry, SSE, durable message acceptance,
  queueing, reconciliation, runtime recovery, and WorkScope ownership. Concurrent
  contributors require no baton or new runtime architecture.
- **Positive:** Contributor provenance is explicit and durable rather than hidden
  in presentation metadata.
- **Negative:** Writable share links are powerful capabilities. The UI must state
  that anyone with the link can steer the agent, and revocation must stop both
  reads and writes.
- **Negative:** Human decisions need a narrow durable acceptance contract in
  addition to message submission.
- **Neutral:** Static export, forking, presence, reactions, and richer identity can
  be designed independently.

## References

- `specs/auth/requirements.md` REQ-AUTH-004 through REQ-AUTH-008
- `specs/auth/auth.allium` surfaces `OwnerConversation` and `SharedConversation`
- `specs/collaboration/requirements.md`
- `specs/durable-workflows/direct-chat-profile.allium`
- `docs/ios-durable-message-submission.md`
- `WorkScopeId`, `ResourceScopeKey`, and `EffectiveResourceAccess`
- `SendChatApplicationService`, `create_or_redirect_share`,
  `get_shared_conversation`, and `shared_sse_stream`
- `SharePage` and `MessageList`
