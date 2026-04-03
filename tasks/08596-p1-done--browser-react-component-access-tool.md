---
created: 2026-03-02
priority: p1
status: done
artifact: completed
---

# Browser: React Component Access Tool

## Problem

Accessing React component state or context from `browser_eval` currently requires
manual fiber tree walking — finding the `__reactFiber` key on a DOM node, then
traversing `return` / `child` links while checking `tag === 10` (ContextProvider)
or matching display names. This approach is:

- **Fragile**: relies on internal React fiber shape that can change across versions
- **Verbose**: takes 6+ sequential `browser_eval` calls to locate a single context
  value and call a method on it
- **Breaks with minification**: component display names (`FileExplorerContext`,
  etc.) are stripped in production builds, making name-based lookup impossible

### Real Motivation (from Task 595 fix verification)

While verifying the Task 595 fix, the agent needed to call `openFile` on
`FileExplorerContext`. It required:

1. Finding a DOM node with a React fiber key
2. Reading `Object.keys(el)` to identify the fiber property name
3. Walking `fiber.return` links up the tree
4. Checking `fiber.tag === 10` to identify context providers
5. Inspecting `fiber.type.displayName` to find the right provider
6. Extracting `fiber.memoizedProps.value` to get the context value
7. Finally calling `value.openFile(...)`

With a proper tool this would be a single call.

## Solution

Add a `browser_inject_react_devtools` tool (name TBD) that installs a lightweight
`window.__phoenix` helper into the page *before* page JS runs, using CDP's
`Page.addScriptToEvaluateOnNewDocument`. The helper hooks into React's
`__REACT_DEVTOOLS_GLOBAL_HOOK__` — a well-known interface React checks for at
startup. If the hook exists when React initialises, React registers its fiber
roots into it, giving structured access to the live component tree without any
DOM traversal.

### Why `__REACT_DEVTOOLS_GLOBAL_HOOK__` and not raw fiber walking?

React itself guarantees this hook interface for DevTools integration. It is
stable across minor versions and does not depend on minification. The hook
receives `onCommitFiberRoot` callbacks with direct access to the root fiber,
eliminating the need to locate a DOM anchor node first.

### Proposed `window.__phoenix` API

```js
// Find a context value by duck-typing its shape (key presence check)
window.__phoenix.getContext(keys: string[]) => any
// e.g. __phoenix.getContext(['openFile', 'closeFile'])

// Find a context value and call a method on it
window.__phoenix.callContext(keys: string[], method: string, ...args: any[]) => any
// e.g. __phoenix.callContext(['openFile'], 'openFile', '/src/main.rs')

// Get hook state array for a component identified by display name
window.__phoenix.getState(componentName: string) => any[]
// e.g. __phoenix.getState('FileExplorer')
```

All three functions traverse fiber roots registered via the DevTools hook and
search depth-first, so they work regardless of where in the tree the component
lives.

## Design Decisions

### Explicit and opt-in — NOT automatic on every navigate

The agent must explicitly call `browser_inject_react_devtools` before navigating
to (or reloading) the target page. It is NOT injected automatically by
`browser_navigate`.

**Why not auto-inject?**

- Pages with their own `__REACT_DEVTOOLS_GLOBAL_HOOK__` (e.g. apps that ship
  their own DevTools integration, or the Phoenix IDE UI itself) would have their
  hook silently overwritten, breaking their DevTools integration.
- Auto-injection is implicit and surprising — an agent reading a page trace
  would not know the helper is present without checking.
- Explicit opt-in is transparent: the tool call appears in the conversation
  history, making it obvious the hook is active.

### Middle ground considered

| Approach | Pro | Con |
|---|---|---|
| Auto-inject on every navigate | Zero setup | Breaks existing hooks; implicit |
| Explicit `browser_inject_react_devtools` | Transparent; safe | Requires an extra call before navigating |
| Pure fiber walking in `browser_eval` | No new tool | Fragile, verbose, breaks with minification |

Explicit injection is the right balance.

## Implementation Notes

- Tool lives in `src/tools/browser_react.rs` (or similar)
- Uses `Page.addScriptToEvaluateOnNewDocument` CDP method — script survives
  navigations within the same CDP session
- Should return the injected script identifier so a future
  `browser_remove_react_devtools` tool could clean up via
  `Page.removeScriptToEvaluateOnNewDocument`
- The injected script must guard against double-registration (idempotent)
- `getContext` duck-typing: a context value matches if **all** supplied keys are
  present on the object (using `in` operator, not value check)
- `getState`: returns the `memoizedState` linked-list unwound into an array;
  caller must know the hook order (same limitation as React DevTools)
- No React import needed — the hook is installed before React loads

## Acceptance Criteria

- [ ] `browser_inject_react_devtools` tool exists and is registered
- [ ] After calling the tool and (re)navigating to a React page, `window.__phoenix` is available
- [ ] `getContext(['openFile'])` returns the `FileExplorerContext` value in the Phoenix IDE UI
- [ ] `callContext(['openFile'], 'openFile', '/some/path')` opens the file without extra `browser_eval` steps
- [ ] The tool is documented in `specs/browser-react-devtools/executive.md`
- [ ] Injecting on a non-React page is harmless (hook exists but is never called)
- [ ] Double-calling the tool before navigating does not error or double-register
