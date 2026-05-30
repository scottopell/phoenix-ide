# Viewer Slot - Executive Summary

## Requirements Summary

Inside a conversation, the user can open one of three viewers next to (or instead of) the chat: the prose reader for files, the diff viewer for git changes, or the live browser view. Only one viewer is active at a time -- opening one closes any other. Closing the viewer returns the user to the chat. After a cold reload (iOS PWA kill, browser refresh, shared link), the user comes back to exactly the viewer they had open, with the same data, because the URL is the source of truth. Switching to a different conversation always closes the viewer; viewer state never leaks between conversations.

When the user navigates *back* in-app to a conversation they had a viewer open in (e.g. sidebar click on conversation A → conversation B → back to A), the previous viewer is restored from per-conversation localStorage. Cold reload deliberately does NOT restore -- a bare URL on reload reflects user intent ("I shared this link with no viewer params") and must not be silently overridden.

The browser viewer auto-opens on a server-side rising edge (a tool just spawned a browser session for this conversation, slot was empty), and auto-closes on a falling edge (session ended). Manual entry via the launcher chip is the recovery path when auto-open is suppressed.

## Technical Summary

The slot is a discriminated union: `kind ∈ {none, prose, diff, browser}` with per-variant data (`prose_file: ProseFile`) carried only when the discriminator matches. This makes "prose open with no file path" or "two viewers open at once" structurally unrepresentable. It is realized as a single `ViewerSlotProvider` that derives the slot from the URL on every render; there are no coordinating effects, because the discriminated union makes the single-slot mutex structural rather than something imperative code maintains. A thin `FileExplorerProvider` adapter projects the slot's prose state for the file explorer panel and command palette.

The URL search params are authoritative. `?viewer=prose&file=...&root=...`, `?viewer=diff`, `?viewer=browser`, or no `?viewer=` at all. Slot kind transitions are computed from the URL on every render -- in-memory state caches the URL, never overrides it. Cold reload restoration is automatic from this contract. A legacy `?file=...&root=...` URL with no `?viewer=` param is read as prose for backward compatibility.

`patchContext` (modified-line highlights, set when prose is opened from a patch context) is conversation-scoped React state, *not* part of the URL. It is patch provenance, not view identity, and Set<Integer> can't be URL-encoded sensibly. On URL-driven hydration of prose, `patchContext` is null and the prose reader renders the file without highlights -- the correct trade for cold-reload restoration.

The diff endpoint (`GET /api/conversations/:id/diff`) is conversation-keyed, not comparator-keyed: it returns the diff for the conversation, base determined server-side. The diff slot therefore carries no comparator in the URL (`?viewer=diff` alone); the diff viewer fetches the payload (`comparator`, `commit_log`, `committed_diff`, `uncommitted_diff`, truncation flags) on mount, keyed by conversation id. The payload is server data that re-fetches cheaply, so it is never carried in the URL or held in long-lived client state -- and the diff survives cold reload via the URL just like prose.

The browser viewer's `kind = browser` membership is independent of the live `browser_session_active` flag at any given render -- the session can die while the viewer is mounted, producing a brief "session ended" UI window that the falling-edge rule then resolves.

## Requirement Map

Each requirement maps to where its behavior lives in the implementation.

| Requirement | Implementation |
|-------------|----------------|
| **REQ-VS-001:** User Opens Prose Viewer | `ViewerSlotProvider.openProse` + URL search params; `ui/src/contexts/ViewerSlotContext.tsx` |
| **REQ-VS-002:** Unified URL Slot Contract | One `ViewerSlotProvider` derives all three viewer kinds from the URL; `FileExplorerProvider` is a thin adapter projecting prose state |
| **REQ-VS-003:** User Opens Diff Viewer | `?viewer=diff` (conversation-keyed); `ConversationDiffViewer` fetches the payload on mount from `GET /api/conversations/:id/diff` |
| **REQ-VS-004:** User Closes Active Viewer | `ViewerSlotProvider.close` clears the `viewer`/`file`/`root` params via `setSearchParams` |
| **REQ-VS-005:** URL Hydrates Prose | `deriveSlot` reads `?viewer=prose&file=&root=` (and legacy `?file=&root=`) on every render |
| **REQ-VS-006:** URL Hydrates Diff and Browser | `?viewer=diff` / `?viewer=browser` hydrate from the URL; the diff re-fetches on mount, so cold reload restores both |
| **REQ-VS-007:** Single-Slot Mutex | Structural: one `viewer` param at a time. The discriminated union enforces it; no coordinating effects |
| **REQ-VS-008:** Browser Session Rising Edge Auto-Open | `ViewerSlotProvider` watches the prev-vs-current `browserSessionActive` edge (scoped to the conversation) and opens the browser slot only when the slot is empty |
| **REQ-VS-009:** Browser Session Falling Edge Auto-Close | Same effect closes the slot on the falling edge when `kind = browser`, without clearing last-viewer storage (a system close, not a user close) |
| **REQ-VS-010:** Conversation Change Resets Slot | URL path change drops `?viewer=` params (react-router doesn't preserve search across `navigate('/c/B')`); `patchContext` resets via `useScopedState` on `scopeKey` change |
| **REQ-VS-011:** Patch Context for Prose | `ViewerSlotProvider` carries `patchContext` in `useScopedState` alongside the URL-driven file path |
| **REQ-VS-012:** Malformed URL Normalization | `deriveSlot` flags `?viewer=prose` without a file (or an unknown `?viewer=` value) as malformed; an effect normalizes it to none via `setSearchParams` |
| **REQ-VS-013:** Browser Slot Independent of Live Session | `BrowserViewPanel` renders an "ended" state when `browser_session_active = false`; the slot doesn't auto-close until the falling-edge rule fires |
| **REQ-VS-014:** Per-Conversation Viewer Persistence on In-App Nav | localStorage-backed last-viewer map (`phoenix:lastviewer:<slug>` → URL params snapshot) in `ui/src/storage/lastViewerStorage.ts`, covering all three viewer kinds. `ViewerSlotProvider` writes on every non-empty slot, clears on explicit user close, and restores on in-app *entry* (a `scopeKey` change with `useLocation().key !== 'default'`) when the URL is bare. Restore is gated on conversation entry rather than any URL change, so a programmatic URL clear (normalization, browser falling-edge close) can't immediately re-restore the closed viewer. Cold reload deliberately does not restore (D1). Hard-delete cascade clears the entry via `useConversationsRefresh.ts` |

The slot is realized as a single `ViewerSlotProvider` (mounted in `DesktopLayout`, which wraps every conversation route) that derives its state from the URL, with no coordinating effects -- the discriminated union enforces the mutex. The diff payload is a viewer-mounted fetch keyed on the conversation id, since the diff endpoint is conversation-keyed rather than comparator-addressable.

## Validation

The `.allium` file passes `allium check` with 0 errors. `allium plan` derives test obligations across value-type equality, entity field presence per `when` clause, transition coverage, surface event provision, and invariant satisfiability — those obligations are the test target, met by `ui/src/contexts/ViewerSlotContext.test.tsx` and the prose-state coverage in `ui/src/components/FileExplorer/FileExplorerContext.test.tsx`.

## Cross-Spec Relationships

- **bedrock**: `Conversation.browser_session_active` is server-authoritative and drives the rising/falling edge events here. The bedrock spec governs *when* that flag flips; this spec governs *what the slot does* in response. `Conversation.is_terminal` gates whether opening a viewer is permitted.
- **conversation_atom**: the slot is rendered alongside the atom-driven chat column, but the slot is NOT an atom field. Atom fields are server-state; slot state is user-action-driven view state.
- **projects**: the diff comparator's grammar (e.g. `HEAD..base`, branch refs) is owned by the diff endpoint, not this spec. The slot treats the comparator opaquely.
