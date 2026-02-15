---
created: 2026-02-15
priority: p2
status: ready
---

# Context Continuation Feature

## Summary

Implement graceful conversation termination when context window approaches capacity. At 90% usage, trigger a continuation flow that summarizes the session and guides the user to start a fresh conversation.

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

WHEN LLM response indicates context usage >= 90% of model's context window
THE SYSTEM SHALL trigger continuation flow
AND prevent further user messages

WHEN calculating context usage
THE SYSTEM SHALL use total tokens from LLM response usage data
AND compare against model-specific context window size

**Rationale:** 90% threshold leaves comfortable room (~20k tokens on 200k models) for the continuation prompt and response while avoiding hard failures.

---

### REQ-BED-020: Continuation Prompt

WHEN continuation flow is triggered
THE SYSTEM SHALL send a tool-less LLM request with continuation prompt
AND the prompt SHALL request a concise summary of the session

WHEN continuation response is received
THE SYSTEM SHALL store it as a continuation message
AND transition to ContextExhausted state

**Rationale:** The summary preserves session context for the user to seed a new conversation.

---

### REQ-BED-021: Context Exhausted State

WHEN conversation enters ContextExhausted state
THE SYSTEM SHALL reject new user messages with explanatory error
AND display the continuation summary prominently
AND offer "Start New Conversation" action

WHEN user starts new conversation from exhausted conversation
THE SYSTEM SHALL optionally pre-populate with continuation summary (user choice)
AND preserve link to parent conversation for reference

**Rationale:** Clear terminal state prevents confusion. Optional summary seeding enables continuity without forcing it.

---

### REQ-BED-022: Model-Specific Context Limits

WHEN determining context threshold
THE SYSTEM SHALL use the context window size for the conversation's model
AND support models with different limits (128k, 200k, etc.)

WHEN model context window is unknown
THE SYSTEM SHALL default to the smallest known model limit

**Rationale:** Different models have different capacities. Defaulting to the smallest known limit ensures safe behavior with unknown models.

---

### REQ-BED-023: Context Warning Indicator

WHEN context usage exceeds 80% of model's context window
THE SYSTEM SHALL display a warning indicator in the context usage display
AND offer user option to trigger continuation manually

WHEN user manually triggers continuation
THE SYSTEM SHALL behave identically to automatic continuation at threshold

**Rationale:** Users may want to wrap up a conversation naturally before hitting the hard limit. Early warning with manual trigger gives control.

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
| LlmRequesting | LlmResponse | usage >= 90% threshold | *trigger continuation* |
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
pub const CONTINUATION_THRESHOLD: f64 = 0.90;
pub const WARNING_THRESHOLD: f64 = 0.80;

/// The continuation prompt (specifics TBD - keep generic for now)
pub const CONTINUATION_PROMPT: &str = r#"
The conversation context is nearly full. Please provide a brief continuation summary that could seed a new conversation.
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

1. **Context indicator**: Show warning state at > 80% with manual continuation trigger, critical at > 90%
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
- [ ] Sub-agents handle their own context exhaustion independently

## Design Decisions

### Tool Chain Interruption

If context threshold is exceeded mid-tool-chain, **cancel remaining tools** and trigger continuation. The existing cancellation machinery produces synthetic "cancelled" tool results that render cleanly to the LLM—no special handling needed beyond invoking the same cancel path.

This prevents tools from pushing context over the hard limit while maintaining message chain integrity.

### Sub-Agent Context

Sub-agents have **independent context budgets**. Each sub-agent tracks its own context usage against its own model's limit. If a sub-agent exhausts context:
- It fails with a context exhaustion error (similar to other fatal errors)
- Parent receives the failure as a sub-agent result
- Parent's context is unaffected

This is simpler than shared budgets and avoids race conditions.

### State Persistence

`ContextExhausted` is a persisted conversation state in the database. On server restart, conversations restore to their persisted state naturally—no special recovery logic needed.

## Related

- Task 518: Investigation comparing rustey-shelley behavior
- REQ-BED-012: Existing context tracking requirement (to be superseded)
- `src/llm/models.rs`: Model context window definitions
