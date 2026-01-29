---
created: 2026-01-29
priority: p2
status: ready
---

# Refactor Executor to Use Trait Abstractions

## Summary

Refactor `ConversationRuntime` in `src/runtime/executor.rs` to use the trait abstractions (`MessageStore`, `StateStore`, `LlmClient`, `ToolExecutor`) instead of concrete types.

## Context

In Phase 3 of the state machine fix, we created trait abstractions and mock implementations for testing. However, the actual executor still uses concrete types:

```rust
pub struct ConversationRuntime {
    db: Database,              // Should be: S: Storage
    llm_registry: Arc<ModelRegistry>,  // Should be: L: LlmClient
    tool_registry: ToolRegistry,       // Should be: T: ToolExecutor
    ...
}
```

This prevents us from using the mocks for integration testing the full executor flow.

## Acceptance Criteria

- [ ] `ConversationRuntime` is generic over `Storage`, `LlmClient`, `ToolExecutor`
- [ ] Production code uses concrete implementations via type aliases or factory functions
- [ ] Integration tests can instantiate runtime with mock implementations
- [ ] All existing tests continue to pass

## Notes

This is the remaining work from Phase 3. The traits and mocks exist in:
- `src/runtime/traits.rs`
- `src/runtime/testing.rs`

Consider creating a `RuntimeBuilder` pattern for ergonomic construction.
