# Consolidate text file viewer onto Pierre CodeView

## Goal

Route plain `text` MetaViewer payloads through `PhoenixFileCodeView` so text and code share one virtualized, single-scroll render path. Delete the bespoke rich per-line `TextViewerBody` path and its dead CSS.

## Scope

- Update `ui/src/components/viewer/MetaViewer.tsx`
  - Treat `kind: 'text'` like `kind: 'code'` for `usePierreCode`.
  - Remove/dead-code the rich `case 'text'` body path.
  - Drop MetaViewer-owned text line refs, patch shading, and first-modified auto-scroll now handled by Pierre for text.
- Update `ui/src/components/viewer/FileViewer.tsx`
  - In `buildPayload`, let `text` skip `plainLargeText` like `code` because Pierre virtualizes.
  - Leave `plainLargeText` for markdown and html source fallback only.
- Update `ui/src/components/viewer/TextViewerBody.tsx`
  - Keep only the `plainLargeText` `<pre>` fallback body.
  - Delete unreachable rich per-line rendering.
- Remove dead `.viewer-text*` rich text CSS from `ui/src/index.css`.
- Update `ui/src/components/viewer/MetaViewer.test.tsx` for Pierre-based text rendering.
- Update QA fixtures/docs from #358:
  - Repoint `large-text-fallback-{dark,light}` to markdown so fallback banner + `<pre>` still run.
  - Update `patch-context-dark` for the new owner of patch handling, either markdown MetaViewer path or Pierre path with matching selectors/notes.
  - Update `long-lines-text-dark` settle selector to `.phoenix-file-codeview [data-line]` and keep it as the plain `.txt` no-grammar counterpart to code.

## Non-goals

- Do not change markdown prose rendering.
- Do not change html preview/source behavior.
- Do not change Pierre or `PhoenixFileCodeView` internals.

## Validation

- `./dev.py check --lanes tsc,ui-lint,vitest`
- `./dev.py qa meta-viewer`
  - `long-lines-text-dark` shows one pane scroll, no per-line scrollbars.
  - `large-text-fallback-*` still shows fallback banner and `<pre>`.
