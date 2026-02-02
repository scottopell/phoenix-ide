---
created: 2026-01-31
priority: p3
status: done
---

# Context Window Usage Indicator

## Summary

Show users how much of the LLM context window has been used.

## Context

The backend tracks token usage per message (REQ-API-003 returns `context_window_size`). Long conversations can hit context limits, causing truncation or errors. Users should see this coming.

## Acceptance Criteria

- [x] Visual indicator showing context usage (progress bar or percentage)
- [x] Display in conversation header or status bar
- [x] Warning state when approaching limit (e.g., >80%)
- [x] Critical state near limit (e.g., >95%)
- [x] Tooltip showing exact token counts
- [x] Update after each message

## Notes

- Backend returns `context_window_size` in conversation response
- Need to know max context size per model (from `/api/models`?)
- Models have different limits: Haiku ~200k, Sonnet ~200k, Opus ~200k
- Consider showing input vs output token breakdown
- May want to suggest "start new conversation" when critical
