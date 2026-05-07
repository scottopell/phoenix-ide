---
created: 2026-05-07
priority: p1
status: ready
artifact: ui/src
---

Triage pass: every piece of UI state that lives across conversation
switches must either reset cleanly or persist intentionally. We keep
catching individual bugs (most recently the browser-view panel staying
"open" across conversation switches — caught at PR #31 review only
because a rebase forced it into view). The class of bug is generic;
the audit needs to be class-wide, not bug-by-bug.

## Symptom shape

User navigates `/c/<slug-A>` → `/c/<slug-B>` (or via the sidebar). The
URL and the conversation atom flip, but some long-lived component
keeps stale state from A: a panel stays open, a viewer stays mounted
on A's payload, a "hasActivated" sticky flag persists, a focused
element refocuses against the wrong DOM, etc.

## Mechanism in use today (task 02703 family)

Conversation-scoped React providers take `scopeKey={slug}` and reset
their state synchronously when it changes, via the "adjust state
during render" pattern:

```tsx
const [trackedScope, setTrackedScope] = useState(scopeKey);
if (trackedScope !== scopeKey) {
  setTrackedScope(scopeKey);
  if (payload !== null) setPayload(null);
}
```

Already wired: `ReviewNotesProvider`, `DiffViewerStateProvider`,
`BrowserViewStateProvider`, `FileExplorerProvider`.

## Triage scope

**Step 1 — Contexts.** Walk every `createContext` site and decide:
conversation-scoped, chain-scoped, or global? For each
conversation-scoped one without `scopeKey`, fix it. Initial sweep
candidates that need verification (not exhaustive):
- `ChainContext` (chain-scoped — different reset key, but same class
  of bug)
- `useFocusScope` (probably element-scoped, confirm)
- Any new providers added since 02703 that drifted from the pattern

**Step 2 — Local `useState` in long-lived components.** Components
mounted above `<Routes>` or that survive slug changes (e.g. layout
shells, sidebar, command palette, model picker, voice input) can hold
state that should be conversation-scoped. Audit each long-lived
component:
- Does any piece of state describe "what the user has open / typed /
  selected for this conversation"?
- If yes, does it reset on slug change? If no, fix.

**Step 3 — Hooks.** `useResizablePane`-style hooks with persistent
keys: confirm the persistence is intentional (panel widths SHOULD
survive — that's user-level UX). Identify any hooks that key off
mount but should key off conversation.

**Step 4 — Mechanism, not just fixes.** New providers will keep
drifting from the pattern unless we make it harder to forget:
- ESLint custom rule or ast-grep query that flags
  conversation-scoped providers without `scopeKey`?
- A `useConversationScopedState(slug, initial)` hook that bakes the
  reset in, so individual providers don't reimplement it?
- Doc page in `ui/AGENTS.md` or a comment header on
  `ConversationPage.tsx` calling out the rule?

Pick whichever has the best cost/benefit; don't try to do all three.

## Acceptance

- A written audit list (markdown in this task body or a follow-up
  spec) classifying every `createContext` site as
  conversation-scoped / chain-scoped / global, with status (fixed /
  already correct / intentionally persistent).
- Every conversation-scoped provider has `scopeKey` and a unit test
  verifying it resets on change.
- One concrete drift-prevention mechanism shipped (lint rule, shared
  hook, or convention doc) so this class of bug stops re-appearing on
  every new context.
- A regression test or two simulating the slug-A → slug-B navigation
  for whichever provider lacked the test.

## Why p1

Each individual bug is small but the class is everywhere. Each one
that ships erodes trust in the conversation-switching UX (one of the
most-used flows). Mechanism work cuts the long-tail rate, not just
the current backlog.
