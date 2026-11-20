# ADR-272: Preserve Gemini tool-result images

- **Status:** Accepted
- **Date:** 2026-11-20
- **Affects:** REQ-LLM-004, REQ-BT-003, REQ-BT-006

## Context

Gemini gateway models were registered on Chat Completions, whose generic
tool-result projection dropped images. Screenshot and read-image tools therefore
returned pixels to the runtime but only text reached Gemini. A paired gateway
probe accepted text plus image_url parts in a tool-role message and identified a
synthetic image's color; the text-only projection returned UNKNOWN. The native
generateContent route also accepted the image, but a new backend was unnecessary
to restore this capability.

## Options considered

1. Enable tool-result images on every Chat-compatible route without evidence.
2. Move Gemini to a new native backend to restore one verified capability.
3. Qualify the built-in Gemini Chat route and preserve images in place.

## Decision

Choose option 3. A typed Chat route selects public OpenAI controls, qualified
built-in Gemini behavior, or generic compatible behavior. Built-in Gemini models
on configured non-public Chat endpoints send tool text followed by ordered
image_url data parts under the original tool_call_id. Empty-image results remain
strings. Registered identity selects support; request aliases and display family
do not grant it. Operator-supplied endpoints assert the selected built-in model's
compatible route; this does not qualify arbitrary external models.

## Consequences

Gemini can inspect images returned by ordinary tools without rewriting tool IDs,
arguments, private signatures, or persisted content. Streaming and non-streaming
requests share the projection. Unqualified Chat routes retain logged omission.
Adding another model or external-route capability requires explicit qualification.

## References

- [LLM requirements](../llm/requirements.md), REQ-LLM-004.
- [Chat Completions contract](../llm/chat-completions.allium).
- [Browser tool requirements](../browser-tool/requirements.md), REQ-BT-003/006.
- [ADR-271](271_preserve-gemini-tool-signatures-through-chat-completions.md).
- `ChatRoute::for_model`, `chat_tool_result_content`, and
  `gemini_gateway_tool_result_images`.
