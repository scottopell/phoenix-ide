---
created: 2026-02-15
priority: p2
status: in-progress
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

### New States

```rust
/// Awaiting continuation summary from LLM (tool-less request in flight)
AwaitingContinuation {
    /// The LLM response that triggered continuation (for context)
    trigger_response: AgentMessageContent,
    /// Tool calls that were requested but not executed
    rejected_tool_calls: Vec<ToolCall>,
    /// Usage data from the triggering response
    trigger_usage: UsageData,
}

/// Context window exhausted - conversation is read-only
ContextExhausted {
    /// The continuation summary from the LLM
    summary: String,
    /// Final context usage when exhausted
    final_usage: UsageData,
}
```

### New Events

```rust
/// Continuation summary received from LLM
ContinuationResponse {
    summary: String,
    usage: UsageData,
}

/// Continuation request failed after retries
ContinuationFailed {
    error: String,
}
```

### Transitions

| Current State | Event | Condition | Next State |
|--------------|-------|-----------|------------|
| LlmRequesting | LlmResponse | usage >= 90% AND mode = ThresholdBasedContinuation | AwaitingContinuation |
| LlmRequesting | LlmResponse | usage >= 90% AND mode = IntentionallyUnhandled | Failed (ContextExhausted) |
| AwaitingContinuation | ContinuationResponse | - | ContextExhausted |
| AwaitingContinuation | ContinuationFailed | - | ContextExhausted (fallback summary) |
| AwaitingContinuation | UserCancel | - | ContextExhausted ("Cancelled") |
| ContextExhausted | UserMessage | - | **REJECT** "context exhausted" |
| ContextExhausted | * | - | ContextExhausted (no-op) |

### Effects

```rust
/// Request continuation summary from LLM (no tools)
Effect::RequestContinuation {
    /// Tool calls that were requested but not executed (for summary context)
    rejected_tool_calls: Vec<ToolCall>,
}

/// Notify client of context exhaustion
Effect::NotifyContextExhausted { summary: String }
```

### ConvContext Addition

```rust
struct ConvContext {
    // ... existing fields ...
    
    /// How this conversation handles context exhaustion
    context_exhaustion_behavior: ContextExhaustionBehavior,
}

enum ContextExhaustionBehavior {
    ThresholdBasedContinuation,
    IntentionallyUnhandled,
}
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

### Core Flow
- [ ] Threshold check after every LLM response (in transition function)
- [ ] Check happens BEFORE entering ToolExecuting (tools are rejected, not cancelled)
- [ ] Continuation prompt sent when threshold exceeded
- [ ] Summary stored as continuation message type
- [ ] State machine transitions to ContextExhausted
- [ ] User messages rejected with clear error in ContextExhausted state

### Failure Handling
- [ ] If continuation LLM request fails, use fallback summary
- [ ] Fallback still transitions to ContextExhausted (user not blocked)

### Context Exhaustion Behavior Modes
- [ ] Normal conversations use `ThresholdBasedContinuation`
- [ ] Sub-agents use `IntentionallyUnhandled`
- [ ] Sub-agents fail with `ErrorKind::ContextExhausted` instead of continuation flow
- [ ] Parent receives sub-agent failure and can spawn replacement

### UI
- [ ] UI shows exhausted state with summary prominently
- [ ] "Start New Conversation" button works
- [ ] Optional: pre-populate new conversation with continuation summary
- [ ] Warning indicator at 80% with manual trigger option

### Model Support
- [ ] Model-specific thresholds (128k vs 200k models)
- [ ] Unknown models default to smallest known limit (conservative)

## Design Decisions

### Context Check Timing: After LlmResponse Only

Context threshold is checked **after receiving `LlmResponse`**, before any tools execute. If threshold exceeded:

1. **Do NOT enter `ToolExecuting`** — skip the normal tool path entirely
2. Persist the `LlmResponse` content as an agent message (preserves what LLM said)
3. Emit `Effect::RequestContinuation` (tool-less LLM request)
4. Transition to a new `AwaitingContinuation` state

This means tools requested in that response are **rejected, not cancelled**. The LLM's tool_calls are acknowledged in the continuation summary ("I was about to run bash and patch, but context is full...").

**Why this is simpler:**
- No mid-execution interruption
- No synthetic cancelled tool results
- Clear decision point: we check once, at the response boundary
- The continuation prompt can mention what was requested but not executed

**Implementation location:** In `transition()`, the `(LlmRequesting, LlmResponse)` arm checks `usage` against threshold BEFORE the `if tool_calls.is_empty()` branch.

### Context Exhaustion Behavior Enum

Conversations have a `ContextExhaustionBehavior` that defines how they handle approaching context limits:

```rust
enum ContextExhaustionBehavior {
    /// Normal conversations: trigger continuation at 90% threshold
    ThresholdBasedContinuation,
    /// Sub-agents: fail immediately on context exhaustion (no continuation flow)
    IntentionallyUnhandled,
}
```

**Normal conversations** (user-initiated) use `ThresholdBasedContinuation`:
- At 90%: trigger continuation flow
- Graceful summary, read-only state, "Start New" button

**Sub-agents** use `IntentionallyUnhandled`:
- No continuation flow — sub-agents shouldn't run long enough to exhaust context
- If a sub-agent somehow hits 90%, it fails with `ErrorKind::ContextExhausted`
- Parent receives failure as `SubAgentResult::Failure`
- Parent can handle by spawning a fresh sub-agent with refined task

This is stored in `ConvContext` and set at conversation creation time.

### Continuation Request Failure Handling

The continuation LLM request is a tool-less summary request. It can fail.

**If continuation request fails (after standard retries):**
1. Transition to `ContextExhausted` anyway
2. Use a **fallback summary**: "Context limit reached. The continuation summary could not be generated. Please start a new conversation."
3. Log the failure for debugging
4. User still sees the "Start New Conversation" button

**Rationale:** The conversation is unrecoverable regardless. A failed summary shouldn't block the user from moving on.

### State Persistence

`ContextExhausted` is a persisted conversation state in the database. On server restart, conversations restore to their persisted state naturally—no special recovery logic needed.

## Migration: Existing Conversations Near Threshold

When this feature ships, existing conversations may already be at 80-95% context usage. Three options:

### Option A: Check on Next LlmResponse Only (Recommended)

**Behavior:** Existing conversations continue normally. Threshold check happens on the NEXT `LlmResponse` after upgrade.

**Pros:**
- Zero migration complexity
- Natural flow — users finish what they're doing, hit threshold organically
- No surprise state changes on startup

**Cons:**
- A conversation at 89% might get one more exchange before continuation triggers
- Extremely full conversations (>95%) might fail on next request

**Risk:** Low. The 90% threshold leaves buffer for one more exchange.

---

### Option B: Check All Active Conversations on Startup

**Behavior:** On server start, scan all conversations. Any at ≥90% immediately transition to `ContextExhausted` with a generic summary.

**Pros:**
- Proactive — no risk of over-limit failures
- Clean state across the board

**Cons:**
- Disruptive — users return to find conversations marked exhausted
- Generic summary (can't call LLM during migration)
- Migration logic that only runs once

**Risk:** Medium. User confusion if conversation suddenly read-only.

---

### Option C: Retroactive Continuation on First Access

**Behavior:** When user opens a conversation post-upgrade, check context. If ≥90%, trigger continuation flow before allowing interaction.

**Pros:**
- User sees the continuation happen (not magic state change)
- Gets real LLM-generated summary
- Lazy — only migrates accessed conversations

**Cons:**
- Blocks conversation loading with LLM request
- Complex: need "migration pending" state
- Race conditions if user sends message during migration

**Risk:** High complexity for marginal benefit.

---

**Recommendation:** Option A. The 90% threshold provides sufficient buffer. Conversations at 85-89% get one more natural exchange. Conversations already over 95% are rare and will fail gracefully on next attempt (standard error handling applies).

## Related

- Task 518: Investigation comparing rustey-shelley behavior
- REQ-BED-012: Existing context tracking requirement (to be superseded)
- `src/llm/models.rs`: Model context window definitions
