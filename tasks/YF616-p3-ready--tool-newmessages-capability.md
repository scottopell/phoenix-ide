---
artifact: src/tools.rs ToolOutput newMessages field + Skill tool convergence
created: 2026-04-03
priority: p3
status: ready
---

# Tool system: support newMessages alongside tool results

## Summary

The reference implementation's Skill tool returns `{ data: { success: true }, newMessages: [...] }` -- the tool result is lightweight, and the skill content is injected as user-role messages in the conversation history. Phoenix's tool system only supports `ToolOutput::success(text)` -- no way to inject messages alongside the result.

This blocks full REQ-SK-005 convergence: the LLM Skill tool currently returns the skill body as a tool result string (informational weight), not as a user message (authoritative weight). Both invocation paths use `invoke_skill()` for identical content, but the delivery mechanism differs.

## Done When

- `ToolOutput` (or `ToolResult`) has an optional `new_messages: Vec<MessageContent>` field
- The executor persists `new_messages` as conversation messages when processing a tool result
- `SkillTool` returns `ToolOutput::success("Skill invoked")` with `new_messages: [MessageContent::Skill(...)]`
- Both `/skill` (user path) and Skill tool (LLM path) produce identical conversation history: a `MessageContent::Skill` user-role message
- The state machine handles the new messages correctly (persists them before the next LLM request)

## Context

- Reference: `~/Downloads/src/tools/SkillTool/SkillTool.ts` lines 735-755 (`newMessages` pattern)
- Current TODO: `src/tools/skill.rs` line 3 documents this gap
- Spec: `specs/skills/requirements.md` REQ-SK-005
- Only the Skill tool needs this today -- no other tool injects messages
