---
created: 2026-04-08
priority: p4
status: ready
artifact: src/runtime/executor.rs
---

# Merge consecutive user-role messages for Bedrock compatibility

## Problem

`build_llm_messages` can produce consecutive user-role messages in the
LLM request array (e.g., tool_result followed by a system message delivered
as user-role). The first-party Anthropic API handles this gracefully by
merging consecutive same-role messages server-side into a single turn.
However, Amazon Bedrock's Anthropic integration rejects consecutive
same-role messages with a validation error.

Phoenix currently only targets the Anthropic API (direct or via gateway),
so this is not a bug today. But if Bedrock support is added in the future,
this will break.

## Fix

After `build_llm_messages` constructs the message array, add a post-processing
pass that merges consecutive same-role messages by concatenating their content
block arrays into a single message. This is a safe transformation -- the
semantic content is identical.

## Done when

- [ ] No consecutive same-role messages in the output of `build_llm_messages`
- [ ] Existing behavior unchanged for Anthropic API (server-side merge is redundant)
- [ ] Bedrock-compatible message arrays produced
