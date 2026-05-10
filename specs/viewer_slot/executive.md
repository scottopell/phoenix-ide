# Viewer Slot - Executive Summary

## Requirements Summary

Inside a conversation, the user can open one of three viewers next to (or instead of) the chat: the prose reader for files, the diff viewer for git changes, or the live browser view. Only one viewer is active at a time -- opening one closes any other. Closing the viewer returns the user to the chat. After a cold reload (iOS PWA kill, browser refresh, shared link), the user comes back to exactly the viewer they had open, with the same data, because the URL is the source of truth. Switching to a different conversation always closes the viewer; viewer state never leaks between conversations.

When the user navigates *back* in-app to a conversation they had a viewer open in (e.g. sidebar click on conversation A → conversation B → back to A), the previous viewer is restored from per-conversation localStorage. Cold reload deliberately does NOT restore -- a bare URL on reload reflects user intent ("I shared this link with no viewer params") and must not be silently overridden.

The browser viewer auto-opens on a server-side rising edge (a tool just spawned a browser session for this conversation, slot was empty), and auto-closes on a falling edge (session ended). Manual entry via the launcher chip is the recovery path when auto-open is suppressed.

## Technical Summary

The slot is a discriminated union: `kind ∈ {none, prose, diff, browser}` with per-variant data (`prose_file: ProseFile`, `diff_key: DiffComparator`) carried only when the discriminator matches. This makes "prose open with no file path" or "two viewers open at once" structurally unrepresentable, replacing the current implementation's three independent React contexts plus three coordinating `useEffect`s in `ConversationPage.tsx` with one type.

The URL search params are authoritative. `?viewer=prose&file=...&root=...`, `?viewer=diff&commit=...&base=...`, `?viewer=browser`, or no `?viewer=` at all. Slot kind transitions are computed from the URL on every render -- in-memory state caches the URL, never overrides it. Cold reload restoration is automatic from this contract; the prose-only URL persistence shipped in PR #47 is the first slice of this design and the existing `FileExplorerProvider` is the prototype the unified provider will follow.

`patchContext` (modified-line highlights, set when prose is opened from a patch context) is conversation-scoped React state, *not* part of the URL. It is patch provenance, not view identity, and Set<Integer> can't be URL-encoded sensibly. On URL-driven hydration of prose, `patchContext` is null and the prose reader renders the file without highlights -- the correct trade for cold-reload restoration.

The diff payload (`commit_log`, `committed_diff`, `uncommitted_diff`, truncation flags) is server-fetched on viewer mount, NOT carried in URL or held in long-lived client state. `DiffComparator` is the URL-encodable key; the payload is regenerated cheaply on demand.

The browser viewer's `kind = browser` membership is independent of the live `browser_session_active` flag at any given render -- the session can die while the viewer is mounted, producing a brief "session ended" UI window that the falling-edge rule then resolves.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-VS-001:** User Opens Prose Viewer | ✅ Complete | `FileExplorerProvider.openFile` + URL search params (PR #47); `ui/src/components/FileExplorer/FileExplorerContext.tsx` |
| **REQ-VS-002:** Unified URL Slot Contract | 🚧 Partial | Today: prose is URL-driven; diff and browser are scoped React state. Unification is the deliverable spec'd here -- one provider, one URL contract, three viewers |
| **REQ-VS-003:** User Opens Diff Viewer | 🚧 Partial | Today: `DiffViewerStateProvider` carries the full payload in scoped state. Spec wants URL holding the comparator; viewer re-fetches the payload on mount |
| **REQ-VS-004:** User Closes Active Viewer | ✅ Complete | Per-viewer close handlers in ConversationPage; URL clears via `setSearchParams` |
| **REQ-VS-005:** URL Hydrates Prose | ✅ Complete | `FileExplorerContext.tsx` reads `?file=` / `?root=` on mount |
| **REQ-VS-006:** URL Hydrates Diff and Browser | ❌ Not Started | Diff and browser don't read or write the URL today; cold reload loses them |
| **REQ-VS-007:** Single-Slot Mutex | 🚧 Partial | Enforced today by two coordinating `useEffect`s in `ConversationPage.tsx:158-162` (file open → close diff) and `:167-171` (anything else open → close browser), plus imperative clearing in the `handleOpenBrowserView` click handler at `:176-180`. Spec wants the discriminated union to make this structural — the effects deleted, the type system enforces |
| **REQ-VS-008:** Browser Session Rising Edge Auto-Open | ✅ Complete | `ConversationPage.tsx:505-521` watches the prev-vs-current edge and calls `openPanel()` when slot is empty |
| **REQ-VS-009:** Browser Session Falling Edge Auto-Close | ✅ Complete | Same effect: `wasActive && !browserSessionActive` → `closeBrowserView()` |
| **REQ-VS-010:** Conversation Change Resets Slot | ✅ Complete | URL path change naturally drops `?viewer=` params (react-router doesn't preserve search across `navigate('/c/B')`); scoped state resets via `useScopedState` on `scopeKey` change |
| **REQ-VS-011:** Patch Context for Prose | ✅ Complete | `FileExplorerContext.tsx` carries `patchContext` in `useScopedState` alongside the URL-driven file path |
| **REQ-VS-012:** Malformed URL Normalization | ❌ Not Started | Today, `?viewer=prose` without `?file=` is undefined behaviour. Spec mandates normalization to `?viewer=` (none) and a corrective `setSearchParams` |
| **REQ-VS-013:** Browser Slot Independent of Live Session | ✅ Complete | `BrowserViewPanel` renders an "ended" state when `browser_session_active = false`; the slot doesn't auto-close until the falling-edge rule fires |
| **REQ-VS-014:** Per-Conversation Viewer Persistence on In-App Nav | ✅ Complete (prose-only) | localStorage-backed last-viewer map (`phoenix:lastviewer:<slug>` → URL params snapshot) in `ui/src/components/FileExplorer/lastViewerStorage.ts`. `FileExplorerProvider` writes on every prose open, clears on explicit close, and restores on in-app entry (`useLocation().key !== 'default'`) when the URL is bare. Cold reload deliberately does not restore (D1). Hard-delete cascade clears the entry via `useConversationsRefresh.ts`. Diff/browser viewers join when REQ-VS-006 lands |

**Progress:** 8 of 14 complete, 4 partial, 2 not started

The path to ✅ across the board is one focused task: collapse the three providers (`FileExplorerProvider`, `DiffViewerStateProvider`, `BrowserViewStateProvider`) into a single `ViewerSlotProvider` that derives its state from the URL, deletes the three coordinating effects in `ConversationPage.tsx`, and lets the type system enforce the mutex. The diff payload moves to a viewer-mounted fetch keyed on the URL comparator. PR #47's prose work is the prototype; this spec is the contract the unification needs to satisfy.

## Validation

The `.allium` file passes `allium check` with 0 errors. `allium plan` derives 58 test obligations across value-type equality, entity field presence per `when` clause, transition coverage, surface event provision, and invariant satisfiability — those obligations are the test target for the unification work.

## Cross-Spec Relationships

- **bedrock**: `Conversation.browser_session_active` is server-authoritative and drives the rising/falling edge events here. The bedrock spec governs *when* that flag flips; this spec governs *what the slot does* in response. `Conversation.is_terminal` gates whether opening a viewer is permitted.
- **conversation_atom**: the slot is rendered alongside the atom-driven chat column, but the slot is NOT an atom field. Atom fields are server-state; slot state is user-action-driven view state.
- **projects**: the diff comparator's grammar (e.g. `HEAD..base`, branch refs) is owned by the diff endpoint, not this spec. The slot treats the comparator opaquely.
