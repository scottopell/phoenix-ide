---
created: 2025-02-07
priority: p3
status: ready
---

# Investigate: Context Continuation at 95% Threshold

## ⚠️ INVESTIGATION ONLY

**This task is for investigation and documentation only. Do NOT implement any code fixes.**

Deliver findings as a report appended to this file. If features are missing or differ from rustey-shelley, document them and recommend approaches, but do not write implementation code.

## Summary

Compare context window overflow handling between rustey-shelley and phoenix-ide. Goal: ensure graceful degradation when approaching context limits.

## The Feature in rustey-shelley

Commit `f4485b5` and issue `rustey-shelley-7pp` implemented automatic context continuation:

### Constants
```rust
pub(crate) const MAX_CONTEXT_TOKENS: u64 = 200_000;
pub(crate) const CONTINUATION_THRESHOLD: u64 = (MAX_CONTEXT_TOKENS as f64 * 0.95) as u64; // ~190k

const CONTINUATION_PROMPT: &str = r#"The conversation context is nearly full. Please reflect on the session and provide a brief summary noting:

1. Current task (if any) - use project-specific identifiers (e.g., bd issue IDs)
2. The 3 most relevant files
3. What's worth following up on?

Keep your response concise - this will be used to seed a new conversation."#;
```

### Behavior
1. After each LLM response, check `input_tokens + output_tokens`
2. If >= 95% of max context, trigger continuation
3. Send tool-less request with continuation prompt
4. Store response as `MessageType::Continuation`
5. UI shows banner: "Context limit reached" with "Start New Conversation" button
6. Input is disabled - conversation is effectively read-only

### Key Functions
```rust
fn get_context_size(response: &LlmResponse) -> u64 {
    response.usage.as_ref().map_or(0, |u| {
        u64::from(u.input_tokens).saturating_add(u64::from(u.output_tokens))
    })
}

fn should_trigger_continuation(response: &LlmResponse) -> bool {
    get_context_size(response) >= CONTINUATION_THRESHOLD
}
```

## Investigation Tasks

### 1. Find phoenix-ide's context handling

- [ ] Search for: `context`, `token`, `continuation`, `threshold`, `95`, `190000`
- [ ] Check `src/runtime/` for context tracking
- [ ] Check `src/state_machine/` for continuation states
- [ ] Look at `SseEvent` for context_window_size field

### 2. Analyze current behavior

- [ ] Does phoenix-ide track context usage?
- [ ] What happens when context is nearly full?
- [ ] Is there a continuation mechanism?
- [ ] Does UI show context usage indicator?

### 3. Test the scenario

- [ ] Find or create a long conversation
- [ ] Check API response for context_window_size
- [ ] Verify UI displays context usage
- [ ] Push to ~95% and see what happens

### 4. Compare UX details

- [ ] Is there a continuation prompt? What does it ask?
- [ ] How is the summary stored?
- [ ] Can user continue after continuation, or is it final?
- [ ] Is the threshold configurable per model?

## Pit of Success Analysis

1. **Model-aware thresholds:** Different models have different context windows
2. **Graceful degradation:** Don't error, guide user to new conversation
3. **Summary preservation:** The continuation summary should seed next conversation
4. **State machine state:** `Continuation` as explicit final state, not error

## Reference Files

**rustey-shelley:**
- `src/agent/loop.rs` - constants, `should_trigger_continuation()`, `send_continuation_request()`
- `src/db/models.rs` - `MessageType::Continuation`
- UI components for continuation banner

**phoenix-ide:**
- `src/runtime/` - look for context tracking
- `src/state_machine/state.rs` - any continuation state?
- `src/llm/` - usage tracking
- `ui/src/` - context indicator components

## Success Criteria

- Document phoenix-ide's context overflow handling
- Verify threshold is appropriate (model-specific?)
- Verify UX guides user gracefully
- If missing or different, **document gaps and propose recommendations** (do not implement)

---

## Investigation Findings

*(Append findings below this line)*
