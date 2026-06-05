# Add exhaustive matching lints and plan nested state reducers

## Goal

Make wildcard enum matching fail by default, then migrate state-machine reducers toward a simple nested-by-state style where Rust exhaustiveness checking catches newly added states and events.

## Immediate lint change

Add explicit workspace Clippy denies in the root `Cargo.toml`:

```toml
[workspace.lints.clippy]
wildcard_enum_match_arm = "deny"
match_wildcard_for_single_variants = "deny"
```

Then run the relevant checks and classify every failure as either:

- an intentional catch-all that needs a narrow local `#[allow(...)]`, or
- a risky fallback that should be rewritten explicitly.

## Preferred reducer direction: nested exhaustive match by state

The current core reducer is broadly shaped like this:

```rust
fn transition_core(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, &event) {
        (CoreState::Idle | CoreState::Error { .. }, CoreEvent::UserMessage { .. }) => {
            start_llm(context, event)
        }

        (CoreState::LlmRequesting { .. }, CoreEvent::LlmResponse { .. }) => {
            handle_core_llm_response(state, context, event)
        }

        (CoreState::ToolExecuting { .. }, CoreEvent::ToolComplete { .. })
        | (CoreState::ToolExecuting { .. }, CoreEvent::SpawnAgentsComplete { .. }) => {
            handle_core_tool_complete(state, event)
        }

        // Problem: new CoreState/CoreEvent variants silently land here.
        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}
```

Refactor toward this shape instead:

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
        CoreState::CancellingTool {
            tool_use_id,
            skipped_tools,
            completed_results,
            assistant_message,
            pending_sub_agents,
        } => transition_core_cancelling_tool(
            tool_use_id,
            skipped_tools,
            completed_results,
            assistant_message,
            pending_sub_agents,
            event,
        ),
        CoreState::AwaitingSubAgents {
            pending,
            completed_results,
            spawn_tool_id,
        } => transition_core_awaiting_sub_agents(
            pending,
            completed_results,
            spawn_tool_id.as_deref(),
            event,
        ),
        CoreState::CancellingSubAgents {
            pending,
            completed_results,
        } => transition_core_cancelling_sub_agents(pending, completed_results, event),
        CoreState::Error { message, error_kind } => {
            transition_core_error(message, error_kind, context, event)
        }
        CoreState::AwaitingContinuation {
            rejected_tool_calls,
            attempt,
        } => transition_core_awaiting_continuation(rejected_tool_calls, *attempt, event),
    }
}
```

Each state-specific helper should also avoid `_` arms:

```rust
fn transition_core_idle(
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match event {
        CoreEvent::UserMessage {
            text,
            llm_text,
            images,
            files,
            message_id,
            user_agent,
            skill_invocation,
        } => Ok(CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
            .with_effect(Effect::persist_user_message(
                text,
                llm_text,
                images,
                files,
                message_id,
                user_agent,
                skill_invocation,
                false,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change())
            .with_effect(Effect::RequestLlm)),

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

A busy state helper would look like:

```rust
fn transition_core_llm_requesting(
    attempt: u32,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    let state = CoreState::LlmRequesting { attempt };

    match event {
        CoreEvent::LlmResponse { .. } => handle_core_llm_response(&state, context, event),

        CoreEvent::LlmError { .. } | CoreEvent::RetryTimeout { .. } => {
            handle_core_error_retry(&state, event)
        }

        CoreEvent::UserCancel { .. } => handle_core_cancellation(&state, event),

        CoreEvent::UserMessage { .. } => Err(TransitionError::AgentBusy),

        CoreEvent::SteerDrainedUserMessages { entries } => {
            drain_steering_messages_mid_turn(attempt, entries)
        }

        CoreEvent::UserTriggerContinuation => absorb_stale_user_trigger_continuation(&state),

        CoreEvent::ToolComplete { .. }
        | CoreEvent::ToolAborted { .. }
        | CoreEvent::SpawnAgentsComplete { .. }
        | CoreEvent::SubAgentResult { .. }
        | CoreEvent::ContinuationResponse { .. }
        | CoreEvent::ContinuationFailed { .. } => invalid_core_transition("LlmRequesting", event),
    }
}
```

The important properties are:

1. `match state` has one arm per `CoreState` variant and no `_` arm.
2. Each state helper has one arm per `CoreEvent` variant, grouped only when the outcome is genuinely identical.
3. Terminal/absorbing behavior remains explicit by state, e.g. `Terminal` and `HandedOff` in the parent reducer, not hidden behind a final fallback.
4. Invalid transitions are still first-class outcomes, but they are represented by explicit variant lists rather than a wildcard sink.

## Parent reducer shape

Apply the same style to `transition_parent`:

```rust
fn transition_parent(
    state: &ParentState,
    context: &ConvContext,
    event: ParentEvent,
) -> Result<ParentTransitionResult, TransitionError> {
    match state {
        ParentState::Core(core_state) => transition_parent_core(core_state, context, event),
        ParentState::AwaitingRecovery { message, error_kind, recovery_kind } => {
            transition_parent_awaiting_recovery(message, error_kind, recovery_kind, event)
        }
        ParentState::AwaitingTaskApproval { task_file, title, priority, plan } => {
            transition_parent_awaiting_task_approval(task_file, title, *priority, plan, event)
        }
        ParentState::AwaitingUserResponse { questions, tool_use_id } => {
            transition_parent_awaiting_user_response(questions, tool_use_id, event)
        }
        ParentState::ContextExhausted { summary } => {
            transition_parent_context_exhausted(summary, event)
        }
        ParentState::HandedOff { successor_conv_id } => {
            transition_parent_handed_off(successor_conv_id, event)
        }
        ParentState::Terminal => transition_parent_terminal(event),
    }
}
```

For absorbing states, keep the absorption explicit:

```rust
fn transition_parent_terminal(
    event: ParentEvent,
) -> Result<ParentTransitionResult, TransitionError> {
    match event {
        ParentEvent::Core(CoreEvent::UserMessage { .. }) => {
            Err(TransitionError::ConversationTerminal)
        }

        ParentEvent::Core(CoreEvent::UserCancel { .. })
        | ParentEvent::Core(CoreEvent::LlmResponse { .. })
        | ParentEvent::Core(CoreEvent::LlmError { .. })
        | ParentEvent::Core(CoreEvent::RetryTimeout { .. })
        | ParentEvent::Core(CoreEvent::ToolComplete { .. })
        | ParentEvent::Core(CoreEvent::ToolAborted { .. })
        | ParentEvent::Core(CoreEvent::SpawnAgentsComplete { .. })
        | ParentEvent::Core(CoreEvent::SubAgentResult { .. })
        | ParentEvent::Core(CoreEvent::ContinuationResponse { .. })
        | ParentEvent::Core(CoreEvent::ContinuationFailed { .. })
        | ParentEvent::Core(CoreEvent::UserTriggerContinuation)
        | ParentEvent::Core(CoreEvent::SteerDrainedUserMessages { .. })
        | ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided { .. })
        | ParentEvent::Parent(ParentOnlyEvent::TaskHandoffComplete { .. })
        | ParentEvent::Parent(ParentOnlyEvent::UserQuestionResponse { .. })
        | ParentEvent::Parent(ParentOnlyEvent::UserQuestionDismissed)
        | ParentEvent::Parent(ParentOnlyEvent::CredentialBecameAvailable)
        | ParentEvent::Parent(ParentOnlyEvent::CredentialHelperFailed { .. })
        | ParentEvent::Parent(ParentOnlyEvent::TaskResolved { .. }) => {
            Ok(ParentTransitionResult::new(ParentState::Terminal))
        }
    }
}
```

This is intentionally verbose: if a new `ParentOnlyEvent` or `CoreEvent` is added, this helper fails to compile until the terminal-state behavior is decided.

## Sub-agent reducer shape

Apply the same style to `transition_sub_agent`:

```rust
fn transition_sub_agent(
    state: &SubAgentState,
    context: &ConvContext,
    event: SubAgentEvent,
) -> Result<SubAgentTransitionResult, TransitionError> {
    match state {
        SubAgentState::Core(core_state) => {
            transition_sub_agent_core(core_state, context, event)
        }
        SubAgentState::Completed { result } => {
            transition_sub_agent_completed(result, event)
        }
        SubAgentState::Failed { error, error_kind } => {
            transition_sub_agent_failed(error, error_kind, event)
        }
    }
}
```

Completed/failed absorption should list every event explicitly rather than using `_event`:

```rust
fn transition_sub_agent_completed(
    result: &str,
    event: SubAgentEvent,
) -> Result<SubAgentTransitionResult, TransitionError> {
    match event {
        SubAgentEvent::Core(CoreEvent::UserMessage { .. })
        | SubAgentEvent::Core(CoreEvent::UserCancel { .. })
        | SubAgentEvent::Core(CoreEvent::LlmResponse { .. })
        | SubAgentEvent::Core(CoreEvent::LlmError { .. })
        | SubAgentEvent::Core(CoreEvent::RetryTimeout { .. })
        | SubAgentEvent::Core(CoreEvent::ToolComplete { .. })
        | SubAgentEvent::Core(CoreEvent::ToolAborted { .. })
        | SubAgentEvent::Core(CoreEvent::SpawnAgentsComplete { .. })
        | SubAgentEvent::Core(CoreEvent::SubAgentResult { .. })
        | SubAgentEvent::Core(CoreEvent::ContinuationResponse { .. })
        | SubAgentEvent::Core(CoreEvent::ContinuationFailed { .. })
        | SubAgentEvent::Core(CoreEvent::UserTriggerContinuation)
        | SubAgentEvent::Core(CoreEvent::SteerDrainedUserMessages { .. })
        | SubAgentEvent::SubAgent(SubAgentOnlyEvent::GraceTurnExhausted { .. }) => {
            Ok(SubAgentTransitionResult::new(SubAgentState::Completed {
                result: result.to_string(),
            }))
        }
    }
}
```

## Implementation scope

1. Add the two Clippy denies.
2. Run checks and collect all lint failures.
3. Add local `allow` only where needed to avoid blocking the lint rollout.
4. Prepare the reducer refactor as a follow-up implementation plan using the nested-by-state shape above.
5. Do not attempt the full reducer migration in the lint rollout unless the lint fallout is already small and localized.

## Acceptance criteria

- Workspace lint config explicitly denies wildcard enum match arms and wildcard-for-single-variant matches.
- The lint rollout has a clear list of intentional local exceptions, if any.
- The state-machine reducer migration plan focuses on nested exhaustive matching by state and omits competing approaches.
- The plan identifies `transition_core`, `transition_parent`, and `transition_sub_agent` as the primary migration targets.
