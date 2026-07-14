# Add fullscreen Mermaid diagram viewing with native browser navigation

## Context

Phoenix renders fenced Mermaid blocks through the shared `MermaidDiagram` component across conversations, task/proposal review, and Markdown file viewing. The inline rendering is intentionally constrained to its surrounding content width, which makes larger diagrams hard to inspect.

A fullscreen view should rely on the browser's own document/image navigation rather than adding custom pan, zoom, drag, wheel, or transform behavior to Phoenix.

## User-visible behavior

- A successfully rendered Mermaid diagram offers a clearly labelled fullscreen/open action alongside its existing source-copy control.
- Activating it opens the rendered SVG as a standalone browser document in a new tab, giving the diagram the full viewport.
- The standalone rendering preserves the current Phoenix light/dark Mermaid theme and diagram content.
- Users navigate the standalone diagram with native browser scrolling/panning and browser zoom or pinch gestures.
- The action is unavailable while the diagram is rendering, when rendering failed, and when only source is being viewed.
- Source/diagram switching, source copying, render-error fallback, and every existing Mermaid surface continue to behave as before.

## Implementation plan

1. Extend the shared `ui/src/components/MermaidDiagram.tsx` renderer so a rendered SVG has a browser-openable SVG URL.
   - Build the resource from the exact successful Mermaid render output with the `image/svg+xml` media type.
   - Manage object-URL replacement and cleanup when the render, theme, source, or component lifetime changes.
   - Expose it through a semantic link opened with `target="_blank"` and `rel="noopener noreferrer"`, preserving direct browser navigation and avoiding popup-script behavior.
2. Add a compact fullscreen/open icon action to the Mermaid toolbar with accessible label/title and styling colocated with the existing Mermaid styles.
3. Do not add an in-app fullscreen overlay, Fullscreen API state, wheel listeners, pointer-drag handlers, SVG transforms, zoom controls, or dependencies. The browser remains the sole owner of panning and zooming.
4. Add focused component tests covering:
   - the action appearing only after a successful diagram render,
   - its standalone SVG resource and safe new-tab attributes,
   - its absence for loading/error and source-only states,
   - object URL cleanup on replacement/unmount,
   - preservation of the existing Mermaid source and error behavior.
5. Run the focused UI tests plus the repository check, and manually verify a large diagram in a real browser at desktop and narrow viewport sizes, including native browser zoom/pinch and scrolling.

## Acceptance criteria

- Every surface using `MermaidDiagram` gains the fullscreen action without surface-specific integration.
- The action opens the rendered diagram by itself in a browser tab and does not navigate the Phoenix tab.
- Pan/scroll and zoom interactions are supplied by the browser; Phoenix contains no custom interaction implementation for them.
- Blob/object URLs do not leak as diagrams rerender or unmount.
- Existing Mermaid rendering, theming, source controls, accessibility, and failure fallback remain intact.
