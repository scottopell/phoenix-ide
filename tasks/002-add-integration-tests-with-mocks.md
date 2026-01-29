---
created: 2026-01-29
priority: p2
status: blocked
---

# Add Integration Tests Using Mock Infrastructure

## Summary

Write integration tests that exercise the full executor flow using the mock implementations (MockLlmClient, MockToolExecutor, InMemoryStorage).

## Context

We have mock infrastructure but no integration tests that verify:
- Full conversation flow (user message → LLM → tools → LLM → done)
- Error handling and retry logic
- Cancellation behavior
- Multi-tool execution sequences

## Acceptance Criteria

- [ ] Test: Simple text response flow (no tools)
- [ ] Test: Single tool execution flow
- [ ] Test: Multiple tool execution flow
- [ ] Test: LLM error with retry
- [ ] Test: User cancellation mid-execution
- [ ] Test: Tool failure handling

## Notes

**Blocked by:** Task 001 (Refactor executor to use traits)

Once the executor is generic, tests would look like:

```rust
#[tokio::test]
async fn test_conversation_with_tools() {
    let storage = InMemoryStorage::new();
    let llm = MockLlmClient::new("test");
    llm.queue_response(response_with_tool_call());
    llm.queue_response(response_text_only());
    
    let tools = MockToolExecutor::new()
        .with_tool("bash", ToolOutput::success("output"));
    
    let runtime = ConversationRuntime::new(
        context, state, storage, llm, tools, ...);
    
    // Send user message and verify flow
}
```
