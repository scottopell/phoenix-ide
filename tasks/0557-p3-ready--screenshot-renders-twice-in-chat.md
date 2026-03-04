---
created: 2026-02-19
priority: p3
status: ready
---

# Screenshot renders twice in chat UI

## Summary

When an agent takes a screenshot, the image appears twice in the chat: once
from the `browser_take_screenshot` tool result and again from the subsequent
`read_image` call. The second call is currently necessary for the LLM to
actually see the image, but results in a confusing UX.

## Context

`browser_take_screenshot` puts the PNG in `display_data`, which is stored in
the DB and broadcast to the UI — so the user sees the image inline. However,
`ContentBlock::ToolResult` only carries a `content: String` field, so the
image is **not** sent to the LLM as part of the tool result. The LLM only
receives `"Screenshot taken (saved as /tmp/phoenix-screenshot-xxx.png)"`.

To actually see the screenshot, the agent must follow up with `read_image`,
which returns the base64 PNG inline in its tool result. That second call is
what gives the LLM vision of the page — but from the user's perspective it
looks redundant because the image renders in the chat twice.

## Two candidate fixes

**Option A — `browser_take_screenshot` returns the image inline**
Add image support to `ContentBlock::ToolResult` (Anthropic's API supports
multi-part tool results with image blocks). The screenshot tool returns the
PNG directly in the tool result, the LLM sees it immediately, and `read_image`
is no longer needed. `display_data` can be dropped or kept for the UI.

Trade-off: requires extending `ContentBlock` and updating the Anthropic/OpenAI
translators to handle image blocks in tool results. OpenAI may not support
this natively.

**Option B — suppress `display_data` on `browser_take_screenshot`**
Keep the current flow (LLM sees image via `read_image`) but don't emit
`display_data` from the screenshot tool. The image renders once, from
`read_image`. Simpler change but the agent still makes two tool calls.

Trade-off: slightly more LLM round-trips; the `read_image` call is
an implementation detail leaking into agent behavior.

## Acceptance Criteria

- [ ] Screenshot appears exactly once in the chat UI per agent screenshot action
- [ ] The LLM receives the image content and can reason about it
- [ ] No redundant tool calls visible to the user

## Notes

- Anthropic tool result format does support multi-part content including images:
  `{"type": "tool_result", "tool_use_id": "...", "content": [{"type": "image", ...}]}`
- OpenAI tool results only support string content — Option A would need
  provider-specific handling or a fallback to Option B for OpenAI
- Option A is the better UX fix; Option B is the safer/simpler fix
