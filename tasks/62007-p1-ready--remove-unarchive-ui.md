---
created: 2026-05-23
priority: p1
status: ready
artifact: ui/src/components/Sidebar.tsx
---

# Remove unarchive UI affordances (archive is now terminal)

## What

Backend in PR #135 removed the `unarchive_conversation` and
`unarchive_chain` endpoints. Specs (`specs/bedrock/`, `specs/api/`,
`specs/tmux-integration/`, `specs/browser-tool/`) now state archive is
a terminal lifecycle transition — the resource-cleanup cascade runs at
archive time and the conversation cannot resume in-place.

UI still exposes unarchive surfaces that now hit dead endpoints. Need
to strip them:

- `ui/src/components/Sidebar.tsx` — `handleUnarchive` callback + any
  menu items / buttons that fire it.
- `ui/src/components/ConversationList.tsx` — `onUnarchive` /
  `onUnarchiveChain` props + the buttons that render them. Cascade
  upwards to anything that supplies these props.
- `ui/src/syncQueue.ts` — `case 'unarchive':` and
  `case 'unarchive_chain':` replay branches.
- `ui/src/cache.ts` — `unarchive` and `unarchive_chain` op variants.
- `ui/src/api.ts` — `unarchiveConversation` and `unarchiveChain`
  methods.
- `ui/src/generated/ChainView.ts` — generated comment mentions an
  "Unarchive button"; check what's actually being generated and update
  the source if needed (or just regenerate after the chain code is
  cleaned).

## Why p1

User has a zero-tolerance position on this — archive being reversible
was identified as a design mistake during PR #135 review. Backend
already disagrees with the UI; leaving the buttons live means clicking
them produces a 404 (or worse, a confusing partial state). Ship the
backend now, fix the UI as the very next task.

## Validation

- Click-test every conversation/chain context menu in the sidebar:
  archive is the only terminal-style affordance shown; no "unarchive"
  anywhere.
- Archived conversations still appear in the archived list (still
  readable / inspectable, just not resumable).
- `./dev.py check` — vitest + eslint + tsc + e2e all pass.
- Manual: archive a conversation, confirm no UI affordance offers
  resumption.

## Out of scope

- Removing the `archived` column or any data migration. The flag stays
  — it just becomes write-once (archive → true, never back).
- Renaming "archive" to anything else. The verb is fine; only the
  semantics change.
- Adding any new "restore from archive" UX (e.g. duplicate-as-new
  conversation). If users need that, capture as a separate followup.

## Context

Surfaced during Copilot review of PR #135 (the resource-cleanup
cascade unification). PR #135 / PR #136 / PR #139 land first with the
backend + spec changes; this task closes the gap on the UI side.
