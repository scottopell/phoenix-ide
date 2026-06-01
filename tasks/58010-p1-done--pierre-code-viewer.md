Render the code file viewer through @pierre/diffs CodeView (file item) with the same annotation overlay, gutter affordances, and typed scroll/jump the diff viewer uses, replacing the react-syntax-highlighter + AnnotatableBlock path in CodeViewerBody.

Motivation: CodeViewerBody synchronously highlights the whole file and wraps every row in AnnotatableBlock, the first-open freeze hot path. Pierre virtualizes, eliminating the freeze and removing the need for the plainLargeText fallback for code.

Scope: code payloads only. Plain text and HTML source mode are fast-follows (tasks 58008, 58009).

Decisions:
- New PhoenixFileCodeView wrapper rendered through MetaViewer for kind=code; bodyScroll=children.
- New pierreFileMapping pure module (file item + LineAnnotation mapping, render signature, scroll targets), unit-tested.
- Full notes parity: gutter +, click, touch long-press, inline overlay, panel jump+flash.
- patchContext modified-line shading preserved via unsafeCSS line decoration (no native Pierre primitive); auto-scroll to first change via typed scrollTo.
- Scroll restoration via Pierre onScroll + scrollTo position. cmd+A span-select and selection-copy dropped for code (copy-all button retained).
- Stop classifying code to plainLargeText; keep that fallback for text/markdown.
- Route code through PhoenixFileCodeView. Keep CodeViewerBody — HTML source mode still uses it until task 58009 migrates it; keep AnnotatableBlock (text still uses it).
