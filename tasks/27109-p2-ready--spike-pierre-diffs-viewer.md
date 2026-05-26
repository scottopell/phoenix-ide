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
