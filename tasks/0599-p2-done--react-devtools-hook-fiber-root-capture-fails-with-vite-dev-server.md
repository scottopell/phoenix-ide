---
created: 2026-03-02
number: 599
priority: p2
status: done
slug: react-devtools-hook-fiber-root-capture-fails-with-vite-dev-server
---

# browser_inject_react_devtools: onCommitFiberRoot never fires with Vite dev server

## Problem

When `browser_inject_react_devtools` is used to navigate to the Phoenix IDE Vite dev server (http://localhost:8042), the `onCommitFiberRoot` callback is never called. As a result, `window.__phoenix.listContexts()` always returns `[]` even though React IS rendering (React Router future flag warnings appear in console logs).

## Evidence

From QA testing (task 596):

- `window.__phoenix` is installed and accessible (typeof = "object")
- `window.__REACT_DEVTOOLS_GLOBAL_HOOK__` is present with `supportsFiber: true`
- `window.__REACT_DEVTOOLS_GLOBAL_HOOK__.hasRenderers` is false (renderers Map exists but is empty)
- React Router warnings DO fire in console (React is executing module code)
- `#root` element exists but has 0 child nodes after 10+ seconds
- No React fiber keys (`__reactFiber$*`) found anywhere in the DOM
- `browser_wait_for_selector('#root > *', 10s)` always times out
- Screenshot shows a blank white page

## Contrast with Playwright MCP

The Playwright MCP browser (used in the same QA session) renders the Phoenix UI fully:
- Full conversation list visible
- React fiber keys present: `__reactFiber$bzcucyu5v4v` on DIV elements
- 5 context providers found via fiber walking
- `getContext(['openFile', 'closeFile'])` returns the FileExplorerContext with `openFile` as a function

## Root Cause Hypothesis

Phoenix's browser session uses `--headless=new` mode Chromium. Vite dev server serves `main.tsx` as a `type="module"` ES module. There appears to be a timing or rendering issue where:

1. `addScriptToEvaluateOnNewDocument` installs the hook correctly
2. Vite's module loads (React Router warnings confirm JS execution)
3. But `ReactDOM.createRoot(...).render(...)` in `main.tsx` never commits to the DOM

Possible causes:
- Chromium headless mode with `--disable-gpu` + `--disable-software-rasterizer` prevents React 18's concurrent scheduler from executing work items
- React 18's `createRoot` defers initial work to a scheduler task that never runs in this headless context
- Vite's `@vite/client` module does something that interferes with React's initial render in headless mode

## Reproduction Steps

1. `./dev.py restart` to ensure latest code
2. Create a new conversation via phoenix-client.py
3. In the conversation: `browser_inject_react_devtools`
4. `browser_navigate http://localhost:8042`
5. `browser_eval` with expression `document.querySelector('#root').childNodes.length` → returns 0
6. `browser_eval` with expression `window.__phoenix.listContexts().length` → returns 0
7. `browser_wait_for_selector '#root > *' timeout=10s` → times out

## Impact

The `browser_inject_react_devtools` tool fails its primary acceptance criterion:
> `getContext(['openFile'])` returns the `FileExplorerContext` value in the Phoenix IDE UI

An agent using this tool against the Vite dev server will always get empty context results. The tool DOES work structurally (injection succeeds, hook is present) and would work correctly if React were rendering.

## Possible Fixes

1. Add `--enable-javascript` or equivalent flag to the Chromium launch config
2. Test with `--headless=old` vs `--headless=new` mode
3. Try removing `--disable-gpu` / `--disable-software-rasterizer` flags
4. Build the UI (`cd ui && npm run build`) so Phoenix serves the embedded static build instead of Vite dev mode
5. Add `browser_inject_react_devtools` documentation note about this limitation

## Related

- Task 596: browser_inject_react_devtools implementation
- Session config: `src/tools/browser/session.rs` (BrowserConfig builder)
