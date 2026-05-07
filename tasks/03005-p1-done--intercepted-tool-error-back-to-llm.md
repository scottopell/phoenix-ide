---
created: 2026-05-07
priority: p1
status: done
artifact: pending
---

# intercepted-tool-error-back-to-llm

## Plan

# Feed intercepted-tool constraint violations back to the LLM, not the user

## Summary

When the LLM calls `propose_task` or `ask_user_question` alongside other tools in the same response, the system currently transitions to `CoreState::Error` — a terminal, user-visible state. The error should instead be returned to the LLM as `ToolResult::error(...)` entries so the agent can self-correct and retry, identical to how regular tool errors work.

The same bug exists for sub-agents calling `submit_result`/`submit_error` alongside other tools (transitions to `SubAgentState::Failed` instead of feeding back an error).

## Context

- Bug lives in `src/state_machine/transition.rs` at the `InterceptedToolNotSole` handling (~lines 1429–1441 for `propose_task`, ~lines 1481–1493 for `ask_user_question`, ~lines 1821–1834 for terminal sub-agent tools)
- The Allium spec `specs/bedrock/bedrock.allium` rule `InterceptedToolNotSole` (line 314) also codifies the wrong behavior (`ensures: conversation.core_status = error`)
- Both need to be fixed together

## What to do

### 1. `src/state_machine/transition.rs` — three sites

**`propose_task` (and identically `ask_user_question`)** — replace the `CoreState::Error` transition with:
```rust
if tool_calls.len() > 1 {
    let msg = "propose_task must be the only tool in response".to_string();
    let display_data = compute_bash_display_data(&content, &context.working_dir);
    let assistant_message = AssistantMessage::new(content, Some(usage_data), display_data);
    let error_results: Vec<ToolResult> = tool_calls
        .iter()
        .map(|t| ToolResult::error(t.id.clone(), msg.clone()))
        .collect();
    let checkpoint = CheckpointData::tool_round(assistant_message, error_results)
        .expect("error results have same count as tool_calls");
    return Ok(
        ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(notify_llm_requesting(1))
            .with_effect(Effect::RequestLlm),
    );
}
```

Apply the same pattern for `ask_user_question`.

**`submit_result`/`submit_error` (sub-agent)** — replace `SubAgentState::Failed` with error tool results fed back through the sub-agent's LLM loop (transition back to sub-agent's `LlmRequesting`), using the same pattern adapted for sub-agent state/effects.

### 2. `specs/bedrock/bedrock.allium` — update `InterceptedToolNotSole`

Replace the current rule:
```
rule InterceptedToolNotSole {
    ...
    ensures: conversation.core_status = error
    ensures: conversation.error_message = intercepted_tool_name(tool_calls) + " must be the only tool in response"
}
```

With the corrected behavior:
```
rule InterceptedToolNotSole {
    when: LlmResponds(conversation, content, tool_calls, end_turn, usage)
    requires: conversation.core_status = llm_requesting
    requires: tool_calls.count > 1
    requires: tool_calls.any(t => is_intercepted_tool(t.name))
    let msg = intercepted_tool_name(tool_calls) + " must be the only tool in response"
    ensures: conversation.core_status = llm_requesting
    ensures: conversation.retry_attempt = 1
    ensures: ToolRoundCheckpointed(conversation)  -- with error results for all tool_calls
    ensures: LlmRequestDispatched(conversation)
}
```

### 3. Update/add tests

- Update any existing tests that assert `CoreState::Error` for this case
- Add a test: `propose_task` + another tool in the same response → transitions to `LlmRequesting`, checkpoint contains error `ToolResult`s for all tool calls, LLM is re-requested

## Acceptance criteria

- [ ] When the LLM sends `propose_task` + any other tool, the conversation stays in `LlmRequesting` (not `Error`), all tool calls get `ToolResult::error(...)`, the LLM gets another turn
- [ ] Same for `ask_user_question` + other tools
- [ ] Allium spec `InterceptedToolNotSole` reflects the new behavior
- [ ] `./dev.py check` passes


## Progress

