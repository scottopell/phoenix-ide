# ADR-023: Native collaboration evolves share mode into multi-driver co-driving

- **Status:** Accepted
- **Date:** 2026-07-22
- **Affects:** REQ-COLLAB-001 through REQ-COLLAB-009

## Context

Phoenix already supports share links. The share journey is the natural place for
collaboration: a user shares a conversation, collaborators open it, everyone sees
the same transcript, and the group works with one LLM agent together.

The product fork is whether live sharing should remain a read-only viewer mode
with a separate co-driver mode beside it, or whether share mode itself should
become multiplayer. The collaboration model also has to decide whether Phoenix
should enforce turn-taking or allow multiple humans to steer at the same time.

## Options considered

1. **Separate co-driver mode beside read-only share** — preserve current live
   share semantics and add a second invitation or role for co-driving. This keeps
   old safety properties simple, but splits the user journey and makes the main
   multiplayer feature feel separate from sharing.
2. **Baton-based co-driving** — exactly one participant is the active driver for
   conversation-advancing actions. This gives Phoenix a simple mutation gate, but
   it imposes meeting protocol on a workflow that may be faster and messier when
   several people can contribute at once.
3. **Share mode evolves into multi-driver co-driving** — the shared live
   conversation becomes the multiplayer surface. Any participant with contributor
   identity can submit messages or decisions; Phoenix serializes accepted actions
   on the server and attributes every human contribution. This embraces the share
   journey and supports chaotic collaboration, but requires careful queueing,
   rejection, and attribution semantics.
4. **Awareness-first collaboration** — add presence, reactions, pointers, or help
   requests before allowing participants to steer the agent. This is lower risk,
   but it misses the central goal: collaborators need to work directly with the
   agent, not only advise the owner.

## Decision

Phoenix native collaboration evolves live share mode into multi-driver
co-driving. A shared live conversation is the collaboration surface. Participants
establish contributor identity and may send human instructions or submit human
decisions without acquiring a baton or being the single active driver.

Phoenix accepts the chaos at the product level and controls it at the server
boundary: accepted actions are serialized into one authoritative conversation
order, contributor identity is persisted with each human action, and stale or
conflicting submissions are queued or rejected with visible explanations.

Passive read-only sharing should move to a separate single-page HTML export
flow. That export can be implemented independently and does not need to block the
first live collaboration slice.

## Consequences

- **Positive:** The feature grows from the existing share journey; participants
  can collaborate naturally without negotiating a baton; the model supports more
  than a pair or trio; attribution keeps the transcript accountable.
- **Negative:** The server must handle ordering, state-stale submissions,
  first-valid decision acceptance, queued actions, and clear rejection messages.
  This is more complex than a single-driver gate.
- **Neutral:** Static HTML export and conversation forking remain useful adjacent
  ideas, but they are separate work from the first multiplayer implementation.

## References

- `specs/auth/requirements.md` REQ-AUTH-004 through REQ-AUTH-008
- `specs/auth/auth.allium` surfaces `OwnerConversation` and `SharedConversation`
- `specs/collaboration/requirements.md`
- `create_or_redirect_share`, `get_shared_conversation`, and `shared_sse_stream`
- `SharePage` and `MessageList`
