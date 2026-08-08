# Make Mermaid rendering resilient to deployed bundle changes

## Observed journey

- A user opens a conversation containing a fenced Mermaid diagram. The shared `MermaidDiagram` surface selects **Diagram**, but displays `Mermaid render failed. Importing a module script failed.` and falls back to source.
- A browser refresh with cache disabled does not provide an actionable recovery path.
- The exact error text is emitted by the browser's module loader, before `mermaid.render`; it is not a Mermaid syntax error.

## Verified findings

- `MermaidDiagram` performs a second-tier runtime `import('mermaid')` only when a diagram mounts (`ui/src/components/MermaidDiagram.tsx`). The Vite production output turns this into a request for a hashed `mermaid.core-*.js` chunk plus a preload dependency list.
- The mock model already provides `[[scenario:mermaid]]`, with both flowchart and sequence-diagram fences (`crates/phoenix-llm/src/mock.rs`), but browser E2E does not exercise it against production-built assets.
- Unit tests mock the `mermaid` package, so they verify component behavior but cannot catch a missing, stale, or unloadable production module chunk.
- On the installed 0.11.2 production server, Chromium could load the current `MessageComponents` chunk, its complete Mermaid preload list, and `mermaid.core` with JavaScript MIME types; both the reported source and both mock diagrams rendered successfully when executed directly. This disproves a Mermaid grammar/configuration failure in the currently served asset set.
- Production static assets and SPA HTML currently receive no explicit `Cache-Control` or validators from `api/assets.rs`. A long-lived page can therefore retain a parent chunk that names a no-longer-served child hash after a deployment. Vite emits `vite:preloadError` for this class of failure, but Phoenix has no recovery handler.
- The user-facing error is lossy: `errorMessage` displays only the browser's generic message, not the failed asset URL or a recovery action.

## Failure model

The Mermaid source crosses this boundary:

`assistant markdown -> MermaidDiagram -> Vite runtime import/preload -> hashed embedded asset -> Mermaid renderer`

The observed failure occurs at the runtime import/preload boundary. Mermaid is nested lazy-loaded after the conversation UI is already usable, so a deployed-asset generation change, a negative browser module-cache entry, or a transient chunk request failure can reject the import while all surrounding UI remains healthy. The component catches that rejection and permanently enters its error state; it neither retries nor distinguishes module acquisition from diagram parsing.

The currently deployed asset generation is internally coherent in Chromium, so the original browser-specific request/response remains an unknown. Preserve the failed URL and browser console/network evidence in future regression diagnostics rather than claiming a parser failure.

## Owning invariant

A valid fenced Mermaid diagram from the currently loaded Phoenix UI generation must either render without a late cross-generation asset dependency or recover safely from one bounded module-acquisition failure. A module-load failure must not be presented as a diagram syntax failure.

## Proposed scope

1. Remove or harden Mermaid's second-tier lazy import seam. Prefer making Mermaid part of the already-loaded conversation/markdown renderer module graph unless a measured bundle constraint requires laziness. If laziness remains, add a bounded, generation-aware recovery path that cannot reload-loop or discard user draft/queue state.
2. Give SPA HTML and content-hashed assets explicit, complementary cache semantics in `crates/phoenix-ide/src/api/assets.rs`: HTML must revalidate; immutable hashed assets may be cached. Account for the fact that a new embedded binary does not retain old hashes, rather than assuming `immutable` alone makes in-flight old pages safe.
3. Separate module-acquisition failures from Mermaid parse/render failures in `MermaidDiagram`. Preserve source fallback, expose an actionable retry/reload affordance where safe, and log enough detail (including the rejected asset URL when available) to diagnose the next occurrence.
4. Add production-build browser coverage that starts an isolated mock-model server, sends `[[scenario:mermaid]]`, and verifies both diagrams render from emitted hashed assets. Add a deployment-generation or controlled chunk-failure case covering the selected recovery policy.
5. Add backend response-header tests and focused component tests for bounded recovery, successful retry, persistent parse failure, and no retry/reload loop.

Likely starting symbols:

- `ui/src/components/MermaidDiagram.tsx`
- `ui/src/main.tsx` if handling Vite's `vite:preloadError`
- `crates/phoenix-ide/src/api/assets.rs`
- `crates/phoenix-llm/src/mock.rs` (`[[scenario:mermaid]]`, reuse rather than add another fixture)
- browser/E2E harness around `tests/e2e/`

## Acceptance evidence

- In a production-built mock-server run, `[[scenario:mermaid]]` renders its flowchart and sequence diagram with no module-loader console errors.
- A controlled stale/missing Mermaid chunk follows the documented bounded recovery path and subsequently renders when a coherent generation is available.
- A malformed Mermaid fence remains on source with a parse-specific error and does not trigger page reload or module retry.
- Cache-header tests prove HTML revalidation and the selected hashed-asset policy for embedded and development fallback responses.
- `./dev.py check` passes.

## Risks and non-goals

- Do not add a service worker or parallel asset manifest/cache representation.
- Do not retain unlimited historical UI bundles in the binary.
- Do not unconditionally reload on every dynamic-import rejection; that can loop during outages and can disrupt active work.
- Do not change Mermaid grammar, theme, source syntax, or fullscreen behavior unless regression evidence requires it.
- Keep the fix focused on the production asset-generation/import boundary; general offline support is out of scope.
