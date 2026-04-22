---
created: 2026-04-22
priority: p1
status: ready
artifact: ui/src/components/CommandPalette/sources/FileSource.ts
---

# Cmd+P file search not returning results in current conversation's cwd

## Symptom

User reports Cmd+P command palette used to search files in the current
conversation's working directory but stopped doing so at some point. Opening
the palette while inside a conversation route (`/c/<slug>`) no longer shows
files from `conversation.cwd` -- only Conversations appear (or nothing at all).

Server-side evidence: 9 days of `phoenix.log` (2026-04-13 through 2026-04-22,
~45k lines) contain zero `GET /api/conversations/:id/files/search` requests
from the UI. The endpoint works when hit directly via curl
(`curl .../files/search?q=readme` returns a valid ranked list), so the
regression is entirely client-side -- the palette is not firing the search.

## Reproduction

1. `./dev.py up`, open the UI, navigate to any conversation (`/c/<slug>`).
2. Press Cmd+P on desktop (>=1025px wide).
3. Palette opens with input placeholder "Search conversations...".
4. Type a filename fragment you know exists in the conversation's cwd
   (e.g. `readme`, `handlers.rs`).
5. Expected: Files group appears under Conversations group with up to 15
   fuzzy-matched paths. Enter opens the file in ProseReader.
6. Observed: No Files group. Only Conversations (or "No results" if the
   query doesn't match any conversation slug).
7. Confirm with Network tab / `grep files/search phoenix.log` -- no
   `/files/search` request is made regardless of query.

## Root cause hypothesis

**Highest-likelihood:** the async search `useEffect` in
`ui/src/components/CommandPalette/CommandPalette.tsx:102-126` is being
continuously re-aborted before its 120ms debounce ever fires. Chain of
reasoning:

- `DesktopLayout.tsx:80-84` polls `loadConversations(true)` every 5s and calls
  `setConversations(freshActive)` with a *new array reference* each time,
  even when content is unchanged.
- `CommandPalette.tsx:43-46,49-57` recomputes `activeConversation` and
  `sources` via `useMemo` with `conversations` in the dep list. New array
  ref -> new `sources` ref every 5s, and a *new `FileSource` instance* is
  created via `createFileSource(...)` on every recompute (the factory has no
  identity caching).
- The search effect at `CommandPalette.tsx:102-126` has `sources` in its dep
  array. Every recompute fires the cleanup (clears the debounce timer) and
  re-enters the body, which calls `searchAbortRef.current?.abort()` and then
  `setTimeout(... 120ms)`. The 5s interval is much larger than 120ms so the
  debounce should still fire in principle, but in the common interleaving
  (typing while the 5s poll fires mid-stroke) in-flight requests get aborted
  after they start but before results arrive.

That alone wouldn't produce *zero* requests, though. Something stronger is
happening. Two additional candidates the fixer should verify first:

1. **`sources` ref churn from `actions` -> `dispatch`**: `actions` is
   recomputed whenever `conversations` changes (dep list at
   `CommandPalette.tsx:73`), which bumps the `dispatch` `useCallback` ref
   (`CommandPalette.tsx:77-83`). Even though `dispatch` isn't in the search
   effect's deps, any subtree re-render driven by the 5s poll can interleave
   with React's StrictMode effect double-invoke and produce pathological
   abort-before-timeout sequences in dev.
2. **Placeholder and empty-query behavior mislead the user/eye:**
   `CommandPaletteInput.tsx:19` always sets the placeholder to
   `"Search conversations..."` -- no mention of files -- and
   `FileSource.ts:26` returns `[]` for empty queries. If the user's mental
   model is "palette opens -> shows files for current cwd by default," the
   new behavior (types nothing, sees no files) looks like a regression even
   when the search path is functional. This is a UX regression independent
   of the technical one above and worth fixing in the same task.

The refactor that introduced this model is commit **`8c36dc9`** (2026-03-02)
"refactor: make PaletteSource.search() async, rewrite FileSource to use
search API", with follow-up fixes `091a3d9` and `cb203f6` the same day. No
subsequent commit removes or disables `FileSource` -- it is still wired in at
`CommandPalette.tsx:49-57`. The regression is in the effect/dep wiring or
the empty-query UX, not a deletion.

## Investigation log

1. **Component inventory.** `ui/src/components/CommandPalette/` has
   `CommandPalette.tsx`, `CommandPaletteInput.tsx`, `CommandPaletteResults.tsx`,
   `stateMachine.ts`, `types.ts`, `fuzzyMatch.ts`, `sources/` (with
   `ConversationSource.ts` and `FileSource.ts`), `actions/builtInActions.ts`.
   Both sources present, both instantiated.

2. **Wiring verified.** `CommandPalette.tsx:10` imports `createFileSource`;
   lines 49-57 compose sources as
   `[createConversationSource(...), ...(activeConversation ? [createFileSource(id, cwd, openFile)] : [])]`.
   When on a conversation route with a loaded conversation, `FileSource` IS
   in the sources array. The search effect at lines 102-126 maps all sources
   and flattens their results -- no filter excluding file results.

3. **Provider scope verified.** `DesktopLayout.tsx:107,152,155`:
   `<FileExplorerProvider>` wraps `<CommandPalette />`. `useFileExplorer()`
   will resolve. `openFile` from `FileExplorerContext.tsx:9-13` sets
   `proseReaderState`, which `ConversationPage.tsx:635-651` reads to render
   `<ProseReader>` inline. Open-file side effect is fine.

4. **Backend endpoint healthy.** `src/api/handlers.rs:117,2076` -- route
   `GET /api/conversations/:id/files/search` -> `search_conversation_files`.
   Uses `ignore::WalkBuilder` against `conversation.cwd`, fuzzy-ranks via
   `nucleo_matcher`, returns `{ items: [{ path, is_text_file }] }`. Direct
   curl hit against the running dev server (port 8033) returned valid ranked
   results for `q=readme` in under 100ms. Not a backend bug.

5. **`conversation.cwd` correctness verified.** For Work/Managed mode with a
   worktree, `handlers.rs:575-664` sets `effective_cwd` to the worktree path
   (not the project root), so `conversation.cwd` on the wire already points
   at the right filesystem location for file search.

6. **Log forensic.** `grep -c files/search phoenix.log` = 0 before the
   diagnostic curl I ran. Same log shows 7046 hits on `/files/list` (the
   file explorer tree) and many conversation/skill/stream requests, so the
   palette is just not issuing the search at all -- not a silent failure
   after issuing it.

7. **Git archaeology.**
   - `855a13a` (original palette, task 564) -- old FileSource with local flat walk.
   - `d23e34d` (2026-03-02) -- added FileSource to palette.
   - `8c36dc9` (2026-03-02) -- rewrote FileSource to call
     `/api/conversations/:id/files/search`, moved async search out of the
     state machine into the `useEffect` that's suspect here.
   - `091a3d9` (2026-03-02) -- prod-build lint follow-up on the rewrite.
   - `cb203f6` (2026-03-02) -- fixed a *previous* useEffect loop by extracting
     `isOpen`/`searchMode`/`searchQuery` primitives as deps instead of
     `state`. Left `sources` (unstable ref) and `actions` (unstable ref) in
     the deps -- this is the interaction that's suspect now.
   - `a9b8607` (2026-04-01) -- added `useFocusScope` to CommandPalette. Pure
     add-on, doesn't touch the search effect.
   - `fcf5edb` (2026-04-01) -- TS-error cleanup (`return undefined` branch).
   - No commit since 2026-03-02 has modified `FileSource.ts` or the search
     effect body. No commit has removed FileSource from `sources`.

## Suggested fix direction (do not implement in this task)

Two-part fix:

1. **Stabilize the search effect's dep graph.** Either:
   - Stop recomputing `sources` every 5s by memoizing it on stable primitives
     (`activeConversation.id`, `activeConversation.cwd`, `conversations`
     only when length or ids change). `conversations` in the useMemo dep is
     the root cause of the ref churn; we only need it for
     `ConversationSource`, and that source's `search()` closes over the
     array it was constructed with -- so recompute only when the
     conversation *set* actually changes (compare ids/length, or split the
     two sources into independent memos).
   - Or remove `sources` and `actions` from the search effect's dep list
     and read them via a ref (`useRef` kept in sync in a separate effect).
     The effect only needs to re-run on query/mode change, not on source
     identity change.
2. **Fix the empty-query UX so users know files are searchable.** Either
   update `CommandPaletteInput.tsx:19` to show a context-aware placeholder
   (e.g. `"Search conversations and files..."` when an `activeConversation`
   exists) or have `FileSource.search("")` return a small "recent files /
   recently edited" list instead of `[]`.

Validation after fix:
- With the palette open and a conversation active, typing any filename
  fragment should produce a log line
  `GET /api/conversations/:id/files/search?q=... 200` within ~120ms.
- Enter on a file result should swap the conversation pane for `ProseReader`
  scoped to the right `rootDir`.
- No request storm: one search per debounce, not one per 5s poll tick.
- Add a Vitest test that renders `<CommandPalette>` inside
  `<FileExplorerProvider>` with a stubbed `api.searchConversationFiles`, ticks
  past 120ms, asserts the stub was called exactly once after a query change,
  and asserts zero additional calls fire when `conversations` is replaced
  with a new-reference-same-content array (simulating the 5s poll).

## Notes

- No source file was modified during this triage.
- Per Issue Discovery Protocol, related finding worth tracking as a separate
  task if not absorbed into the fix above: `DesktopLayout.tsx:49-74`
  creates a new `conversations` array reference every 5s even when content
  is unchanged. Any consumer that memoizes on `conversations` as a dep
  inherits the ref churn. Consider deep-compare or an id-stable cache hook.
