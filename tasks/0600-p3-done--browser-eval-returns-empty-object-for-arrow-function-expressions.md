---
created: 2026-03-02
number: 600
priority: p3
status: done
slug: browser-eval-returns-empty-object-for-arrow-function-expressions
---

# browser_eval: Arrow function expressions return {} instead of error or result

## Problem

When `browser_eval` is called with an arrow function expression (e.g. `() => typeof window.__phoenix`), the tool returns `{}` instead of:
- The function's return value (if the intent was to call it)
- An error indicating the expression evaluates to a function object

This is confusing behavior. The expression `() => typeof window.__phoenix` defines a function but does NOT call it. CDP's `evaluate` returns the function object itself, which has no JSON-serializable `.value`, causing `browser_eval` to return `{}` (the empty JSON from the unserializable RemoteObject).

## Evidence

From QA testing session:

All of these return `{}`:
- `() => typeof window.__phoenix`
- `() => window.__phoenix.listContexts()`
- `() => window.__REACT_DEVTOOLS_GLOBAL_HOOK__ ? 'hook_present' : 'no_hook'`

The same expressions written as IIFEs or plain expressions work correctly:
- `typeof window.__phoenix` → `"object"`
- `(function(){ return window.__phoenix.listContexts().length; })()` → `0`
- `window.__REACT_DEVTOOLS_GLOBAL_HOOK__ ? 'hook_present' : 'no_hook'` → `"hook_present"`

## Root Cause

CDP's `Runtime.evaluate` treats `() => ...` as a function definition expression, evaluating to a function object. Function objects are not JSON-serializable, so CDP's RemoteObject has no `.value` field. The `browser_eval` implementation falls through to the warning branch and returns `"undefined"` which is then wrapped in the `<javascript_result>` tag — but agents see `{}` because somewhere in the serialization chain, the function object is represented as `{}`.

Looking at `src/tools/browser/tools.rs` line 198-214: when `eval_result.value()` returns None (as it does for function objects), the tool logs a warning and returns `"undefined"`. But the agent in testing saw `{}`, not `"undefined"`.

Actually, looking more carefully: the `{}` may come from the chromiumoxide `.value()` method returning `Some(Value::Object({}))` for function objects rather than `None`. Either way, the result is misleading.

## Impact

Agents familiar with Playwright's `page.evaluate()` API will naturally write `() => expression` style code (Playwright takes function arguments). When used with `browser_eval`, these silently return `{}` instead of executing the function. This caused significant confusion during task 596 QA testing.

## Proposed Fix

Option A: Detect arrow function / function expression patterns in the `expression` input and auto-wrap them as IIFEs. Regex: if expression starts with `(` and contains `=>`, or starts with `function`, wrap with `(expression)()`.

Option B: Document in the tool description that expressions must not be function definitions — they should be plain expressions or IIFEs.

Option C: When `value()` returns None (unserializable result), check if the CDP type is `function` and return a specific error: `"Error: expression evaluated to a function — did you mean (expr)() to call it?"`.

Option B is safe and requires no code change. Option C provides the best DX. Option A is risky (false positives on legitimate function expressions that shouldn't be called).

## Reproduction

1. Create any browser conversation
2. `browser_navigate` to any page
3. `browser_eval` with expression: `() => typeof window`
4. Observe: returns `{}` instead of an error or "function"

## Related

- Task 596 QA testing
- `src/tools/browser/tools.rs` `BrowserEvalTool::run()`
