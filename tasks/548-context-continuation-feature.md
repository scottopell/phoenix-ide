---
created: 2026-02-15
priority: p2
status: ready
---

# Context Continuation Feature

## Summary

Implement graceful conversation termination when context window approaches capacity. At 95% usage, trigger a continuation flow that summarizes the session and guides the user to start a fresh conversation.

## Context

Long conversations eventually hit context limits. Without graceful handling:
- LLM requests fail with cryptic errors
- User loses work-in-progress context
- No guidance on how to continue

The rustey-shelley implementation (commit `f4485b5`) provides a reference:
1. Check usage after each LLM response
2. At 95% threshold, send a tool-less "summarize" prompt
3. Store response as continuation message
4. Conversation becomes read-only with "Start New" button

## Spec Amendments

This feature extends **specs/bedrock/requirements.md**. Add the following requirements:

---

### REQ-BED-019: Context Continuation Threshold

WHEN LLM response indicates context usage >= 95% of model's context window
THE SYSTEM SHALL trigger continuation flow
AND prevent further user messages

WHEN calculating context usage
THE SYSTEM SHALL use `input_tokens + output_tokens` from LLM response
AND compare against model-specific context window size

**Rationale:** 95% threshold leaves room for the continuation prompt and response while avoiding hard failures.

---

### REQ-BED-020: Continuation Prompt

WHEN continuation flow is triggered
THE SYSTEM SHALL send a tool-less LLM request with continuation prompt
AND the prompt SHALL request:
  - Current task summary (with project-specific identifiers)
  - Most relevant files (3-5)
  - Recommended follow-up actions
  - Any blocked/pending work

WHEN continuation response is received
THE SYSTEM SHALL store it as `MessageType::Continuation`
AND transition to `ContextExhausted` state

**Rationale:** The summary preserves session context for the user to seed a new conversation.

---

### REQ-BED-021: Context Exhausted State

WHEN conversation enters `ContextExhausted` state
THE SYSTEM SHALL reject new user messages with explanatory error
AND display the continuation summary prominently
AND offer "Start New Conversation" action

WHEN user starts new conversation from exhausted conversation
THE SYSTEM SHALL pre-populate with continuation summary (optional)
AND preserve link to parent conversation for reference

**Rationale:** Clear terminal state prevents confusion. Summary seeding enables continuity.

---

### REQ-BED-022: Model-Specific Context Limits

WHEN determining context threshold
THE SYSTEM SHALL use the context window size for the conversation's model
AND support models with different limits (128k, 200k, etc.)

WHEN model context window is unknown
THE SYSTEM SHALL default to conservative limit (128k)

**Rationale:** Different models have different capacities. The system already tracks this in `ModelInfo.context_window`.

---

## State Machine Changes

### New State

```rust
/// Context window exhausted - conversation is read-only
ContextExhausted {
    /// The continuation summary from the LLM
    summary: String,
    /// Final context usage when exhausted
    final_usage: UsageData,
}
```

### New Event

```rust
/// Context threshold exceeded, continuation response received
ContextContinuation {
    summary: String,
    usage: UsageData,
}
```

### Transitions

| Current State | Event | Condition | Next State |
|--------------|-------|-----------|------------|
| LlmRequesting | LlmResponse | usage >= 95% threshold | *trigger continuation* |
| LlmRequesting | ContextContinuation | - | ContextExhausted |
| ContextExhausted | UserMessage | - | **REJECT** "context exhausted" |
| ContextExhausted | * | - | ContextExhausted (no-op) |

### Effects

```rust
/// Request continuation summary from LLM (no tools)
Effect::RequestContinuation

/// Notify client of context exhaustion
Effect::NotifyContextExhausted { summary: String }
```

## Implementation Notes

### Constants

```rust
/// Threshold as fraction of context window
pub const CONTINUATION_THRESHOLD: f64 = 0.95;

/// The continuation prompt
pub const CONTINUATION_PROMPT: &str = r#"
The conversation context is nearly full. Please provide a continuation summary:

1. **Current Task**: What were we working on? Include specific identifiers (issue IDs, file paths, function names).

2. **Key Files**: List the 3-5 most relevant files for this task.

3. **Progress**: What's been accomplished? What's remaining?

4. **Next Steps**: What should be done next? Any blockers?

5. **Context to Preserve**: Any important decisions, constraints, or discoveries that shouldn't be lost.

Keep the summary concise but complete - it will seed the next conversation.
"#;
```

### Executor Changes

After receiving `LlmResponse`:

```rust
fn check_continuation_threshold(response: &LlmResponse, model: &ModelInfo) -> bool {
    let used = response.usage.input_tokens + response.usage.output_tokens;
    let threshold = (model.context_window as f64 * CONTINUATION_THRESHOLD) as u64;
    used >= threshold
}
```

If threshold exceeded:
1. Don't send normal `LlmResponse` event
2. Send `RequestContinuation` effect
3. On continuation response, send `ContextContinuation` event

### UI Changes

1. **StateBar**: Show warning color when > 80%, critical when > 95%
2. **ContextExhausted banner**: Display summary, "Start New Conversation" button
3. **Input disabled**: Grey out with explanatory text
4. **New conversation seeding**: Option to copy summary to new conversation

### Database

```sql
-- New message type
ALTER TYPE message_type ADD VALUE 'continuation';

-- Or if using text enum:
-- Just use 'continuation' as message_type value
```

## Acceptance Criteria

- [ ] Threshold check after every LLM response
- [ ] Continuation prompt sent when threshold exceeded
- [ ] Summary stored as continuation message type
- [ ] State machine transitions to ContextExhausted
- [ ] User messages rejected with clear error
- [ ] UI shows exhausted state with summary
- [ ] "Start New Conversation" button works
- [ ] Model-specific thresholds (128k vs 200k models)
- [ ] Works correctly for sub-agents (they have parent's context budget?)

## Open Questions

1. **Sub-agent context**: Do sub-agents share parent's context budget, or have their own? If shared, parent might exhaust while sub-agent is running.

2. **Threshold configurability**: Should users be able to adjust the 95% threshold? Probably not MVP.

3. **Continuation during tool execution**: What if we hit threshold mid-tool-chain? Options:
   - Complete current tool chain, then continue
   - Abort remaining tools, continue immediately
   - Recommendation: Complete the chain, then continue

4. **Pre-emptive warning**: Should we warn at 80%? 90%? Just indicator color change, or explicit message?

## Related

- Task 518: Investigation comparing rustey-shelley behavior
- REQ-BED-012: Existing context tracking requirement (to be superseded)
- `src/llm/models.rs`: Model context window definitions
