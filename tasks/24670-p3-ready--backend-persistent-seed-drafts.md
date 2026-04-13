---
created: 2026-04-12
priority: p3
status: ready
artifact: pending
---

# backend-persistent-seed-drafts

## Problem

The seed primitive (task 24666) stores the pre-filled draft prompt in
browser `localStorage` under the key `seed-draft:<conv-id>` between
the spawning action and the target conversation's first mount. This
works for the common case: spawn → navigate → hydrate → clear.

It fails in these cases:
- User spawns a seeded conversation and reloads the page before the
  InputArea hydrates — the draft is still there (localStorage
  persists), but the user may also have edited and saved a different
  draft via the normal `useDraft` path, causing confusion about which
  one wins
- User spawns on one browser/device and opens the new conversation
  on another — the draft is gone
- Browser localStorage is cleared (private browsing, manual clear) —
  the draft vanishes

None of these is a showstopper for v1. But for a primitive that's
meant to grow into a taskmd panel integration, a "start task from
email link" flow, and possibly cross-user sharing, persistence
belongs on the server.

## Scope

- Add an `initial_draft TEXT` nullable column to the `conversations`
  table via an idempotent `ALTER TABLE` migration
- Accept `initial_draft` on the `POST /api/conversations/new` request
  when a seed payload is present
- Expose `initial_draft` on the conversation API response (same async
  enrichment path that already carries `home_dir`, `seed_parent_slug`,
  etc.)
- Frontend: `ConversationPage` hydration effect prefers the backend
  `initial_draft` over `localStorage[seed-draft:<id>]`. On first
  successful hydrate, issue a `DELETE` or `PATCH` to clear it so
  reloads don't re-hydrate.
  - Alternatively: the backend auto-clears `initial_draft` when the
    first user message is sent via the runtime. Simpler, no extra
    API call.
- Keep the localStorage path as a fallback / transport mechanism, OR
  remove it entirely since the backend is now authoritative. Prefer
  removing — two sources of truth for the same state is a trap.

## Out of scope

- Multi-device sync of draft *edits* (the user typing in the input
  area in one tab, seeing it on another). That's a real-time
  collaboration feature, way beyond this task.
- Encrypting drafts at rest. Conversations aren't encrypted today;
  drafts shouldn't be special.

## Related

- Parent: 24666 (seed primitive v1)
- REQ-SEED-001 explicitly noted "client-side localStorage is
  acceptable for v1; persistence across devices is not required" —
  this task is the v2 of that decision
