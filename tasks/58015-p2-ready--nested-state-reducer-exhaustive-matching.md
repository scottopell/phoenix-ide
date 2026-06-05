# Migrate state-machine reducers to nested exhaustive matching

## Summary

The workspace now denies Clippy wildcard enum match arms, but the main state-machine reducer migration is intentionally deferred from the lint rollout. Migrate the reducers to a nested-by-state shape so Rust exhaustiveness catches newly added states/events without relying on wildcard invalid-transition sinks.

## Primary targets

- `transition_core`
- `transition_parent`
- `transition_sub_agent`

## Direction and motivation

The current reducers encode many valid and invalid transitions in one broad tuple match. That makes the final invalid-transition arm look correct, but it also means a new state or event can silently fall into that catch-all until someone notices at runtime.

The migration should make every new state and every new event force an explicit reducer decision at compile time. The intended shape is nested by state: first decide which state owns the event, then let that state list the event behavior exhaustively.

For example, the core reducer should move toward this shape:

```rust
fn transition_core(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match state {
        CoreState::Idle => transition_core_idle(context, event),
        CoreState::LlmRequesting { attempt } => {
            transition_core_llm_requesting(*attempt, context, event)
        }
        CoreState::ToolExecuting {
            current_tool,
            remaining_tools,
            completed_results,
            pending_sub_agents,
            assistant_message,
        } => transition_core_tool_executing(
            current_tool,
            remaining_tools,
            completed_results,
            pending_sub_agents,
            assistant_message,
            event,
        ),
        CoreState::Error { message, error_kind } => {
            transition_core_error(message, error_kind, context, event)
        }
        // Include every remaining CoreState variant here, with no fallback arm.
    }
}
```

Each state helper should then make invalid transitions explicit by naming the event variants. For example:

```rust
fn transition_core_idle(
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match event {
        CoreEvent::UserMessage { .. } => start_llm_from_idle(context, event),
        CoreEvent::UserTriggerContinuation => handle_core_continuation(
            &CoreState::Idle,
            CoreEvent::UserTriggerContinuation,
        ),
        CoreEvent::LlmResponse { .. } => Ok(CoreTransitionResult::new(CoreState::Idle)),
        CoreEvent::SteerDrainedUserMessages { entries } => {
            drain_steering_messages_from_idle(entries)
        }
        CoreEvent::UserCancel { .. }
        | CoreEvent::LlmError { .. }
        | CoreEvent::RetryTimeout { .. }
        | CoreEvent::ToolComplete { .. }
        | CoreEvent::ToolAborted { .. }
        | CoreEvent::SpawnAgentsComplete { .. }
        | CoreEvent::SubAgentResult { .. }
        | CoreEvent::ContinuationResponse { .. }
        | CoreEvent::ContinuationFailed { .. } => invalid_core_transition("Idle", event),
    }
}
```

The parent and sub-agent reducers should follow the same principle. Absorbing states such as parent `Terminal`, parent `HandedOff`, and sub-agent `Completed`/`Failed` are not special-cased by wildcard arms; they explicitly list every event they absorb or reject. That verbosity is the point: adding a `CoreEvent`, `ParentOnlyEvent`, or `SubAgentOnlyEvent` should fail compilation until the absorbing-state behavior is chosen.

Invalid transitions remain first-class outcomes. The change is that they are represented by explicit variant lists instead of wildcard sinks.

## Acceptance criteria

- `transition_core` is nested by `CoreState` with no wildcard state arm.
- Each core state helper exhaustively matches `CoreEvent` with no wildcard event arm.
- `transition_parent` is nested by `ParentState` with no wildcard state arm.
- Parent absorbing states explicitly list every event they absorb/reject.
- `transition_sub_agent` is nested by `SubAgentState` with no wildcard state arm.
- Sub-agent completed/failed absorption explicitly lists every event.
- Any temporary `#[allow(clippy::wildcard_enum_match_arm)]` covering these reducer paths is removed.
