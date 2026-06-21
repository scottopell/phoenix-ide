Add `qa meta-viewer` as the second member of the `./dev.py qa` subcommand namespace, applying the productionized grounding-panel QA pattern (typed scenarios → Ladle stories → hermetic screenshot capture → docs) to the MetaViewer file/log viewer. Validates that the pattern extends to a second surface "by adding scenarios, not infrastructure," and stops `qa` being a namespace of one.

## Core framing — scope is edge-states only

MetaViewer (`ui/src/components/viewer/MetaViewer.tsx`) is a router over an already-resolved `MetaViewerPayload`: the loader has already fetched and classified the content. Layering:

1. Interior body renderers (`MarkdownViewerBody`, `TextViewerBody`, `HtmlViewerBody`, `ImageViewerBody`, `PhoenixFileCodeView`) — the happy path: "render these bytes of this kind."
2. MetaViewer + `ViewerShell` — the router plus cross-cutting chrome (banners, header extras, notes panel, scroll/copy, HTML source/preview toggle, image takeover).
3. The loader (upstream) — fetch + classify → loading / error / kind detection.

The happy path (tier 1) is already covered by real files: the seeded conversation plus opening a real `.md` / `.rs` / `.png` exercises the renderers for free, and they are individually unit-testable. DO NOT screenshot "markdown renders" / "png shows" — real files own that. The screenshot-QA value is the rare branches a developer will not hit with a normal file.

## Target scenarios — the edge states

Payload-driven (hand-build `MetaViewerPayload`, NO fetch mock — MetaViewer is payload-in):
- Large-file plain-text fallback (`renderMode === 'plainLargeText'`) + its banner (a rare interior mode inside TextViewerBody).
- Patch context: changed-line highlight, first-modified-line auto-scroll, the "N changes from patch" banner.
- HTML source ↔ sandboxed-preview toggle (iframe).
- Image fullscreen takeover (portal).
- Unsupported / opaque / binary kind ("can't render this").
- Notes panel populated + annotation dialog open.

Loader-mock set (small, `window.fetch`-mocked like grounding-panel): loading spinner, fetch error / 404.

## Reuse the grounding-panel recipe

Template: `ui/src/fixtures/groundingPanel/`, `ui/src/stories/grounding-panel.stories.tsx`, `ui/scripts/capture-grounding-panel.mjs`, `docs/qa/grounding-panel.md`. Carry over: one canonical scenario list (id union + scenarios derive from a single source); settled-DOM readiness (not a fixed timer); capture set discovered from Ladle's story manifest; ignored artifact dir (no committed PNGs); `env=node_env()` on the pnpm call; namespaced `./dev.py qa meta-viewer`.

Key difference from grounding-panel: most scenarios need NO fetch mock — pass resolved `MetaViewerPayload` objects directly. Only the loader-level states (loading / error) need a mock.

## Validation

- `./dev.py check --lanes tsc,ui-lint,vitest`
- `./dev.py qa meta-viewer` produces every scenario PNG with rc=0 and no unexpected console errors
- focused vitest for any new payload/classification helper

## Non-goals (separate follow-ups)

- Pixel-diff visual regression gating
- CI job uploading screenshots as PR artifacts
- Re-testing the happy-path renderers (real files + unit tests own those)
- Refactoring MetaViewer itself

## References

- `ui/src/components/viewer/MetaViewer.tsx`, `metaViewerTypes.ts`, `ViewerShell.tsx`, the `*ViewerBody.tsx` renderers
- Pattern source: PR #342 (productionize grounding panel QA) and `docs/qa/grounding-panel.md`
