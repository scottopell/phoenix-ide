# Request typed read-only range decorations from Pierre

## Context

Phoenix indexes complete canonical file and diff payloads for in-viewer find, including content that `@pierre/diffs` has not mounted. Pierre's read-only `CodeView` currently supports typed item/line scrolling and line-level styling through `unsafeCSS`, but it does not expose a supported API for decorating exact character ranges. Phoenix therefore navigates every match correctly but can only distinguish the containing line on Pierre surfaces.

The latest stable checked was `1.2.12`; `1.3.0-beta.9` adds offset-based search to its editor subsystem, but not to ordinary read-only viewers. Phoenix should not enable edit mode merely to obtain search decorations.

## Ideal upstream contract

Propose a typed, controlled read-only decoration API on `CodeView`, conceptually:

```ts
type TextDecoration = {
  id: string;
  itemId: string;
  side?: 'additions' | 'deletions';
  lineNumber: number;
  startColumn: number;
  endColumn: number;
  className: string;
};

<CodeView decorations={decorations} />
```

The exact upstream shape may differ, but it should preserve item, side, line, and source-column identity rather than accepting selectors or DOM nodes.

## Required behavior

- Decorations work for read-only file, unified diff, and split diff items without enabling editor mode.
- Character offsets refer to source text and remain correct when syntax highlighting splits text into multiple token spans.
- Split-view side identity is explicit; a context occurrence can be intentionally decorated once or in both rendered panes without accidental double-counting.
- Decorations survive virtualization unmount/remount, theme changes, item reconciliation, context expansion, and controlled version updates.
- Updating decorations does not mutate file contents, line annotations, review-note state, or selection.
- Decorations can be independently styled as ordinary and active matches and are accessible without changing copied text.
- The API composes with typed `scrollTo` so consumers can index canonical content independently of rendered DOM state.
- A real-browser example/test covers an initially unmounted exact match in a syntax-highlighted file and split diff.

## Phoenix follow-up

Once a stable Pierre release exposes this capability, replace Phoenix's Pierre line-level find styling with exact range decorations while retaining the existing canonical search projections and typed navigation targets. Verify file and split/unified diff behavior, then update the viewer-find executive limitation.
