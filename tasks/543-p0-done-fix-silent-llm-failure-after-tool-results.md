---
created: 2026-02-11
priority: p0
status: done
---

# Task 543: Fix Silent LLM Failure After Tool Results

## Summary

After parallel tool calls complete, the LLM API returned an empty response that was
silently accepted as valid, transitioning to Idle with no agent message.

## Context

Conversation `a8789f5d-ddb7-49ad-a1b5-784fdb62820b` on prod (AI Gateway) shows broken message sequence:

```
seq 1: user     (19:39:08) - prompt
seq 2: agent    (19:39:13) - 4 parallel tool calls
seq 3-6: tool   (19:39:13-14) - All 4 tool results
[40 second gap]
state: idle    (19:39:54) - Should have agent response, got nothing
```

Root cause: `normalize_response()` accepted empty content as valid. Fixed by adding
empty-response guards to all three providers (OpenAI, AI Gateway, Anthropic) plus
rejecting empty tool call names and invalid JSON arguments.

## Acceptance Criteria

- [x] No more silent failures after tool results
- [x] All API errors visible to user
- [x] Empty responses rejected with clear error
- [x] Property-based tests verify invariants

## Notes

- Root cause was task 544 (type safety violation in LlmResponse)
- Fixed in commits a310394 and 9d568af
