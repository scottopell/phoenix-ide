# Anthropic message-history cache breakpoint silently dropped during tool loops

`translate_request` places the message-history `cache_control` breakpoint only
when the last user message's last content block is `Text` or `Image`. During
multi-step agent execution the last user message is a tool-result message whose
only block is a `ToolResult` — which has no `cache_control` field — so the
breakpoint is silently skipped and the accumulating tool-loop tail is never
written to Anthropic's prompt cache.

## Verified locations
- `crates/phoenix-ide/src/llm/anthropic.rs:570-584` — breakpoint placement:
  `if let Some(AnthropicContentBlock::Text { cache_control, .. }
  | AnthropicContentBlock::Image { cache_control, .. }) =
  last_user.content.last_mut()`. A `ToolResult` last block falls through with
  no breakpoint set.
- `crates/phoenix-ide/src/llm/anthropic.rs:991-997` —
  `AnthropicContentBlock::ToolResult` has no `cache_control` field; only
  `Text` (`:976-980`) and `Image` (`:981-985`) carry one.
- `crates/phoenix-ide/src/runtime/executor.rs:2240-2248` —
  `build_llm_messages_static` emits each stored tool result as its own
  user-role `LlmMessage` carrying a single `ToolResult` block. So whenever the
  previous turn was a tool round, the last user message is a `ToolResult`.

## Impact (why p1)
Agent conversations are tool-loop-dominated. After the first genuine user-text
turn, every subsequent tool-loop request has no message breakpoint: Anthropic
still cache-*reads* the prefix cached at that last user-text turn, but the
`tool_use`/`tool_result` blocks accumulated since are never cache-*written*.
They are re-sent and re-charged at full input-token price on every loop turn
until the next real user message resets a breakpoint. A continuous, silent cost
regression on the hottest path. (OpenAI is unaffected — it uses automatic
prefix caching via `prompt_cache_key` and needs no explicit breakpoints.)

## Fix direction
Add `cache_control: Option<CacheControl>` (with
`#[serde(skip_serializing_if = "Option::is_none")]`) to
`AnthropicContentBlock::ToolResult`, and extend the breakpoint match in
`translate_request` to set it on a trailing `ToolResult` block. Anthropic
permits `cache_control` on `tool_result` blocks and allows 4 breakpoints;
Phoenix uses at most 3 (system, last tool, last user message), so this reuses
the existing message-breakpoint slot rather than adding one. The
`ContentBlock::ToolResult -> AnthropicContentBlock::ToolResult` translation in
`translate_message` must thread `cache_control: None` by default.

`ToolResult` is the only non-Text/Image block that realistically ends a user
message, so adding the field there is the minimal correct-by-construction fix.

## Acceptance
- A tool-loop request (last user message is a `ToolResult`) carries a
  message-history `cache_control` breakpoint.
- Regression test asserting the breakpoint lands on a trailing `tool_result`
  block.
- `./dev.py check` passes.

## Related
- Discovered while exploring the caching model for the tool-result eviction
  task (p2, drafted alongside this one).
- Deferred-tools interaction: when tool search is active the *tools* breakpoint
  is also skipped (`anthropic.rs:545`, `has_deferred`), making a correct
  message breakpoint more important.
