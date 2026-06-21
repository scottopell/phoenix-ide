# MetaViewer QA

`./dev.py qa meta-viewer` is the second member of the `./dev.py qa` namespace,
applying the grounding-panel QA pattern (typed scenarios → Ladle stories →
hermetic screenshot capture) to the file/log viewer. The capture engine is
shared (`ui/scripts/capture-ladle-surface.mjs`); this surface is "a config, not
new infrastructure".

## Scope: edge states only

`MetaViewer` (`ui/src/components/viewer/MetaViewer.tsx`) is a *router over an
already-resolved* `MetaViewerPayload` — the loader has fetched and classified
the bytes; MetaViewer never fetches. Three layers sit underneath it:

1. **Body renderers** (`MarkdownViewerBody`, `TextViewerBody`, `HtmlViewerBody`,
   `ImageViewerBody`, `PhoenixFileCodeView`) — the happy path, "render these
   bytes of this kind."
2. **MetaViewer + `ViewerShell`** — the router plus cross-cutting chrome
   (banners, header toggles, notes panel, HTML source/preview, image takeover).
3. **The loader** (`FileViewer`) — fetch + classify → loading / error / kind.

The happy path is already covered by real files (the seeded conversation, or
opening any `.md` / `.rs` / `.png`) plus per-renderer unit tests. So this QA does
**not** screenshot "markdown renders" / "png shows". It screenshots the *rare
branches* a developer will not hit with a normal file.

Because MetaViewer is payload-in, most scenarios hand it a hand-built
`MetaViewerPayload` directly — **no fetch mock**. Only the two loader-level
states (loading / error) mount the real `FileViewer` behind a mocked
`window.fetch`.

## Covered scenarios

Payload-driven (`<MetaViewer payload>`, zero network):

- `large-text-fallback-dark` / `large-text-fallback-light` — `plainLargeText`
  render mode + its banner (a rare interior mode of `TextViewerBody`); the light
  variant proves theme plumbing for this surface.
- `patch-context-dark` — changed-line highlight, first-modified-line auto-scroll,
  and the "N changes from patch" banner. Uses a `text` payload because that path
  is MetaViewer-owned; `code` patch context is handled inside Pierre's CodeView
  and is out of scope here.
- `long-lines-text-dark` / `long-lines-code-dark` — lines (including unbreakable
  tokens) far wider than the viewport, in the plain-text body and the Pierre code
  body respectively. Establishes the horizontal-overflow user story: does a long
  line wrap, get a horizontal scrollbar, or clip?
- `html-source-dark` — HTML source mode (highlighted, annotatable) with the
  Preview / Open-in-browser header toggles.
- `html-preview-dark` — the sandboxed-preview iframe (`sandbox="allow-same-origin"`,
  no scripts), reached by toggling Preview.
- `image-takeover-dark` — fullscreen image takeover (rendered through a portal).
- `notes-panel-dark` — populated review-notes side panel.
- `annotation-dialog-dark` — the line-annotation dialog open.

Loader-mock (`<FileViewer>` + mocked `fetch`):

- `loading-dark` — the loading spinner (mock never resolves, but issues no real
  request, so capture still reaches `networkidle`).
- `error-dark` — the read-error surface. This is the honest "cannot render this"
  state: opaque/binary files are non-openable upstream and never reach MetaViewer
  as a payload, so there is no in-router "unsupported kind" branch to screenshot.

## Fixtures and stories

```text
ui/src/fixtures/metaViewer/
  types.ts          canonical scenario list (id union + capture set derive from it)
  scenarios.ts      hand-built MetaViewerPayload objects + loader states
  mockApi.ts        window.fetch mock (loader scenarios only)
  renderFixture.tsx providers + settled-DOM readiness + scripted interactions
ui/src/stories/meta-viewer.stories.tsx
```

Run the story harness directly:

```bash
cd ui
pnpm ladle
# http://127.0.0.1:61123/?story=meta-viewer--html-preview-dark
```

## Regenerating screenshots

```bash
./dev.py qa meta-viewer
# or, from ui/:  pnpm qa:meta-viewer
```

The command installs Playwright's Chromium, boots Ladle on a deterministic port,
visits each `meta-viewer--*` story (discovered from Ladle's manifest), waits for
the fixture's `data-meta-viewer-fixture-ready` marker, fails on unexpected
console errors, and writes PNGs to:

```text
ui/qa-artifacts/meta-viewer/
```

That directory is git-ignored. Do not commit regenerated PNGs; share them as
local/CI artifacts when needed.

## Coverage split

| Layer | Covers | Does not cover |
| --- | --- | --- |
| Vitest + real files | happy-path body renderers, classification logic | rare cross-cutting chrome states |
| Ladle (`qa meta-viewer`) | large-file fallback, patch context, HTML preview, image takeover, notes/annotation, loading/error | normal file rendering (real files own it) |
