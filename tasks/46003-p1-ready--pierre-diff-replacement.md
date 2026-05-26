# Pierre Diff Replacement

## Summary

Replace Phoenix's homegrown unified diff parser/renderer with Pierre's `@pierre/diffs` React `CodeView` stack, after the `MetaViewer Refactor` has landed. This task focuses only on the diff viewer. It should preserve Phoenix's existing review-note semantics while improving large-diff rendering through Pierre's virtualized diff/code review surface.

This is the implementation follow-up to the completed Pierre diff spike and is strictly sequenced after the MetaViewer API cleanup.

## Dependency

Blocked by: `MetaViewer Refactor`.

Do not start this as a production migration until the MetaViewer refactor has merged and the diff viewer is represented as a stable typed MetaViewer payload/body. Prototypes are fine, but the final PR should integrate through the new MetaViewer shape.

## Decisions already made

- Priority: P1.
- Scope is diff viewer only.
- Do not use this task to replace ProseReader/code/plain-text viewing with Pierre CodeView.
- Do not redesign Markdown/HTML/image file viewing.
- Adopt `@pierre/diffs` with a Phoenix wrapper/shim, not as a blind renderer swap.
- Preserve current `ReviewNotesContext` anchor semantics initially.
- Do not rely on unverified Plannotator implementation assumptions; public `@pierre/diffs` API evidence is sufficient.

## Background

The completed spike found that `@pierre/diffs@1.2.0` exposes the key APIs Phoenix needs:

- React `CodeView` component for many files/diffs.
- `parsePatchFiles(data, cacheKeyPrefix?, throwOnError?)` for raw unified diff parsing.
- Diff items shaped like `{ id, type: 'diff', fileDiff, annotations?, version?, collapsed? }`.
- `DiffLineAnnotation<T>` with `side`, `lineNumber`, and metadata.
- line interaction hooks such as `onLineClick`, `onLineNumberClick`, `onLineEnter`, `onLineLeave`.
- custom annotation rendering via `renderAnnotation(annotation, item)`.
- target scrolling via `scrollTo({ type: 'line', id, lineNumber, side, align, behavior })`.

The spike conclusion: Phoenix can retain per-line diff comments without brittle DOM scraping.

## Goals

1. Add `@pierre/diffs` to the UI and use it for diff rendering.
2. Replace the current homegrown diff display path in `ui/src/components/viewer/DiffView.tsx` / `diffParse.ts` with a Phoenix wrapper around Pierre `CodeView`.
3. Preserve committed and uncommitted diff sections as explicit namespaces.
4. Preserve existing diff note anchors and formatted note output.
5. Preserve line-level and file-level note interactions.
6. Improve large-diff rendering by relying on Pierre's virtualized/windowed CodeView surface.
7. Validate edge cases that the spike identified as important.

## Non-goals

- Do not replace code/plain-text file viewing with Pierre CodeView in this task.
- Do not change Markdown, HTML, or image viewer behavior.
- Do not redesign the full viewer shell; this task should plug into the MetaViewer shape from the prerequisite task.
- Do not migrate `ReviewNotesContext` to a new anchor schema unless a small compatibility adapter is unavoidable. If a new schema is needed, file a follow-up task instead of expanding this one.
- Do not depend on DOM scraping for annotations, note indicators, line identity, or jump behavior.

## Current behavior to preserve

- Entry point fetches conversation diffs and opens the conversation-scoped diff viewer payload.
- Diff viewer displays two sections:
  - committed changes vs comparator
  - uncommitted changes
- Truncation metadata and saturated/lower-bound indicators remain visible.
- Diff anchors continue to include:
  - `kind: 'diff'`
  - `section: 'committed' | 'uncommitted'`
  - `filePath`
  - `oldLine`
  - `newLine`
  - `diffPos`
- Users can add notes on add/delete/context lines.
- Users can add file-level notes from file headers or an equivalent explicit file-level affordance.
- Existing note indicators remain visible or are replaced with an equivalent Pierre-rendered indicator.
- NotesPanel can jump back to annotated diff lines.
- Sending notes into chat preserves current `formatNotesForSend` output semantics.
- File/prose viewer and diff viewer remain mutually exclusive as they are after the MetaViewer refactor.

## Recommended implementation shape

Create a Phoenix-owned wrapper around Pierre rather than letting Pierre types leak through the app:

- `PhoenixDiffCodeView` or equivalent:
  - accepts the MetaViewer diff payload from the prerequisite task
  - parses committed and uncommitted raw diffs separately with `parsePatchFiles`
  - converts parsed files into Pierre `CodeViewItem[]`
  - maps Phoenix review notes into Pierre annotations
  - maps Pierre line events back into Phoenix note anchors
  - exposes/uses `scrollTo` for NotesPanel jump

Stable item id convention:

- `committed:${filePath}`
- `uncommitted:${filePath}`

Keep section identity explicit. Do not rely on file path alone; the same file can appear in both committed and uncommitted sections.

Recommended note mapping:

- Phoenix `section` -> item id prefix and annotation metadata.
- Phoenix `filePath` -> parsed file name / item id suffix.
- Phoenix `newLine` -> Pierre `side: 'additions', lineNumber: newLine`.
- Phoenix `oldLine` -> Pierre `side: 'deletions', lineNumber: oldLine`.
- Phoenix `diffPos` -> preserved in Phoenix metadata for compatibility, but the new renderer should not depend on it as the primary line identity.
- Store the full Phoenix note anchor or an anchor key in annotation metadata to avoid collisions.

File-level notes:

- Pierre line annotations are not file-level annotations.
- Implement file-level note affordances through a wrapper header, `renderHeaderPrefix`, `renderHeaderMetadata`, `renderCustomHeader`, or equivalent supported header hook.
- Preserve formatted output semantics for file-level diff notes.

## Edge cases to validate

Use real or generated diffs covering:

- multi-file diffs
- add/delete/context lines
- rename with content change
- deleted files
- added files
- binary-file markers
- no-newline-at-EOF markers
- truncated committed diff
- truncated uncommitted diff
- saturated/lower-bound truncation indicator
- same file path appearing in committed and uncommitted sections
- empty committed section with non-empty uncommitted section and vice versa
- large diff around or above the spike size: ~1.28 MiB / ~24k lines / 100+ files

## Performance expectations

The spike found parsing is not the main bottleneck; browser rendering all rows is likely the issue. This task should validate that Pierre's CodeView virtualization improves or at least does not regress large diff interaction.

Required measurements before/after or old/new where practical:

- initial render of a large diff
- scroll responsiveness on a large diff
- add-note interaction latency
- NotesPanel jump-to-line latency

Use the project's existing browser profiling approach where possible. Do not rely solely on subjective visual inspection.

## Theming and accessibility requirements

- Integrate Pierre styling with Phoenix dark/light themes intentionally.
- Do not assume current `.diff-*` CSS classes apply to Pierre-rendered content.
- Preserve keyboard and pointer affordances for adding notes.
- Preserve touch/long-press behavior if it exists after the MetaViewer refactor.
- Validate screen-reader labels and focus behavior for note actions, headers, and the viewer shell.

## Acceptance criteria

- `@pierre/diffs` is added as a UI dependency and used by the diff viewer path.
- The homegrown diff row rendering path is removed or fully bypassed for production diff viewing.
- `diffParse.ts` is deleted, retired, or reduced to a compatibility adapter with a clear reason. It should not remain as a parallel authoritative parser.
- No annotation, note indicator, line identity, or jump behavior depends on querying/scraping Pierre's DOM.
- Existing Phoenix diff note anchors continue to work with `ReviewNotesContext`.
- Users can add notes to diff lines and file headers/equivalent file-level targets.
- NotesPanel jump uses Pierre's supported scroll target API or an equivalent typed handle, not DOM scraping.
- Existing note formatting sent to chat remains compatible.
- Committed and uncommitted sections cannot collide in annotation identity.
- Edge cases listed above are covered by automated tests where reasonable and manual/browser validation notes where automation is impractical.
- Large-diff behavior is measured and documented in the PR/task notes.
- `./dev.py check` passes.

## Suggested tests

- Unit tests for Phoenix anchor <-> Pierre annotation mapping.
- Unit tests for committed/uncommitted item id generation and collision avoidance.
- Unit tests for parsing multiple diff sections into CodeView items.
- Component tests for:
  - add note on added/deleted/context lines
  - note indicator rendering
  - NotesPanel jump invoking typed scroll target
  - file-level note action
  - truncation banners/metadata
- Regression tests for rename/delete/binary/no-newline cases where feasible.

## Follow-ups explicitly out of scope

- Replace code/plain-text file viewing with Pierre CodeView.
- Rework Markdown/HTML rendering.
- Redesign review-note anchor schema.
- Broad viewer visual redesign.
