# Native Collaboration -- Executive Summary

## Overview

Native collaboration evolves share mode into multiplayer mode. The shared URL is
not a separate read-only demo surface plus a second co-driver surface; it is the
live place where collaborators join, see the same transcript, and steer the same
agent.

The recommended first slice allows many participants to send messages and submit
human decisions. Phoenix keeps the experience accountable by establishing a
contributor identity for each participant, recording who submitted each action,
and serializing accepted actions at the server boundary.

## Recommended first slice

1. **Join:** the owner shares the live conversation URL. A joining browser
   establishes a contributor identity before mutation controls are enabled.
2. **Synchronize:** every participant sees the same history, live transcript,
   queued human actions, and contributor labels.
3. **Drive:** any participant can send a human instruction. Phoenix accepts and
   orders submissions on the server.
4. **Decide:** any participant can answer an agent prompt or approve a pending
   single-choice decision. The first valid server-accepted decision wins; later
   conflicting decisions are rejected with a visible explanation.
5. **Attribute:** accepted human messages, task decisions, prompt answers, and
   continuation decisions record and render the contributor.
6. **Explain queue/reject outcomes:** if a submitted action cannot apply to the
   current state, Phoenix either queues it for the next valid human-input point or
   tells the submitter why it was rejected.

## Collaborative opportunities anchored to share mode

| Opportunity | Extension point | Planning note |
|---|---|---|
| Live multiplayer join flow | `create_or_redirect_share`, share-token persistence, auth/session handling | Evolve the current share URL into the live co-driving entry point instead of adding a separate co-driver mode. |
| Shared live transcript | `get_shared_conversation`, `shared_sse_stream`, `SharePage`, owner conversation SSE | Reuse history hydration and SSE broadcast patterns, then add contributor identity and queued-action events. |
| Multi-driver steering | owner composer/actions, send-chat endpoint, runtime handle lookup/wake paths | Allow many participants to send; order accepted actions at the server boundary. |
| Contributor identity | message persistence, approval/task decision records, `MessageList` rendering | Attribution must be persisted with human actions, not inferred from browser state. |
| Shared approval flows | task proposal, user-question, continuation surfaces | Use first-valid server acceptance for single-choice decisions; reject conflicting later decisions visibly. |
| Presence and help signals | SSE event model, lightweight collaboration metadata | Treat as awareness only; no hidden mutation authority. |
| Static read-only sharing | future export endpoint and simple HTML rendering | Defer. A single-page HTML export should eventually cover passive sharing without keeping read-only live share mode as the main product path. |
| Fork from this point | conversation creation/retrieval and message history boundaries | Defer. Forking is useful for divergent exploration but is too large for the first collaboration slice. |

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| REQ-COLLAB-001 | Planned | Requires changing share-mode semantics from read-only live view to live co-driving surface. |
| REQ-COLLAB-002 | Planned | Needs contributor identity for shared-link participants before accepting actions. |
| REQ-COLLAB-003 | Planned | Removes baton/single-driver constraint and avoids artificial participant caps. |
| REQ-COLLAB-004 | Planned | Needs server-side action ordering and queued-action broadcast. |
| REQ-COLLAB-005 | Planned | Needs explicit queue-or-reject behavior for state-stale submissions. |
| REQ-COLLAB-006 | Planned | Needs first-valid acceptance semantics for single-choice human decisions. |
| REQ-COLLAB-007 | Planned | Needs runtime wake/start policy tied to accepted live-share actions. |
| REQ-COLLAB-008 | Planned | Awareness events can follow after the multi-driver contract exists. |
| REQ-COLLAB-009 | Planned | Requires durable contributor attribution and accepted-action replay. |

## Spec work before implementation

- Update `specs/auth/` so live share mode is no longer specified as permanently
  read-only once collaboration is implemented.
- Add a collaboration Allium spec for actors, contributor identity, action
  ordering, first-valid decisions, stale-action queue/reject behavior, and
  reconnect replay.
- Define SSE wire variants for contributor presence, accepted queued actions,
  rejected submissions, and collaboration state before adding UI validation in
  `ui/src/sseSchemas.ts`.
- Define persistence shape for contributor identities, accepted human actions,
  queued submissions, and decision attribution before adding APIs.

## Acceptance criteria for the first implementation slice

- Multiple browser sessions can join one live shared conversation and establish
  contributor identities.
- Every participant sees the same history, live transcript, queued human actions,
  and contributor labels.
- More than one participant can submit human messages without a baton or active
  driver role.
- Concurrent submissions appear in one server-defined order for every
  participant.
- For a single-choice approval or prompt, the first valid server-accepted decision
  wins and later conflicting submissions are rejected visibly.
- Refreshing a participant browser restores transcript, accepted queued actions,
  and contributor attribution.

## Deferred work and boundaries

- Single-page HTML export for passive read-only sharing is valuable but belongs in
  a separate implementation session.
- Forking from a shared point is valuable for divergent exploration but is too
  large for the first collaboration slice.
- SSE remains server-to-client; it is not a reverse-control channel.
- CRDT/OT collaborative text editing is out of scope unless shared draft editing
  becomes a requirement.
- Phoenix should avoid artificial participant caps; practical resource limits may
  exist, but the product model is not pair/trio-only.
