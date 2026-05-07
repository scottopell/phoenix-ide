---
created: 2026-05-07
priority: p1
status: in-progress
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


## Audit pass 2026-05-07

### `createContext` inventory

| Context | Scope | Status |
| --- | --- | --- |
| `ReviewNotesContext` (`ui/src/contexts/ReviewNotesContext.tsx`) | Conversation | Fixed/already correct: provider receives `scopeKey={slug}` from `ConversationPage`; migrated to shared `useScopedState`; provider reset test exists. |
| `DiffViewerStateContext` (`ui/src/contexts/ViewerStateContext.tsx`) | Conversation | Fixed/already correct: provider receives `scopeKey={slug}` from `ConversationPage`; migrated to shared `useScopedState`; provider reset test exists. |
| `BrowserViewStateContext` (`ui/src/contexts/ViewerStateContext.tsx`) | Conversation | Fixed/already correct: provider receives `scopeKey={slug}` from `ConversationPage`; migrated to shared `useScopedState`; provider reset test exists. |
| `FileExplorerContext` (`ui/src/components/FileExplorer/fileExplorerTypes.ts`) | Conversation while on `/c/:slug`; shared undefined scope off conversation routes | Fixed/already correct: `DesktopLayout` derives `activeSlug` and passes it to `FileExplorerProvider`; migrated to shared `useScopedState`; provider reset test exists. |
| `ConversationContext` (`ui/src/conversation/ConversationContext.ts`) | Global app store | Intentionally persistent. It owns normalized conversation atoms and the refresh driver; route consumers select by slug. Resetting on slug change would lose cache/store state. |
| `ChainContext` (`ui/src/chain/ChainContext.ts`) | Global app store for chains | Intentionally persistent. Chain pages select by root conversation id; the store itself is not chain-scoped UI state. Follow-up audit should inspect `ChainPage` local state separately. |
| `ThemeContext` (`ui/src/hooks/useTheme.ts`) | Global user preference | Intentionally persistent across conversations and routes. |
| `FocusScopeContext` (`ui/src/hooks/useFocusScope.tsx`) | Element/modal focus stack | Intentionally element-scoped. Consumers register/unregister on mount with stable ids; it tracks active overlays/readers, not conversation payload. |
| `TreeCollectionsCtx` (`ui/src/components/FileExplorer/FileTree.tsx`) | Local render optimization context | Intentionally local to a `FileTree` render. It exposes derived tree collections to descendants and does not outlive the tree instance as cross-conversation state. |

### Long-lived state notes

- `AppRoutes.showHelp`: global keyboard-help UI, intentionally persistent and not conversation data.
- `DesktopLayout.isDesktop`: viewport-derived shell state, intentionally global.
- `DesktopLayout` pane sizes/collapsed flags via `useResizablePane`: user-level layout preferences in localStorage, intentionally persistent across conversations.
- `DesktopLayout` toasts via `useToast`: shell feedback state, not conversation payload; no reset required.
- `ConversationPage` viewer slots are provider-backed and now share the reset mechanism above.
- `ConversationPage` page-local transient state (`error`, `conversationIdForSSE`, task-approval/credential/welcome overlays, image overlay, terminal expansion) still needs the next pass: classify each as conversation-scoped or intentionally transient/global and add route-level slug-switch regressions where needed.

### Drift-prevention mechanism

Selected mechanism: shared `useScopedState(scopeKey, initialValue)` in `ui/src/hooks/useScopedState.ts`. Conversation-scoped providers now call this instead of hand-rolling `trackedScope` state. The hook itself has unit tests, and provider-specific reset tests remain as behavior guards.

## Why p1

Each individual bug is small but the class is everywhere. Each one
that ships erodes trust in the conversation-switching UX (one of the
most-used flows). Mechanism work cuts the long-tail rate, not just
the current backlog.
