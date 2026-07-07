# Open chat messages in the annotatable side panel

## Goal

Add a message context-menu action so a user can right-click a long chat message, choose **Open in sidepanel**, and review it in the existing MetaViewer-style side panel with inline markdown annotations and the notes panel/send workflow.

This should reuse the existing markdown annotation UX while keeping message review structurally distinct from file review: a chat message is not a file, so notes should not be smuggled through `filePath`/`absolutePath` as if they referred to server filesystem content.

## Proposed design

### 1. Add a message viewer slot kind

Extend the unified viewer slot to support one more mutually-exclusive viewer:

- `ViewerSlot` gains `{ kind: 'message'; message: { sequenceId: number } }` (or message id if that is the more stable UI address; prefer whatever the loaded conversation atom can resolve reliably).
- URL shape: `?viewer=message&message=<id-or-sequence>`.
- `ViewerSlotCommands` gains `openMessage(...)`.
- `deriveSlot`, `clearSlotParams`, malformed normalization, per-conversation last-viewer persistence, and close behavior include the new message variant.
- Opening a message obeys the existing single-slot mutex: it replaces file/diff/browser/inspect viewers.

Cold reload should restore the message viewer from the URL after the conversation messages load. If the referenced message no longer exists in the loaded conversation, render a small viewer-shell error/empty state with Close rather than silently opening chat only.

### 2. Add the context-menu action

Update `MessageContextMenu` to show **Open in sidepanel** for messages whose markdown representation is non-empty:

- Use the existing `getMessageMarkdown(message)` helper for eligibility and content semantics.
- Keep the current native-menu escape hatches (`shift` right-click, links/images/inputs).
- On click, call `openMessage(...)` and close the menu.
- Preserve copy/select/tool-output menu behavior.

### 3. Render message markdown through MetaViewer-style chrome

Introduce a small message viewer adapter/component that resolves the opened message from the current conversation messages and builds a renderable markdown document:

- Title examples: `Agent message #42`, `User message #17`, or similar concise labels.
- Content: `getMessageMarkdown(message)`.
- Render body with the same `MarkdownViewerBody`, `ViewerShell`, annotation dialog, notes panel, copy, scroll/jump behavior, and send button affordances users already know from markdown files.
- It should work in the same responsive locations as other slot viewers:
  - mobile/overlay
  - narrow desktop full-pane replacement
  - wide desktop split pane

Avoid routing this through `FileViewer`, because there is no file fetch and no server filesystem path.

### 4. Make review notes structurally support message anchors

Extend review notes with a typed message anchor, for example:

```ts
{ kind: 'message'; messageId?: string; sequenceId: number; lineNumber: number }
```

Then add selector/formatting support:

- A message-scoped notes selector returns only notes for the open message.
- The message viewer hook adds/removes/sends notes using `kind: 'message'` anchors.
- `formatNotesForSend` renders message sections as message comments rather than file paths, e.g.
  - `### Agent message #42`
  - `- **Line 12**: ...`
- File and diff formatting remain unchanged.

This avoids pretending message notes are file notes and keeps future consumers able to distinguish “please respond to this chat message” from “please edit this file”.

### 5. Update specs for the new slot kind

`specs/viewer_slot/` currently models the slot as `{none, prose, diff, browser}` in prose and Allium, while the implementation already also has `inspect`. Update the slot spec to include the message viewer variant alongside the existing realized variants:

- Executive summary: mention message review as another slot kind and update the URL contract / requirement map.
- Allium: extend `ViewerKind`, slot variant data, transitions, URL hydration/malformed rules, persistence rules, and validation text.
- Run the spec authoring checklist from `specs/AUTHORING.md`; run `allium check specs/viewer_slot/viewer_slot.allium` if the CLI is available.

If the spec already has drift around the existing `inspect` variant, include that correction as part of this task rather than adding more drift.

## Tests

Add/update UI tests around:

1. `MessageContextMenu`
   - right-clicking a text message shows **Open in sidepanel**;
   - clicking it calls `openMessage` / updates URL params;
   - messages with no markdown content do not show the action.

2. `ViewerSlotContext`
   - `openMessage` writes only the message slot params and clears other slot params;
   - `deriveSlot` hydrates a valid message URL;
   - malformed message URLs normalize to none;
   - last-viewer persistence round-trips message slot params.

3. Message viewer rendering/notes
   - opens the selected message markdown in a viewer shell;
   - annotation creates a message-scoped note;
   - notes panel count/jump/send works;
   - `formatNotesForSend` formats message anchors distinctly from file/diff anchors.

4. `ConversationPage` responsive placement
   - message viewer participates in the same overlay/narrow-desktop/split-pane branches as prose/diff/browser/inspect.

## Validation

- Run targeted UI tests for the changed components/contexts.
- Run `./dev.py check` before committing.
- If specs are changed, run the relevant Allium validation and ensure generated/type checks remain clean.

## Open implementation choices

- Prefer message id over `sequence_id` if the current atom reliably exposes it for all persisted/historical messages. If not, use `sequence_id` because the existing context menu already addresses DOM messages by `data-sequence-id`.
- Decide whether message notes should share the global review-notes pile with file/diff notes (likely yes, so one Send action can submit all review feedback) while still using a distinct `kind: 'message'` anchor.
