//! Conversation runtime executor
//!
//! The executor loop receives inputs from two sources:
//! - User events via `event_rx` (`UserMessage`, `UserCancel`, etc.) → routed to `transition()`
//! - Effect outcomes via `outcome_rx` (`LlmOutcome`, `ToolOutcome`, etc.) → routed to `handle_outcome()`
//!
//! Background tasks receive typed `oneshot::Sender<T>` for their outcome type.
//! A `Sender<ToolOutcome>` physically cannot send an `LlmOutcome`.
//! The executor wraps received outcomes in `EffectOutcome` for `handle_outcome()`.

use super::traits::{LlmClient, Storage, ToolExecutor};
use super::{SseEvent, SubAgentCancelRequest, SubAgentSpawnRequest};

use crate::db::{MessageContent, ToolResult};
use crate::llm::{ContentBlock, LlmMessage, LlmRequest, MessageRole, ModelRegistry, SystemContent};
use crate::state_machine::outcome::{EffectOutcome, LlmOutcome, ToolOutcome};
use crate::state_machine::state::{SubAgentMode, ToolCall, ToolInput};
use crate::state_machine::{
    handle_outcome, transition, ConvContext, ConvState, Effect, Event, StepResult,
};
use crate::system_prompt::{build_system_prompt, ModeContext};
use crate::tools::{BrowserSessionManager, ToolContext};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Safety-net wall-clock timeout for sub-agents (REQ-SA-006).
/// Primary enforcement is max turns (REQ-PROJ-008). This catches stuck tool execution.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_mins(20);

/// Default cap on consecutive LLM requests within a single parent-conversation
/// user turn. Distinct from sub-agent `max_turns`: this resets on every
/// `Event::UserMessage`, so a long conversation is never penalised — only a
/// runaway `tool_use` burst within one turn. Overridable via the
/// `PHOENIX_PARENT_TOOL_CYCLE_CAP` env var; set to `0` to disable.
///
/// Set deliberately high — this is a backup safety-net, not a budget.
/// A well-behaved agent + real user is expected to stay far below it;
/// hitting this cap means something is stuck or looping.
const DEFAULT_PARENT_TOOL_CYCLE_CAP: u32 = 1000;

/// Resolve the parent-conversation tool-use cycle cap from the environment,
/// falling back to [`DEFAULT_PARENT_TOOL_CYCLE_CAP`]. A malformed value logs
/// a warning and uses the default. Called once per runtime at construction.
fn parent_tool_cycle_cap_from_env() -> u32 {
    let Ok(raw) = std::env::var("PHOENIX_PARENT_TOOL_CYCLE_CAP") else {
        return DEFAULT_PARENT_TOOL_CYCLE_CAP;
    };
    raw.parse::<u32>().unwrap_or_else(|_| {
        tracing::warn!(
            raw = %raw,
            default = DEFAULT_PARENT_TOOL_CYCLE_CAP,
            "PHOENIX_PARENT_TOOL_CYCLE_CAP is not a non-negative integer; using default"
        );
        DEFAULT_PARENT_TOOL_CYCLE_CAP
    })
}

/// Generic conversation runtime that can work with any storage, LLM, and tool implementations
pub struct ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    context: ConvContext,
    state: ConvState,
    storage: S,
    llm_client: Arc<L>,
    tool_executor: Arc<T>,
    /// Browser session manager for `ToolContext`
    browser_sessions: Arc<BrowserSessionManager>,
    /// LLM registry for `ToolContext`
    llm_registry: Arc<ModelRegistry>,
    /// Active PTY terminal sessions — passed to `ToolContext` for `read_terminal` tool.
    terminals: crate::terminal::ActiveTerminals,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: broadcast::Sender<SseEvent>,
    /// Token to cancel running tool execution
    tool_cancel_token: Option<CancellationToken>,
    /// Handle to the spawned LLM task — aborted on cancel to drop the HTTP connection
    llm_task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Channel to notify parent of sub-agent completion (sub-agent only)
    parent_event_tx: Option<mpsc::Sender<Event>>,
    /// Channel to request sub-agent spawning (parent only)
    spawn_tx: Option<mpsc::Sender<SubAgentSpawnRequest>>,
    /// Channel to request sub-agent cancellation (parent only)
    cancel_tx: Option<mpsc::Sender<SubAgentCancelRequest>>,
    /// Buffer for `SubAgentResult` events received before entering `AwaitingSubAgents`.
    /// Pre-allocated with capacity = sub-agent count when spawning (FM-6 prevention).
    sub_agent_result_buffer: Vec<Event>,
    /// Deadline for sub-agent completion — set when entering `AwaitingSubAgents` (REQ-SA-006)
    sub_agent_deadline: Option<tokio::time::Instant>,
    /// Count of active Work-mode sub-agents for one-writer constraint (REQ-PROJ-008)
    active_work_subagents: u32,
    /// LLM turn counter for sub-agents (REQ-PROJ-008 max turns enforcement)
    llm_turn_count: u32,
    /// Whether this sub-agent has been given its grace turn (one extra LLM turn to call `submit_result`)
    grace_turn_granted: bool,
    /// LLM request counter for parent conversations. Resets on every
    /// `Event::UserMessage`, so a long conversation with many turns is fine;
    /// only runaway tool-use bursts within a single user turn trip the cap.
    /// Guards against tasks 24684 + 24680 (a provider that keeps asking for
    /// a missing tool can otherwise loop until the DB runs out of space).
    /// Task 24684 was originally numbered 24679 in commit history — see
    /// the task file for the rebase-time renumbering note.
    parent_tool_cycle_count: u32,
    /// Cap on `parent_tool_cycle_count` before the runtime halts and emits
    /// a system message. `0` disables the cap. Read once at construction
    /// time from `PHOENIX_PARENT_TOOL_CYCLE_CAP`, with
    /// [`DEFAULT_PARENT_TOOL_CYCLE_CAP`] as the fallback. Tests that want
    /// to exercise the cap deterministically use [`Self::with_parent_tool_cycle_cap`].
    parent_tool_cycle_cap: u32,
    /// Typed outcome channel — background tasks send `EffectOutcome` here.
    /// Each task gets a typed `oneshot::Sender<T>` that constrains what it can send,
    /// then the forwarder wraps the result in `EffectOutcome` for this channel.
    outcome_tx: mpsc::Sender<EffectOutcome>,
    outcome_rx: mpsc::Receiver<EffectOutcome>,
    /// Credential helper for recovery settlement (REQ-BED-030).
    /// When the state is `AwaitingRecovery`, the select loop awaits `settled.notified()`.
    credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: ConvContext,
        state: ConvState,
        storage: S,
        llm_client: L,
        tool_executor: T,
        browser_sessions: Arc<BrowserSessionManager>,
        llm_registry: Arc<ModelRegistry>,
        terminals: crate::terminal::ActiveTerminals,
        event_rx: mpsc::Receiver<Event>,
        event_tx: mpsc::Sender<Event>,
        broadcast_tx: broadcast::Sender<SseEvent>,
    ) -> Self {
        // Outcome channel for typed effect results.
        // Background tasks send typed outcomes (LlmOutcome, ToolOutcome, etc.)
        // through oneshot channels, then forwarders wrap them in EffectOutcome
        // for this unified channel.
        let (outcome_tx, outcome_rx) = mpsc::channel::<EffectOutcome>(64);

        Self {
            context,
            state,
            storage,
            llm_client: Arc::new(llm_client),
            tool_executor: Arc::new(tool_executor),
            browser_sessions,
            llm_registry,
            terminals,
            event_rx,
            event_tx,
            broadcast_tx,
            tool_cancel_token: None,
            llm_task_handle: None,
            parent_event_tx: None,
            spawn_tx: None,
            cancel_tx: None,
            sub_agent_result_buffer: Vec::new(),
            sub_agent_deadline: None,
            active_work_subagents: 0,
            llm_turn_count: 0,
            grace_turn_granted: false,
            parent_tool_cycle_count: 0,
            parent_tool_cycle_cap: parent_tool_cycle_cap_from_env(),
            outcome_tx,
            outcome_rx,
            credential_helper: None,
        }
    }

    /// Set the credential helper for recovery settlement (REQ-BED-030).
    pub fn with_credential_helper(
        mut self,
        helper: Option<Arc<crate::llm::CredentialHelper>>,
    ) -> Self {
        self.credential_helper = helper;
        self
    }

    /// Override the parent tool-use cycle cap. Test-only: production code
    /// relies on the env-var default set in [`Self::new`].
    #[cfg(test)]
    pub fn with_parent_tool_cycle_cap(mut self, cap: u32) -> Self {
        self.parent_tool_cycle_cap = cap;
        self
    }

    /// Set the parent event channel (for sub-agents)
    pub fn with_parent(mut self, parent_tx: mpsc::Sender<Event>) -> Self {
        self.parent_event_tx = Some(parent_tx);
        self
    }

    /// Set the spawn/cancel channels (for parent conversations)
    pub fn with_spawn_channels(
        mut self,
        spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
        cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    ) -> Self {
        self.spawn_tx = Some(spawn_tx);
        self.cancel_tx = Some(cancel_tx);
        self
    }

    #[allow(clippy::too_many_lines)] // Sequential event loop; splitting hurts readability
    pub async fn run(mut self) {
        tracing::info!(conv_id = %self.context.conversation_id, "Starting conversation runtime");

        // Check if we need to resume an interrupted operation
        // This handles crash recovery for in-flight LLM requests
        if let ConvState::LlmRequesting { .. } = &self.state {
            tracing::info!(conv_id = %self.context.conversation_id, "Resuming interrupted LLM request");
            if let Err(e) = self.execute_effect(Effect::RequestLlm).await {
                tracing::error!(error = %e, "Failed to resume LLM request");
                let _ = self.broadcast_tx.send(SseEvent::Error {
                    error: crate::runtime::user_facing_error::UserFacingError::with_action(
                        "resume the LLM request",
                    ),
                });
            }
        }

        // REQ-BED-030: crash recovery for AwaitingRecovery.
        // If the credential helper is still running, the select loop will pick it up.
        // If it already settled, handle it immediately.
        if matches!(self.state, ConvState::AwaitingRecovery { .. }) {
            if let Some(ref helper) = self.credential_helper {
                let status = helper.credential_status().await;
                if !matches!(
                    status,
                    crate::llm::credential_helper::CredentialStatus::Running
                ) {
                    self.handle_credential_settlement().await;
                }
            } else {
                // No credential helper available after restart — fall through to error.
                if let Err(e) = self
                    .process_event(Event::CredentialHelperFailed {
                        message: "Credential helper not available after restart".to_string(),
                    })
                    .await
                {
                    tracing::error!(error = %e, "Error handling post-restart credential recovery");
                }
            }
        }

        // Process events and outcomes in a loop - no recursion
        // Four input sources:
        //   event_rx    — user events + legacy executor events (continuation, sub-agent results)
        //   outcome_rx  — typed effect outcomes (LLM, tool, persist, retry)
        //   deadline    — sub-agent timeout (REQ-SA-006, FM-6 prevention)
        //   recovery    — credential helper settlement (REQ-BED-030)
        loop {
            // Copy deadline before select to avoid borrow conflict
            let deadline = self.sub_agent_deadline;
            let awaiting_recovery = matches!(self.state, ConvState::AwaitingRecovery { .. });

            tokio::select! {
                Some(event) = self.event_rx.recv() => {
                    if let Err(e) = self.process_event(event).await {
                        // process_event already broadcast a typed
                        // SseEvent::Error at the source if appropriate
                        // (task 24682). No double-broadcast here.
                        tracing::error!(error = %e, "Error handling event");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event();
                        return;
                    }
                }
                Some(outcome) = self.outcome_rx.recv() => {
                    if let Err(e) = self.process_outcome(outcome).await {
                        tracing::warn!(error = %e, "Outcome rejected by state machine");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event();
                        return;
                    }
                }
                // REQ-SA-006: sub-agent deadline expired — cancel all pending agents
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if deadline.is_some() => {
                    self.handle_sub_agent_timeout().await;
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event();
                        return;
                    }
                }
                // REQ-BED-030: credential helper settled while awaiting recovery
                () = async {
                    match &self.credential_helper {
                        Some(helper) => helper.wait_for_settlement().await,
                        None => std::future::pending::<()>().await,
                    }
                }, if awaiting_recovery && self.credential_helper.is_some() => {
                    self.handle_credential_settlement().await;
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event();
                        return;
                    }
                }
                else => break,
            }
        }

        tracing::info!(conv_id = %self.context.conversation_id, "Conversation runtime stopped");
    }

    /// REQ-BED-030: credential helper settled while in `AwaitingRecovery`.
    /// Check the helper's new status and inject the appropriate event.
    async fn handle_credential_settlement(&mut self) {
        let Some(ref helper) = self.credential_helper else {
            return;
        };
        let status = helper.credential_status().await;
        let event = if status == crate::llm::credential_helper::CredentialStatus::Valid {
            tracing::info!("Credential helper succeeded, retrying LLM request");
            Event::CredentialBecameAvailable
        } else {
            tracing::info!(
                ?status,
                "Credential helper settled without valid credential"
            );
            Event::CredentialHelperFailed {
                message: "Authentication failed — click Retry to try again".to_string(),
            }
        };
        if let Err(e) = self.process_event(event).await {
            tracing::error!(error = %e, "Error handling credential settlement event");
        }
    }

    /// Broadcast `ConversationBecameTerminal` to all SSE subscribers.
    ///
    /// Send errors (no active receivers) are intentionally ignored.
    fn emit_terminal_lifecycle_event(&self) {
        let _ = self.broadcast_tx.send(SseEvent::ConversationBecameTerminal);
    }

    /// Process a typed effect outcome from a background task.
    ///
    /// Routes through `handle_outcome()` (pure SM function). Invalid outcomes
    /// are logged and discarded — state unchanged.
    async fn process_outcome(&mut self, outcome: EffectOutcome) -> Result<(), String> {
        let result = match handle_outcome(&self.state, &self.context, outcome) {
            Ok(r) => r,
            Err(invalid) => {
                tracing::warn!(
                    reason = %invalid.reason,
                    state = self.state.variant_name(),
                    "Rejected invalid outcome — state unchanged"
                );
                return Err(invalid.reason);
            }
        };

        // Apply transition result and process any generated events
        let mut events_to_process = self.apply_transition_result(result).await?;

        // Process chained events (e.g., SpawnAgentsComplete from execute_effect)
        while let Some(event) = events_to_process.pop() {
            let chained_result = match transition(&self.state, &self.context, event) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "Chained event from outcome rejected");
                    continue;
                }
            };
            let more_events = self.apply_transition_result(chained_result).await?;
            events_to_process.extend(more_events);
        }

        Ok(())
    }

    async fn process_event(&mut self, event: Event) -> Result<(), String> {
        // A fresh user turn always resets the parent tool-cycle counter
        // (task 24680). Cap logic lives in the `Effect::RequestLlm` handler.
        if matches!(event, Event::UserMessage { .. }) {
            self.parent_tool_cycle_count = 0;
        }

        // Check if this is a SubAgentResult that needs buffering
        if let Event::SubAgentResult { .. } = &event {
            if !self.can_handle_sub_agent_result() {
                tracing::debug!("Buffering SubAgentResult, parent not in AwaitingSubAgents");
                self.sub_agent_result_buffer.push(event);
                return Ok(());
            }
        }

        // We need to process events in a loop to handle chained effects
        let mut events_to_process = vec![event];

        while let Some(current_event) = events_to_process.pop() {
            // Decrement one-writer counter when a Work sub-agent completes (REQ-PROJ-008)
            if let Event::SubAgentResult { ref agent_id, .. } = current_event {
                if let ConvState::AwaitingSubAgents { ref pending, .. }
                | ConvState::CancellingSubAgents { ref pending, .. } = self.state
                {
                    if let Some(agent) = pending.iter().find(|p| p.agent_id == *agent_id) {
                        if agent.mode == SubAgentMode::Work {
                            self.active_work_subagents =
                                self.active_work_subagents.saturating_sub(1);
                        }
                    }
                }
            }

            // Pure state transition
            let result = match transition(&self.state, &self.context, current_event) {
                Ok(r) => r,
                Err(e) => {
                    // Task 24682: surface a humanised, kind-aware error
                    // payload via SSE, never the raw `Debug` formatting.
                    // The full `TransitionError` is logged separately so
                    // operators can still diagnose it.
                    tracing::warn!(
                        error = %e,
                        state = self.state.variant_name(),
                        "Transition rejected"
                    );
                    let _ = self.broadcast_tx.send(SseEvent::Error {
                        error: crate::runtime::user_facing_error::from_transition_error(&e),
                    });
                    return Err(e.to_string());
                }
            };

            let generated_events = self.apply_transition_result(result).await?;
            events_to_process.extend(generated_events);
        }

        Ok(())
    }

    /// Apply a `TransitionResult` from either `transition()` or `handle_outcome()`.
    ///
    /// Updates state, drains sub-agent buffer if entering `AwaitingSubAgents`,
    /// dispatches effects. Returns any synchronously generated events
    /// (e.g., from `SpawnAgentsComplete`).
    async fn apply_transition_result(
        &mut self,
        result: crate::state_machine::transition::TransitionResult,
    ) -> Result<Vec<Event>, String> {
        let mut generated_events = Vec::new();

        // Update state
        let old_state = std::mem::replace(&mut self.state, result.new_state.clone());

        // Log notable state transitions at INFO. "Notable" means transitions that cross
        // a meaningful phase boundary (idle↔active, entering/leaving tool execution,
        // terminal states) are logged at DEBUG to keep steady-state noise low.
        // Variant names come from `ConvState::variant_name` so the set of
        // names is maintained in exactly one place.
        {
            let from = old_state.variant_name();
            let to = self.state.variant_name();
            if from != to {
                let notable = matches!(
                    &self.state,
                    ConvState::Idle
                        | ConvState::ToolExecuting { .. }
                        | ConvState::AwaitingSubAgents { .. }
                        | ConvState::Completed { .. }
                        | ConvState::Failed { .. }
                        | ConvState::Error { .. }
                        | ConvState::ContextExhausted { .. }
                        | ConvState::AwaitingTaskApproval { .. }
                        | ConvState::AwaitingUserResponse { .. }
                        | ConvState::Terminal
                );
                if notable {
                    tracing::info!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                } else {
                    tracing::debug!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                }
            }
        }

        let entering_awaiting = !matches!(
            old_state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        ) && matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        );
        let leaving_awaiting = matches!(
            old_state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        ) && !matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        );

        // Drain buffer when entering AwaitingSubAgents
        if entering_awaiting {
            let buffered = std::mem::take(&mut self.sub_agent_result_buffer);
            if !buffered.is_empty() {
                tracing::debug!(count = buffered.len(), "Draining buffered SubAgentResults");
                generated_events.extend(buffered);
            }
            // Set deadline (REQ-SA-006): timeout starts when parent enters AwaitingSubAgents
            self.sub_agent_deadline = Some(tokio::time::Instant::now() + DEFAULT_SUBAGENT_TIMEOUT);
            tracing::debug!(
                timeout_secs = DEFAULT_SUBAGENT_TIMEOUT.as_secs(),
                "Sub-agent deadline set"
            );
        }

        // Clear deadline when leaving AwaitingSubAgents/CancellingSubAgents
        if leaving_awaiting {
            self.sub_agent_deadline = None;
        }

        // Execute effects and collect generated events
        for effect in result.effects {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        Ok(generated_events)
    }

    /// Check if the current state can handle `SubAgentResult` events
    fn can_handle_sub_agent_result(&self) -> bool {
        matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        )
    }

    /// Handle sub-agent timeout: cancel all pending agents and inject `TimedOut` results.
    ///
    /// Called from the executor select loop when `sub_agent_deadline` fires (REQ-SA-006).
    async fn handle_sub_agent_timeout(&mut self) {
        use crate::state_machine::state::SubAgentOutcome;

        self.sub_agent_deadline = None;

        let pending_ids: Vec<(String, String)> =
            if let ConvState::AwaitingSubAgents { pending, .. } = &self.state {
                pending
                    .iter()
                    .map(|p| (p.agent_id.clone(), p.task.clone()))
                    .collect()
            } else {
                // Deadline fired but state already moved on — nothing to do
                return;
            };

        tracing::warn!(
            count = pending_ids.len(),
            "Sub-agent timeout reached, cancelling pending agents"
        );

        // Cancel the actual sub-agent runtimes
        if let Some(cancel_tx) = &self.cancel_tx {
            let ids: Vec<String> = pending_ids.iter().map(|(id, _)| id.clone()).collect();
            let request = SubAgentCancelRequest {
                ids,
                parent_conversation_id: self.context.conversation_id.clone(),
                parent_event_tx: self.event_tx.clone(),
            };
            if let Err(e) = cancel_tx.send(request).await {
                tracing::error!(error = %e, "Failed to send cancel request for timed-out agents");
            }
        }

        // Inject TimedOut results for each pending agent — transitions state normally
        for (agent_id, _task) in pending_ids {
            let event = Event::SubAgentResult {
                agent_id,
                outcome: SubAgentOutcome::TimedOut,
            };
            if let Err(e) = self.process_event(event).await {
                tracing::warn!(error = %e, "Failed to process timeout result for sub-agent");
            }
        }
    }

    /// Handle the hard stop after grace turn (REQ-BED-026 `SubAgentTurnLimitHardStop`):
    /// extract last assistant text from conversation history and notify parent.
    ///
    /// Extract partial result from conversation history and send `GraceTurnExhausted`
    /// event to the state machine. The SM handles the transition and emits `NotifyParent`.
    async fn handle_grace_turn_hard_stop(&mut self) {
        // Extract last assistant text from conversation history (I/O — belongs in executor)
        let partial_result = match self
            .storage
            .get_messages(&self.context.conversation_id)
            .await
        {
            Ok(messages) => {
                // Walk backward to find the last assistant message with text content blocks
                let mut text = None;
                for msg in messages.iter().rev() {
                    if let MessageContent::Agent(blocks) = &msg.content {
                        let text_parts: Vec<&str> = blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect();
                        if !text_parts.is_empty() {
                            text = Some(text_parts.join("\n\n"));
                            break;
                        }
                    }
                }
                text
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read messages for partial result extraction");
                None
            }
        };

        // Send GraceTurnExhausted event to the state machine.
        // The state machine handles the transition to Completed/Failed
        // and emits NotifyParent as an effect.
        let _ = self
            .event_tx
            .send(Event::GraceTurnExhausted {
                result: partial_result,
            })
            .await;
    }

    /// Halt a parent conversation that has exceeded its tool-use cycle cap
    /// (task 24680). Persists a user-visible system message explaining what
    /// happened, then sends `Event::UserCancel` so the state machine
    /// transitions `LlmRequesting → Idle` via the normal abort path. The
    /// next user message will reset the counter and resume normal operation.
    ///
    /// `attempted` is the attempt number that tripped the guard — strictly
    /// `cap + 1` for the first trip of a turn, but the signature makes the
    /// off-by-one explicit to operators reading logs or the system message:
    /// "attempt #{attempted} exceeds cap of {cap}" reads unambiguously,
    /// while a bare "limit reached ({cap})" invites confusion about whether
    /// the counter shown elsewhere (`cap + 1`) is a bug.
    async fn halt_parent_cycle_cap(&mut self, cap: u32, attempted: u32) {
        let msg_id = uuid::Uuid::new_v4().to_string();
        let text = format!(
            "Tool-use iteration limit reached: attempted LLM call #{attempted} exceeds the cap \
             of {cap} consecutive calls without a user message. Halted to prevent a runaway \
             agent loop. Send another message to continue — the counter resets on every user \
             turn. If this keeps happening, check recent tool results for a stuck call. \
             Override via the PHOENIX_PARENT_TOOL_CYCLE_CAP env var (0 disables)."
        );
        let content = crate::db::MessageContent::system(text);

        match self
            .storage
            .add_message(&msg_id, &self.context.conversation_id, &content, None, None)
            .await
        {
            Ok(msg) => {
                let _ = self.broadcast_tx.send(SseEvent::Message { message: msg });
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to persist parent cycle cap system message"
                );
            }
        }

        let _ = self
            .event_tx
            .send(Event::UserCancel {
                reason: Some(format!("parent_tool_cycle_cap_exceeded ({cap})")),
            })
            .await;
    }

    /// Handle the `spawn_agents` tool specially:
    /// 1. Parse tasks and generate agent IDs
    /// 2. Send spawn requests to `RuntimeManager` for each task
    /// 3. Return `SpawnAgentsComplete` event
    #[allow(clippy::too_many_lines)]
    async fn handle_spawn_agents_tool(&mut self, tool: ToolCall) -> Result<Option<Event>, String> {
        use crate::state_machine::state::{PendingSubAgent, SpawnAgentsInput, SubAgentSpec};

        let tool_use_id = tool.id.clone();
        let input_value = tool.input.to_value();

        // Parse the spawn_agents input
        let input: SpawnAgentsInput = match serde_json::from_value(input_value) {
            Ok(i) => i,
            Err(e) => {
                // Return error as regular tool completion
                let result = ToolResult::error(tool_use_id.clone(), format!("Invalid input: {e}"));
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        };

        if input.tasks.is_empty() {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "At least one task is required".to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        // Bounded buffer: pre-allocate with capacity = sub-agent count (FM-6 prevention)
        self.sub_agent_result_buffer = Vec::with_capacity(input.tasks.len());

        // --- Mode validation and one-writer constraint (REQ-PROJ-008) ---
        let parent_allows_work = matches!(
            self.context.mode_context,
            Some(ModeContext::Work { .. } | ModeContext::Direct)
        );

        let mut work_count_in_batch = 0u32;
        for task in &input.tasks {
            let mode = task.mode.unwrap_or_default();
            if mode == SubAgentMode::Work {
                if !parent_allows_work {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        "Work sub-agents require the parent to be in Work mode. \
                         Use mode: \"explore\" or omit mode for read-only sub-agents."
                            .to_string(),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
                work_count_in_batch += 1;
            }
        }

        if work_count_in_batch > 1 {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "Only one Work sub-agent can be spawned per call. \
                 Split into separate spawn_agents calls if you need sequential Work sub-agents."
                    .to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        if work_count_in_batch > 0 && self.active_work_subagents > 0 {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "A Work sub-agent is already active. Only one Work sub-agent \
                 can run at a time per parent conversation. Wait for it to complete \
                 before spawning another."
                    .to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        // Generate agent IDs and prepare spawn specs
        let mut spawned = Vec::new();
        let parent_cwd = self.context.working_dir.to_string_lossy().to_string();

        for task in &input.tasks {
            let agent_id = uuid::Uuid::new_v4().to_string();
            let cwd = task.cwd.clone().unwrap_or_else(|| parent_cwd.clone());
            let mode = task.mode.unwrap_or_default();

            // Resolve model (REQ-PROJ-008)
            let resolved_model = if let Some(ref model) = task.model {
                if self.llm_registry.get(model).is_none() {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!(
                            "Unknown model '{}'. Available: {:?}",
                            model,
                            self.llm_registry.available_models()
                        ),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
                model.clone()
            } else {
                match mode {
                    SubAgentMode::Explore => self
                        .llm_registry
                        .cheap_model_id_for_provider(&self.context.model_id),
                    SubAgentMode::Work => self.context.model_id.clone(),
                }
            };

            // Resolve max turns (REQ-PROJ-008)
            let max_turns = task.max_turns.unwrap_or(match mode {
                SubAgentMode::Explore => 20,
                SubAgentMode::Work => 50,
            });

            spawned.push(PendingSubAgent {
                agent_id: agent_id.clone(),
                task: task.task.clone(),
                mode,
            });

            // Send spawn request to RuntimeManager
            if let Some(spawn_tx) = &self.spawn_tx {
                let spec = SubAgentSpec {
                    agent_id,
                    task: task.task.clone(),
                    cwd,
                    timeout: DEFAULT_SUBAGENT_TIMEOUT,
                    mode,
                    model_id: resolved_model,
                    max_turns,
                };
                let request = SubAgentSpawnRequest {
                    spec,
                    parent_conversation_id: self.context.conversation_id.clone(),
                    parent_event_tx: self.event_tx.clone(),
                };
                if let Err(e) = spawn_tx.send(request).await {
                    tracing::error!(error = %e, "Failed to send spawn request");
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!("Failed to spawn sub-agents: {e}"),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
            } else {
                tracing::warn!("No spawn channel configured, cannot spawn sub-agents");
                let result = ToolResult::error(
                    tool_use_id.clone(),
                    "Sub-agent spawning not configured".to_string(),
                );
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        }

        // Track active Work sub-agents for one-writer constraint (REQ-PROJ-008)
        self.active_work_subagents += work_count_in_batch;

        // Build success result
        let agent_ids: Vec<&str> = spawned.iter().map(|p| p.agent_id.as_str()).collect();
        let output = format!(
            "Spawning {} sub-agent(s): {}",
            spawned.len(),
            agent_ids.join(", ")
        );
        let result = ToolResult {
            tool_use_id: tool_use_id.clone(),
            success: true,
            output,
            is_error: false,
            display_data: None,
            images: vec![],
        };

        // Send SpawnAgentsComplete event (synchronously returned, not async)
        Ok(Some(Event::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        }))
    }

    /// Execute an effect and optionally return a generated event
    #[allow(clippy::too_many_lines)] // Effect handling is inherently complex
    async fn execute_effect(&mut self, effect: Effect) -> Result<Option<Event>, String> {
        match effect {
            Effect::PersistMessage {
                content,
                display_data,
                usage_data,
                message_id,
            } => {
                let msg = self
                    .storage
                    .add_message(
                        &message_id,
                        &self.context.conversation_id,
                        &content,
                        display_data.as_ref(),
                        usage_data.as_ref(),
                    )
                    .await?;

                // Broadcast to clients (display_data already computed at effect creation)
                let _ = self.broadcast_tx.send(SseEvent::Message { message: msg });
                Ok(None)
            }

            Effect::PersistState => {
                // Persist the full state as JSON
                self.storage
                    .update_state(&self.context.conversation_id, &self.state)
                    .await?;

                // Broadcast state change with full state data
                let _ = self.broadcast_tx.send(SseEvent::StateChange {
                    state: self.state.clone(),
                    display_state: self.state.display_state().as_str().to_string(),
                });
                Ok(None)
            }

            Effect::RequestLlm => {
                // Parent-conversation tool-use cycle cap (task 24680). Sub-agents
                // have their own lifetime cap below (REQ-PROJ-008); this branch
                // only fires for parent conversations. The counter is reset at
                // the top of `process_event` on every `Event::UserMessage`.
                if !self.context.is_sub_agent && self.parent_tool_cycle_cap > 0 {
                    self.parent_tool_cycle_count += 1;
                    if self.parent_tool_cycle_count > self.parent_tool_cycle_cap {
                        let cap = self.parent_tool_cycle_cap;
                        let attempted = self.parent_tool_cycle_count;
                        tracing::warn!(
                            conv_id = %self.context.conversation_id,
                            attempted,
                            cap,
                            "parent conversation attempted to exceed tool-use cycle cap; halting"
                        );
                        self.halt_parent_cycle_cap(cap, attempted).await;
                        return Ok(None);
                    }
                }

                // Max turns enforcement (REQ-PROJ-008, REQ-BED-026): sub-agents have a
                // finite turn budget. Grace turn mechanism gives the model one extra LLM
                // turn to call submit_result before hard-stopping.
                if self.context.max_turns > 0 {
                    self.llm_turn_count += 1;
                    if self.llm_turn_count > self.context.max_turns {
                        if self.grace_turn_granted {
                            // Second hit: hard stop with partial result extraction
                            // (REQ-BED-026 SubAgentTurnLimitHardStop)
                            tracing::info!(
                                conv_id = %self.context.conversation_id,
                                turns = self.llm_turn_count,
                                max = self.context.max_turns,
                                "Sub-agent grace turn exhausted, extracting partial results"
                            );

                            self.handle_grace_turn_hard_stop().await;
                            return Ok(None);
                        }

                        // First hit: grant grace turn (REQ-BED-026 SubAgentTurnLimitGraceTurn)
                        self.grace_turn_granted = true;
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            turns = self.llm_turn_count,
                            max = self.context.max_turns,
                            "Sub-agent reached turn limit, granting grace turn"
                        );

                        // Inject a meta user message prompting submit_result.
                        // Uses UserContent::meta() so it appears in the LLM context
                        // via the existing User message path (not System, which is
                        // UI-only bookkeeping and not sent to the LLM).
                        let msg_id = uuid::Uuid::new_v4().to_string();
                        let content = MessageContent::User(
                            crate::db::UserContent::meta(
                                "You have reached your turn limit. Please call submit_result now \
                                 with whatever findings you have so far. Do not call any other tools.",
                            ),
                        );
                        if let Err(e) = self
                            .storage
                            .add_message(
                                &msg_id,
                                &self.context.conversation_id,
                                &content,
                                None,
                                None,
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "Failed to persist grace turn message");
                        }

                        // Allow the normal LLM request to proceed (don't return, don't
                        // send UserCancel). The meta message will appear in the next
                        // build_llm_messages call as a user-role message.
                    }
                }

                // Typed oneshot channel: background task gets Sender<LlmOutcome>,
                // physically cannot send a ToolOutcome or other type.
                let (llm_tx, llm_rx) = oneshot::channel::<LlmOutcome>();
                let outcome_tx = self.outcome_tx.clone();

                let llm_client = self.llm_client.clone();
                let tool_executor = self.tool_executor.clone();
                let storage = self.storage.clone();
                let conv_id = self.context.conversation_id.clone();
                let working_dir = self.context.working_dir.clone();
                let is_sub_agent = self.context.is_sub_agent;
                let mode_context = self.context.mode_context.clone();

                // Token streaming channel (REQ-BED-025).
                //
                // Broadcast so the forwarding task can subscribe before the LLM
                // task starts emitting chunks. The forwarder bridges this
                // per-request broadcast to `self.broadcast_tx` (the per-
                // conversation SSE broadcast) as `SseEvent::Token`.
                //
                // Task 24683: the LLM task owns the forwarder's `JoinHandle`
                // and awaits it after the LLM call finishes. That forces a
                // happens-before barrier so every `SseEvent::Token` has been
                // sent to `self.broadcast_tx` before the main executor loop
                // is ever told the call is done (and therefore before it
                // broadcasts `SseEvent::Message`). Without this barrier a
                // trailing Token could land on the SSE channel after its
                // Message, producing a phantom streaming buffer on the
                // client (the "repeated message" bug).
                let (chunk_tx, chunk_rx) = broadcast::channel::<crate::llm::TokenChunk>(256);
                let request_id = uuid::Uuid::new_v4().to_string();

                let broadcast_tx_for_tokens = self.broadcast_tx.clone();
                let request_id_for_fwd = request_id.clone();
                let forwarder_handle = tokio::spawn(async move {
                    let mut rx = chunk_rx;
                    loop {
                        match rx.recv().await {
                            Ok(crate::llm::TokenChunk::Text(text)) => {
                                let _ = broadcast_tx_for_tokens.send(SseEvent::Token {
                                    text,
                                    request_id: request_id_for_fwd.clone(),
                                });
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::debug!(n, "Token forwarding lagged — some tokens dropped");
                            }
                        }
                    }
                });

                let handle = tokio::spawn(async move {
                    if is_sub_agent {
                        tracing::info!(
                            conv_id = %conv_id,
                            request_id = %request_id,
                            sub_agent = true,
                            "Making LLM request"
                        );
                    } else {
                        tracing::info!(
                            conv_id = %conv_id,
                            request_id = %request_id,
                            "Making LLM request"
                        );
                    }

                    // Build messages from history
                    let messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                        Ok(m) => m,
                        Err(e) => {
                            // Build error → treated as InvalidRequest
                            let _ = llm_tx.send(LlmOutcome::NetworkError { message: e });
                            return;
                        }
                    };

                    // Build system prompt with AGENTS.md content + mode context
                    let system_prompt =
                        build_system_prompt(&working_dir, is_sub_agent, mode_context.as_ref());

                    // Build request — normalize messages against current tool set
                    // to remove tool_use/tool_result blocks for tools no longer
                    // available (e.g., propose_task after Explore→Work transition).
                    let tools = tool_executor.definitions().await;
                    let tool_names: std::collections::HashSet<&str> =
                        tools.iter().map(|t| t.name.as_str()).collect();
                    let messages = strip_unavailable_tool_blocks(messages, &tool_names);

                    let request = LlmRequest {
                        system: vec![SystemContent::cached(&system_prompt)],
                        messages,
                        tools,
                        max_tokens: Some(16_384),
                    };

                    // Use streaming — chunk_tx forwards text tokens to SSE clients.
                    let llm_outcome = match llm_client.complete_streaming(&request, &chunk_tx).await
                    {
                        Ok(response) => {
                            // Extract tool calls from content and convert to typed ToolCall
                            let tool_calls: Vec<ToolCall> = response
                                .tool_uses()
                                .into_iter()
                                .map(|(id, name, input)| {
                                    let typed_input =
                                        ToolInput::from_name_and_value(name, input.clone());
                                    ToolCall::new(id.to_string(), typed_input)
                                })
                                .collect();

                            let usage = &response.usage;
                            tracing::info!(
                                input = usage.input_tokens,
                                output = usage.output_tokens,
                                cache_write = usage.cache_creation_tokens,
                                cache_read = usage.cache_read_tokens,
                                "LLM response token usage"
                            );

                            LlmOutcome::Response {
                                content: response.content,
                                tool_calls,
                                end_turn: response.end_turn,
                                usage: response.usage,
                            }
                        }
                        Err(e) => llm_error_to_outcome(e),
                    };

                    // Happens-before barrier for task 24683: close the chunk
                    // broadcast and wait for the forwarder to drain any
                    // trailing tokens before the outcome (and therefore the
                    // eventual `SseEvent::Message`) is allowed to proceed.
                    //
                    //   1. Drop `chunk_tx` explicitly. Relying on the end of
                    //      the closure isn't enough — we need the forwarder
                    //      to see `Err(Closed)` *before* the `.await` below.
                    //   2. Await the forwarder's `JoinHandle`. This suspends
                    //      this task until every buffered `TokenChunk` has
                    //      been broadcast as `SseEvent::Token`.
                    //   3. Only then send the outcome that will cause the
                    //      main executor loop to broadcast `SseEvent::Message`.
                    drop(chunk_tx);
                    if let Err(e) = forwarder_handle.await {
                        tracing::warn!(error = ?e, "token forwarder task joined with error");
                    }

                    let _ = llm_tx.send(llm_outcome);
                });
                self.llm_task_handle = Some(handle);

                // Forward the typed outcome to the unified outcome channel
                tokio::spawn(async move {
                    if let Ok(llm_outcome) = llm_rx.await {
                        let _ = outcome_tx.send(EffectOutcome::Llm(llm_outcome)).await;
                    }
                });

                Ok(None)
            }

            Effect::ExecuteTool { tool } => {
                // Special handling for spawn_agents tool
                if tool.name() == "spawn_agents" {
                    return self.handle_spawn_agents_tool(tool).await;
                }

                // Typed oneshot channel: background task gets Sender<ToolOutcome>,
                // physically cannot send an LlmOutcome or other type.
                let (tool_tx, tool_rx) = oneshot::channel::<ToolOutcome>();
                let outcome_tx = self.outcome_tx.clone();

                // Create cancellation token for this tool execution
                let cancel_token = CancellationToken::new();
                self.tool_cancel_token = Some(cancel_token.clone());
                let cancel_token_check = cancel_token.clone();

                // Create ToolContext for this invocation
                let tool_ctx = ToolContext::new(
                    cancel_token,
                    self.context.conversation_id.clone(),
                    self.context.working_dir.clone(),
                    self.browser_sessions.clone(),
                    self.llm_registry.clone(),
                    self.terminals.clone(),
                );

                let conv_id = self.context.conversation_id.clone();
                let tool_executor = self.tool_executor.clone();
                let tool_use_id = tool.id.clone();
                let tool_name = tool.name().to_string();
                let tool_input = tool.input.to_value();

                tokio::spawn(async move {
                    tracing::info!(
                        conv_id = %conv_id,
                        tool = %tool_name,
                        id = %tool_use_id,
                        "Executing tool"
                    );
                    let tool_start = std::time::Instant::now();

                    let output = tool_executor
                        .execute(&tool_name, tool_input, tool_ctx)
                        .await;

                    // Check if the tool was cancelled via the cancellation token.
                    // IMPORTANT: We check the token state, NOT the output string.
                    // The state machine only accepts ToolAborted from CancellingTool state,
                    // which is entered when AbortTool effect cancels the token.
                    let tool_outcome = if cancel_token_check.is_cancelled() {
                        tracing::info!(
                            conv_id = %conv_id,
                            tool = %tool_name,
                            id = %tool_use_id,
                            "Tool cancelled"
                        );
                        ToolOutcome::Aborted {
                            tool_use_id,
                            reason: crate::state_machine::AbortReason::CancellationRequested,
                        }
                    } else {
                        use crate::db::ToolContentImage;
                        if let Some(out) = output {
                            tracing::info!(
                                conv_id = %conv_id,
                                tool = %tool_name,
                                id = %tool_use_id,
                                duration_ms = u64::try_from(tool_start.elapsed().as_millis()).unwrap_or(u64::MAX),
                                success = out.success,
                                "Tool completed"
                            );
                            let images = out
                                .images
                                .into_iter()
                                .map(|img| ToolContentImage {
                                    media_type: img.media_type,
                                    data: img.data,
                                })
                                .collect();
                            ToolOutcome::Completed(ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                success: out.success,
                                output: out.output,
                                is_error: !out.success,
                                display_data: out.display_data,
                                images,
                            })
                        } else {
                            tracing::warn!(
                                conv_id = %conv_id,
                                tool = %tool_name,
                                id = %tool_use_id,
                                "Tool not found"
                            );
                            ToolOutcome::Failed {
                                tool_use_id,
                                error: format!("Unknown tool: {tool_name}"),
                            }
                        }
                    };
                    // Send typed outcome through oneshot channel
                    let _ = tool_tx.send(tool_outcome);
                });

                // Forward the typed outcome to the unified outcome channel
                tokio::spawn(async move {
                    if let Ok(tool_outcome) = tool_rx.await {
                        let _ = outcome_tx.send(EffectOutcome::Tool(tool_outcome)).await;
                    }
                });

                Ok(None)
            }

            Effect::ScheduleRetry { delay, attempt } => {
                // Typed oneshot for retry timeout
                let outcome_tx = self.outcome_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = outcome_tx
                        .send(EffectOutcome::RetryTimeout { attempt })
                        .await;
                });
                Ok(None)
            }

            Effect::NotifyClient { event_type, data } => {
                match event_type.as_str() {
                    "agent_done" => {
                        let _ = self.broadcast_tx.send(SseEvent::AgentDone);
                    }
                    "state_change" => {
                        // data should contain the full state object; deserialize to typed ConvState
                        if let Some(state_val) = data.get("state") {
                            match serde_json::from_value::<ConvState>(state_val.clone()) {
                                Ok(typed_state) => {
                                    let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                        state: typed_state,
                                        display_state: self
                                            .state
                                            .display_state()
                                            .as_str()
                                            .to_string(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to deserialize NotifyClient state_change into ConvState; \
                                         falling back to current executor state"
                                    );
                                    let _ = self.broadcast_tx.send(SseEvent::StateChange {
                                        state: self.state.clone(),
                                        display_state: self
                                            .state
                                            .display_state()
                                            .as_str()
                                            .to_string(),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                Ok(None)
            }

            Effect::PersistCheckpoint { data } => {
                use crate::state_machine::CheckpointData;
                match data {
                    CheckpointData::ToolRound {
                        assistant_message,
                        tool_results,
                    } => {
                        // Persist assistant message
                        let agent_content = MessageContent::agent(assistant_message.content);
                        let agent_msg = self
                            .storage
                            .add_message(
                                &assistant_message.message_id,
                                &self.context.conversation_id,
                                &agent_content,
                                assistant_message.display_data.as_ref(),
                                assistant_message.usage.as_ref(),
                            )
                            .await?;
                        let _ = self
                            .broadcast_tx
                            .send(SseEvent::Message { message: agent_msg });

                        // Persist all tool results
                        for result in tool_results {
                            let tool_content = MessageContent::tool(
                                &result.tool_use_id,
                                &result.output,
                                result.is_error,
                            );
                            let tool_msg_id = format!("{}-result", result.tool_use_id);
                            let tool_msg = self
                                .storage
                                .add_message(
                                    &tool_msg_id,
                                    &self.context.conversation_id,
                                    &tool_content,
                                    result.display_data.as_ref(),
                                    None,
                                )
                                .await?;
                            let _ = self
                                .broadcast_tx
                                .send(SseEvent::Message { message: tool_msg });
                        }
                    }
                }
                Ok(None)
            }

            Effect::PersistToolResults { results } => {
                for result in results {
                    let content =
                        MessageContent::tool(&result.tool_use_id, &result.output, result.is_error);
                    let tool_msg_id = uuid::Uuid::new_v4().to_string();
                    let msg = self
                        .storage
                        .add_message(
                            &tool_msg_id,
                            &self.context.conversation_id,
                            &content,
                            None,
                            None,
                        )
                        .await?;

                    // Tool results don't contain bash tool_use blocks, no enrichment needed
                    let _ = self.broadcast_tx.send(SseEvent::Message { message: msg });
                }
                Ok(None)
            }

            Effect::AbortTool { tool_use_id } => {
                // Signal abort to running tool
                tracing::info!(tool_id = %tool_use_id, "Aborting tool execution");
                if let Some(token) = self.tool_cancel_token.take() {
                    token.cancel();
                }
                // The spawned task will send ToolAborted event when it sees cancellation
                Ok(None)
            }

            Effect::AbortLlm => {
                tracing::info!("Aborting LLM request");
                if let Some(handle) = self.llm_task_handle.take() {
                    handle.abort();
                }
                Ok(None)
            }

            Effect::CancelSubAgents { ids } => {
                tracing::info!(?ids, "Cancelling sub-agents");

                if let Some(cancel_tx) = &self.cancel_tx {
                    let request = SubAgentCancelRequest {
                        ids,
                        parent_conversation_id: self.context.conversation_id.clone(),
                        parent_event_tx: self.event_tx.clone(),
                    };
                    if let Err(e) = cancel_tx.send(request).await {
                        tracing::error!(error = %e, "Failed to send cancel request");
                    }
                } else {
                    tracing::warn!("No cancel channel configured, cannot cancel sub-agents");
                }
                Ok(None)
            }

            Effect::NotifyParent { outcome } => {
                tracing::info!(?outcome, "Notifying parent of sub-agent completion");

                if let Some(parent_tx) = &self.parent_event_tx {
                    let event = Event::SubAgentResult {
                        agent_id: self.context.conversation_id.clone(),
                        outcome,
                    };
                    if let Err(e) = parent_tx.send(event).await {
                        // Parent may have terminated - that's OK
                        tracing::warn!(error = %e, "Failed to notify parent (may have terminated)");
                    }
                } else {
                    tracing::warn!("No parent channel configured for sub-agent");
                }
                Ok(None)
            }

            Effect::PersistSubAgentResults {
                results,
                spawn_tool_id,
            } => {
                // Build the display_data for subagent results
                let display_data = serde_json::json!({
                    "type": "subagent_summary",
                    "results": results
                });

                // If we have a spawn_tool_id, update its message's content (for LLM history)
                // and display_data (for UI). The message was persisted as "{spawn_tool_id}-result".
                if let Some(tool_id) = spawn_tool_id {
                    use crate::state_machine::state::SubAgentOutcome;
                    let message_id = format!("{tool_id}-result");

                    // Build a human-readable summary of sub-agent outcomes for the LLM.
                    // This replaces the initial "Spawning N sub-agents..." acknowledgement so
                    // build_llm_messages_static feeds the actual results to the model.
                    let llm_content = results
                        .iter()
                        .map(|r| {
                            let outcome = match &r.outcome {
                                SubAgentOutcome::Success { result } => {
                                    format!("Result: {result}")
                                }
                                SubAgentOutcome::Failure { error, .. } => {
                                    format!("Failed: {error}")
                                }
                                SubAgentOutcome::TimedOut => {
                                    "Timed out: sub-agent exceeded its time limit".to_string()
                                }
                            };
                            format!("Task: \"{}\"\n{outcome}", r.task)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let llm_content = format!(
                        "Sub-agent results ({} completed):\n\n{llm_content}",
                        results.len()
                    );

                    if let Err(e) = self
                        .storage
                        .update_tool_message_content(&message_id, &llm_content)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            message_id = %message_id,
                            "Failed to update spawn_agents message content with sub-agent results"
                        );
                    }

                    if let Err(e) = self
                        .storage
                        .update_message_display_data(&message_id, &display_data)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            message_id = %message_id,
                            "Failed to update spawn_agents message display_data"
                        );
                    } else {
                        // Fetch the updated message and broadcast it
                        // This allows the frontend to update its message state
                        match self.storage.get_message_by_id(&message_id).await {
                            Ok(updated_msg) => {
                                // This is a tool result message, not an agent message
                                // No bash enrichment needed
                                let _ = self.broadcast_tx.send(SseEvent::Message {
                                    message: updated_msg,
                                });
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    message_id = %message_id,
                                    "Failed to fetch updated message for broadcast"
                                );
                            }
                        }
                    }
                } else {
                    // No spawn_tool_id - create a standalone summary message
                    // This happens when spawn_agents wasn't the last tool in a batch
                    let summary_text = format!("{} sub-agent(s) completed", results.len());
                    let content = crate::db::MessageContent::tool(
                        uuid::Uuid::new_v4().to_string(),
                        &summary_text,
                        false,
                    );
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let message = self
                        .storage
                        .add_message(
                            &msg_id,
                            &self.context.conversation_id,
                            &content,
                            Some(&display_data),
                            None,
                        )
                        .await?;

                    // Broadcast the new message (tool message, no bash enrichment needed)
                    let _ = self.broadcast_tx.send(SseEvent::Message { message });
                }

                Ok(None)
            }

            Effect::RequestContinuation {
                rejected_tool_calls,
            } => {
                // REQ-BED-020: Request continuation summary (tool-less LLM request)
                self.request_continuation(rejected_tool_calls);
                Ok(None)
            }

            Effect::NotifyContextExhausted { summary } => {
                // REQ-BED-021: Notify client of context exhaustion
                let _ = self.broadcast_tx.send(SseEvent::StateChange {
                    state: ConvState::ContextExhausted { summary },
                    display_state: self.state.display_state().as_str().to_string(),
                });
                Ok(None)
            }

            Effect::ApproveTask {
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task(title, priority, plan).await?;
                Ok(None)
            }

            Effect::ResolveTask {
                system_message,
                repo_root,
            } => {
                self.execute_resolve_task(system_message, repo_root).await?;
                Ok(None)
            }
        }
    }

    /// Build LLM messages from conversation history (instance method)
    #[allow(dead_code)] // May be useful for non-spawned code paths
    async fn build_llm_messages(&self) -> Result<Vec<LlmMessage>, String> {
        Self::build_llm_messages_static(&self.storage, &self.context.conversation_id).await
    }

    /// Build LLM messages from conversation history (static, for spawned tasks)
    async fn build_llm_messages_static(
        storage: &S,
        conv_id: &str,
    ) -> Result<Vec<LlmMessage>, String> {
        use crate::db::{MessageContent, ToolContent};
        use crate::llm::ImageSource;

        let db_messages = storage.get_messages(conv_id).await?;

        let mut messages = Vec::new();

        for msg in db_messages {
            match &msg.content {
                MessageContent::User(user_content) => {
                    // Use llm_text when expansion occurred (REQ-IR-001, REQ-IR-006):
                    // the model sees the fully resolved form while the DB stores the shorthand.
                    let text_for_llm = user_content.llm_text();
                    let mut content = vec![ContentBlock::text(text_for_llm)];

                    // Add images (REQ-BED-013)
                    for img in &user_content.images {
                        content.push(ContentBlock::Image {
                            source: img.to_image_source(),
                        });
                    }

                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content,
                    });
                }

                MessageContent::Agent(blocks) => {
                    messages.push(LlmMessage {
                        role: MessageRole::Assistant,
                        content: blocks.clone(),
                    });
                }

                MessageContent::Tool(ToolContent {
                    tool_use_id,
                    content,
                    is_error,
                    images,
                }) => {
                    // Convert stored ToolContentImages to LLM ImageSources
                    let image_sources: Vec<ImageSource> = images
                        .iter()
                        .map(|img| ImageSource::Base64 {
                            media_type: img.media_type.clone(),
                            data: img.data.clone(),
                        })
                        .collect();

                    // Tool results go in user message
                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content.clone(),
                            images: image_sources,
                            is_error: *is_error,
                        }],
                    });
                }

                // Skill messages are delivered as user-role messages (REQ-SK-002)
                MessageContent::Skill(skill_content) => {
                    messages.push(LlmMessage {
                        role: MessageRole::User,
                        content: vec![ContentBlock::text(&skill_content.body)],
                    });
                }

                // Ignore system, error, and continuation messages.
                // System messages are UI-only bookkeeping (restart markers, task
                // file renames, diff snapshots). LLM-directed messages use
                // MessageContent::User with is_meta (e.g., grace turn prompt).
                MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_) => {}
            }
        }

        Ok(messages)
    }

    /// Request continuation summary from LLM (REQ-BED-020)
    #[allow(clippy::needless_pass_by_value)] // Consistent with Effect signature
    fn request_continuation(&mut self, rejected_tool_calls: Vec<ToolCall>) {
        let llm_client = Arc::clone(&self.llm_client);
        let storage = self.storage.clone();
        let event_tx = self.event_tx.clone();
        let conv_id = self.context.conversation_id.clone();

        // Build continuation prompt
        let continuation_prompt = build_continuation_prompt(&rejected_tool_calls);

        let handle = tokio::spawn(async move {
            // Build messages from history and add continuation request
            let mut messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build messages for continuation");
                    let _ = event_tx.send(Event::ContinuationFailed { error: e }).await;
                    return;
                }
            };

            // Add synthetic tool results for rejected tool calls to maintain valid conversation
            // history. These tools were never executed because context was exhausted before
            // they could run.
            for rejected_tool in &rejected_tool_calls {
                messages.push(LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: rejected_tool.id.clone(),
                        content: "Tool execution was skipped — context limit reached before this tool could run.".to_string(),
                        images: vec![],
                        is_error: false,
                    }],
                });
            }

            // Add the continuation request as a user message
            messages.push(LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(&continuation_prompt)],
            });

            // Build a tool-less request
            let request = LlmRequest {
                messages,
                system: vec![SystemContent::new(
                    "You are wrapping up a conversation that has reached its context limit. \
                    Provide a concise summary to help continue in a new conversation.",
                )],
                tools: vec![],          // No tools for continuation
                max_tokens: Some(2000), // Limit summary length
            };

            match llm_client.complete(&request).await {
                Ok(response) => {
                    // Extract the text content as summary
                    let summary = response
                        .content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::Text { text } = block {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let _ = event_tx.send(Event::ContinuationResponse { summary }).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Continuation LLM request failed");
                    // Send LlmError so the state machine's AwaitingContinuation retry logic fires.
                    // The attempt field is ignored by that arm (tracked in state), so 0 is fine.
                    let _ = event_tx
                        .send(Event::LlmError {
                            message: e.message.clone(),
                            error_kind: llm_error_to_db_error(e.kind),
                            attempt: 0,
                            recovery_in_progress: e.recovery_in_progress,
                        })
                        .await;
                }
            }
        });
        self.llm_task_handle = Some(handle);
    }

    /// REQ-BED-028: Execute git operations for task approval.
    ///
    /// Sequence: dirty tree check -> assign task ID -> mkdir tasks/ -> write task file ->
    /// git commit -> check branch collision -> create branch -> checkout -> update `conv_mode`.
    ///
    /// On failure: revert in-memory state to `AwaitingTaskApproval` so the user can retry.
    /// Collision check on retry handles partial state.
    /// Handle task resolution: finalize conversation state/mode/cwd, inject system message,
    /// and broadcast SSE events. Called after git operations have already completed.
    async fn execute_resolve_task(
        &mut self,
        system_message: String,
        repo_root: String,
    ) -> Result<(), String> {
        let conv_id = &self.context.conversation_id;

        // Atomically update state, mode, and cwd
        self.storage
            .update_state(conv_id, &ConvState::Terminal)
            .await?;
        self.storage
            .update_conversation_mode(conv_id, &crate::db::ConvMode::Explore)
            .await?;
        self.storage
            .update_conversation_cwd(conv_id, &repo_root)
            .await?;

        // Inject system message
        let msg_id = uuid::Uuid::new_v4().to_string();
        let msg = self
            .storage
            .add_message(
                &msg_id,
                conv_id,
                &MessageContent::system(&system_message),
                None,
                None,
            )
            .await?;

        // Broadcast SSE events
        let _ = self.broadcast_tx.send(SseEvent::Message { message: msg });
        let _ = self.broadcast_tx.send(SseEvent::StateChange {
            state: ConvState::Terminal,
            display_state: ConvState::Terminal.display_state().as_str().to_string(),
        });
        let _ = self.broadcast_tx.send(SseEvent::ConversationUpdate {
            update: crate::runtime::ConversationMetadataUpdate {
                cwd: Some(repo_root),
                branch_name: None,
                worktree_path: None,
                conv_mode_label: Some("Explore".to_string()),
                base_branch: None,
                commits_behind: None,
                commits_ahead: None,
                task_title: None,
            },
        });

        Ok(())
    }

    async fn execute_approve_task(
        &mut self,
        title: String,
        priority: String,
        plan: String,
    ) -> Result<(), String> {
        let cwd = self.context.working_dir.clone();
        let conv_id = self.context.conversation_id.clone();
        let desired_base_branch = self.context.desired_base_branch.clone();
        let storage = self.storage.clone();

        // Clone for state revert on failure (originals moved into spawn_blocking)
        let title_backup = title.clone();
        let priority_backup = priority.clone();
        let plan_backup = plan.clone();

        // Run blocking git/fs operations on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(
                &cwd,
                &conv_id,
                &title,
                &priority,
                &plan,
                desired_base_branch.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        match result {
            Ok(approval_result) => {
                // Update conversation mode to Work (includes worktree_path, base_branch, task_number)
                let work_mode = crate::db::ConvMode::Work {
                    branch_name: approval_result.branch_name.clone(),
                    worktree_path: approval_result.worktree_path.clone(),
                    base_branch: approval_result.base_branch.clone(),
                    task_id: approval_result.task_id.clone(),
                    task_title: approval_result.task_title.clone(),
                };
                storage
                    .update_conversation_mode(&self.context.conversation_id, &work_mode)
                    .await?;

                // Update conversation CWD to the worktree path
                storage
                    .update_conversation_cwd(
                        &self.context.conversation_id,
                        &approval_result.worktree_path,
                    )
                    .await?;

                // Replace working_dir to point at the worktree directory.
                // Field-level mutation (not full replacement) so we don't lose
                // is_sub_agent, context_exhaustion_behavior, or future fields.
                self.context.working_dir = std::path::PathBuf::from(&approval_result.worktree_path);

                // Upgrade tool registry from Explore to Work mode so the agent
                // gets bash, patch, etc. for the rest of this conversation.
                self.tool_executor.upgrade_to_work_mode();

                tracing::info!(
                    task_id = %approval_result.task_id,
                    branch = %approval_result.branch_name,
                    worktree = %approval_result.worktree_path,
                    first_task = approval_result.first_task,
                    "Task approved — worktree created"
                );

                // Persist as a user message so the LLM sees the approval + plan context.
                // The propose_task tool_use/result get stripped from history (tool not in
                // Work registry), so this message carries the plan forward. Must be the
                // last message before the next LLM call to avoid ending on an assistant
                // message (Anthropic rejects trailing assistant as "prefill").
                let branch_msg = format!(
                    "Task approved. You are on branch {} in {}.\n\n\
                     ## Approved plan: {}\n\n\
                     Priority: {}\n\n\
                     {}",
                    approval_result.branch_name,
                    approval_result.worktree_path,
                    title_backup,
                    priority_backup,
                    plan_backup,
                );
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = MessageContent::User(crate::db::UserContent::meta(&branch_msg));
                let msg = self
                    .storage
                    .add_message(&msg_id, &self.context.conversation_id, &content, None, None)
                    .await?;
                let _ = self.broadcast_tx.send(SseEvent::Message { message: msg });

                // Push updated conversation metadata to the client so it
                // reflects the new cwd, branch, worktree_path, and mode label
                // without requiring a reconnect.
                let _ = self.broadcast_tx.send(SseEvent::ConversationUpdate {
                    update: crate::runtime::ConversationMetadataUpdate {
                        cwd: Some(approval_result.worktree_path.clone()),
                        branch_name: Some(approval_result.branch_name.clone()),
                        worktree_path: Some(approval_result.worktree_path.clone()),
                        conv_mode_label: Some("Work".to_string()),
                        base_branch: Some(approval_result.base_branch.clone()),
                        commits_behind: None,
                        commits_ahead: None,
                        task_title: Some(approval_result.task_title.clone()),
                    },
                });

                Ok(())
            }
            Err(e) => {
                tracing::error!(error = %e, "Task approval git operations failed");

                // Revert in-memory state to AwaitingTaskApproval so the user can retry.
                // The DB still has AwaitingTaskApproval (PersistState hasn't run for the
                // new Idle state yet), so this keeps memory and DB consistent.
                self.state = ConvState::AwaitingTaskApproval {
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };

                // Broadcast an error so the UI knows, but don't propagate — the
                // conversation stays in AwaitingTaskApproval for retry.
                // Task 24682: use the typed UserFacingError. `e` is the
                // approval-pipeline error (Display-formatted, no Debug leak)
                // so it's safe to inline as the human detail.
                let _ = self.broadcast_tx.send(SseEvent::Error {
                    error: crate::runtime::user_facing_error::UserFacingError::retryable(
                        "Task approval failed",
                        format!(
                            "Phoenix could not finalise the task: {e}. The conversation \
                             stays in approval state — try approving again or abandon."
                        ),
                    ),
                });

                Ok(())
            }
        }
    }
}

/// Result of a successful task approval
struct TaskApprovalResult {
    task_id: String,
    task_title: String,
    branch_name: String,
    first_task: bool,
    /// Absolute path to the git worktree created for this conversation
    worktree_path: String,
    /// The branch that was checked out when the task was approved (merge target)
    base_branch: String,
}

/// Remove `tool_use` and `tool_result` blocks that reference tools not in the current set.
///
/// Handles mode transitions (e.g., Explore -> Work) where the tool set changes
/// but the conversation history contains `tool_use` blocks for the old set.
/// Anthropic's API rejects requests where `tool_use` blocks reference unavailable tools.
///
/// The DB history is not modified -- this operates on the in-memory message Vec only.
fn strip_unavailable_tool_blocks(
    messages: Vec<LlmMessage>,
    available_tools: &std::collections::HashSet<&str>,
) -> Vec<LlmMessage> {
    use crate::llm::ContentBlock;

    // First pass: collect IDs of tool_use blocks we're going to strip
    let mut stripped_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in &messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                if !available_tools.contains(name.as_str()) {
                    stripped_ids.insert(id.clone());
                }
            }
        }
    }

    if stripped_ids.is_empty() {
        return messages;
    }

    tracing::debug!(
        count = stripped_ids.len(),
        "Stripping tool_use/tool_result blocks for unavailable tools"
    );

    // Second pass: filter out stripped tool_use/tool_result blocks.
    // For ToolSearchToolResult, remove individual bad references but keep the block
    // (it's paired with a ServerToolUse that we must not orphan).
    messages
        .into_iter()
        .map(|msg| {
            let filtered: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { ref id, .. } => {
                        if stripped_ids.contains(id) {
                            None
                        } else {
                            Some(block)
                        }
                    }
                    ContentBlock::ToolResult {
                        ref tool_use_id, ..
                    } => {
                        if stripped_ids.contains(tool_use_id) {
                            None
                        } else {
                            Some(block)
                        }
                    }
                    // Filter individual unavailable references but keep the block
                    ContentBlock::ToolSearchToolResult {
                        tool_use_id,
                        mut content,
                    } => {
                        content
                            .tool_references
                            .retain(|r| available_tools.contains(r.tool_name.as_str()));
                        Some(ContentBlock::ToolSearchToolResult {
                            tool_use_id,
                            content,
                        })
                    }
                    // ServerToolUse blocks are server-side — never strip
                    _ => Some(block),
                })
                .collect();
            LlmMessage {
                role: msg.role,
                content: filtered,
            }
        })
        // Drop messages that became empty after filtering
        .filter(|msg| !msg.content.is_empty())
        .collect()
}

/// Get the next task ID using taskmd-core library.
fn get_next_task_id(tasks_dir: &std::path::Path) -> String {
    taskmd_core::ids::next_id(tasks_dir)
}

/// Run a git command in the given directory, returning stdout on success or an error message.
///
/// All git operations use a dedicated bot identity and disable commit signing
/// to avoid depending on the user's SSH agent (which breaks in workspaces/tmux).
pub(crate) fn run_git(cwd: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .map_err(|e| format!("Failed to run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

/// Global mutex serializing the scan-tasks + write + commit sequence.
/// Task approval is rare; a single mutex is sufficient.
/// Also used by Complete/Abandon flows (task 0604) for git-on-main-checkout operations.
pub(crate) static TASK_APPROVAL_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Blocking implementation of approve task git operations.
/// Runs on a blocking thread via `spawn_blocking`.
#[allow(clippy::too_many_lines)] // Sequential git flow; splitting hurts readability
fn execute_approve_task_blocking(
    cwd: &std::path::Path,
    conv_id: &str,
    title: &str,
    priority: &str,
    plan: &str,
    desired_base_branch: Option<&str>,
) -> Result<TaskApprovalResult, String> {
    use std::io::Write;

    // Serialize the entire scan + write + commit sequence to prevent
    // concurrent approvals from getting the same task number.
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // 0. Determine the base branch: use desired if set, otherwise current HEAD.
    //    REQ-PROJ-022: always single-branch fetch before worktree creation.
    let base_branch = if let Some(desired) = desired_base_branch {
        desired.to_string()
    } else {
        let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        let branch = branch.trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            return Err(
                "Cannot determine current branch (detached HEAD?). Check out a branch before approving."
                    .to_string(),
            );
        }
        branch
    };

    // REQ-PROJ-022: single-branch fetch to get the latest remote tip.
    let refspec = format!("refs/heads/{base_branch}:refs/remotes/origin/{base_branch}");
    if let Err(e) = run_git(cwd, &["fetch", "origin", &refspec]) {
        tracing::debug!(
            branch = %base_branch,
            error = %e,
            "Single-branch fetch failed (non-fatal, using local ref)"
        );
    }

    // Materialize branch: ensure a local ref exists.
    let has_local = run_git(cwd, &["rev-parse", "--verify", &base_branch]).is_ok();
    let remote_ref = format!("origin/{base_branch}");
    let has_remote = run_git(cwd, &["rev-parse", "--verify", &remote_ref]).is_ok();

    if has_local && has_remote {
        // Fast-forward local to remote tip if possible.
        // `git fetch origin <branch>:<branch>` does this atomically, but only if
        // <branch> is not currently checked out. Use update-ref as fallback.
        let local_sha = run_git(cwd, &["rev-parse", &base_branch]).unwrap_or_default();
        let remote_sha = run_git(cwd, &["rev-parse", &remote_ref]).unwrap_or_default();
        if local_sha.trim() != remote_sha.trim() {
            // Check if fast-forward is possible (remote is descendant of local).
            if run_git(
                cwd,
                &["merge-base", "--is-ancestor", &base_branch, &remote_ref],
            )
            .is_ok()
            {
                // Safe to fast-forward. Only works if branch is not checked out.
                let current_head =
                    run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
                if current_head.trim() == base_branch {
                    tracing::debug!(
                        branch = %base_branch,
                        "Cannot fast-forward: branch is currently checked out"
                    );
                } else {
                    let _ = run_git(
                        cwd,
                        &[
                            "update-ref",
                            &format!("refs/heads/{base_branch}"),
                            remote_sha.trim(),
                        ],
                    );
                    tracing::info!(branch = %base_branch, "Fast-forwarded local branch to remote tip");
                }
            } else {
                tracing::debug!(
                    branch = %base_branch,
                    "Local and remote have diverged; using local ref as-is"
                );
            }
        }
    } else if !has_local && has_remote {
        // Remote-only: create local tracking branch.
        run_git(cwd, &["branch", "--track", &base_branch, &remote_ref]).map_err(|e| {
            format!("Failed to create local branch '{base_branch}' from {remote_ref}: {e}")
        })?;
        tracing::info!(
            branch = %base_branch,
            "Created local tracking branch from remote"
        );
    } else if !has_local && !has_remote {
        return Err(format!(
            "Branch '{base_branch}' not found locally or at origin"
        ));
    }
    // has_local && !has_remote: local-only branch, use as-is.

    let current_head = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let on_base_branch = base_branch == current_head;

    let tasks_dir = cwd.join("tasks");

    // Track whether tasks/ existed before we create it
    let first_task = !tasks_dir.exists();

    // 2. Create tasks/ directory if needed
    if first_task {
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks/ directory: {e}"))?;
        tracing::info!("Created tasks/ directory (first task for this project)");
    }

    // 3. Assign task ID via taskmd-core
    let task_id = get_next_task_id(&tasks_dir);

    // 4. Derive slug from title
    let slug = taskmd_core::filename::derive_slug(title);
    if slug.is_empty() {
        return Err("Cannot derive a valid slug from the task title".to_string());
    }

    // 5. Write task file (taskmd format: DDNNN-pX-status--slug.md)
    let filename = taskmd_core::filename::format_filename(&task_id, priority, "in-progress", &slug);
    let filepath = tasks_dir.join(&filename);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let branch_name = format!("task-{task_id}-{slug}");

    let task_content = format!(
        "---\n\
         created: {today}\n\
         priority: {priority}\n\
         status: in-progress\n\
         artifact: pending\n\
         ---\n\
         \n\
         # {title}\n\
         \n\
         ## Plan\n\
         \n\
         {plan}\n\
         \n\
         ## Progress\n\
         \n"
    );

    let mut file = std::fs::File::create(&filepath)
        .map_err(|e| format!("Failed to create task file {}: {e}", filepath.display()))?;
    file.write_all(task_content.as_bytes())
        .map_err(|e| format!("Failed to write task file: {e}"))?;
    tracing::info!(file = %filepath.display(), "Task file written");

    // 5. Create .phoenix/worktrees/ directory for the worktree.
    let phoenix_dir = cwd.join(".phoenix").join("worktrees");
    std::fs::create_dir_all(&phoenix_dir)
        .map_err(|e| format!("Failed to create .phoenix/worktrees/: {e}"))?;

    let worktree_path = phoenix_dir.join(conv_id);
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    // 6. Branch collision check
    let branch_exists = run_git(cwd, &["rev-parse", "--verify", &branch_name]).is_ok();
    if branch_exists {
        let merge_base = run_git(cwd, &["merge-base", "--is-ancestor", &branch_name, "HEAD"]);
        if merge_base.is_ok() {
            run_git(cwd, &["branch", "-d", &branch_name])?;
            tracing::info!(branch = %branch_name, "Deleted stale fully-merged branch");
        } else {
            return Err(format!(
                "Branch '{branch_name}' already exists and is not fully merged. \
                 Please resolve this manually before approving."
            ));
        }
    }

    if on_base_branch {
        // 7a. On base branch: commit task file to cwd first, then create branch + worktree.
        //     This ensures the branch (and worktree) include the task file.

        // Ensure .gitignore contains .phoenix/
        ensure_gitignore_has_phoenix(cwd)?;

        let relative_path = format!("tasks/{filename}");
        run_git(cwd, &["add", &relative_path])?;

        let commit_msg = format!("task {task_id}: {title}");
        if let Err(e) = run_git(cwd, &["commit", "-m", &commit_msg]) {
            let _ = run_git(cwd, &["reset", "HEAD"]);
            let _ = std::fs::remove_file(&filepath);
            return Err(format!("Failed to commit task file: {e}"));
        }
        tracing::info!(commit_msg = %commit_msg, "Task file committed on base branch");

        if let Err(e) = run_git(cwd, &["branch", &branch_name]) {
            let _ = run_git(cwd, &["reset", "--hard", "HEAD~1"]);
            return Err(format!("Failed to create branch '{branch_name}': {e}"));
        }

        if let Err(e) = run_git(cwd, &["worktree", "add", &worktree_path_str, &branch_name]) {
            let _ = run_git(cwd, &["branch", "-D", &branch_name]);
            let _ = run_git(cwd, &["reset", "--hard", "HEAD~1"]);
            return Err(format!("Failed to create worktree: {e}"));
        }
    } else {
        // 7b. Off base branch: create worktree + branch from desired base in one step,
        //     then write and commit the task file in the worktree.
        if let Err(e) = run_git(
            cwd,
            &[
                "worktree",
                "add",
                "-b",
                &branch_name,
                &worktree_path_str,
                &base_branch,
            ],
        ) {
            return Err(format!(
                "Failed to create worktree from base '{base_branch}': {e}"
            ));
        }

        // Remove the task file from cwd (it was written there for ID generation)
        // and re-create it in the worktree's tasks/ directory.
        let _ = std::fs::remove_file(&filepath);

        let wt_tasks_dir = worktree_path.join("tasks");
        std::fs::create_dir_all(&wt_tasks_dir)
            .map_err(|e| format!("Failed to create tasks/ in worktree: {e}"))?;

        let wt_filepath = wt_tasks_dir.join(&filename);
        let mut wt_file = std::fs::File::create(&wt_filepath)
            .map_err(|e| format!("Failed to create task file in worktree: {e}"))?;
        wt_file
            .write_all(task_content.as_bytes())
            .map_err(|e| format!("Failed to write task file in worktree: {e}"))?;

        // Ensure .gitignore contains .phoenix/ in the worktree
        ensure_gitignore_has_phoenix(&worktree_path)?;

        let relative_path = format!("tasks/{filename}");
        run_git(&worktree_path, &["add", &relative_path])?;

        let commit_msg = format!("task {task_id}: {title}");
        if let Err(e) = run_git(&worktree_path, &["commit", "-m", &commit_msg]) {
            let _ = run_git(cwd, &["worktree", "remove", &worktree_path_str, "--force"]);
            let _ = run_git(cwd, &["branch", "-D", &branch_name]);
            return Err(format!("Failed to commit task file in worktree: {e}"));
        }
        tracing::info!(commit_msg = %commit_msg, "Task file committed in worktree");
    }

    tracing::info!(
        branch = %branch_name,
        worktree = %worktree_path_str,
        on_base_branch,
        "Created git worktree"
    );

    Ok(TaskApprovalResult {
        task_id,
        task_title: title.to_string(),
        branch_name,
        first_task,
        worktree_path: worktree_path_str,
        base_branch,
    })
}

/// Ensure .gitignore in the given directory contains `.phoenix/`.
/// Creates .gitignore if it doesn't exist. Stages the change if modified.
pub(crate) fn ensure_gitignore_has_phoenix(dir: &std::path::Path) -> Result<(), String> {
    use std::io::Write as _;

    let gitignore_path = dir.join(".gitignore");
    let needs_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)
            .map_err(|e| format!("Failed to read .gitignore: {e}"))?;
        !content.lines().any(|line| line.trim() == ".phoenix/")
    } else {
        true
    };

    if needs_update {
        let needs_leading_newline = gitignore_path.exists()
            && std::fs::read(&gitignore_path)
                .ok()
                .is_some_and(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"));
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to open .gitignore: {e}"))?;
        if needs_leading_newline {
            writeln!(f).map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        }
        writeln!(f, ".phoenix/").map_err(|e| format!("Failed to write .gitignore: {e}"))?;
        run_git(dir, &["add", ".gitignore"])?;
        tracing::info!(dir = %dir.display(), "Added .phoenix/ to .gitignore");
    }

    Ok(())
}

/// Build the continuation prompt (REQ-BED-020)
fn build_continuation_prompt(rejected_tool_calls: &[ToolCall]) -> String {
    let mut prompt = String::from(
        "The conversation context is nearly full. Please provide a brief continuation summary \
        that could seed a new conversation.\n\n\
        Include:\n\
        1. Current task status and progress\n\
        2. Key files, concepts, or decisions discussed\n\
        3. Suggested next steps to continue the work\n\n\
        Keep your response concise and actionable.",
    );

    if !rejected_tool_calls.is_empty() {
        use std::fmt::Write;
        prompt.push_str(
            "\n\nNote: The following tool calls were requested but not executed due to context limits:\n",
        );
        for tool in rejected_tool_calls {
            let _ = writeln!(prompt, "- {}", tool.name());
        }
        prompt.push_str("Include these pending actions in your summary.");
    }

    prompt
}

fn llm_error_to_db_error(kind: crate::llm::LlmErrorKind) -> crate::db::ErrorKind {
    // Explicit match arms — no catch-all. The compiler enforces exhaustiveness.
    match kind {
        crate::llm::LlmErrorKind::Auth => crate::db::ErrorKind::Auth,
        crate::llm::LlmErrorKind::RateLimit => crate::db::ErrorKind::RateLimit,
        crate::llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        crate::llm::LlmErrorKind::ServerError => crate::db::ErrorKind::ServerError,
        crate::llm::LlmErrorKind::ContentFilter => crate::db::ErrorKind::ContentFilter,
        crate::llm::LlmErrorKind::ContextWindowExceeded => crate::db::ErrorKind::ContextExhausted,
    }
}

/// Convert an LLM error into a typed `LlmOutcome`.
/// Explicit match arms — the compiler enforces exhaustiveness.
fn llm_error_to_outcome(error: crate::llm::LlmError) -> LlmOutcome {
    use crate::llm::LlmErrorKind;
    match error.kind {
        LlmErrorKind::RateLimit => LlmOutcome::RateLimited { retry_after: None },
        LlmErrorKind::ServerError => LlmOutcome::ServerError {
            status: 500,
            body: error.message,
        },
        LlmErrorKind::Network => LlmOutcome::NetworkError {
            message: error.message,
        },
        LlmErrorKind::ContextWindowExceeded => LlmOutcome::TokenBudgetExceeded,
        LlmErrorKind::Auth => LlmOutcome::AuthError {
            message: error.message,
            recovery_in_progress: error.recovery_in_progress,
        },
        LlmErrorKind::InvalidRequest | LlmErrorKind::ContentFilter => LlmOutcome::RequestRejected {
            message: error.message,
        },
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;
    use crate::llm::LlmErrorKind;

    #[test]
    fn test_llm_error_to_db_error_mapping() {
        // Test all mappings are explicit and correct
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Auth),
            crate::db::ErrorKind::Auth
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::RateLimit),
            crate::db::ErrorKind::RateLimit
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Network),
            crate::db::ErrorKind::Network
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::InvalidRequest),
            crate::db::ErrorKind::InvalidRequest
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ServerError),
            crate::db::ErrorKind::ServerError,
            "ServerError must map to ServerError"
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContentFilter),
            crate::db::ErrorKind::ContentFilter
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContextWindowExceeded),
            crate::db::ErrorKind::ContextExhausted
        );
    }

    #[test]
    fn test_server_error_is_retryable_after_mapping() {
        // This is the critical test - ServerError from LLM must be retryable
        let llm_error = LlmErrorKind::ServerError;
        let db_error = llm_error_to_db_error(llm_error);
        assert!(
            db_error.is_retryable(),
            "ServerError must be retryable after mapping to db::ErrorKind"
        );
    }
}
