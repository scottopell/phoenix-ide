---
created: 2026-02-07
priority: p1
status: done
---

# Browser Tools Need Headless/Sandbox Configuration

## Summary

The browser tools use chromiumoxide's auto-detection for Chrome path and default settings. In server environments, this likely fails due to sandbox restrictions and missing display.

## Context

From the implementation notes:
- Uses `.with_head()` which falls back to headless if no display
- Chrome path uses auto-detection
- No explicit sandbox configuration

The error seen was:
```
Failed to launch browser: Browser process exited with status ExitStatus(unix_...
```

This is typically caused by Chrome's sandbox requiring specific kernel capabilities that aren't available in containers/VMs.

## Required Configuration

For server environments, Chrome typically needs:
```
--headless
--no-sandbox
--disable-gpu
--disable-dev-shm-usage
```

## Acceptance Criteria

- [ ] Add explicit `--no-sandbox` flag for server environments
- [ ] Add `--disable-dev-shm-usage` to avoid shared memory issues
- [ ] Make Chrome path configurable via environment variable
- [ ] Add `--headless=new` for newer Chrome headless mode
- [ ] Log Chrome launch command for debugging
- [ ] Document required Chrome installation for deployment

## Relevant Files

- `src/tools/browser/session.rs` - `BrowserSession::new()` launch config

## Notes

This is likely the root cause of task 501 (browser launch failure). Fixing the launch config should resolve that issue.

Consider making sandbox mode configurable for development (with sandbox) vs production (no sandbox).
