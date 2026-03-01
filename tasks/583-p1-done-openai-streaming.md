---
created: 2026-03-01
number: 583
priority: p1
status: done
slug: openai-streaming
title: "Token streaming for OpenAI Responses API and chat/completions"
---

# OpenAI Token Streaming

## Context

Task 582 implemented Anthropic SSE streaming. The `service.rs` `complete_streaming()`
falls back to non-streaming for `ApiFormat::OpenAIChat` with a `tracing::debug!` log.
This task fills that gap.

Two sub-paths exist under `ApiFormat::OpenAIChat` (see `openai.rs`):

1. **Responses API** (`/v1/responses`) — all non-Fireworks OpenAI models (gpt-4o, gpt-5, o4-mini, etc.)
2. **Chat completions** (`/v1/chat/completions`) — Fireworks models

Both need streaming implementations.

## Responses API SSE event format

When `stream: true` is added to the `ResponsesApiRequest`, the server sends:

```
event: response.output_text.delta
data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hello"}

event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"...","name":"...","arguments":"..."}}

event: response.done
data: {"type":"response.done","response":{"status":"completed","output":[...],"usage":{"input_tokens":N,"output_tokens":N}}}
```

Key events:
- `response.output_text.delta` → emit `TokenChunk::Text(delta)`
- `response.done` → extract full response (usage, all output items) — use as the final `LlmResponse`

## Chat completions SSE event format

When `stream: true` (already present in `OpenAIRequest`), the server sends:

```
data: {"id":"...","choices":[{"delta":{"content":"Hello"},"finish_reason":null,"index":0}]}
data: {"id":"...","choices":[{"delta":{"tool_calls":[{"index":0,"id":"...","function":{"name":"bash","arguments":"{\"c"}}]},"finish_reason":null}]}
data: [DONE]
```

Key structure:
- `choices[0].delta.content` → emit `TokenChunk::Text`
- `choices[0].delta.tool_calls` → accumulate tool call arguments by index
- `[DONE]` sentinel or `finish_reason` → stop
- Usage comes in a final chunk: `{"usage":{"prompt_tokens":N,"completion_tokens":N}}`

## What to Do

1. **Add `stream: Option<bool>` to `ResponsesApiRequest`** (already `skip_serializing_if = "Option::is_none"`)

2. **Add `complete_streaming_responses_api()`** — HTTP setup mirrors `complete_responses_api()`,
   plus SSE byte-stream loop similar to `anthropic::complete_streaming`. Emit
   `TokenChunk::Text` on `response.output_text.delta`. On `response.done`, parse
   the embedded full response and call `normalize_responses_api_response()`.

3. **Add `complete_streaming_chat_api()`** — similar loop. Emit `TokenChunk::Text` on
   `choices[0].delta.content`. Accumulate tool-call argument deltas by index. On
   `[DONE]`, assemble a synthetic `OpenAIResponse` and call `normalize_response()`.
   Usage arrives in a separate final chunk before `[DONE]`.

4. **Add `pub async fn complete_streaming()` to `openai.rs`** dispatching to the
   appropriate sub-function via `uses_responses_api()`.

5. **Wire in `service.rs`**: replace the fallback stub with
   `openai::complete_streaming(...)`.

## Acceptance Criteria

- OpenAI/GPT models stream tokens to the UI the same as Anthropic
- Fireworks models stream tokens via chat completions path
- Tool calls still work correctly after streaming (arguments accumulated correctly)
- `./dev.py check` passes

## Files

- `src/llm/openai.rs` — streaming implementations
- `src/llm/service.rs` — wire `OpenAIChat` branch
