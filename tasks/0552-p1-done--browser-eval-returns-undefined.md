---
title: browser_eval returns undefined for valid JavaScript expressions
status: done
priority: p1
created: 2026-02-16
---

## Problem

The `browser_eval` tool returns `undefined` for JavaScript expressions that should return values. This makes the browser tools unreliable for QA and automation.

## Observed Behavior

1. **Valid expressions returning undefined:**
   ```javascript
   document.body.innerText  // returns undefined (should return string)
   document.body.innerHTML.slice(0, 2000)  // returns undefined
   JSON.stringify({bodyText: document.body.innerText})  // returns undefined
   ```

2. **Fetch calls appear to execute but console.log output not captured:**
   ```javascript
   fetch('/api/conversations/test-exhausted')
     .then(r => r.json())
     .then(data => console.log('Conversation state:', JSON.stringify(data, null, 2)));
   // Returns undefined, console output not visible in browser_recent_console_logs
   ```

3. **Reference errors work correctly** (expected behavior):
   ```javascript
   test  // Throws ReferenceError: test is not defined
   ```

## Expected Behavior

- `document.body.innerText` should return the page's text content
- `JSON.stringify(...)` should return a JSON string
- Console logs from async operations should appear in `browser_recent_console_logs`

## Context

Discovered during QA of task 551 (context exhausted state restoration). Was trying to verify UI state programmatically but had to fall back to curl + screenshots due to these issues.

## Impact

- Cannot reliably automate UI testing
- Cannot programmatically verify page content
- Forces reliance on visual screenshot inspection

## Files to Investigate

- `src/tools/browser.rs` - browser tool implementation
- Browser CDP protocol handling for JavaScript evaluation

## Reproduction

1. Start dev server: `./dev.py up`
2. Navigate to any page with browser_navigate
3. Try `browser_eval` with `document.body.innerText`
4. Observe undefined result instead of page text
