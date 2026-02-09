---
created: 2026-02-09
priority: p1
status: ready
---

# Audit OpenAI Provider for Completeness

## Summary

Review the OpenAI LLM provider implementation (`src/llm/openai.rs`) for missing or incomplete functionality, particularly around the Responses API used by GPT-5.x Codex models.

## Context

We discovered that the Responses API implementation was missing tool call (function_call) parsing entirely - it only handled `message` type outputs. This caused empty responses when the model tried to use tools. There may be other gaps.

### Known Issue Fixed

- `function_call` outputs were not parsed - fixed in commit cbb0e90

## Areas to Review

### Responses API (`complete_responses_api`)

- [ ] Verify all output types are handled (`message`, `function_call`, `reasoning`, others?)
- [ ] Check if `function_call` can have multiple calls in one response
- [ ] Review error response parsing
- [ ] Check for streaming support (if applicable)
- [ ] Verify tool result submission format for multi-turn conversations
- [ ] Check if there are other fields in the response we should capture

### Chat Completions API (`complete_chat`)

- [ ] Compare tool call handling between Chat and Responses APIs
- [ ] Verify tool_choice parameter support
- [ ] Check parallel tool calls handling
- [ ] Review function calling format differences

### Request Building

- [ ] Verify `translate_to_responses_request` correctly formats:
  - System prompts
  - Multi-turn conversations with tool results
  - Images/vision (if supported)
- [ ] Check max_tokens vs max_output_tokens handling
- [ ] Verify tool schema translation

### General

- [ ] Add integration tests for Responses API
- [ ] Consider adding response logging (debug level) to catch issues earlier
- [ ] Review OpenAI API changelog for recent changes

## Reference

- OpenAI Responses API docs: https://platform.openai.com/docs/api-reference/responses
- GPT-5.x Codex models use this endpoint

## Notes

This was a production bug - user hit empty response bubble with gpt-5.2-codex.
