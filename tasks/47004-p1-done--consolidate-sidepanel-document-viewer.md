# Consolidate side-panel document viewing behind one source-aware viewer

## Problem

Phoenix has two partially divergent paths for document-like content in the viewer slot:

```text
Markdown file:
FileViewer → MetaViewer → ViewerShell → MarkdownViewerBody

Conversation message:
MessageViewer → ViewerShell → MarkdownViewerBody
```

They share visual chrome and Markdown rendering, but independently compose find, copy, Escape precedence, line navigation, scroll behavior, and review-note lifecycle. The divergence is user-visible: a Markdown file opened in the side panel owns `Cmd/Ctrl+F`, while a message opened from its context menu falls through to unusable native browser find.

Do not fix this by adding a second find integration to `MessageViewer`. Consolidate document-like side-panel content behind one resolved viewer so shared behavior cannot drift.

## Architectural direction

Generalize the current file-oriented `MetaViewerPayload` into a source-aware resolved-content model. Loaders/resolvers remain outside the viewer:

- A file adapter fetches and classifies filesystem content.
- A message adapter resolves canonical message Markdown from conversation state.
- Both produce a typed resolved-content payload consumed by one document viewer.
- The document viewer owns shared shell composition, find, copy, Escape precedence, rendered-body routing, line registration/navigation, and common accessibility behavior exactly once.
- Source-specific review notes, identity, scroll persistence, and capabilities remain typed variants rather than optional file fields or invented message paths.

A representative shape is:

```ts
type ViewerSource =
  | {
      kind: 'file';
      absolutePath: string;
      filePath: string;
      rootDir: string;
      patchContext?: PatchContext;
    }
  | {
      kind: 'message';
      messageId: string;
      sequenceId: number;
    };

type ResolvedViewerContent =
  | { kind: 'markdown'; source: ViewerSource; content: string }
  | { kind: 'code'; source: FileSource; content: string; language: string }
  | { kind: 'text'; source: FileSource; content: string }
  | { kind: 'html'; source: FileSource; content: string; previewUrl: string }
  | { kind: 'image'; source: FileSource; url: string; mimeType: string };
```

The final design may differ, but it must make invalid capability combinations unrepresentable. In particular, messages cannot carry file paths, patch context, HTML preview URLs, or file scroll keys; file-only content kinds cannot accept message sources.

## Requirements

- One resolved document-viewer composition owns find, copy, viewer chrome, Escape hierarchy, line navigation, and body routing for both files and conversation messages.
- `MessageViewer` becomes a thin message-to-payload adapter or is removed; it must not independently compose `ViewerShell` and `MarkdownViewerBody` afterward.
- File loading/error states remain outside the resolved viewer and do not fabricate partial payloads.
- File and message review-note anchors remain structurally distinct and retain their current formatting/send semantics.
- Scroll/persistence identity is source-aware: absolute paths for files and stable message identity for messages, with no fake path convention.
- The shared find adapter searches canonical content and owns `Cmd/Ctrl+F` for the active document, including the message side panel.
- Annotation dialogs, notes panels, and confirmation dialogs remain higher-priority keyboard sub-contexts than document find.
- Existing Markdown, Pierre code/text, large-text, HTML source/preview, and image eligibility behavior is preserved.
- The viewer-slot discriminated union and URL contract remain source-of-truth for which viewer is active.
- Component names and documentation reflect the resulting boundary; do not retain a generic `MetaViewer` name if the implementation remains file-only.

## Migration approach

1. Inventory every `ViewerShell` composition and document-like viewer route to confirm the complete consolidation boundary.
2. Introduce source and resolved-content discriminated unions with exhaustive tests for valid/invalid combinations.
3. Separate source-neutral document behavior from file/message review adapters.
4. Route file payloads through the new model without changing existing behavior.
5. Route conversation-message payloads through the same composition and remove the duplicate shell/body path.
6. Update viewer-find and viewer-slot requirements/executive coverage to describe the unified architecture.
7. Remove superseded helpers, duplicated keyboard listeners, and compatibility fields rather than leaving two representations.

## Verification

- Opening a Markdown file and opening a message use the same document viewer and expose the same find, copy, keyboard, and accessibility behavior.
- `Cmd/Ctrl+F` in a message side panel opens Phoenix find, searches complete message Markdown, navigates every occurrence, and never opens native browser find.
- Escape closes find before the message/file viewer; a second Escape follows normal viewer close/confirmation behavior.
- Switching between file and message viewer-slot variants clears local find state and cannot leak notes, scroll targets, or source identity.
- File review notes remain path/line anchored; message notes remain sequence/message/line anchored.
- Existing file patch highlighting, jump-to-line, scroll restoration, HTML preview eligibility, Pierre navigation, message copying, and notes sending remain covered.
- Tests demonstrate that message sources cannot construct file-only payload variants and vice versa.
- Real-browser QA covers both side-panel entry paths at desktop split-pane width.

## Acceptance criteria

- There is one document-viewer behavior path rather than parallel file and message compositions.
- The message side-panel native-find bug is resolved through consolidation, not a one-off listener or duplicated adapter.
- Source-specific capabilities and identities are enforced by discriminated types.
- No fake paths, optional-field capability soup, DOM-scraped search identity, or duplicate canonical content representation is introduced.
