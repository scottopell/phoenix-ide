# React UI performance pass: do less work

## Goal

Take an evidence-driven performance pass through the React UI with the principle: **good performance means doing as little as possible**. Prefer removing subscriptions, renders, loops, DOM reads/writes, effects, and bundle work over adding memoization ceremony.

## Starting observations

The UI already has several important optimizations in place:

- `ConversationStore` uses routed `useSyncExternalStore` subscriptions, so unrelated conversations should not re-render on token churn.
- `MessageList` isolates streaming text outside the memoized historical message body.
- Heavy panels in `ConversationPage` are already lazy-loaded.
- Conversation rows and message components have existing `React.memo` coverage and regression tests.

That means this pass should be **profile-first** and targeted. Avoid adding broad `useMemo` / `useCallback` unless it demonstrably prevents downstream work.

## Profiling plan

Use the new perf tooling plus Chrome/browser profiling to establish before/after evidence:

1. Start the dev app with `./dev.py up`.
2. Capture baseline scenarios with `browser_profile.run_scenario` and/or Chrome trace:
   - Open a conversation with many messages.
   - Stream/update a conversation while historical messages are present.
   - Type in the composer with autocomplete closed.
   - Type `/` or `@` with autocomplete open.
   - Open/close sidebar menus and navigate between conversations.
3. Use `browser_profile.why_render` or React render instrumentation where useful to identify avoidable renders.
4. Use CPU traces for any suspicious hot path before changing it.
5. Re-run the same scenarios after each logical change and keep raw before/after samples in the final report.

## Optimization targets to investigate

Focus on places that may still do unnecessary work:

### 1. Composer typing path

Investigate `ui/src/components/InputArea.tsx`:

- Confirm whether regular keystrokes avoid file/skill fuzzy matching and autocomplete work when no trigger is active.
- Ensure trigger detection, auto-resize, and selection handling do not force unnecessary state updates.
- Verify voice/autocomplete state is not subscribed or recalculated on paths that do not need it.
- Prefer early exits and narrower state updates over memoizing everything.

### 2. Message list streaming path

Investigate `ui/src/components/MessageList.tsx`, `StreamingMessage.tsx`, and `MessageComponents.tsx`:

- Confirm token arrivals only re-render the streaming subtree, not historical messages or message context menu work.
- Look for derived collections rebuilt during streaming (`filter`, `slice`, `Set`, `Map`) and either prove they are outside token churn or reduce them.
- Check scroll/ResizeObserver logic for avoidable state writes during streaming.
- Avoid full virtualization rewrites unless the trace proves the current bottom-anchored window is the bottleneck.

### 3. Sidebar/list path

Investigate `ConversationList.tsx`, `DesktopLayout.tsx`, and `useConversationsList`:

- Confirm polling/SSE updates do not rebuild or re-render unchanged rows.
- Reduce repeated per-row formatting/path parsing if it shows up in traces.
- Ensure menu/open state changes affect only the relevant row/chain block.

### 4. Bundle and lazy-loading path

Use the Vite build output to identify avoidable initial bundle cost:

- Look for accidental eager imports of heavy modules (`react-syntax-highlighter`, xterm, browser/diff panels).
- Prefer conditional/lazy import only when it removes real initial work.
- Do not split tiny components just to increase chunk count.

## Guardrails

- No behavior changes.
- No speculative memoization: every optimization should be tied to observed unnecessary work or a clear structural no-op prevention.
- Remove redundant work where possible; memoization is second choice.
- Keep state local and subscriptions narrow.
- Add or update tests when changing render-stability assumptions.
- Run `./dev.py check` before committing.

## Expected output

- A small set of targeted UI performance changes.
- Before/after profiling notes with raw samples or trace summaries.
- Tests for any changed render-stability behavior.
- A local commit with the completed work.
