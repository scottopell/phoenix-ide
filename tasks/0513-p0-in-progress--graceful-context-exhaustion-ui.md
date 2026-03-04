---
created: 2026-02-08
priority: p0
status: in-progress
---

# Graceful Context Exhaustion UI

## Summary

When a conversation exceeds the token limit (context exhaustion), the UI should clearly communicate this to the user and provide options to recover.

## Context

When a conversation's prompt exceeds the model's token limit (e.g., 200k tokens for Claude), the API returns an error:
```json
{
  "state": {
    "type": "error",
    "error_kind": "invalid_request", 
    "message": "prompt is too long: 227515 tokens > 200000 maximum"
  }
}
```

Previously, the UI showed "Ready" status even when in this error state, leaving users confused about why their messages weren't being processed.

A partial fix was implemented (commit 791e995) to show "Token limit exceeded" in the status indicator, but more work is needed for a complete solution.

## Acceptance Criteria

- [x] Status indicator shows error state (red dot + "Token limit exceeded") - DONE
- [ ] Add a more prominent error banner/toast when context is exhausted
- [ ] Provide clear user guidance on what to do:
  - Start a new conversation
  - Fork from an earlier point (if supported)
  - Option to compact/summarize conversation history (future feature)
- [ ] Consider showing context window usage proactively (e.g., "85% of context used")
- [ ] Prevent sending new messages when in error state (or show warning)
- [ ] Add tooltip or "learn more" link explaining token limits

## Technical Notes

- The `convState` is set to `'error'` and `stateData.message` contains the error details
- Context window size is available via `context_window_size` in SSE init data
- Error detection regex: `errorMsg.includes('tokens')` for token-related errors

## Related Files

- `ui/src/components/InputArea.tsx` - Status indicator logic
- `ui/src/pages/ConversationPage.tsx` - State management
- `ui/src/index.css` - `.dot.error` styling exists

## Notes

This was discovered when a conversation hit 227k tokens. The user sent messages but saw "Ready" status with no response, not realizing the conversation was dead.
