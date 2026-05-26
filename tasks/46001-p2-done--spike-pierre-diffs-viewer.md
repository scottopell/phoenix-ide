# Spike: replace Phoenix diff viewer with Pierre `diffs`

## Context

Our current custom diff viewer (`ui/src/components/viewer/DiffView.tsx` + `diffParse`) is functional but "pretty meh" and gets slow on big diffs. Pierre published a `diffs` library release that may be a better foundation:

- https://github.com/pierrecomputer/pierre/releases/tag/diffs-v1.2.0

This should be treated as a spike, not a blind rewrite. The key product constraint is that we **must retain the prose-reader-style ability to comment on individual lines**. Today diff notes flow through `ReviewNotesContext`, are anchored by section/file/line/diff position, appear in `NotesPanel`, can jump back to the line, and are formatted into the chat input on send.

There is reason to be optimistic: the same `diffs` library is believed to power Plannotator, which offers a similar "comment on line" interaction. Validate that assumption and whether the public API exposes enough hooks for Phoenix's review-note flow.

## Goals

- Evaluate whether Pierre `diffs` v1.2.0 can replace Phoenix's custom diff renderer/parser.
- Determine whether it supports, or can cleanly be adapted to support, per-line comments/annotations.
- Compare expected performance on large diffs against the current custom viewer.
- Produce a concrete recommendation: adopt, adopt with wrapper/shim, wait for API changes, or keep/custom-improve current viewer.

## Current Phoenix behavior to preserve

Inventory and verify these during the spike:

- Entry point: `WorkActions` fetches `api.getConversationDiff(conversationId)` and opens the conversation-scoped `DiffViewerStateContext` payload.
- Viewer: `ui/src/components/viewer/DiffView.tsx` renders committed and uncommitted unified diffs separately.
- Parsing: `ui/src/components/viewer/diffParse.ts` derives file segments, line kind, `oldLine`, `newLine`, and `diffPos`.
- Notes: `ReviewNotesContext` supports anchors with:
  - `kind: 'diff'`
  - `section: 'committed' | 'uncommitted'`
  - `filePath`
  - `newLine` / `oldLine`
  - `diffPos`
- UX:
  - Add a note on add/del/context lines.
  - Add a file-level note from the `diff --git` header.
  - Show which lines have notes.
  - Jump from `NotesPanel` back to the annotated line.
  - Send notes into the chat input with the same semantics as prose-reader notes.
  - Keep file/prose viewer and diff viewer mutually exclusive.

## Spike plan

1. Inspect `diffs` v1.2.0 docs/source/examples.
   - Identify React/package entry points, expected input format, styling requirements, and bundling implications.
   - Verify whether it renders unified diffs directly or expects a parsed model.

2. Build a small local prototype or throwaway branch integration.
   - Feed it representative Phoenix committed/uncommitted diff payloads.
   - Confirm behavior for multi-file diffs, binary files, renames, deleted files, no-newline markers, and truncated diffs.

3. Validate annotation support.
   - Determine how to map rendered rows back to Phoenix note anchors.
   - Required: stable per-line callbacks or row render hooks with enough metadata to recover `filePath`, side/line number, and a durable position.
   - Required: a way to render Phoenix note indicators and attach click/long-press/add-note actions.
   - Required: a way to scroll/jump to a note after it is created.
   - Investigate Plannotator usage as evidence for the intended API.

4. Compare performance.
   - Use at least one large real-world diff that is currently slow.
   - Capture rough render/interact timings for current `DiffView` vs prototype.
   - Note whether virtualization is built in, available as an option, or still needs a Phoenix wrapper.

5. Assess integration shape.
   - Decide whether `diffParse.ts` can be deleted, retained only as an adapter, or replaced by `diffs` metadata.
   - Identify any needed changes to `ReviewNotesContext` anchors before committing to them.
   - Check accessibility and keyboard/mouse/touch affordances for the add-note interaction.
   - Check CSS/theming fit with `ViewerShell` and existing dark/light styles.

## Deliverables

- A short written recommendation in the task/PR notes with:
  - adopt/no-adopt decision,
  - annotation API findings,
  - performance findings,
  - risks and unknowns,
  - estimated implementation plan if adoption is recommended.
- If adoption is recommended, a follow-up implementation task or PR plan covering the actual migration.
- If adoption is not recommended, specific reasons and a fallback plan for improving the current viewer performance/UX.

## Acceptance criteria for the spike

- The recommendation explicitly answers: can Phoenix retain per-line diff comments with `diffs` v1.2.0 without a brittle DOM-scraping integration?
- The recommendation covers Plannotator/API evidence, not just visual inspection.
- Large-diff performance is measured or at least exercised with representative data.
- Existing prose-reader note behavior remains a non-negotiable requirement in any proposed migration path.

## Spike recommendation — 2026-05-26

### Decision

**Adopt `@pierre/diffs` v1.2.0 with a Phoenix wrapper/shim**, not as a blind renderer swap.

The public API is strong enough to preserve Phoenix's non-negotiable per-line diff comments without brittle DOM scraping. The migration should be a focused follow-up task that replaces the current raw-DOM row renderer with a `CodeView`-based adapter while keeping `ReviewNotesContext` semantics stable.

### API and annotation findings

Relevant public API surface in `@pierre/diffs@1.2.0`:

- Package entry points:
  - `@pierre/diffs` for vanilla/core APIs.
  - `@pierre/diffs/react` for React components.
  - `@pierre/diffs/ssr` and worker entry points exist, but Phoenix can start with the React entry point.
- Raw unified diff parsing:
  - `parsePatchFiles(data, cacheKeyPrefix?, throwOnError?)` parses multi-file raw patch/diff strings into `ParsedPatch[]` with `FileDiffMetadata[]`.
  - `processFile(fileDiffString, { isGitDiff: true, ... })` parses a single file diff.
- High-level viewer:
  - `CodeView` renders a single virtualized review surface over `CodeViewItem[]` entries.
  - Diff items are `{ id, type: 'diff', fileDiff, annotations?, version?, collapsed? }`.
  - The v1.2.0 release notes explicitly position `CodeView` as the default API for many files/diffs, owning virtualization, layout reconciliation, scroll anchoring, selection, sticky headers, and target-based scrolling.
- Annotation hooks:
  - `DiffLineAnnotation<T>` supports `{ side: 'deletions' | 'additions', lineNumber, metadata }`.
  - React `CodeView` exposes `renderAnnotation(annotation, item)` for custom note indicators/rows.
  - `CodeViewOptions` exposes `onLineClick`, `onLineNumberClick`, `onLineEnter`, `onLineLeave`, selection callbacks, and `renderGutterUtility`.
  - Diff line event metadata includes `lineNumber`, `annotationSide`, and `lineType`; `CodeView` callback overloads also include item context, so Phoenix can recover the item id/file path/section without DOM traversal.
  - `scrollTo({ type: 'line', id, lineNumber, side, align, behavior })` supports note-panel jump-to-line behavior.

Answer to the key acceptance question: **yes, Phoenix can retain per-line diff comments with `diffs` v1.2.0 without DOM scraping**. The integration should map each Phoenix section/file to a stable `CodeViewItem.id` such as `committed:src/foo.ts` and keep note identity in `metadata`.

Recommended annotation mapping:

- Existing Phoenix anchor:
  - `section`: include in `CodeViewItem.id` and in annotation metadata.
  - `filePath`: `fileDiff.name` / item id.
  - `newLine`: maps to `side: 'additions', lineNumber: newLine`.
  - `oldLine`: maps to `side: 'deletions', lineNumber: oldLine`.
  - `diffPos`: keep as Phoenix metadata during the shim, but do not make the new renderer depend on it.
- New annotation metadata should include the full Phoenix note anchor or an anchor key. This avoids collisions where the same line number appears in committed and uncommitted sections.
- File-level notes are not a first-class `DiffLineAnnotation`; handle them through `renderHeaderPrefix` / `renderHeaderMetadata` / `renderCustomHeader` or a small wrapper header above each item.

Plannotator evidence:

- I could not verify Plannotator source directly; public GitHub code search requires authentication and no public Plannotator source was found during this spike.
- The public `diffs` release notes and package typings provide direct API evidence for the required capability: the README advertises a "Flexible annotation framework for injecting comments, annotations, and more", and v1.2.0 `CodeView` exposes annotation render hooks, line click hooks, line selection, and scroll-to-line targets.
- Therefore the recommendation does **not** rely on a Plannotator assumption. If Plannotator uses this library, it is corroborating evidence only; the public API itself is sufficient.

### Prototype / representative data findings

I exercised `@pierre/diffs@1.2.0` locally against a generated representative patch containing:

- 163 file entries.
- 24,186 unified-diff lines.
- ~1.28 MiB patch text.
- Multi-file changes.
- Rename with content change.
- Deleted file.
- Binary-file marker.
- No-newline-at-EOF marker.

The parser recognized:

- Rename as `type: 'rename-changed'` with `prevName` and `name`.
- Delete as `type: 'deleted'`.
- Binary marker as a diff item with zero hunks/lines, suitable for a header-only row.
- Regular generated files with hunk metadata, addition/deletion line arrays, and `isPartial: true`.

Rough local timing on Node 22, 20 iterations after package install:

| Operation | Median |
| --- | ---: |
| Current Phoenix `parseUnifiedDiff`-equivalent parser | ~3.7 ms |
| `diffs.parsePatchFiles` on same 1.28 MiB patch | ~7.8 ms |
| `diffs.processFile` on one file | ~0.056 ms |
| `diffs.iterateOverDiff` over all parsed rows | ~1.0 ms |

Interpretation:

- `diffs` parsing is slightly slower than Phoenix's permissive parser for this synthetic patch, but still single-digit milliseconds at ~1.28 MiB.
- The current Phoenix performance problem is likely dominated by rendering all rows as React DOM nodes, not parsing alone.
- `CodeView`'s built-in virtualization, item pooling, windowed rendering, scroll anchoring, and worker/highlighter path are the major expected performance win.
- We should still benchmark browser render/interact timing during the actual migration because this spike did not land a full React prototype in Phoenix.

### Integration shape

Recommended follow-up implementation plan:

1. Add `@pierre/diffs` as a UI dependency.
2. Create a `PhoenixDiffCodeView` wrapper that accepts the current `DiffViewProps` payloads.
3. Parse committed and uncommitted raw diffs separately with `parsePatchFiles` so the existing `section` namespace remains explicit.
4. Convert parsed files into `CodeViewItem[]` with stable ids:
   - `committed:${fileDiff.name}`
   - `uncommitted:${fileDiff.name}`
5. Keep `ReviewNotesContext` initially unchanged.
   - Convert notes into `DiffLineAnnotation<PhoenixDiffAnnotationMetadata>[]` per item.
   - Store the full Phoenix anchor in annotation metadata.
6. Wire line actions through `CodeViewOptions.onLineClick` / `onLineNumberClick` / selection callbacks rather than DOM event delegation.
7. Implement note indicators through `renderAnnotation` and file-level actions through header render hooks.
8. Implement NotesPanel jump via `CodeViewHandle.scrollTo({ type: 'line', id, lineNumber, side, align: 'center', behavior: 'smooth' })`.
9. Preserve existing `formatNotesForSend` output semantics. Do not change prose-reader behavior.
10. After parity tests pass, delete or retire `diffParse.ts`. If `diffPos` remains in persisted/temporary note anchors, retain a small adapter function until anchor shape is intentionally migrated.

### Risks and unknowns

- `diffs` annotation identity is line-number + side, not Phoenix's `diffPos`. This is workable but requires a shim while `ReviewNotesContext` still carries `diffPos`.
- File-level notes need a Phoenix wrapper/header hook; they are not the same primitive as line annotations.
- CSS/theming requires work because `diffs` uses a web-component/shadow-DOM style model and Shiki themes. Phoenix should plan explicit `unsafeCSS`/theme integration rather than assuming current `.diff-*` classes apply.
- Accessibility needs hands-on validation. The library exposes line click/selection/gutter hooks, but Phoenix must preserve keyboard/touch affordances for adding notes.
- The Plannotator implementation assumption remains unverified from source; do not cite it as the reason to adopt.
- Browser render timings still need to be measured in the implementation PR with real Phoenix diffs and note interactions.

### Fallback plan if migration stalls

If the wrapper proves too hard to theme or to align with note UX, keep Phoenix's parser and improve the current viewer by:

- virtualizing diff rows,
- rendering committed/uncommitted sections as windowed lists,
- memoizing parsed segments by raw diff body,
- replacing `diffPos`-only refs with section/file/line keyed lookup,
- deferring syntax highlighting/line decoration for large diffs.

This fallback is likely more work than the `CodeView` wrapper and would still duplicate capabilities that `diffs` already provides.
