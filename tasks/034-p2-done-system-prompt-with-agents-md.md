---
created: 2025-02-02
priority: p2
status: done
---

# Add system prompt that includes AGENTS.md content

## Summary

Add a well-crafted system prompt for the LLM that includes relevant content from AGENTS.md files, similar to how Shelley handles guidance files.

## Context

Currently phoenix-ide sends messages to the LLM without a system prompt that provides project context. A good system prompt should:

1. Establish the agent's role and capabilities
2. Include relevant guidance from AGENTS.md files in the working directory hierarchy
3. Provide information about available tools

This mirrors Shelley's approach where guidance files (AGENT.md, dear_llm.md, etc.) are automatically included in the system prompt.

## Acceptance Criteria

- [x] System prompt includes agent role/identity
- [x] AGENTS.md files are discovered from cwd up to filesystem root
- [x] More deeply nested AGENTS.md files take precedence over parent ones
- [x] System prompt includes information about available tools (via tool definitions in LlmRequest)
- [x] System prompt is constructed at conversation start and cached appropriately

## Implementation Notes

- Created `src/system_prompt.rs` module
- Supports both `AGENTS.md` and `AGENT.md` file names
- Files discovered from cwd up to root, ordered root-first so more specific (nested) files appear last
- Content wrapped in `<project_guidance>` XML tags with path comments
- Tool information provided via LlmRequest.tools (standard Claude API pattern)

## Notes

- Look at Shelley's implementation for reference on guidance file handling
- Consider whether to support multiple guidance file names (AGENTS.md, AGENT.md, dear_llm.md)
- The system prompt should be concise but informative
