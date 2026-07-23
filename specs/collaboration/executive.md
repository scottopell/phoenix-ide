# Native Collaboration -- Executive Summary

## Overview

Native collaboration evolves the existing live share URL into multiplayer mode.
Participants identify themselves, then use the shared page to send messages and
answer pending agent decisions in one conversation. There is no co-driver mode,
baton, participant runtime, or collaboration-specific message queue.

Most hard message-delivery work belongs to foundations already under review:

- PR #556 defines durable target-bound message acceptance, exact-ID retries,
  conflict detection, queued steering, reconciliation independent of transcript,
  runtime delivery after acceptance, and crash recovery.
- PR #557 separates the conversation's durable WorkScope and resource ownership
  from actor authority. Share participants submit into the conversation; they do
  not become tool or resource actors.

The remaining collaboration work is contributor identity and attribution,
token-scoped mutation routes and UI, and atomic attributed decisions such as task
approval and question response.

## Foundation map

| Need | Foundation | Collaboration delta |
|---|---|---|
| Shared history and live state | Existing share hydration, SSE, and `SharePage` | Make the page interactive after identity is established. |
| Concurrent human messages | PR #556 direct-chat acceptance and queued steering | Authorize the same service through the share token and preserve contributor provenance. |
| Retry and reconnect | PR #556 exact-ID reconciliation | Reuse it from `SharePage`; do not infer acceptance from SSE. |
| Agent and worktree execution | PR #557 WorkScope ownership and resource authority | Keep participants outside resource authority; all actions enter one conversation runtime. |
| Contributor accountability | Not provided by either foundation | Add normalized contributor identity and attribution to human messages and decisions. |
| Simultaneous decisions | Existing state checks are not a durable idempotency contract | Add one atomic, retry-safe winner per pending decision and record its contributor. |

## Recommended implementation slice

1. **Identify:** joining the live share page asks for a display name and creates or
   resumes an opaque contributor identity scoped to that live share.
2. **Send:** the shared composer calls a token-authorized adapter over the same
   message application service used by the owner. It supplies a client-generated
   message ID and contributor ID.
3. **Observe:** every participant sees attributed persisted messages and the
   existing queued-message state. The submitter sees typed created/replayed,
   accepted/queued/cancelled, conflict, and rejection outcomes.
4. **Reconnect:** unresolved submissions use exact-ID reconciliation. SSE remains
   the live update channel, not acceptance authority.
5. **Decide:** shared task approvals and question answers use an atomic
   first-valid-winner contract, preserve contributor attribution, and return an
   idempotent replay or visible conflict.
6. **Revoke:** revoking the share token cuts off both shared reads and mutations.

## Persistence direction

Use normalized relational records:

- a contributor record with opaque ID, share token/session scope, display label,
  and timestamps;
- contributor identity on durable accepted human turns and materialized human
  messages;
- contributor identity on accepted human decision records.

Do not place contributor identity only in browser state, `display_data`,
`user_agent`, or an SSE event. Do not duplicate the message queue or WorkScope
model.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| REQ-COLLAB-001 | Planned | Share hydration/SSE exist; identity, composer, and decision UI are missing. |
| REQ-COLLAB-002 | Planned | No contributor identity model exists. |
| REQ-COLLAB-003 | Foundation in review | PR #556 supplies durable submission; contributor provenance and token-authorized adapter remain. |
| REQ-COLLAB-004 | Planned | Existing handlers check state but do not provide an attributed idempotent decision contract. |
| REQ-COLLAB-005 | Foundation in review | Existing token scoping and PR #557's authority split help; mutation allowlist and revocation behavior remain. |
| REQ-COLLAB-006 | Foundation in review | PR #556 supplies reconciliation; shared-page adoption and attributed replay remain. |

## Acceptance criteria

- Multiple browsers can join one live share URL and establish distinct contributor
  identities without creating separate runtimes or work scopes.
- Each participant can submit a message with a unique client message ID.
- Concurrent messages converge through the existing durable acceptance and
  steering contracts; each participant sees the authoritative outcome.
- Materialized human messages show durable contributor attribution.
- Refresh after an ambiguous response reconciles by message ID without duplicate
  acceptance.
- Any identified participant can submit a pending supported decision; one valid
  decision wins atomically, records its contributor, and later conflicts are
  explained.
- Revoking the share token blocks subsequent shared reads and mutations.
- Share participants cannot access owner-only controls or direct work-environment
  resources.

## Deferred work

- single-page HTML export for passive read-only sharing;
- conversation forking;
- presence, typing indicators, reactions, pointers, and help signals;
- avatars, accounts, identity-provider integration, and role systems;
- CRDT/OT shared draft editing;
- arbitrary owner lifecycle or resource controls from the shared page.
