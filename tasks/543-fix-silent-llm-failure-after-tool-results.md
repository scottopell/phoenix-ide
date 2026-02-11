# Task 543: Fix Silent LLM Failure After Tool Results

**Status**: Open
**Priority**: High
**Created**: 2026-02-11

## Problem

Conversation `a8789f5d-ddb7-49ad-a1b5-784fdb62820b` on prod (AI Gateway) shows broken message sequence:

```
seq 1: user     (19:39:08) ✓
seq 2: agent    (19:39:13) ✓ - 4 parallel tool calls
seq 3-6: tool   (19:39:13-14) ✓ - All 4 tool results
[40 second gap]
state: idle    (19:39:54) ❌ - Should have agent response, got nothing
```

After receiving all tool results, the agent should have generated another message analyzing them. Instead:

1. **40-second gap** suggests LLM API call was attempted
2. Call either **failed or returned invalid response**
3. State transitioned to `Idle` instead of `Error`
4. **No error message** persisted to user
5. **No error logged** (production logging too sparse)

## Expected Behavior

After tool results complete:
1. Transition to `LlmRequesting`
2. Make API call to get agent's analysis
3. If call fails → `Error` state with user-visible message
4. If call succeeds → `Idle` (no more tools) or `ToolExecuting` (more tools)

## Root Cause Candidates

1. **AI Gateway auth expiry** - Token expired mid-conversation
2. **API timeout** - Call took too long, error not handled properly
3. **Malformed response** - AI Gateway returned invalid format
4. **Silent error** - Exception caught but not propagated to state machine
5. **State machine bug** - Wrong transition on certain error types

## Reproduction Steps

1. Start conversation with AI Gateway enabled
2. Send message that triggers parallel tool calls
3. Wait for all tool results to complete
4. Observe if agent responds or goes silent

## Debug Information Needed

- **Error logs** from LLM API call (not captured in prod)
- **AI Gateway response** (if any)
- **State machine transitions** during 40-second gap
- **Auth token status** at time of failure

## Proposed Fixes

### 1. Better Error Handling (High Priority)
- Ensure all LLM API errors transition to `Error` state, not `Idle`
- Log full error details before transitioning
- Include user-facing error message in state

### 2. Improved Logging (Medium Priority)
- Add structured logging for all LLM API calls
- Log request/response bodies (sanitized)
- Track state transitions with timestamps

### 3. Auth Refresh (AI Gateway Specific)
- Detect auth expiry before making call
- Auto-refresh token if possible
- Clear error message if refresh fails

### 4. Timeout Handling
- Set explicit timeout for LLM API calls
- Transition to `Error` on timeout, not `Idle`
- Log timeout details

## Testing

- [ ] Reproduce with expired AI Gateway token
- [ ] Test with network timeout simulation
- [ ] Verify error states are user-visible
- [ ] Check logs contain actionable error info
- [ ] Ensure state machine never silently goes `Idle` on errors

## Related Files

- `src/runtime/executor.rs` - LLM API call handling
- `src/state_machine/transition.rs` - Error state transitions
- `src/llm/ai_gateway.rs` - AI Gateway client
- `src/llm/mod.rs` - Error types

## Success Criteria

- No more silent failures after tool results
- All API errors visible to user
- Detailed error logs in production
- Conversation never stuck in limbo
