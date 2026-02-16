---
created: 2026-02-07
priority: p2
status: done
---

# Add Browser Tools End-to-End Tests

## Summary

The browser tools have never been tested end-to-end. They compile but runtime behavior needs verification.

## Context

Browser tools implemented:
- `browser_navigate` - Navigate to URL
- `browser_eval` - Execute JavaScript
- `browser_take_screenshot` - Capture page screenshot  
- `browser_resize` - Resize viewport
- `browser_recent_console_logs` - Get console output
- `browser_clear_console_logs` - Clear console logs

All tools pass compilation and the tool registry test confirms they're registered, but actual runtime behavior with a real browser hasn't been tested.

## Acceptance Criteria

- [ ] Integration test that launches browser and navigates to example.com
- [ ] Test that screenshot is created and contains valid PNG data
- [ ] Test that JavaScript evaluation returns correct values
- [ ] Test that viewport resize works correctly
- [ ] Test session persistence across multiple tool calls
- [ ] Test error handling when browser fails to launch
- [ ] Tests can be skipped in CI if Chrome not available (feature flag)

## Notes

May need to install Chrome in CI environment or use a headless browser testing approach. Consider using `#[ignore]` attribute with manual test runs initially.

Relevant test file: `src/tools/browser/tests.rs` (to be created)
