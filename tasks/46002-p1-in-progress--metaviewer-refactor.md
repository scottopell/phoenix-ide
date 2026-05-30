# MetaViewer Refactor

## Summary

Refactor Phoenix's viewer stack into a clean typed `MetaViewer` architecture before replacing the diff renderer. Today `ProseReader` owns too many unrelated responsibilities: Markdown rendering, code/text line rendering, HTML source rendering, sandboxed HTML iframe preview, file-note behavior, scroll restoration, changed-line highlighting, copy behavior, and annotation plumbing. `FileViewer` recently added image rendering as a separate option in `ViewerShell`, which points toward the right direction: a central viewer shell/router with specialized body renderers.

This task establishes the final internal viewer API shape so the follow-up Pierre diff replacement can land against stable abstractions rather than inheriting today's ProseReader/DiffView coupling.

## Decisions already made

- Priority: P1.
- This task is strictly sequenced before `pierre-diff-replacement`.
- Visible UX change is allowed only as small cleanup required by the refactor: normalize header actions, loading/error/image states, renderer naming, and shell behavior where it reduces future integration risk.
- `MetaViewer` renders already-resolved payloads. It does **not** own `/api/files/read` or diff fetching in this task, but this task **does** define the typed boundary between viewer open requests and resolved render payloads.
- Establish the canonical viewer slot/state contract in this task: file, diff, image, HTML, and future viewer kinds should flow through one mutually exclusive viewer model, including `ViewerStateContext` and viewer restoration behavior where applicable.
- Retire the `ProseReader` component/name internally. Do not keep it as a long-term compatibility wrapper.
- Do not add `@pierre/diffs` in this task. Define the final diff payload/API shape using current DiffView underneath.
- Review-note behavior should move toward shared hooks, not one giant MetaViewer god-component and not fully duplicated renderer-owned logic.

## Aligned scope decisions

Resolved with the task owner before implementation. These refine (and where noted,
supersede) the looser prose above.

- **Single-slot unification is in scope, in full.** Collapse `FileExplorerProvider`,
  `DiffViewerStateProvider`, and `BrowserViewStateProvider` into one
  `ViewerSlotProvider` whose state is the discriminated union already specified in
  `specs/viewer_slot/` (`kind ∈ {none, prose, diff, browser}`, URL as source of
  truth). Delete the three coordinating `useEffect`s in `ConversationPage.tsx`; the
  type system enforces the mutex. This drives `specs/viewer_slot/` REQ-VS-002/003/006/007/012
  to complete; its 58 Allium obligations are the acceptance target for the slot half.
- **Browser is a slot member, not a resolved payload.** `kind = browser` participates
  in the mutex, but carries a live-session handle (`browser_session_active`) routed to
  the existing browser component — it is not a `MetaViewerPayload`. MetaViewer renders
  only resolved content kinds.
- **Per-kind loaders; `FileViewer` keeps its role.** `FileViewer` stays the file
  loader, retyped to emit a typed `MetaViewerPayload`. The diff viewer gets a parallel
  **mount-time** loader keyed on the URL comparator, moving the payload out of
  `DiffViewerStateProvider` (per viewer_slot REQ-VS-003/006). No central resolver / no
  DI registry — viewers fetch their own payloads on mount, matching `specs/viewer_slot/`.
- **Image becomes a first-class `ImageViewerBody`**, out of `FileViewer`'s inline
  special-case.
- **Full rename now.** Remove `ProseReader` as a production import path; rename
  `prose-reader-*` CSS classes to renderer-neutral names. Bounded test-only aliases are
  the only permitted shim.
- **Dedup file-type classification.** The server already returns `file_type`
  (`specs/prose-feedback/` REQ-PF-004) while the client re-derives via
  `getFileType`/`getLanguage`. Extract one tested typed utility and resolve the
  redundant client-side derivation.
- **Split review-note hooks.** `useFileReviewNotes` + `useDiffReviewNotes` over a small
  shared core. Preserve all three anchor kinds (`file`, `diff`, `diff-file`) and
  `formatNotesForSend` semantics. Do not build a generic cross-renderer framework.
- **`TaskApprovalReader`: audit + document.** Reuse extracted primitives only where
  trivial; otherwise document why approval-plan rendering stays separate. It layers on
  top of the slot (see `specs/viewer_slot/` exclusions) and is out of the mutex.
- **Specs: update existing only.** Refresh `specs/viewer_slot/` status and
  `specs/prose-feedback/` for the renderer-body split + file-type dedup. No new
  `specs/metaviewer/` directory and no new Allium — payload routing is a data transform
  (AGENTS.md), and the slot state machine is already specified in `specs/viewer_slot/`.
- **Delivery: one PR, commit-staged** on `claude/p1-tasks-pierre-diff-A8fbM`
  (extract bodies → unified slot/state contract → notes hooks → rename/CSS/spec
  cleanup). `./dev.py check` green.

## Goals

1. Introduce a typed central `MetaViewer` API that routes resolved viewer payloads to specialized body renderers inside `ViewerShell`.
2. Split current `ProseReader` responsibilities into focused renderer bodies:
   - `MarkdownViewerBody`
   - `CodeViewerBody`
   - `TextViewerBody`
   - `HtmlViewerBody`
   - `ImageViewerBody`
   - a current-diff adapter/body that preserves today's `DiffView` behavior until the Pierre replacement task
3. Preserve existing review-note semantics for files and diffs.
4. Preserve current FileViewer, DiffViewer, NotesPanel, and WorkActions user behavior while cleaning internal component boundaries.
5. Make the follow-up Pierre diff migration a renderer swap behind the final MetaViewer/diff body contract, not another architectural refactor.
6. Define the viewer open-state contract: canonical open request types, resolved payload types, one mutually exclusive viewer slot, and restoration behavior for file/diff/image/HTML viewer kinds.
7. Extract file type and language classification out of `ProseReader` into a tested typed utility.
8. Audit `TaskApprovalReader` for reusable Markdown/annotation primitives and either reuse the extracted pieces or document why approval-plan rendering remains separate.

## Non-goals

- Do not replace the diff renderer with Pierre in this task.
- Do not add `@pierre/diffs` in this task.
- Do not replace ProseReader code/plain-text rendering with Pierre CodeView yet.
- Do not redesign the viewer UI beyond small cleanup necessary to make the architecture coherent.
- Do not remove existing HTML preview/open-in-browser functionality.
- Do not change review-note formatted output semantics.
- Do not change message rendering (`MessageComponents`, `StreamingMessage`) except if a tiny shared utility extraction is clearly required and low-risk.
- Do not design a fully generic cross-renderer annotation/scroll capability framework in this task. Standardize practical shared hooks for today's file-review lifecycle; Pierre-specific typed `scrollTo`/annotation integration belongs in the follow-up diff replacement task.

## Current responsibilities to split

Current files/components to inspect and preserve:

- `ui/src/components/FileViewer.tsx`
  - file loading
  - text/image dispatch
  - loading/error/image states
- `ui/src/components/ProseReader.tsx`
  - Markdown rendering via `ReactMarkdown` + `remark-gfm`
  - code/source rendering via `SyntaxHighlighter`
  - plain text line rendering
  - HTML source/preview toggle
  - sandboxed iframe preview via `/preview${absolutePath}`
  - open HTML in browser
  - file review notes
  - annotation dialog
  - notes panel
  - scroll restore and jump-to-line/block
  - patch-context modified-line highlighting
  - copy file contents
- `ui/src/components/DiffViewer.tsx`
  - compatibility shim into `DiffView`
- `ui/src/components/viewer/DiffView.tsx`
  - committed/uncommitted diff display
  - diff notes, anchors, NotesPanel jump/send
- `ui/src/components/viewer/ViewerShell.tsx`
- `ui/src/components/viewer/NotesPanel.tsx`
- `ui/src/components/viewer/AnnotationDialog.tsx`
- `ui/src/components/viewer/formatNotes.ts`
- `ui/src/contexts/ReviewNotesContext.tsx`
- `ui/src/contexts/ViewerStateContext.tsx`
- `ui/src/storage/lastViewerStorage.ts`
- `ui/src/components/TaskApprovalReader.tsx`
  - rendered Markdown approval-plan body
  - annotatable Markdown blocks
  - candidate for shared Markdown/annotation primitives, or explicit documented separation

## Proposed target shape

Exact filenames can change during implementation, but the resulting concepts should be explicit:

```ts
type MetaViewerPayload =
  | MarkdownViewerPayload
  | CodeViewerPayload
  | TextViewerPayload
  | HtmlViewerPayload
  | ImageViewerPayload
  | DiffViewerPayload;
```

The payload is **resolved/renderable**. For files, `FileViewer` remains the loader and normalizes `/api/files/read` results into a `MetaViewerPayload`.

Also define the request/payload boundary explicitly:

```ts
type ViewerOpenRequest =
  | FileViewerOpenRequest
  | DiffViewerOpenRequest
  | ImageViewerOpenRequest
  | HtmlViewerOpenRequest;
```

`ViewerOpenRequest` represents intent such as "open this file path" or "open this conversation diff". Loader/adapters resolve those requests into `MetaViewerPayload`. `MetaViewer` itself renders only resolved payloads.

The viewer state model should expose a single mutually exclusive viewer slot. File, diff, image, HTML preview/source, and future viewer kinds should not be modeled as parallel independent slots. Update `ViewerStateContext`, restoration behavior, and `lastViewerStorage` as needed to make this contract explicit.

Suggested payload properties:

- common:
  - `kind`
  - `title`
  - `absolutePath` or stable viewer identity where applicable
  - `onClose`
  - `onSendNotes`
  - `inline`
- text-like file payloads:
  - `filePath`
  - `rootDir`
  - `content`
  - optional `patchContext`
- image payload:
  - `url`
  - `mimeType`
  - `fileName`
  - `absolutePath`
- diff payload:
  - current committed/uncommitted diff inputs and truncation metadata
  - explicit section identity preserved for notes

Suggested components/hooks:

- `MetaViewer`
  - owns `ViewerShell` composition/routing for ready payloads
  - passes renderer-specific header actions/body/panel/dialog as slots
- `MarkdownViewerBody`
  - owns rendered Markdown only
  - supports block annotations via shared reviewable hook
- `CodeViewerBody`
  - owns syntax-highlighted line rendering for code and HTML source
- `TextViewerBody`
  - owns plain text line rendering
- `HtmlViewerBody`
  - owns HTML-specific mode state and header actions:
    - source mode
    - sandboxed preview iframe
    - open in browser
- `ImageViewerBody`
  - owns image preview inside viewer chrome
- `CurrentDiffViewerBody` or similar
  - wraps today's `DiffView` behavior behind the new MetaViewer payload/API shape
- `useReviewableViewer` / `useFileReviewNotes` / `useDiffReviewNotes`
  - shared hook(s) for add-note, jump, highlight, send, clear, note count, and formatted output
  - avoid copying the same review-note lifecycle into every renderer
  - focus on today's practical file-review lifecycle; do not invent a broad generic renderer capability framework before the Pierre diff task needs one
- `viewerFileTypes` / `viewerLanguage` utility or similar
  - owns file type and syntax language classification that currently lives in `ProseReader`
  - has focused tests so routing and syntax highlighting do not drift

## Behavioral requirements

- File viewing behavior remains equivalent:
  - Markdown files render as rendered Markdown, not raw source.
  - Markdown code fences still syntax-highlight.
  - Code files still render with syntax highlighting and line numbers.
  - Plain text still renders line-by-line with line numbers.
  - HTML keeps both source and preview mode.
  - HTML preview remains sandboxed and still supports opening in a browser.
  - HTML preview security semantics remain equivalent: sandbox attributes, script behavior, preview URL behavior, and explicit open-in-browser behavior must not loosen accidentally.
  - Images still render in the viewer shell.
  - Loading and error states remain clear and consistent.
- Review notes remain equivalent:
  - File notes remain anchored as `{ kind: 'file', filePath, lineNumber }`.
  - Diff notes remain anchored as `{ kind: 'diff', section, filePath, oldLine, newLine, diffPos }`.
  - NotesPanel jump behavior continues to work for every current file/diff anchor type.
  - Sending notes uses the existing `formatNotesForSend` semantics.
  - Closing/reopening viewers does not drop pending notes.
- Patch-context behavior remains equivalent:
  - Modified lines are visually marked.
  - Opening a file from a patch context still scrolls to the first modified line where possible.
- Viewer shell behavior remains equivalent or slightly cleaner:
  - inline vs overlay still works.
  - close/Escape behavior still works.
  - header actions remain available in the relevant renderer.
  - file viewer and diff viewer remain mutually exclusive where they are today.
  - viewer restoration/back-to-conversation behavior remains equivalent, but is represented through the new single viewer slot contract.
- Naming/CSS behavior is cleaned up with the refactor:
  - production component/type/file names should no longer imply `ProseReader` owns all viewer modes.
  - stale production CSS class names such as `prose-reader-*` should be renamed to renderer-neutral names as part of the move, except for explicitly bounded temporary migration shims.

## Acceptance criteria

- `ProseReader` is removed as an internal public component/import path, or reduced only to a temporary test-only alias that is not used by production code. Production code uses `MetaViewer` plus specialized viewer bodies. Review checklist: verify production code no longer imports `ProseReader`.
- `FileViewer` still owns file loading but delegates all ready rendering to `MetaViewer` using typed resolved payloads.
- A typed `ViewerOpenRequest` vs `MetaViewerPayload` boundary exists. Open requests represent user/app intent; resolved payloads represent renderable content. MetaViewer renders only the latter.
- `ViewerStateContext` and viewer restoration/storage use one canonical mutually exclusive viewer slot rather than parallel file/diff/image state models. Update `lastViewerStorage` or its successor as needed.
- HTML source/preview/open-browser behavior lives in a dedicated HTML renderer path, not mixed into Markdown/code/text rendering.
- HTML preview sandbox/security behavior is preserved and validated by automated test where practical or explicit manual validation notes where browser security behavior is difficult to assert in unit tests.
- File type and syntax-language classification is extracted from `ProseReader` into a typed utility with focused tests.
- Image rendering is represented as a first-class MetaViewer payload/body, not as an unrelated special case outside the viewer model.
- Diff viewing is represented in the MetaViewer payload/API shape, but still uses the current homegrown diff renderer internally. No Pierre dependency is added.
- Shared review-note hook(s) exist and are used by at least the file-oriented renderers where practical; duplicated note/jump/send code is reduced and the remaining ownership is intentional.
- Existing file-note and diff-note anchors remain compatible with `ReviewNotesContext`.
- Shared file-review hooks standardize current add-note, jump, highlight, send, clear, and note-count behavior where practical. A fully generic cross-renderer annotation/scroll capability framework is not required in this task.
- `TaskApprovalReader` is audited for overlap with the extracted Markdown/annotation primitives. It either reuses appropriate shared pieces or documents why approval-plan rendering remains separate.
- Production CSS/classes and component names are updated away from misleading `prose-reader-*` ownership naming, except for explicitly bounded temporary shims.
- Existing tests are updated or replaced to assert behavior at the new component boundaries.
- Add focused tests for payload routing and at least one behavior-preservation path per renderer kind:
  - markdown
  - code/text
  - HTML source/preview
  - image
  - current diff adapter
  - viewer open request -> resolved payload adapter boundary
  - single-slot viewer state/restoration behavior
  - file type/language classification
- `./dev.py check` passes.

## Implementation notes

- Prefer small mechanical moves first: copy/extract renderer bodies, wire tests, then delete old `ProseReader` import paths.
- Avoid semantic changes while moving code; behavior changes should be explicit and small.
- The final diff payload shape should make the Pierre replacement straightforward, but task 1 must not depend on Pierre types.
- Watch for comments in moved code that describe stale `ProseReader` responsibilities; update or delete them rather than carrying misleading names forward.
- If the split exposes ambiguous ownership between MetaViewer and renderer bodies, prefer typed payloads and renderer-local body state over boolean prop soup.
- MetaViewer should route and compose; it should not simply become a renamed ProseReader that accumulates all renderer-specific state. Pragmatic exceptions are acceptable when the resulting type boundaries and tests remain clear.

## Follow-up dependency

This task blocks the `Pierre Diff Replacement` task. The Pierre migration should not land until this MetaViewer API is merged and stable.
