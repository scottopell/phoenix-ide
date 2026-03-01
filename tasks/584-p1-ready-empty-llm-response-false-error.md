---
created: 2026-02-28
number: 584
priority: p1
status: ready
slug: empty-llm-response-false-error
title: "Empty LLM response after tool result treated as error instead of completion"
---

# Empty LLM Response False Error

## Problem

When claude-haiku (and possibly other models) returns `stop_reason=end_turn` with zero
content blocks after a tool result, the backend raises `LlmError::invalid_response`:

```
Anthropic returned empty response (no content or tool calls,
stop_reason=Some("end_turn"), output_tokens=2, raw_blocks=0)
```

This is semantically valid: the model completed the tool call loop and has nothing
further to say. The runtime should treat it as a successful `agent_done` transition,
not an error.

## Reproduction

1. Open any conversation using claude-haiku-4-5
2. Send: "run `echo hello from qa test` and nothing else"
3. The bash tool executes successfully (output: `hello from qa test`)
4. Backend errors instead of completing cleanly

## Root Cause

`src/llm/anthropic.rs` normalizes content blocks and then rejects responses where
the normalized block list is empty. An empty block list after a tool result with
`stop_reason=end_turn` is valid — the model is just done.

See `src/llm/anthropic.rs` around the `"Anthropic returned empty response"` error path.

## Fix

Before returning the error, check: if `stop_reason == end_turn` and `content.is_empty()`
(no text, no tool_use), treat it as a successful empty response (no LLM message emitted,
conversation transitions to idle). This is distinct from a genuinely malformed response
where raw blocks existed but were all filtered.

Alternatively, emit a synthetic empty-text assistant message so the runtime state machine
always gets _something_ to process.

## Files

- `src/llm/anthropic.rs` — the check ~line 481
- `src/runtime/` — caller that handles `LlmError` and decides next state
