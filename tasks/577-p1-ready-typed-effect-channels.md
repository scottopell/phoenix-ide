---
created: 2026-02-28
number: 577
priority: p1
status: ready
slug: typed-effect-channels
title: "Typed effects with oneshot channels — LlmOutcome, ToolOutcome, SubAgentOutcome, PersistOutcome"
---

# Typed Effect Channels

## Context

Read first:
- `specs/bedrock/design.md` — "Typed Effects with Oneshot Channels" section
- `specs/bedrock/design.md` — "Typed Outcome Enums" section
- `specs/bedrock/design.md` — Appendix A (FM-1 especially)

This is the core architectural change. Effects carry `oneshot::Sender<T>` for their
expected outcome type. The executor runs the background work and sends the result back
on the channel. The channel's type constrains what can come back — you physically cannot
send an `LlmOutcome` down a `Sender<ToolOutcome>`.

This should be done AFTER tasks 574-576 (error enums, persistence model, terminal
lifecycle) because those establish the types this task uses.

## What to Do

### Phase 1: Define the outcome types

Create typed outcome enums. These should be in the state machine module since the SM
consumes them:

```rust
enum LlmOutcome {
    Response(AssistantMessage, TokenUsage),
    RateLimited { retry_after: Option<Duration> },
    ServerError { status: u16, body: String },
    NetworkError { message: String },
    TokenBudgetExceeded { partial: Option<AssistantMessage> },
    Cancelled,
}

enum ToolOutcome {
    Completed(ToolResult),
    Aborted { tool_use_id: ToolUseId, reason: AbortReason },
    Failed { tool_use_id: ToolUseId, error: String },
}

enum AbortReason {
    CancellationRequested,
    Timeout,
    ParentCancelled,
}

enum SubAgentOutcome {
    Success { result: String },
    Failure { error: String, error_kind: ErrorKind },
    TimedOut,
}

enum PersistOutcome {
    Ok,
    Failed { error: String },
}
```

### Phase 2: Modify Effect enum

Replace flat effects with channel-bearing variants:

```rust
enum Effect {
    RequestLlm {
        request: LlmRequest,
        reply: oneshot::Sender<LlmOutcome>,
    },
    ExecuteTool {
        invocation: ToolInvocation,
        reply: oneshot::Sender<ToolOutcome>,
    },
    SpawnSubAgent {
        config: SubAgentConfig,
        reply: oneshot::Sender<SubAgentOutcome>,
    },
    PersistCheckpoint {
        data: CheckpointData,
        reply: oneshot::Sender<PersistOutcome>,
    },
    // Fire-and-forget (no reply):
    BroadcastState { snapshot: StateSnapshot },
    ScheduleRetry { delay: Duration, attempt: u32 },
    CancelSubAgents { ids: Vec<String> },
}
```

### Phase 3: Create `handle_outcome` entry point

Add the second pure transition function alongside the existing one:

```rust
fn handle_outcome(
    state: &ConvState,
    context: &ConvContext,
    outcome: EffectOutcome,
) -> Result<TransitionResult, InvalidOutcome>

enum EffectOutcome {
    Llm(LlmOutcome),
    Tool(ToolOutcome),
    SubAgent(SubAgentOutcome),
    Persist(PersistOutcome),
    RetryTimeout,
}
```

`handle_outcome` returns `Err(InvalidOutcome)` for outcomes that don't make sense in
the current state. The executor logs and discards them — state unchanged.

### Phase 4: Modify executor loop

The executor creates oneshot channels when dispatching effects, then selects across
the receivers:

```rust
loop {
    let input = select_next_input(&mut channels).await;
    match handle_outcome(&state, &context, input) {
        Ok(result) => {
            state = result.new_state;
            for effect in result.effects {
                dispatch_effect(effect, &mut channels);
            }
        }
        Err(invalid) => {
            tracing::warn!(?invalid, "rejected invalid outcome");
            continue;
        }
    }
    // ... terminal check from task 576
}
```

### Phase 5: Migrate each effect handler

For each effect type, update the executor to:
1. Create a `oneshot::channel()`
2. Include the sender in the effect
3. Spawn the background task with the sender
4. The task sends its result on completion
5. The receiver is polled in the select loop

Migrate one effect type at a time. Start with `RequestLlm` (most complex), then
`ExecuteTool`, then `PersistCheckpoint`, then `SpawnSubAgent`.

## Acceptance Criteria

- `Effect` enum carries typed oneshot senders
- `handle_outcome()` exists as a pure function, returns `Result`
- Executor uses `select!` over oneshot receivers
- Invalid outcomes are logged and discarded, not panicked on
- All existing behavior preserved (this is a plumbing refactor)
- `./dev.py check` passes
- Property tests pass, ideally with new tests for `handle_outcome`

## Dependencies

- Task 574 (exhaustive error enums — defines the error variants LlmOutcome uses)
- Task 575 (CheckpointData — defines what PersistCheckpoint carries)
- Task 576 (StepResult::Terminal — the executor loop this modifies)

## Files Likely Involved

- `src/state_machine/effect.rs` — Effect enum, outcome types
- `src/state_machine/transition.rs` — handle_outcome function
- `src/state_machine/state.rs` — EffectOutcome, InvalidOutcome
- `src/runtime/executor.rs` — effect dispatch, select loop
- `src/runtime/` — background task spawning
- `src/state_machine/proptests.rs` — new property tests for handle_outcome
