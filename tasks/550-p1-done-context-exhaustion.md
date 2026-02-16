---
status: done
priority: p1
created: 2025-02-15
---

# Implement Context Exhaustion Handling (REQ-BED-019 through REQ-BED-024)

## Overview

Implement graceful context window exhaustion handling for conversations. When a conversation approaches its model's context limit, the system should generate a continuation summary, transition to a terminal state, and provide clear UI feedback.

## Requirements to Implement

All requirements are in `specs/bedrock/requirements.md`:

| REQ | Title | Summary |
|-----|-------|--------|
| REQ-BED-019 | Context Continuation Threshold | Check at 90% of model's context window, trigger continuation, reject pending tools |
| REQ-BED-020 | Continuation Summary Generation | Tool-less LLM request for summary, handle response/failure with fallback |
| REQ-BED-021 | Context Exhausted State | Terminal state rejecting new messages, display summary for manual copy |
| REQ-BED-022 | Model-Specific Context Limits | Use per-model context window from registry, conservative default for unknown |
| REQ-BED-023 | Context Warning Indicator | 80% warning in UI (already partial), add manual trigger button |
| REQ-BED-024 | Sub-Agent Context Exhaustion | Fail immediately (no continuation), report to parent |

## Existing Design

`specs/bedrock/design.md` contains detailed design for this feature including:
- New states: `AwaitingContinuation`, `ContextExhausted`
- New events: `ContinuationResponse`, `ContinuationFailed`, `UserTriggerContinuation`
- New effects: `RequestContinuation`, `NotifyContextExhausted`
- `ContextExhaustionBehavior` enum for parent vs sub-agent handling
- Transition logic pseudocode
- Constants: `CONTINUATION_THRESHOLD` (0.90), `WARNING_THRESHOLD` (0.80)
- State transition matrix additions

**Follow the design.md closely** - it represents agreed-upon technical decisions. If you find issues or improvements needed, document them but implement as designed first.

## Existing Infrastructure

### Model Registry (REQ-BED-022 - already exists)
- `src/llm/models.rs` - `ModelDef` with `context_window: usize` per model
- `src/llm/registry.rs` - `available_model_info()` returns `ModelInfo` with `context_window`
- `src/api/types.rs` - `ModelInfo` struct has `context_window: usize`
- Context windows vary: Claude = 200k, GPT-4o = 128k, etc.

### Context Tracking (REQ-BED-012 - complete)
- `UsageData::context_window_used()` in `src/db/schema.rs`
- API returns `context_window_size` in conversation responses
- UI `StateBar.tsx` shows context indicator

### UI Warning (REQ-BED-023 - partial)
- `ui/src/components/StateBar.tsx` shows warning at 80%, critical at 95%
- **Problem:** Hardcoded `MAX_CONTEXT_TOKENS = 200_000` - needs model-specific value
- **Missing:** Manual trigger button

### Sub-Agent Infrastructure (REQ-BED-024)
- `spawn_agents` tool in `src/tools/subagent.rs`
- States exist: `AwaitingSubAgents`, `Completed`, `Failed`
- Sub-agent result handling exists

## Implementation Tasks

### Backend (Rust)

1. **State Machine Updates** (`src/state_machine/`)
   - Add `ContextExhaustionBehavior` enum to `state.rs`
   - Add `AwaitingContinuation` and `ContextExhausted` states to `ConvState`
   - Add events: `ContinuationResponse`, `ContinuationFailed`, `UserTriggerContinuation`
   - Add effects: `RequestContinuation`, `NotifyContextExhausted`
   - Update `transition.rs` with threshold check at `LlmRequesting -> LlmResponse`
   - Add all new transitions per design.md state transition matrix

2. **Executor Updates** (`src/runtime/executor.rs`)
   - Handle `RequestContinuation` effect - make tool-less LLM request with continuation prompt
   - Handle continuation response/failure, emit appropriate events

3. **API Updates**
   - Include model's `context_window` in conversation/SSE responses so UI can calculate percentage correctly
   - Add endpoint or SSE event for manual continuation trigger (UserTriggerContinuation)

4. **Context in ConvContext**
   - Add model's `context_window` to `ConvContext` so transition function can check threshold
   - Add `ContextExhaustionBehavior` to `ConvContext`

### Frontend (React/TypeScript)

1. **StateBar.tsx Updates**
   - Use model-specific `context_window` from API instead of hardcoded 200k
   - Add manual trigger button when context >= 80% (small, inline, follows UI philosophy)

2. **Context Exhausted UI**
   - When state is `ContextExhausted`, display continuation summary prominently
   - Summary should be easily copyable (select all, or copy button)
   - Clear messaging: "Context limit reached. Copy the summary below to continue in a new conversation."
   - Follow `ErrorBanner.tsx` pattern for visual consistency

3. **API Types**
   - Update types for new states and events
   - Add `context_window` (model's max) to conversation response type

## Testing

1. **Property Tests** - Add to `src/state_machine/proptests.rs`:
   - Threshold check triggers at correct percentage
   - `ContextExhausted` rejects all user messages
   - Sub-agents fail immediately without continuation flow

2. **Integration Tests**:
   - Simulate conversation reaching 90% threshold
   - Verify continuation summary is generated
   - Verify state transitions correctly

3. **Browser Validation** (use browser tools):
   - Start dev server with `./dev.py up`
   - Create conversation, verify context indicator shows
   - Verify warning appears at 80%
   - Test manual trigger button
   - Verify exhausted state displays summary correctly

## Development Commands

```bash
# Start development servers
./dev.py up

# After Rust changes
./dev.py restart

# Run checks before committing  
./dev.py check

# Stop servers
./dev.py down
```

**Do NOT use `cargo run` directly** - the server requires LLM gateway configuration that `./dev.py` provides.

## UI Design Guidelines

From AGENTS.md - follow these principles:

- **Information density**: Show context percentage inline, don't add redundant labels
- **Progressive disclosure**: Manual trigger only appears when relevant (>= 80%)
- **Feedback patterns**: Use existing color conventions (yellow for warning, red for critical)
- **No modals**: Display exhausted state inline in conversation view
- **Copyable summary**: Make it trivial to copy - either select-all friendly or explicit copy button

## Definition of Done

- [ ] All 6 requirements implemented per specs/bedrock/requirements.md
- [ ] Implementation follows specs/bedrock/design.md
- [ ] Property tests pass for new state transitions
- [ ] `./dev.py check` passes
- [ ] Browser validation confirms:
  - [ ] Context indicator uses model-specific limit
  - [ ] Warning appears at 80%
  - [ ] Manual trigger button works
  - [ ] Exhausted state shows copyable summary
- [ ] Update `specs/bedrock/executive.md` status for REQ-BED-019 through REQ-BED-024
