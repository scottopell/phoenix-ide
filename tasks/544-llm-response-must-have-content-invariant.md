# Task 544: LLM Response Must Have Content (Type Safety)

**Status**: Closed
**Priority**: CRITICAL - Type Safety Violation
**Created**: 2026-02-11

## Problem

`LlmResponse` can be constructed with **empty content**, violating the fundamental invariant that every LLM response must contain either:
- Text content, OR
- Tool calls, OR
- Both

**Current (BROKEN) type:**
```rust
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,  // ❌ Can be empty!
    pub end_turn: bool,
    pub usage: Usage,
}
```

This violates **"Correct by Construction"** design principle - the type system should make invalid states unrepresentable.

## How This Causes Bugs

When AI Gateway returns malformed response:
1. `normalize_response()` builds `content` vec
2. If `message.content` is None/empty AND `tool_calls` is None/empty
3. `content` vec is empty
4. Returns `Ok(LlmResponse { content: vec![], ... })`  ← **Type system allows this!**
5. State machine sees empty content → thinks it's valid completion → goes `Idle`
6. User sees broken conversation with no agent response

## Design Principle Violated

> **Correct by Construction:** Make invalid states unrepresentable in the type system.

An LLM response with NO content is semantically invalid - this should be a compile-time or constructor-time error, not a runtime bug that silently transitions to Idle.

## Proposed Solutions

### Option 1: NonEmptyVec (Compile-Time Safety) ✅ PREFERRED

Use `nonempty` crate or custom type:

```rust
pub struct LlmResponse {
    pub content: NonEmptyVec<ContentBlock>,  // ✅ Cannot be empty
    pub end_turn: bool,
    pub usage: Usage,
}
```

**Pros:**
- Compile-time guarantee
- Impossible to construct invalid response
- Self-documenting invariant

**Cons:**
- Requires `nonempty` dependency or custom type
- API changes (minor)

### Option 2: Validated Constructor (Runtime Safety)

Keep Vec but enforce via constructor:

```rust
impl LlmResponse {
    pub fn new(
        content: Vec<ContentBlock>,
        end_turn: bool,
        usage: Usage,
    ) -> Result<Self, LlmError> {
        if content.is_empty() {
            return Err(LlmError::unknown(
                "LLM response must contain at least one content block"
            ));
        }
        Ok(Self { content, end_turn, usage })
    }
}

// Make fields private, require using constructor
pub struct LlmResponse {
    content: Vec<ContentBlock>,  // Now private
    // ...
}
```

**Pros:**
- No new dependencies
- Clear error message
- Minimal API changes

**Cons:**
- Runtime check (not compile-time)
- Must ensure all code uses constructor

### Option 3: Split Response Types

```rust
pub enum LlmResponse {
    TextOnly { text: String, usage: Usage },
    ToolCalls { tools: NonEmptyVec<ToolUse>, usage: Usage },
    Mixed { text: String, tools: NonEmptyVec<ToolUse>, usage: Usage },
}
```

**Pros:**
- Very explicit, self-documenting
- Compile-time safety

**Cons:**
- Larger refactor
- More complex pattern matching

## Implementation Plan

1. **Add validation to `normalize_response()`** (immediate hotfix)
   ```rust
   // In ai_gateway.rs::normalize_response()
   if content.is_empty() {
       return Err(LlmError::unknown(
           "AI Gateway returned response with no content or tool calls"
       ));
   }

   Ok(LlmResponse { content, end_turn, usage })
   ```

2. **Add same validation to all LLM providers** (OpenAI, etc.)

3. **Choose long-term solution** (Option 1 or 2)

4. **Refactor to use validated construction**

5. **Add proptest invariant**
   ```rust
   #[test]
   fn llm_response_never_empty(resp in arb_llm_response()) {
       assert!(!resp.content.is_empty());
   }
   ```

## Files to Change

- `src/llm/types.rs` - Response type definition
- `src/llm/ai_gateway.rs` - normalize_response validation
- `src/llm/openai.rs` - normalize_response validation
- `src/llm/anthropic.rs` - normalize_response validation (if exists)
- All LLM provider implementations

## Testing

- [ ] Unit test: Empty content vec returns error
- [ ] Unit test: Single text block succeeds
- [ ] Unit test: Single tool call succeeds
- [ ] Integration test: Malformed API response handled correctly
- [ ] Proptest: All generated responses have content

## Success Criteria

- [ ] Impossible to construct `LlmResponse` with empty content
- [ ] All LLM providers validate responses
- [ ] Clear error message when API returns no content
- [ ] Type system documents the invariant
- [ ] No silent failures to Idle state

## Related Issues

- Task 543: Silent LLM failure after tool results
- This is the ROOT CAUSE of that bug

## References

- "Correct by Construction" design principle
- Rust API Guidelines: C-VALIDATE (validation at boundaries)
