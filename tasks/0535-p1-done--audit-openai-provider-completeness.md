---
created: 2026-02-09
priority: p1
status: done
---

# Audit OpenAI Provider for Completeness

## Summary

Review the OpenAI LLM provider implementation (`src/llm/openai.rs`) for missing or incomplete functionality, particularly around the Responses API used by GPT-5.x Codex models.

## Context

We discovered that the Responses API implementation was missing tool call (function_call) parsing entirely - it only handled `message` type outputs. This caused empty responses when the model tried to use tools. There may be other gaps.

### Issues Fixed

- `function_call` outputs were not parsed - fixed in commit cbb0e90
- **Multi-turn tool results not sent** - fixed in commit fcf3eb3
  - Responses API `input` now uses proper conversation array format
  - Added `instructions` field for system prompts
  - Tool results sent as `function_call_output` items
- **Chat Completions parallel tool calls lost** - fixed in commit fcf3eb3
  - `translate_message` now returns `Vec<OpenAIMessage>` to handle multiple items
  - Multiple tool calls preserved in single assistant message
  - Tool results properly split into separate "tool" role messages

## Areas Reviewed

### Responses API (`complete_responses_api`)

- [x] Verify all output types are handled (`message`, `function_call`, `reasoning`, others?)
  - `message` → extracts text content
  - `function_call` → converts to ToolUse
  - `reasoning` → skipped (internal thinking)
  - Unknown types → logged at debug level
- [x] Check if `function_call` can have multiple calls in one response
  - Yes, handled - we iterate over all outputs
- [x] Review error response parsing
  - Properly extracts error.message with HTTP status code mapping
- [ ] Check for streaming support (if applicable)
  - Not implemented, not needed for current use case
- [x] Verify tool result submission format for multi-turn conversations
  - Fixed: Now sends `function_call_output` items properly
- [x] Check if there are other fields in the response we should capture
  - `status` used to determine end_turn
  - Usage tokens captured

### Chat Completions API (`complete_chat`)

- [x] Compare tool call handling between Chat and Responses APIs
  - Both now properly handle tool calls
  - Chat: tool_calls array in message
  - Responses: function_call output items
- [ ] Verify tool_choice parameter support
  - Not implemented (uses default auto)
- [x] Check parallel tool calls handling
  - Fixed: Multiple tool calls preserved in single assistant message
- [x] Review function calling format differences
  - Handled in translate_message/translate_to_responses_request

### Request Building

- [x] Verify `translate_to_responses_request` correctly formats:
  - [x] System prompts → uses `instructions` field
  - [x] Multi-turn conversations with tool results → fixed
  - [ ] Images/vision (if supported) → not implemented, logs warning
- [x] Check max_tokens vs max_output_tokens handling
  - Responses API uses max_output_tokens
  - Chat API uses max_tokens or max_completion_tokens based on model
- [x] Verify tool schema translation
  - Both APIs convert tools to function format correctly

### General

- [x] Add integration tests for Responses API
  - Tested manually with gpt-5.2-codex multi-step coding tasks
- [ ] Consider adding response logging (debug level) to catch issues earlier
- [ ] Review OpenAI API changelog for recent changes

## Reference

- OpenAI Responses API docs: https://platform.openai.com/docs/api-reference/responses
- GPT-5.x Codex models use this endpoint

## Notes

This was a production bug - user hit empty response bubble with gpt-5.2-codex.
