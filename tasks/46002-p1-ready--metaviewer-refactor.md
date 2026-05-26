# MetaViewer Refactor

## Summary

Refactor Phoenix's viewer stack into a clean typed `MetaViewer` architecture before replacing the diff renderer. Today `ProseReader` owns too many unrelated responsibilities: Markdown rendering, code/text line rendering, HTML source rendering, sandboxed HTML iframe preview, file-note behavior, scroll restoration, changed-line highlighting, copy behavior, and annotation plumbing. `FileViewer` recently added image rendering as a separate option in `ViewerShell`, which points toward the right direction: a central viewer shell/router with specialized body renderers.

This task establishes the final internal viewer API shape so the follow-up Pierre diff replacement can land against stable abstractions rather than inheriting today's ProseReader/DiffView coupling.

## Decisions already made

- Priority: P1.
- This task is strictly sequenced before `pierre-diff-replacement`.
- Visible UX change is allowed only as small cleanup required by the refactor: normalize header actions, loading/error/image states, renderer naming, and shell behavior where it reduces future integration risk.
- `MetaViewer` renders already-resolved payloads. It does **not** own `/api/files/read` or diff fetching in this task.
- Retire the `ProseReader` component/name internally. Do not keep it as a long-term compatibility wrapper.
- Do not add `@pierre/diffs` in this task. Define the final diff payload/API shape using current DiffView underneath.
- Review-note behavior should move toward shared hooks, not one giant MetaViewer god-component and not fully duplicated renderer-owned logic.

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

## Non-goals

- Do not replace the diff renderer with Pierre in this task.
- Do not add `@pierre/diffs` in this task.
- Do not replace ProseReader code/plain-text rendering with Pierre CodeView yet.
- Do not redesign the viewer UI beyond small cleanup necessary to make the architecture coherent.
- Do not remove existing HTML preview/open-in-browser functionality.
- Do not change review-note formatted output semantics.
- Do not change message rendering (`MessageComponents`, `StreamingMessage`) except if a tiny shared utility extraction is clearly required and low-risk.

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

## Behavioral requirements

- File viewing behavior remains equivalent:
  - Markdown files render as rendered Markdown, not raw source.
  - Markdown code fences still syntax-highlight.
  - Code files still render with syntax highlighting and line numbers.
  - Plain text still renders line-by-line with line numbers.
  - HTML keeps both source and preview mode.
  - HTML preview remains sandboxed and still supports opening in a browser.
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

## Acceptance criteria

- `ProseReader` is removed as an internal public component/import path, or reduced only to a temporary test-only alias that is not used by production code. Production code uses `MetaViewer` plus specialized viewer bodies.
- `FileViewer` still owns file loading but delegates all ready rendering to `MetaViewer` using typed resolved payloads.
- HTML source/preview/open-browser behavior lives in a dedicated HTML renderer path, not mixed into Markdown/code/text rendering.
- Image rendering is represented as a first-class MetaViewer payload/body, not as an unrelated special case outside the viewer model.
- Diff viewing is represented in the MetaViewer payload/API shape, but still uses the current homegrown diff renderer internally. No Pierre dependency is added.
- Shared review-note hook(s) exist and are used by at least the file-oriented renderers where practical; duplicated note/jump/send code is reduced and the remaining ownership is intentional.
- Existing file-note and diff-note anchors remain compatible with `ReviewNotesContext`.
- Existing tests are updated or replaced to assert behavior at the new component boundaries.
- Add focused tests for payload routing and at least one behavior-preservation path per renderer kind:
  - markdown
  - code/text
  - HTML source/preview
  - image
  - current diff adapter
- `./dev.py check` passes.

## Implementation notes

- Prefer small mechanical moves first: copy/extract renderer bodies, wire tests, then delete old `ProseReader` import paths.
- Avoid semantic changes while moving code; behavior changes should be explicit and small.
- The final diff payload shape should make the Pierre replacement straightforward, but task 1 must not depend on Pierre types.
- Watch for comments in moved code that describe stale `ProseReader` responsibilities; update or delete them rather than carrying misleading names forward.
- If the split exposes ambiguous ownership between MetaViewer and renderer bodies, prefer typed payloads and renderer-local body state over boolean prop soup.

## Follow-up dependency

This task blocks the `Pierre Diff Replacement` task. The Pierre migration should not land until this MetaViewer API is merged and stable.
