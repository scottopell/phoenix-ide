---
created: 2026-03-02
priority: p1
status: done
---

# Anthropic max_tokens too low + stream truncation drops content

## Summary

Two compounding bugs cause an error when Claude exhausts its output budget:

1. **`max_tokens` hardcoded to 8192** — far too low for modern Claude 4.x models (which support 16k–64k+ output tokens). The model hits the ceiling on any substantive response.

2. **Stream truncation silently drops the in-progress content block.** When Anthropic cuts off at `max_tokens`, it does NOT send `content_block_stop` for the block being generated. Our `StreamAccumulator` only commits blocks on `content_block_stop`, so the entire response is lost. `normalize_response` then sees `raw_blocks=0` and returns an error.

## Reproduction

Observed in prod on conversation `conversation-mode-sub-agent-review` (claude-sonnet-4-6):
```
Anthropic returned empty response (no content or tool calls, stop_reason=Some("max_tokens"), output_tokens=8192, raw_blocks=0)
```

## Root Cause

### Event sequence when max_tokens is hit mid-block:
1. `content_block_start` → accumulator starts collecting text in `current_text`
2. Many `content_block_delta` events → text grows
3. **No `content_block_stop`** — Anthropic truncates the stream
4. `message_delta` with `stop_reason: "max_tokens"`, `output_tokens: 8192`
5. `message_stop` → `done = true`
6. `into_response()` → `content_blocks` is empty → error

### Code locations:
- `src/runtime/executor.rs:626` — hardcoded `max_tokens: Some(8192)`
- `src/llm/anthropic.rs:367` — default fallback `unwrap_or(8192)`
- `src/llm/anthropic.rs:156-180` — `on_block_stop()` is only place blocks get committed
- `src/llm/anthropic.rs:183-200` — `into_response()` doesn't flush in-progress blocks

## Fix

1. In `into_response()`, flush any in-progress block before building the response.
2. Increase `max_tokens` to 16384 (the executor request) and the Anthropic fallback default.

## Acceptance Criteria
- [ ] `StreamAccumulator::into_response()` flushes incomplete blocks
- [ ] `max_tokens` raised from 8192 to 16384
- [ ] `cargo test` passes
