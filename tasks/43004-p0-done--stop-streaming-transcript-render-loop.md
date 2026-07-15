# Stop the streaming transcript render loop that blanks conversations

## Incident

Production long conversations consistently become blank with minified React error `#185` (maximum update depth exceeded). The failure reproduces every few seconds while an assistant message is actively arriving over SSE; rapid scrolling to the top and back toward the tail makes it easy to trigger. Once React aborts the render tree, the conversation page is empty.

The accompanying `interactive-widget` warning and blocked Cloudflare Insights beacon are unrelated browser/content-blocker noise. The terminal WebSocket disconnect may be consequential or independent and must not be treated as the root cause without evidence.

This is a P0 regression against REQ-VT-003/004/008/010/012: a dynamically measured, streaming transcript must retain a populated contiguous viewport and preserve reader/tail ownership without entering a render loop.

## Leading failure mechanism

`VirtualTranscript` currently has several synchronous update paths that can feed one another:

- mounted row/header ref callbacks measure DOM and call `publish()` during React commit;
- `ResizeObserver` reconciliation may compensate `scrollTop` and call `publish()`;
- the resulting scroll event recomputes the range and calls `publish()`;
- post-commit layout effects synchronously notify `MessageList`, whose range/height/pinned handlers may dispatch more positioning or scroll-machine work;
- a distant rapid scroll mounts estimated rows, whose real heterogeneous extents can move the anchored window and mount another set of rows, producing a chain of nested commit-phase updates;
- active SSE growth/finalization supplies recurring extent and state changes that repeatedly expose the chain.

The implementation phase must first capture the deterministic failing sequence and identify the exact cycle. Do not land a speculative debounce or timer. A self-authored-scroll guard is one candidate only if the reproduction proves scroll-event reentry is part of the cycle; batching/deferment of initial row measurements or edge-triggering external notifications may instead be required.

## Plan

1. Reproduce in an unminified development build with a production-scale long transcript and active SSE updates. Capture the full React stack and instrument bounded counters around row/header refs, `ResizeObserver`, `setScrollerScrollTop`, `handleScroll`, `publish`, and the three outward callbacks to identify the repeating edge.
2. Add a deterministic component regression that models the proven sequence: a long heterogeneous-height transcript, a distant top/tail scroll, mounted-row measurement churn, and recurring streaming growth/finalization. Make programmatic `scrollTop` writes emit realistic scroll events. The test must fail with error 185 or a strict bounded-update assertion before the fix.
3. Break the synchronous cycle at its ownership boundary while retaining `VirtualTranscript` as the sole physical authority. Reconciliation must be idempotent: no-op geometry/range/pinned observations do not publish, and a physical correction cannot recursively reinterpret itself as new user input.
4. Preserve all existing behavior: bounded contiguous rendering, stable keys, anchor compensation within 2 CSS px, reader ownership while off-tail, durable tail following while pinned, initial tail placement, prefix restoration, and exact-once positioning completion.
5. Add an integration/browser regression using the real `VirtualTranscript` (not the passthrough mock): while content is streaming, repeatedly move between distant transcript regions and assert that visible render units remain mounted, no uncaught React error occurs, and the page remains interactive. Exercise Chromium and WebKit/Safari ordering at minimum; use the project’s cross-browser conformance path for Firefox as applicable.
6. Run focused VirtualTranscript, MessageList, scroll-policy, transcript-positioning, and fixture tests, then `./dev.py check`. Verify manually against an actively streaming long conversation before deployment.

## Acceptance criteria

- The reported long-conversation + active-SSE reproduction can run continuously through repeated top/tail scrolling without React error 185 or a blank page.
- The regression test proves updates per scroll/measurement/streaming operation are bounded and fails on the pre-fix implementation.
- Streaming growth preserves the current reader anchor off-tail; pinned users remain at the tail.
- At every settled viewport state with transcript content, the rendered range is non-empty and contains visible content rather than spacer-only blank space.
- Programmatic compensation and browser-generated scroll/resize delivery cannot form an unbounded feedback loop.
- No timing-based debounce, retry, polling, or swallowed React error is used as the fix.
- Existing geometric continuity and positioning tests remain green, and cross-browser browser coverage passes.
