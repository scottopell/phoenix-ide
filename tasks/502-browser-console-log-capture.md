---
created: 2026-02-07
priority: p2
status: ready
---

# Browser Console Log Capture Not Implemented

## Summary

The `browser_recent_console_logs` tool exists but console logs are not actually being captured from the browser. The CDP event listener in `BrowserSession::new()` just logs events but doesn't store them.

## Context

From the continuation prompt:
> **Console log capture** - `add_console_log` is implemented but the CDP event listener in `BrowserSession::new()` doesn't actually hook it up yet (the handler task just logs events, doesn't capture them).

## Relevant Files

- `src/tools/browser/session.rs` - `BrowserSession::new()` spawns event handler task
- `src/tools/browser/session.rs` - `add_console_log()` method exists but isn't called
- `src/tools/browser/tools.rs` - `BrowserRecentConsoleLogsTool` implementation

## Acceptance Criteria

- [ ] CDP `Runtime.consoleAPICalled` events are captured and stored
- [ ] `browser_recent_console_logs` returns actual console output
- [ ] Console logs are stored with timestamps
- [ ] `browser_clear_console_logs` properly clears stored logs
- [ ] Add limit on stored log count to prevent memory issues

## Notes

The infrastructure is mostly there - just needs to wire up the event handler to call `add_console_log()` on the session.
