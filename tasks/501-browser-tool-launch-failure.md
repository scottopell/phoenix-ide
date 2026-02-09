---
created: 2026-02-07
priority: p1
status: done
---

# Browser Tool Launch Failure - Chrome Process Exit

## Summary

The `browser_navigate` tool fails to launch Chrome with error:
```
Failed to get browser: Failed to launch browser: Browser process exited with status ExitStatus(unix_...
```

## Context

Observed in conversation `cnn-lite-design-aesthetic-analysis` when the LLM tried to navigate to `https://lite.cnn.com`. The browser tools are registered and the LLM correctly attempts to use them, but Chrome fails to launch.

Possible causes:
1. Chrome sandbox issues in the server environment
2. Missing Chrome dependencies
3. Chromiumoxide configuration issues (headless mode, sandbox flags)
4. Leftover Chrome processes or lock files from previous runs

## Reproduction

1. Start Phoenix with `./dev.py up`
2. Create conversation asking to navigate to a URL using browser tools
3. Observe the browser_navigate tool failure

## Relevant Files

- `src/tools/browser/session.rs` - Browser launch configuration
- `src/tools/browser/tools.rs` - Tool implementations

## Acceptance Criteria

- [ ] Browser tools successfully launch Chrome in headless mode
- [ ] Add proper error messages that help diagnose launch failures
- [ ] Consider adding `--no-sandbox` flag for server environments
- [ ] Add cleanup of stale Chrome processes/lock files on startup
- [ ] Add integration test for browser tool basic functionality

## Notes

The LLM tried to work around the failure by running `pkill -f chromium` and `rm -rf /tmp/chromiumoxide-runner`, suggesting there may be stale processes or lock files causing issues.

Chromiumoxide browser launch config is in `BrowserSession::new()` in session.rs.
