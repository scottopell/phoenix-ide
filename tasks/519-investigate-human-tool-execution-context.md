---
created: 2025-02-07
priority: p3
status: ready
---

# Investigate: Human Tool Execution and LLM Context

## ⚠️ INVESTIGATION ONLY

**This task is for investigation and documentation only. Do NOT implement any code fixes.**

Deliver findings as a report appended to this file. If features are missing or bugs are found, document them and recommend approaches, but do not write implementation code.

## Summary

Compare how rustey-shelley and phoenix-ide handle human-initiated tool executions (the `!command` feature). Key question: Does the LLM see what the human did?

## The Feature in rustey-shelley

rustey-shelley has a full "bang mode" feature where users can execute tools directly:

```
!ls -la                           # Run bash command
!@patch {"path": "file.txt", ...}  # Run any tool with JSON
```

This is powered by:

1. **Actor tracking:** Every action records WHO did it (Human, LlmAgent, System)
2. **Execute endpoint:** `POST /api/conversation/:id/execute`
3. **MessageType::Tool:** Distinct from user/agent messages
4. **History conversion:** Human tool executions converted to text for LLM

### The LLM Context Problem (commit bc50eac)

**Problem:** Human-executed commands weren't visible to the LLM.

**Why it matters:** If user runs `!git status` and then asks "what changed?", the LLM has no context.

**Solution:** Convert human tool executions to descriptive text:

```rust
fn format_human_tool_execution(tool_name: &str, input: &Value, output: &str, is_error: bool) -> String {
    // Returns:
    // [The human user executed a bash command that succeeded]
    // Command: git status
    // Output: ...
}
```

This avoids Claude API's `tool_use`/`tool_result` pairing requirement (which must come from assistant) while giving the LLM context.

## Investigation Tasks

### 1. Does phoenix-ide support human tool execution?

- [ ] Search for: `execute`, `!`, `bang`, `human`, `Actor`
- [ ] Check if there's a UI for running commands directly
- [ ] Check API for an execute endpoint

### 2. If supported, how is it stored?

- [ ] What message type is used?
- [ ] Is there actor/origin tracking?
- [ ] How is it displayed in UI?

### 3. Does the LLM see human executions?

- [ ] Trace history loading for LLM requests
- [ ] Are human tool executions included? How?
- [ ] If not included, is that intentional?

### 4. Claude API constraint analysis

Claude API requires:
- `tool_use` blocks must come from `assistant` role
- `tool_result` must follow `tool_use` with matching ID

Human tool executions can't use this format. Options:
- Convert to text (rustey-shelley approach)
- Omit from LLM context (loses information)
- Inject as synthetic assistant message (hacky)

## Pit of Success Analysis

If phoenix-ide adds human tool execution:

1. **Type-safe message origin:** Enum for `MessageOrigin { LlmInitiated, HumanInitiated, System }`
2. **History builder pattern:** Explicitly handle each origin when building LLM context
3. **Test coverage:** "Human runs command, asks about it, LLM responds with context"

## Reference Files

**rustey-shelley:**
- `src/agent/types.rs` - `Actor`, `ActorKind`, `ToolExecution`
- `src/agent/loop.rs` - `format_human_tool_execution()`, `load_history()`
- `src/api/handlers.rs` - `execute()` endpoint
- Commit `bc50eac` - the fix

**phoenix-ide:**
- `src/api/` - look for execute endpoint
- `src/db.rs` or `src/db/` - message types
- `src/runtime/` - history loading
- `ui/src/` - any bang mode UI

## Success Criteria

- Document whether phoenix-ide supports human tool execution
- If supported, verify LLM sees the context
- If not supported, document whether it's planned
- If gaps exist, **propose design recommendations** (do not implement)

---

## Investigation Findings

*(Append findings below this line)*
