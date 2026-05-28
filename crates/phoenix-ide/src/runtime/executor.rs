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
use super::{
    SseBroadcaster, SseEvent, SubAgentCancelRequest, SubAgentSpawnRequest, TaskApprovalHandoffData,
    TaskApprovalHandoffRequest,
};

use crate::db::{MessageContent, ToolOutcome, ToolResult};
use crate::llm::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, ModelRegistry, PromptCacheKey, SystemContent,
};
use crate::state_machine::outcome::{EffectOutcome, LlmOutcome, ToolExecOutcome};
use crate::state_machine::state::ModeKind;
use crate::state_machine::state::{
    SubAgentMode, SubAgentOutcome, SubAgentResult, ToolCall, ToolInput,
};
use crate::state_machine::{
    handle_outcome, tool_result_message_id, transition, CheckpointData, ConvContext, ConvState,
    Effect, Event, StepResult,
};
use crate::system_prompt::{build_system_prompt, ModeContext};
use crate::tools::{BrowserSessionManager, ToolContext};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Safety-net wall-clock timeout for sub-agents (REQ-SA-006).
/// Primary enforcement is max turns (REQ-PROJ-008). This catches stuck tool execution.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_mins(20);

/// Map a producer-side [`crate::tools::ToolOutput`] onto a persisted
/// [`ToolOutcome`].
///
/// A total match on the `ToolOutput` enum — the structural payoff of making
/// it an enum: the variant, not an independently-settable `success: bool`,
/// decides the outcome, so a success can never be persisted as an error or
/// vice versa. There is deliberately no `Cancelled` arm — cancellation is
/// detected by the executor via the cancellation token before this mapping
/// is ever reached.
fn tool_output_to_outcome(out: crate::tools::ToolOutput) -> ToolOutcome {
    use crate::db::ToolContentImage;
    use crate::tools::{ToolImage, ToolOutput};
    let convert = |images: Vec<ToolImage>| -> Vec<ToolContentImage> {
        images
            .into_iter()
            .map(|img| ToolContentImage {
                media_type: img.media_type,
                data: img.data,
            })
            .collect()
    };
    match out {
        ToolOutput::Success {
            output,
            images,
            display_data,
        } => ToolOutcome::Success {
            output,
            display_data,
            images: convert(images),
        },
        ToolOutput::Error {
            output,
            images,
            display_data,
        } => ToolOutcome::Error {
            output,
            display_data,
            images: convert(images),
        },
    }
}

/// Decide whether `path` is inside the worktree rooted at `root`.
///
/// Three stages, each closing a class of bypass:
///
/// 1. Reject non-absolute paths. Relative overrides are ambiguous (no
///    defined resolution base) and not worth modelling.
/// 2. Canonicalise `root`. The worktree must exist; if it doesn't,
///    the comparison is meaningless and we reject (fail closed).
/// 3. Canonicalise the deepest existing ancestor of `path` and check
///    that ancestor lies inside the canonical root. `canonicalize`
///    resolves `..` and symlinks together (it's a `realpath` call),
///    so neither construct can escape:
///    `/worktree/../escape` -> ancestor `/worktree/..` -> canonical
///    `/parent_of_worktree` -> rejected.
///    `/worktree/escape/newdir` where `escape` symlinks to `/outside`
///    -> ancestor `/worktree/escape` -> canonical `/outside` ->
///    rejected.
///    Conversely, an in-worktree path that just happens to use `..`
///    to traverse internally (e.g. `/worktree/src/../tests`)
///    canonicalises back to a subpath of the worktree -> accepted.
fn path_is_within(path: &str, root: &str) -> bool {
    use std::path::{Path, PathBuf};
    let raw_path = Path::new(path);
    if !raw_path.is_absolute() {
        return false;
    }
    let Ok(canon_root) = std::fs::canonicalize(Path::new(root)) else {
        return false;
    };
    let mut anchor = PathBuf::from(raw_path);
    loop {
        if let Ok(canon) = std::fs::canonicalize(&anchor) {
            return canon == canon_root || canon.starts_with(&canon_root);
        }
        if !anchor.pop() {
            return false;
        }
    }
}

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
    /// Server clock at which the conversation entered `state` — initialised
    /// from the loaded `Conversation.state_updated_at` (or `Utc::now()` for
    /// fresh runtimes) and bumped to `Utc::now()` whenever `state` is
    /// reassigned by `apply_transition`. Carried on every `SseEvent::StateChange`
    /// emission so the client's elapsed-time display can derive
    /// `now() - state_updated_at` without any per-event timestamping
    /// (specs/working-phase-visibility/ REQ-WPV-001). The in-memory value
    /// is sub-millisecond ahead of the DB row's `state_updated_at` because
    /// the DB write uses its own `Utc::now()`; the drift is below the
    /// display's per-second rounding so the parity is preserved end-to-end.
    state_updated_at: DateTime<Utc>,
    storage: S,
    llm_client: Arc<L>,
    tool_executor: Arc<T>,
    /// Browser session manager for `ToolContext`
    browser_sessions: Arc<BrowserSessionManager>,
    /// Bash handle registry for `ToolContext` (REQ-BASH-014).
    bash_handles: Arc<crate::tools::BashHandleRegistry>,
    /// Tmux server registry for `ToolContext` (REQ-TMUX-013).
    tmux_registry: Arc<crate::tools::TmuxRegistry>,
    /// LLM registry for `ToolContext`
    llm_registry: Arc<ModelRegistry>,
    /// Active PTY terminal sessions — passed to `ToolContext` for `read_terminal` tool.
    terminals: crate::terminal::ActiveTerminals,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: SseBroadcaster,
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
    handoff_tx: Option<mpsc::Sender<TaskApprovalHandoffRequest>>,
    /// Buffer for `SubAgentResult` events received before entering `AwaitingSubAgents`.
    /// Pre-allocated with capacity = sub-agent count when spawning (FM-6 prevention).
    sub_agent_result_buffer: Vec<Event>,
    /// Steering messages queued while the conversation was busy. Delivered
    /// one-at-a-time (FIFO) when the conversation next enters `Idle`.
    /// Loaded from DB at executor startup; persisted back after each enqueue
    /// or dequeue.
    steering_queue: Vec<crate::state_machine::event::SteerEntry>,
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
        bash_handles: Arc<crate::tools::BashHandleRegistry>,
        tmux_registry: Arc<crate::tools::TmuxRegistry>,
        llm_registry: Arc<ModelRegistry>,
        terminals: crate::terminal::ActiveTerminals,
        event_rx: mpsc::Receiver<Event>,
        event_tx: mpsc::Sender<Event>,
        broadcast_tx: SseBroadcaster,
    ) -> Self {
        // Outcome channel for typed effect results.
        // Background tasks send typed outcomes (LlmOutcome, ToolOutcome, etc.)
        // through oneshot channels, then forwarders wrap them in EffectOutcome
        // for this unified channel.
        let (outcome_tx, outcome_rx) = mpsc::channel::<EffectOutcome>(64);

        Self {
            context,
            state,
            state_updated_at: Utc::now(),
            storage,
            llm_client: Arc::new(llm_client),
            tool_executor: Arc::new(tool_executor),
            browser_sessions,
            bash_handles,
            tmux_registry,
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
            handoff_tx: None,
            sub_agent_result_buffer: Vec::new(),
            steering_queue: Vec::new(),
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

    /// Override the initial `state_updated_at` from the loaded DB row so
    /// the very first `SseEvent::StateChange` after resume carries the
    /// real entry timestamp rather than the runtime-construction time.
    /// New (never-persisted) conversations don't call this — the
    /// constructor default (`Utc::now()`) is correct for them.
    pub fn with_state_updated_at(mut self, ts: DateTime<Utc>) -> Self {
        self.state_updated_at = ts;
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

    pub fn with_task_handoff_channel(
        mut self,
        handoff_tx: mpsc::Sender<TaskApprovalHandoffRequest>,
    ) -> Self {
        self.handoff_tx = Some(handoff_tx);
        self
    }

    /// Initialise the steering queue from a previously-persisted snapshot
    /// (loaded from `conversations.steering_queue` at executor startup).
    pub fn with_steering_queue(
        mut self,
        queue: Vec<crate::state_machine::event::SteerEntry>,
    ) -> Self {
        self.steering_queue = queue;
        self
    }

    #[allow(clippy::too_many_lines)] // Sequential event loop; splitting hurts readability
    pub async fn run(mut self) {
        tracing::info!(conv_id = %self.context.conversation_id, "Starting conversation runtime");

        // Check if we need to resume an interrupted operation
        // This handles crash recovery for in-flight LLM requests
        if let ConvState::LlmRequesting { .. } | ConvState::SeededLlmRequesting { .. } = &self.state
        {
            tracing::info!(conv_id = %self.context.conversation_id, "Resuming interrupted LLM request");
            if let Err(e) = self.execute_effect(Effect::RequestLlm).await {
                tracing::error!(error = %e, "Failed to resume LLM request");
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
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
                    // Eviction shutdown signal — exit cleanly so the broadcaster
                    // is dropped and connected SSE clients detect the closed
                    // stream and trigger a reconnect to the new runtime.
                    if matches!(event, Event::Shutdown) {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            "Runtime shutdown signal received; exiting executor loop"
                        );
                        return;
                    }
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

    /// Broadcast `ConversationBecameTerminal` to all SSE subscribers and clean
    /// up any lingering worktree.
    ///
    /// Send errors (no active receivers) are intentionally ignored.
    fn emit_terminal_lifecycle_event(&self) {
        self.cleanup_worktree_if_present();
        let _ = self
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationBecameTerminal { sequence_id: seq });
    }

    /// Remove the conversation's worktree if it still exists on disk.
    ///
    /// REQ-PROJ-028 creates worktrees at first message. If the user never
    /// approved a task (Explore mode), the worktree and temp branch leak.
    /// Work/Branch conversations clean up via mark-merged/abandon before
    /// reaching `ConvState::Terminal`, so this is a no-op for them in the
    /// normal case.
    ///
    /// REQ-BED-031 / approved-task handoff: `ContextExhausted` and
    /// `HandedOff` are terminal states whose worktree must be preserved for a
    /// successor conversation. Skip cleanup in those states; reconcile /
    /// abandon / mark-merged are the only paths permitted to remove the worktree.
    fn cleanup_worktree_if_present(&self) {
        if matches!(
            self.state,
            ConvState::ContextExhausted { .. } | ConvState::HandedOff { .. }
        ) {
            tracing::debug!(
                conv_id = %self.context.conversation_id,
                "skipping terminal worktree cleanup for successor-owned worktree"
            );
            return;
        }

        // Direct mode and legacy Managed have working_dir == repo_root, so fall
        // back to working_dir when the strict Phoenix-worktree predicate fails.
        let wd = &self.context.working_dir;
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(wd).unwrap_or_else(|| wd.clone());

        let worktree_path = repo_root
            .join(".phoenix")
            .join("worktrees")
            .join(&self.context.conversation_id);

        if worktree_path.exists() {
            let worktree_str = worktree_path.to_string_lossy().to_string();
            tracing::info!(worktree = %worktree_str, "Cleaning up worktree on terminal");
            let _ = crate::git_ops::run_git(
                &repo_root,
                &["worktree", "remove", &worktree_str, "--force"],
            );
        }
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

        // Steering messages are buffered rather than fed to the state machine.
        // They are delivered as `UserMessage` when the conversation next enters `Idle`.
        if let Event::SteerMessage {
            text,
            llm_text,
            images,
            message_id,
            user_agent,
            skill_invocation,
        } = event
        {
            let entry = crate::state_machine::event::SteerEntry {
                text,
                llm_text,
                images,
                message_id: message_id.clone(),
                user_agent,
                skill_invocation,
            };
            self.steering_queue.push(entry);
            let queue_position = self.steering_queue.len() - 1;
            // Persist updated queue
            if let Err(e) = self
                .storage
                .update_steering_queue(&self.context.conversation_id, &self.steering_queue)
                .await
            {
                tracing::warn!(error = %e, "Failed to persist steering queue");
            }
            // Notify the UI so it can show the queued indicator
            let _ = self
                .broadcast_tx
                .send_seq(|seq| SseEvent::SteerMessageQueued {
                    sequence_id: seq,
                    message_id,
                    queue_position,
                });
            return Ok(());
        }

        // Cancel steering: remove entry from in-memory queue.
        // DB is already updated by the cancel handler before this event arrives.
        if let Event::CancelSteerMessage { message_id } = event {
            let before = self.steering_queue.len();
            self.steering_queue.retain(|e| e.message_id != message_id);
            tracing::info!(
                conv_id = %self.context.conversation_id,
                %message_id,
                removed = before - self.steering_queue.len(),
                "Steering message cancelled in executor"
            );
            return Ok(());
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
                    let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                        sequence_id: seq,
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

        // Update state. Bump the entry timestamp on every reassignment so
        // every SseEvent::StateChange the executor subsequently emits
        // carries a fresh, server-authoritative state_updated_at
        // (specs/working-phase-visibility/ REQ-WPV-001). The DB write in
        // `persist_state_effect` uses its own `Utc::now()`; the resulting
        // sub-millisecond drift is below the client display's per-second
        // rounding.
        let old_state = std::mem::replace(&mut self.state, result.new_state.clone());
        self.state_updated_at = Utc::now();

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
                        | ConvState::HandedOff { .. }
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

        // Steering-queue drain: process synchronously so persist effects land
        // BEFORE any RequestLlm in the current effect list. This eliminates a
        // race where a mid-turn drain would persist steers concurrently with
        // (or after) the LLM task reading the DB; if the in-flight LLM then
        // returned a no-tool response, the conversation would settle to Idle
        // with the steers persisted but unanswered.
        if let Some(drain_event) = self.maybe_drain_steering_queue(&old_state) {
            self.run_effects_with_inline_drain(result.effects, drain_event, &mut generated_events)
                .await?;
        } else {
            for effect in result.effects {
                if let Some(gen_event) = self.execute_effect(effect).await? {
                    generated_events.push(gen_event);
                }
            }
        }

        Ok(generated_events)
    }

    /// Defer any `RequestLlm` in `original_effects`, run the rest, then process
    /// the drain event's persist effects inline, then run the deferred
    /// `RequestLlm`. Guarantees the spawned LLM task reads a DB that already
    /// contains the steered messages.
    async fn run_effects_with_inline_drain(
        &mut self,
        original_effects: Vec<Effect>,
        drain_event: Event,
        generated_events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let mut deferred_request_llm: Option<Effect> = None;
        // Cosmetic (task 60004): when the inline drain enters from Idle, the
        // original transition's Idle state-change SSE would briefly render the
        // conversation as Idle before the drain's Idle->LlmRequesting notify.
        // Persist the Idle state to the DB but suppress its broadcast (and any
        // explicit state_change notify); the drain emits the authoritative
        // LlmRequesting state-change. Mid-turn drains enter from LlmRequesting,
        // not Idle, so their state-change is correct and not suppressed.
        let suppress_intermediate_state_change = matches!(self.state, ConvState::Idle);
        for effect in original_effects {
            if matches!(effect, Effect::RequestLlm) {
                deferred_request_llm = Some(effect);
                continue;
            }
            if suppress_intermediate_state_change {
                match &effect {
                    Effect::PersistState => {
                        self.persist_state_effect(false).await?;
                        continue;
                    }
                    Effect::NotifyStateChange => {
                        continue;
                    }
                    _ => {}
                }
            }
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        let Event::SteerDrainedUserMessages { entries } = drain_event else {
            unreachable!("maybe_drain_steering_queue returns only SteerDrainedUserMessages")
        };
        let drain_result = transition(
            &self.state,
            &self.context,
            Event::SteerDrainedUserMessages { entries },
        )
        .map_err(|e| format!("steering drain transition failed: {e:?}"))?;
        self.state = drain_result.new_state;
        self.state_updated_at = Utc::now();
        for effect in drain_result.effects {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        if let Some(effect) = deferred_request_llm {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }
        Ok(())
    }

    /// Persist the current state to the DB. When `broadcast` is true, also
    /// emit the `StateChange` SSE; the inline-drain path persists with
    /// `broadcast = false` to suppress an intermediate Idle flicker (task
    /// 60004) since the drain emits its own authoritative state-change.
    async fn persist_state_effect(&mut self, broadcast: bool) -> Result<Option<Event>, String> {
        self.storage
            .update_state(&self.context.conversation_id, &self.state)
            .await?;

        if broadcast {
            let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                sequence_id: seq,
                state: self.state.clone(),
                presentation_mode: self.state.presentation_mode().to_string(),
                state_updated_at: self.state_updated_at,
            });
        }
        Ok(None)
    }

    /// If the current state transition is a steering-queue drain hook point and
    /// the queue is non-empty, drain all entries into a single
    /// `SteerDrainedUserMessages` event. The DB queue is NOT touched here; it
    /// is updated later by `Effect::ClearSteeringQueueEntries` once the emitted
    /// event is processed and all `PersistMessage` effects succeed.
    ///
    /// Sub-agents do not have steering queues; this returns `None` for them.
    ///
    /// Hook points (parent conversations only):
    /// - Entering `Idle` from any other state (turn complete; deliver steers
    ///   into the next LLM call).
    /// - Entering `LlmRequesting` from `ToolExecuting`/`AwaitingSubAgents` (mid-
    ///   turn; the prior transition already dispatched `RequestLlm`, so steers
    ///   land in the NEXT LLM call, not the in-flight one).
    fn maybe_drain_steering_queue(&mut self, old_state: &ConvState) -> Option<Event> {
        if self.context.is_sub_agent {
            return None;
        }

        let entering_idle =
            !matches!(old_state, ConvState::Idle) && matches!(self.state, ConvState::Idle);
        let entering_llm_requesting_from_tool_round =
            matches!(
                old_state,
                ConvState::ToolExecuting { .. } | ConvState::AwaitingSubAgents { .. }
            ) && matches!(self.state, ConvState::LlmRequesting { .. });

        if !(entering_idle || entering_llm_requesting_from_tool_round)
            || self.steering_queue.is_empty()
        {
            return None;
        }

        let entries = std::mem::take(&mut self.steering_queue);
        tracing::debug!(
            count = entries.len(),
            entering_idle,
            mid_turn = entering_llm_requesting_from_tool_round,
            "Draining all queued steering messages"
        );
        // DB queue is updated by `Effect::ClearSteeringQueueEntries` AFTER
        // persist effects run, so a crash mid-drain leaves the queue intact
        // for idempotent re-drain on restart.
        Some(Event::SteerDrainedUserMessages { entries })
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

        // Pre-allocate seq from the broadcaster so this message is ordered
        // strictly after any ephemeral events (tokens, state_change) emitted
        // earlier. See PersistBeforeBroadcast in specs/sse_wire/sse_wire.allium.
        let seq = self.broadcast_tx.next_seq();
        match self
            .storage
            .add_message_with_seq(
                &msg_id,
                &self.context.conversation_id,
                seq,
                &content,
                None,
                None,
            )
            .await
        {
            Ok(msg) => {
                let _ = self.broadcast_tx.send_message(msg);
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
        let parent_allows_work = match self.context.mode_context.as_ref() {
            Some(ModeContext::Work { .. } | ModeContext::Direct | ModeContext::Branch { .. }) => {
                true
            }
            Some(ModeContext::Explore) | None => false,
        };

        let mut work_count_in_batch = 0u32;
        for task in &input.tasks {
            let mode = task.mode.unwrap_or_default();
            if mode == SubAgentMode::Work {
                if !parent_allows_work {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        "Work sub-agents require the parent to be in a write-capable mode \
                         (Work, Branch, or Direct). Use mode: \"explore\" or omit mode \
                         for read-only sub-agents."
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

        // cwd-scoping guard (REQ-PROJ-008): a Work sub-agent's overridden
        // `cwd` must stay inside the parent's worktree. Without this guard
        // a Work sub-agent could write outside the worktree because its
        // own runtime would see a different working_dir than the parent.
        // Direct parents have no worktree to scope against -- writes there
        // are unscoped by design -- so the check only fires for parents
        // that own a worktree (Work/Branch).
        let parent_worktree_path: Option<&str> = match self.context.mode_context.as_ref() {
            Some(
                ModeContext::Work { worktree_path, .. } | ModeContext::Branch { worktree_path, .. },
            ) => Some(worktree_path.as_str()),
            _ => None,
        };
        if let Some(worktree_root) = parent_worktree_path {
            for task in &input.tasks {
                let mode = task.mode.unwrap_or_default();
                if mode != SubAgentMode::Work {
                    continue;
                }
                let Some(override_cwd) = task.cwd.as_deref() else {
                    continue;
                };
                if !path_is_within(override_cwd, worktree_root) {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!(
                            "Work sub-agent cwd '{override_cwd}' must be inside the parent's \
                             worktree '{worktree_root}'. Omit `cwd` to inherit the worktree, \
                             or pass an absolute path that resolves under it."
                        ),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
            }
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
        let result = ToolResult::success(tool_use_id.clone(), output);

        // Send SpawnAgentsComplete event (synchronously returned, not async)
        Ok(Some(Event::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        }))
    }

    /// Execute an effect and optionally return a generated event
    #[allow(clippy::too_many_lines)]
    async fn execute_effect(&mut self, effect: Effect) -> Result<Option<Event>, String> {
        match effect {
            Effect::PersistMessage {
                content,
                display_data,
                usage_data,
                message_id,
                idempotent,
            } => {
                // Idempotent path: skip if already persisted. Prevents double-
                // insert (and seq gap) when a SteerDrainedUserMessages re-fires
                // after crash recovery before ClearSteeringQueueEntries ran.
                // Gated to idempotent=true so non-replayable persists pay no
                // extra query.
                if idempotent && self.storage.message_exists(&message_id).await? {
                    tracing::debug!(
                        message_id = %message_id,
                        "Skipping PersistMessage; message already exists"
                    );
                    return Ok(None);
                }

                let seq = self.broadcast_tx.next_seq();
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &message_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        display_data.as_ref(),
                        usage_data.as_ref(),
                    )
                    .await?;

                // Broadcast to clients (display_data already computed at effect creation)
                let _ = self.broadcast_tx.send_message(msg);
                Ok(None)
            }

            Effect::PersistState => self.persist_state_effect(true).await,

            Effect::RequestLlm => self.dispatch_llm_request().await,

            Effect::ExecuteTool { tool } => self.dispatch_tool_execution(tool).await,

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

            Effect::NotifyAgentDone => {
                let _ = self
                    .broadcast_tx
                    .send_seq(|seq| SseEvent::AgentDone { sequence_id: seq });
                Ok(None)
            }

            Effect::NotifyStateChange => {
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                    sequence_id: seq,
                    state: self.state.clone(),
                    presentation_mode: self.state.presentation_mode().to_string(),
                    state_updated_at: self.state_updated_at,
                });
                Ok(None)
            }

            Effect::PersistCheckpoint { data } => self.persist_checkpoint(data).await,

            Effect::BroadcastAssistantMessage { message } => {
                // Broadcast-only: no DB write here. The atomic
                // `PersistCheckpoint` at the end of the tool round performs
                // the durable write (and emits a duplicate `sse_message`
                // that the UI dedups by `message_id`).
                //
                // The eager message is appended to the per-conversation
                // ReplayRing via `send_ephemeral_message` so reconnecting
                // clients during the tool round still see the in-flight
                // assistant content. The eventual persisted Message with
                // the same `message_id` will fire `send_persisted_message`
                // when the checkpoint completes, resetting the ring anchor
                // and discarding this entry; the client dedups by
                // `message_id` via `SseMessageDedupReplay`.
                //
                // `created_at` comes from `AssistantMessage` (captured at
                // LLM-response time) — NOT a fresh `Utc::now()`. The same
                // timestamp is later written to the DB row by
                // `persist_checkpoint`, so a reconnecting client's init
                // payload reads back the same timestamp the UI is already
                // displaying. Without that alignment, the displayed
                // timestamp would jump when init merges the DB row in.
                let seq = self.broadcast_tx.next_seq();
                let agent_content = MessageContent::agent(message.content);
                let db_msg = crate::db::Message {
                    message_id: message.message_id,
                    conversation_id: self.context.conversation_id.clone(),
                    sequence_id: seq,
                    message_type: agent_content.message_type(),
                    content: agent_content,
                    display_data: message.display_data,
                    usage_data: message.usage,
                    created_at: message.created_at,
                };
                let _ = self.broadcast_tx.send_ephemeral_message(db_msg);
                Ok(None)
            }

            Effect::PersistToolResults { results } => {
                for result in results {
                    let content = MessageContent::tool_with_images(
                        &result.tool_use_id,
                        result.output(),
                        result.is_error(),
                        result.images().to_vec(),
                    );
                    let tool_msg_id = uuid::Uuid::new_v4().to_string();
                    let seq = self.broadcast_tx.next_seq();
                    let msg = self
                        .storage
                        .add_message_with_seq(
                            &tool_msg_id,
                            &self.context.conversation_id,
                            seq,
                            &content,
                            None,
                            None,
                        )
                        .await?;

                    // Tool results don't contain bash tool_use blocks, no enrichment needed
                    let _ = self.broadcast_tx.send_message(msg);
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

            Effect::PersistHiddenSystemMarker { marker, message_id } => {
                let seq = self.broadcast_tx.next_seq();
                let content = MessageContent::system(marker);
                let display_data = Some(serde_json::json!({ "hidden": true }));
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &message_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        display_data.as_ref(),
                        None,
                    )
                    .await?;

                let _ = self.broadcast_tx.send_message(msg);
                Ok(None)
            }

            Effect::PersistSubAgentResults {
                results,
                spawn_tool_id,
            } => self.persist_sub_agent_results(results, spawn_tool_id).await,

            Effect::RequestContinuation {
                rejected_tool_calls,
            } => {
                // REQ-BED-020: Request continuation summary (tool-less LLM request)
                self.request_continuation(rejected_tool_calls);
                Ok(None)
            }

            Effect::NotifyContextExhausted { summary } => {
                // REQ-BED-021 / REQ-BED-031: Notify client of context
                // exhaustion. Worktree is intentionally preserved (no
                // auto-cleanup, no conv_mode demotion) — continuation
                // transfer (REQ-BED-030) or a user-initiated abandon /
                // mark-as-merged is the only path that removes it.
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                    sequence_id: seq,
                    state: ConvState::ContextExhausted { summary },
                    presentation_mode: "needs_action".to_string(),
                    state_updated_at: self.state_updated_at,
                });
                Ok(None)
            }

            Effect::ApproveTask {
                task_file,
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task(task_file, title, priority, plan)
                    .await?;
                Ok(None)
            }

            Effect::ApproveTaskFreshHandoff {
                task_file,
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task_fresh_handoff(task_file, title, priority, plan)
                    .await
            }

            Effect::ResolveTask {
                system_message,
                repo_root,
            } => {
                self.execute_resolve_task(system_message, repo_root).await?;
                Ok(None)
            }

            Effect::ClearSteeringQueueEntries { message_ids } => {
                if let Err(e) = self
                    .storage
                    .remove_steering_entries(&self.context.conversation_id, &message_ids)
                    .await
                {
                    tracing::warn!(error = %e, "Failed to remove drained steering entries");
                }
                Ok(None)
            }
        }
    }

    /// Dispatch an LLM request: enforce turn/cycle caps, inject grace-turn
    /// messages, build the streaming pipeline, and spawn the LLM task.
    #[allow(clippy::too_many_lines)]
    async fn dispatch_llm_request(&mut self) -> Result<Option<Event>, String> {
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
                let content = MessageContent::User(crate::db::UserContent::meta(
                    "You have reached your turn limit. Please call submit_result now \
                         with whatever findings you have so far. Do not call any other tools.",
                ));
                if let Err(e) = self
                    .storage
                    .add_message(&msg_id, &self.context.conversation_id, &content, None, None)
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
        // physically cannot send a ToolExecOutcome or other type.
        let (llm_tx, llm_rx) = oneshot::channel::<LlmOutcome>();
        let outcome_tx = self.outcome_tx.clone();

        let llm_client = self.llm_client.clone();
        let tool_executor = self.tool_executor.clone();
        let storage = self.storage.clone();
        let conv_id = self.context.conversation_id.clone();
        let root_conv_id = self.context.root_conversation_id.clone();
        let model_id = self.context.model_id.clone();
        let working_dir = self.context.working_dir.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();
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
            // REQ-WPV-007: emit `SseEvent::LlmFirstByte` exactly once
            // per request, immediately before the first `Token` event
            // for the same `request_id`. The two events get
            // consecutive sequence_ids from the same broadcaster, so
            // the client cannot observe a Token without first
            // observing the marker. If this request completes with
            // zero text chunks (the LLM errored or terminated before
            // emitting any), `LlmFirstByte` is NOT emitted.
            let mut first_text_seen = false;
            loop {
                match rx.recv().await {
                    Ok(crate::llm::TokenChunk::Text(text)) => {
                        if !first_text_seen {
                            first_text_seen = true;
                            let request_id_for_first = request_id_for_fwd.clone();
                            let _ =
                                broadcast_tx_for_tokens.send_seq(|seq| SseEvent::LlmFirstByte {
                                    sequence_id: seq,
                                    request_id: request_id_for_first,
                                });
                        }
                        let _ = broadcast_tx_for_tokens.send_seq(|seq| SseEvent::Token {
                            sequence_id: seq,
                            text,
                            request_id: request_id_for_fwd.clone(),
                        });
                    }
                    Ok(crate::llm::TokenChunk::RateLimitSnapshot(snapshot)) => {
                        let _ =
                            broadcast_tx_for_tokens.send_seq(|seq| SseEvent::RateLimitSnapshot {
                                sequence_id: seq,
                                snapshot: snapshot.clone(),
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
            let system_prompt = build_system_prompt(
                &working_dir,
                &tasks_dir_name,
                is_sub_agent,
                mode_context.as_ref(),
            );

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
                // Every turn in a conversation reuses the same prefix
                // (system prompt + earlier turns), so all turns share one key.
                cache_key: PromptCacheKey::stable(&conv_id),
            };

            // Use streaming — chunk_tx forwards text tokens to SSE clients.
            let llm_outcome = match llm_client.complete_streaming(&request, &chunk_tx).await {
                Ok(response) => {
                    // Extract tool calls from content and convert to typed ToolCall
                    let tool_calls: Vec<ToolCall> = response
                        .tool_uses()
                        .into_iter()
                        .map(|(id, name, input)| {
                            let typed_input = ToolInput::from_name_and_value(name, input.clone());
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

                    // Fire-and-forget: persist token usage for this turn.
                    // Errors are logged and do not affect the conversation.
                    let storage_for_usage = storage.clone();
                    let conv_id_for_usage = conv_id.clone();
                    let root_id_for_usage = root_conv_id.clone();
                    let model_for_usage = model_id.clone();
                    let usage_for_insert = usage.clone();
                    tokio::spawn(async move {
                        if let Err(e) = storage_for_usage
                            .insert_turn_usage(
                                &conv_id_for_usage,
                                &root_id_for_usage,
                                &model_for_usage,
                                &usage_for_insert,
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "failed to write turn_usage row");
                        }
                    });

                    LlmOutcome::Response {
                        content: response.content,
                        tool_calls,
                        end_turn: response.end_turn,
                        usage: response.usage,
                        request_id: request_id.clone(),
                    }
                }
                Err(e) => llm_error_to_outcome(e),
            };

            // Task 67004: a terminal UsageLimitReached carries the
            // structured QuotaDetails parsed from the 429 response
            // headers. Replay it through the chunk channel as a
            // `RateLimitSnapshot` so the codex quota store sees the
            // limit-hit state — same channel the 200 path uses to push
            // per-turn snapshots (see `llm/openai.rs`
            // `complete_streaming`). The ErrorBanner reads from the
            // same store to render reset/credits/promo alongside the
            // plan-aware message.
            if let LlmOutcome::UsageLimitReached { ref details, .. } = llm_outcome {
                let _ = chunk_tx.send(crate::llm::TokenChunk::RateLimitSnapshot(details.clone()));
            }

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

    /// Dispatch tool execution: resolve the tool, build the execution context,
    /// spawn the background task, and wire up the outcome channel.
    #[allow(clippy::too_many_lines)]
    async fn dispatch_tool_execution(&mut self, tool: ToolCall) -> Result<Option<Event>, String> {
        // Special handling for spawn_agents tool
        if tool.name() == "spawn_agents" {
            return self.handle_spawn_agents_tool(tool).await;
        }

        // Typed oneshot channel: background task gets Sender<ToolExecOutcome>,
        // physically cannot send an LlmOutcome or other type.
        let (tool_tx, tool_rx) = oneshot::channel::<ToolExecOutcome>();
        let outcome_tx = self.outcome_tx.clone();

        // Create cancellation token for this tool execution
        let cancel_token = CancellationToken::new();
        self.tool_cancel_token = Some(cancel_token.clone());
        let cancel_token_check = cancel_token.clone();

        // Create ToolContext for this invocation
        // worktree_path: Some for Managed/Branch (working_dir IS the worktree),
        // None for Direct (no worktree — socket keyed to conv_id). Task 03001.
        let tmux_worktree =
            (self.context.mode != ModeKind::Direct).then(|| self.context.working_dir.clone());
        let tool_ctx = ToolContext::new(
            cancel_token,
            self.context.conversation_id.clone(),
            self.context.working_dir.clone(),
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            self.tmux_registry.clone(),
            tmux_worktree,
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
                ToolExecOutcome::Aborted {
                    tool_use_id,
                    reason: crate::state_machine::AbortReason::CancellationRequested,
                }
            } else if let Some(out) = output {
                let duration_ms =
                    u64::try_from(tool_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                tracing::info!(
                    conv_id = %conv_id,
                    tool = %tool_name,
                    id = %tool_use_id,
                    duration_ms,
                    success = out.is_success(),
                    "Tool completed"
                );
                let outcome = tool_output_to_outcome(out);
                ToolExecOutcome::Completed(ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    outcome,
                    duration_ms: Some(duration_ms),
                })
            } else {
                tracing::warn!(
                    conv_id = %conv_id,
                    tool = %tool_name,
                    id = %tool_use_id,
                    "Tool not found"
                );
                ToolExecOutcome::Failed {
                    tool_use_id,
                    error: format!("Unknown tool: {tool_name}"),
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

    /// Persist a checkpoint (assistant message + tool results) atomically.
    async fn persist_checkpoint(&mut self, data: CheckpointData) -> Result<Option<Event>, String> {
        match data {
            CheckpointData::ToolRound {
                assistant_message,
                tool_results,
            } => {
                // Persist assistant message.
                //
                // Uses `add_message_with_seq_at` so the DB row carries the
                // exact `assistant_message.created_at` value the eager
                // broadcast (`Effect::BroadcastAssistantMessage`) already
                // delivered for this `message_id`. Single INSERT, no
                // transient `Utc::now()` value visible to a concurrent
                // reconnect's init read. Without this, a reconnect that
                // landed between the INSERT and a follow-up UPDATE would
                // see a shifted timestamp on the same message_id the UI
                // is already displaying.
                let agent_content = MessageContent::agent(assistant_message.content);
                let agent_seq = self.broadcast_tx.next_seq();
                let agent_msg = self
                    .storage
                    .add_message_with_seq_at(
                        &assistant_message.message_id,
                        &self.context.conversation_id,
                        agent_seq,
                        &agent_content,
                        assistant_message.display_data.as_ref(),
                        assistant_message.usage.as_ref(),
                        assistant_message.created_at,
                    )
                    .await?;
                let _ = self.broadcast_tx.send_message(agent_msg);

                // Persist all tool results
                for result in tool_results {
                    let tool_content = MessageContent::tool_with_images(
                        &result.tool_use_id,
                        result.output(),
                        result.is_error(),
                        result.images().to_vec(),
                    );
                    let tool_msg_id = tool_result_message_id(&result.tool_use_id);
                    let tool_seq = self.broadcast_tx.next_seq();
                    let merged_display =
                        merge_duration_into_display_data(result.display_data(), result.duration_ms);
                    let tool_msg = self
                        .storage
                        .add_message_with_seq(
                            &tool_msg_id,
                            &self.context.conversation_id,
                            tool_seq,
                            &tool_content,
                            merged_display.as_ref(),
                            None,
                        )
                        .await?;
                    let _ = self.broadcast_tx.send_message(tool_msg.clone());
                    // Emit a typed `MessageUpdated` so the live-connection reducer
                    // can populate `display_data.duration_ms` without parsing the
                    // opaque `display_data` blob. Reconnect paths read from the
                    // DB where `duration_ms` is already baked into `display_data`
                    // by `merge_duration_into_display_data` above.
                    if result.duration_ms.is_some() {
                        let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
                            sequence_id: seq,
                            message_id: tool_msg.message_id.clone(),
                            display_data: None,
                            content: None,
                            duration_ms: result.duration_ms,
                        });
                    }
                }
            }
        }
        Ok(None)
    }

    /// Persist aggregated sub-agent results: update the `spawn_agents` message
    /// content and `display_data`, or create a standalone summary message.
    async fn persist_sub_agent_results(
        &mut self,
        results: Vec<SubAgentResult>,
        spawn_tool_id: Option<String>,
    ) -> Result<Option<Event>, String> {
        // Build the display_data for subagent results
        let display_data = serde_json::json!({
            "type": "subagent_summary",
            "results": results
        });

        // If we have a spawn_tool_id, update its message's content (for LLM history)
        // and display_data (for UI).
        if let Some(tool_id) = spawn_tool_id {
            let message_id = tool_result_message_id(&tool_id);

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

            // Both writes must succeed before broadcasting. Otherwise the client
            // would see state the DB can't corroborate on reconnect (full resync
            // from DB would revert the UI to stale values).
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
                return Ok(None);
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
                return Ok(None);
            }

            let updated_content = crate::db::MessageContent::tool(&tool_id, &llm_content, false);
            let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
                sequence_id: seq,
                message_id: message_id.clone(),
                display_data: Some(display_data.clone()),
                content: Some(updated_content),
                duration_ms: None,
            });
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
            let seq = self.broadcast_tx.next_seq();
            let message = self
                .storage
                .add_message_with_seq(
                    &msg_id,
                    &self.context.conversation_id,
                    seq,
                    &content,
                    Some(&display_data),
                    None,
                )
                .await?;

            // Broadcast the new message (tool message, no bash enrichment needed)
            let _ = self.broadcast_tx.send_message(message);
        }

        Ok(None)
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
            let messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build messages for continuation");
                    let _ = event_tx.send(Event::ContinuationFailed { error: e }).await;
                    return;
                }
            };

            // The continuation request is tool-less: strip every tool-related
            // block (regular ToolUse/ToolResult, server-handled ServerToolUse,
            // ToolSearchToolResult, MCP, etc.) so the API doesn't reject the
            // request because history references tools we're no longer
            // declaring. The model still has the assistant's narration to
            // summarize from. Rejected tool calls are described in prose in
            // the continuation prompt instead of synthetic tool_result blocks.
            let mut messages = strip_all_tool_blocks(messages);

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
                // Same conversation as the main loop — different system
                // prompt won't share a prefix in practice, but using the
                // conv id keeps the cache cohort coherent.
                cache_key: PromptCacheKey::stable(&conv_id),
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

    /// Handle task resolution: finalize conversation state/mode/cwd, inject system message,
    /// and broadcast SSE events. Called after git operations have already completed.
    async fn execute_resolve_task(
        &mut self,
        system_message: String,
        repo_root: String,
    ) -> Result<(), String> {
        let conv_id = &self.context.conversation_id;

        // Update state. Mode is preserved (Branch stays Branch, Work stays Work).
        // Bump the in-memory state_updated_at to match the DB write so the
        // Terminal SseEvent::StateChange below carries the correct entry
        // time (REQ-WPV-001). `self.state` itself is not mutated because the
        // runtime is about to exit after this function returns.
        self.storage
            .update_state(conv_id, &ConvState::Terminal)
            .await?;
        self.state_updated_at = Utc::now();
        // Legitimate cwd mutation (task 13012, teardown fallback): the
        // worktree is gone by this point, but API handlers (search_files,
        // list_skills, list_tasks, get_system_prompt) read conv.cwd for
        // terminal conversations without a state guard. Resetting to
        // repo_root gives them a valid directory rather than a deleted
        // worktree path.
        self.storage
            .update_conversation_cwd_recovery_only(conv_id, &repo_root)
            .await?;

        // Inject system message
        let msg_id = uuid::Uuid::new_v4().to_string();
        let seq = self.broadcast_tx.next_seq();
        let msg = self
            .storage
            .add_message_with_seq(
                &msg_id,
                conv_id,
                seq,
                &MessageContent::system(&system_message),
                None,
                None,
            )
            .await?;

        // Broadcast SSE events
        let _ = self.broadcast_tx.send_message(msg);
        let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            state: ConvState::Terminal,
            presentation_mode: ConvState::Terminal.presentation_mode().to_string(),
            state_updated_at: self.state_updated_at,
        });
        let _ = self
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationUpdate {
                sequence_id: seq,
                update: crate::runtime::ConversationMetadataUpdate {
                    cwd: Some(repo_root),
                    branch_name: None,
                    worktree_path: None,
                    conv_mode_label: None,
                    base_branch: None,
                    task_title: None,
                },
            });

        Ok(())
    }

    /// REQ-BED-028: Execute git operations for task approval.
    ///
    /// Sequence: parse on-disk task file -> create worktree (or promote early one) ->
    /// rename status to in-progress if needed -> git commit -> update `conv_mode`.
    ///
    /// On failure: revert in-memory state to `AwaitingTaskApproval` so the user can retry.
    /// Collision check on retry handles partial state.
    #[allow(clippy::too_many_lines)] // NonEmptyString construction adds wrapping lines
    async fn execute_approve_task(
        &mut self,
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    ) -> Result<(), String> {
        let cwd = self.context.working_dir.clone();
        // The spec invariant WorktreePathDerivedFromConversation requires
        // the worktree path to be rooted at the repo root, not at cwd.
        // For Managed conversations cwd IS the Explore worktree; for legacy
        // pre-REQ-PROJ-028 Managed conversations cwd IS already the repo root.
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(&cwd).unwrap_or_else(|| cwd.clone());
        let conv_id = self.context.conversation_id.clone();
        let desired_base_branch = self.context.desired_base_branch.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();
        let storage = self.storage.clone();

        // Clone for state revert on failure (originals moved into spawn_blocking)
        let task_file_backup = task_file.clone();
        let title_backup = title.clone();
        let priority_backup = priority;
        let plan_backup = plan.clone();

        // Run blocking git/fs operations on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(
                &cwd,
                &repo_root,
                &conv_id,
                &tasks_dir_name,
                &task_file,
                &title,
                desired_base_branch.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        match result {
            Ok(approval_result) => {
                // Update conversation mode to Work (includes worktree_path, base_branch, task_number)
                let work_mode = crate::db::ConvMode::Work {
                    branch_name: crate::db::NonEmptyString::new(
                        approval_result.branch_name.clone(),
                    )
                    .expect("branch_name from task approval must be non-empty"),
                    worktree_path: crate::db::NonEmptyString::new(
                        approval_result.worktree_path.clone(),
                    )
                    .expect("worktree_path from task approval must be non-empty"),
                    base_branch: crate::db::NonEmptyString::new(
                        approval_result.base_branch.clone(),
                    )
                    .expect("base_branch from task approval must be non-empty"),
                    task_id: crate::db::NonEmptyString::new(approval_result.task_id.clone())
                        .expect("task_id from task approval must be non-empty"),
                    task_title: crate::db::NonEmptyString::new(approval_result.task_title.clone())
                        .expect("task_title from task approval must be non-empty"),
                };
                storage
                    .update_conversation_mode(&self.context.conversation_id, &work_mode)
                    .await?;

                // Legitimate cwd mutation (task 13012, in-place promotion).
                // For Managed conversations (REQ-PROJ-028): the early Explore
                // worktree is promoted in place (branch rename, same path), so
                // this write is a no-op — worktree_path == conv.cwd already.
                // For legacy Managed conversations whose cwd was the repo root,
                // this is load-bearing: it moves cwd to the new worktree path.
                storage
                    .update_conversation_cwd_recovery_only(
                        &self.context.conversation_id,
                        &approval_result.worktree_path,
                    )
                    .await?;
                self.context.working_dir = std::path::PathBuf::from(&approval_result.worktree_path);

                // Refresh in-memory mode_context so downstream checks
                // (e.g. spawn_agents Work-parent guard) observe Work mode
                // for the rest of this runtime's lifetime. Without this,
                // mode_context stays the Explore value set at runtime start.
                self.context.mode_context = Some(ModeContext::Work {
                    branch_name: approval_result.branch_name.clone(),
                    base_branch: approval_result.base_branch.clone(),
                    worktree_path: approval_result.worktree_path.clone(),
                });

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
                let seq = self.broadcast_tx.next_seq();
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &msg_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        None,
                        None,
                    )
                    .await?;
                let _ = self.broadcast_tx.send_message(msg);

                // Push updated conversation metadata to the client so it
                // reflects the new cwd, branch, worktree_path, and mode label
                // without requiring a reconnect.
                let _ = self
                    .broadcast_tx
                    .send_seq(|seq| SseEvent::ConversationUpdate {
                        sequence_id: seq,
                        update: crate::runtime::ConversationMetadataUpdate {
                            cwd: Some(approval_result.worktree_path.clone()),
                            branch_name: Some(approval_result.branch_name.clone()),
                            worktree_path: Some(approval_result.worktree_path.clone()),
                            conv_mode_label: Some("Work".to_string()),
                            base_branch: Some(approval_result.base_branch.clone()),
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
                    task_file: task_file_backup,
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };
                self.state_updated_at = Utc::now();

                // Broadcast an error so the UI knows, but don't propagate — the
                // conversation stays in AwaitingTaskApproval for retry.
                // Task 24682: use the typed UserFacingError. `e` is the
                // approval-pipeline error (Display-formatted, no Debug leak)
                // so it's safe to inline as the human detail.
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
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
    async fn execute_approve_task_fresh_handoff(
        &mut self,
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    ) -> Result<Option<Event>, String> {
        let cwd = self.context.working_dir.clone();
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(&cwd).unwrap_or_else(|| cwd.clone());
        let conv_id = self.context.conversation_id.clone();
        let desired_base_branch = self.context.desired_base_branch.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();

        let task_file_backup = task_file.clone();
        let title_backup = title.clone();
        let priority_backup = priority;
        let plan_backup = plan.clone();

        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(
                &cwd,
                &repo_root,
                &conv_id,
                &tasks_dir_name,
                &task_file,
                &title,
                desired_base_branch.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        let approval_result = match result {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "Fresh task approval git operations failed");
                self.state = ConvState::AwaitingTaskApproval {
                    task_file: task_file_backup,
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };
                self.state_updated_at = Utc::now();
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
                    error: crate::runtime::user_facing_error::UserFacingError::retryable(
                        "Task approval failed",
                        format!(
                            "Phoenix could not finalise the task: {e}. The conversation \
                             stays in approval state — try approving again or abandon."
                        ),
                    ),
                });
                return Ok(None);
            }
        };

        let Some(handoff_tx) = &self.handoff_tx else {
            return Err(
                "fresh task approval unavailable: runtime handoff channel missing".to_string(),
            );
        };
        let (response_tx, response_rx) = oneshot::channel();
        let request = TaskApprovalHandoffRequest {
            parent_conversation_id: self.context.conversation_id.clone(),
            approval: TaskApprovalHandoffData {
                task_id: approval_result.task_id,
                task_title: approval_result.task_title,
                branch_name: approval_result.branch_name,
                worktree_path: approval_result.worktree_path,
                base_branch: approval_result.base_branch,
                title: title_backup,
                priority: priority_backup,
                plan: plan_backup,
                task_file: approval_result.task_file,
            },
            response_tx,
        };
        handoff_tx
            .send(request)
            .await
            .map_err(|e| format!("failed to request fresh task handoff: {e}"))?;
        let response = response_rx
            .await
            .map_err(|e| format!("fresh task handoff response dropped: {e}"))??;
        Ok(Some(Event::TaskHandoffComplete {
            successor_conv_id: response.successor_conv_id,
        }))
    }
}

/// Result of a successful task approval
struct TaskApprovalResult {
    task_id: String,
    task_title: String,
    branch_name: String,
    first_task: bool,
    task_file: String,
    /// Absolute path to the git worktree created for this conversation
    worktree_path: String,
    /// The branch that was checked out when the task was approved (merge target)
    base_branch: String,
}

/// Drop every tool-related block from the message history.
///
/// Used by the continuation summary path: that request is sent with
/// `tools: []`, so any `tool_use`, `tool_result`, `server_tool_use`,
/// `tool_search_tool_result`, or MCP block in history would cause the
/// API to 400 with "Tool reference X not found in available tools".
/// The summary still has the assistant's text narration to work with.
fn strip_all_tool_blocks(messages: Vec<LlmMessage>) -> Vec<LlmMessage> {
    use crate::llm::ContentBlock;

    messages
        .into_iter()
        .map(|msg| {
            let filtered: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { .. } | ContentBlock::Image { .. }
                    )
                })
                .collect();
            LlmMessage {
                role: msg.role,
                content: filtered,
            }
        })
        .filter(|msg| !msg.content.is_empty())
        .collect()
}

/// Merge a `duration_ms` value into an existing `display_data` JSON blob.
///
/// If `duration_ms` is `None`, returns a clone of the existing data unchanged.
/// If `display_data` is `None`, returns `{ "duration_ms": ms }` when a
/// duration is present. If both are `Some`, inserts `duration_ms` into the
/// existing object without overwriting any tool-specific fields.
fn merge_duration_into_display_data(
    existing: Option<&serde_json::Value>,
    duration_ms: Option<u64>,
) -> Option<serde_json::Value> {
    match (existing, duration_ms) {
        (None, None) => None,
        (Some(v), None) => Some(v.clone()),
        (None, Some(ms)) => Some(serde_json::json!({ "duration_ms": ms })),
        (Some(v), Some(ms)) => {
            let mut merged = v.clone();
            if let Some(obj) = merged.as_object_mut() {
                obj.insert(
                    "duration_ms".to_string(),
                    serde_json::Value::Number(ms.into()),
                );
            }
            Some(merged)
        }
    }
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

#[cfg(test)]
mod strip_tool_blocks_tests {
    use super::*;
    use crate::llm::{
        ContentBlock, ImageSource, LlmMessage, MessageRole, ToolReference, ToolSearchResultContent,
    };

    fn user_text(s: &str) -> LlmMessage {
        LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(s)],
        }
    }

    fn assistant(blocks: Vec<ContentBlock>) -> LlmMessage {
        LlmMessage {
            role: MessageRole::Assistant,
            content: blocks,
        }
    }

    fn user(blocks: Vec<ContentBlock>) -> LlmMessage {
        LlmMessage {
            role: MessageRole::User,
            content: blocks,
        }
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "ok".into(),
            images: vec![],
            is_error: false,
        }
    }

    fn server_tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ServerToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn tool_search_result(tool_use_id: &str, refs: &[&str]) -> ContentBlock {
        ContentBlock::ToolSearchToolResult {
            tool_use_id: tool_use_id.into(),
            content: ToolSearchResultContent {
                r#type: "tool_search_tool_search_result".into(),
                tool_references: refs
                    .iter()
                    .map(|n| ToolReference {
                        r#type: "tool_reference".into(),
                        tool_name: (*n).into(),
                    })
                    .collect(),
                error_code: None,
            },
        }
    }

    // ----- strip_all_tool_blocks -----

    #[test]
    fn strip_all_keeps_text_only_history_unchanged() {
        let msgs = vec![
            user_text("hello"),
            assistant(vec![ContentBlock::text("hi")]),
        ];
        let out = strip_all_tool_blocks(msgs.clone());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content.len(), 1);
        assert_eq!(out[1].content.len(), 1);
    }

    #[test]
    fn strip_all_keeps_image_blocks() {
        // Continuation summary should retain images (e.g. screenshots from
        // the user's earlier turns) so the model has the visual context.
        let msgs = vec![user(vec![
            ContentBlock::text("look"),
            ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "AAA".into(),
                },
            },
        ])];
        let out = strip_all_tool_blocks(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.len(), 2);
    }

    #[test]
    fn strip_all_drops_regular_tool_use_and_result() {
        let msgs = vec![
            assistant(vec![
                ContentBlock::text("calling bash"),
                tool_use("t1", "bash"),
            ]),
            user(vec![tool_result("t1")]),
            assistant(vec![ContentBlock::text("done")]),
        ];
        let out = strip_all_tool_blocks(msgs);
        assert_eq!(
            out.len(),
            2,
            "tool-result-only user message must be dropped"
        );
        // First survivor: assistant with just text (tool_use stripped)
        assert_eq!(out[0].content.len(), 1);
        assert!(
            matches!(&out[0].content[0], ContentBlock::Text { text } if text == "calling bash")
        );
        // Second survivor: assistant "done"
        assert!(matches!(&out[1].content[0], ContentBlock::Text { text } if text == "done"));
    }

    #[test]
    fn strip_all_drops_server_tool_use_and_tool_search_result() {
        // Reproduces the production failure shape: tool_search-discovered MCP
        // tool referenced in history with tools=[] in the request.
        let msgs = vec![
            assistant(vec![
                server_tool_use("srv1", "tool_search_tool_regex"),
                tool_search_result("srv1", &["datadog-mcp-prod__aggregate_events"]),
                tool_use("call1", "datadog-mcp-prod__aggregate_events"),
            ]),
            user(vec![tool_result("call1")]),
            assistant(vec![ContentBlock::text("summary so far")]),
        ];
        let out = strip_all_tool_blocks(msgs);
        // Only the two text-bearing messages should remain (the tool_result
        // user message and the all-server-blocks message after stripping
        // are empty, but the first assistant survives if it had any text;
        // here it had no text, so it should be dropped entirely).
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0].content[0], ContentBlock::Text { text } if text == "summary so far")
        );
    }

    #[test]
    fn strip_all_drops_messages_that_become_empty() {
        let msgs = vec![
            assistant(vec![tool_use("t1", "bash")]),
            user(vec![tool_result("t1")]),
        ];
        let out = strip_all_tool_blocks(msgs);
        assert!(out.is_empty());
    }

    // ----- strip_unavailable_tool_blocks -----

    #[test]
    fn strip_unavailable_noop_when_all_tools_available() {
        let available: std::collections::HashSet<&str> = ["bash", "patch"].into_iter().collect();
        let msgs = vec![
            assistant(vec![ContentBlock::text("x"), tool_use("t1", "bash")]),
            user(vec![tool_result("t1")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs.clone(), &available);
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[0].content.len(), 2);
        assert_eq!(out[1].content.len(), 1);
    }

    #[test]
    fn strip_unavailable_removes_tool_use_and_paired_result() {
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![
            assistant(vec![
                ContentBlock::text("mixed"),
                tool_use("keep", "bash"),
                tool_use("drop", "propose_task"),
            ]),
            user(vec![tool_result("keep"), tool_result("drop")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        // Assistant: text + the bash tool_use survive; propose_task tool_use is gone
        assert_eq!(out[0].content.len(), 2);
        assert!(out[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "keep")));
        // User message: only the tool_result for the surviving tool_use remains
        assert_eq!(out[1].content.len(), 1);
        assert!(
            matches!(&out[1].content[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "keep")
        );
    }

    #[test]
    fn strip_unavailable_filters_tool_search_references_in_place() {
        // Mode transition mid-conversation: some referenced tools are gone.
        // The ToolSearchToolResult block must survive (paired with its
        // ServerToolUse) but its tool_references list is filtered.
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![assistant(vec![
            tool_use("ghost", "removed_tool"), // forces stripped_ids non-empty path
            server_tool_use("srv1", "tool_search_tool_regex"),
            tool_search_result("srv1", &["bash", "removed_tool", "other"]),
        ])];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        assert_eq!(out.len(), 1);

        let ts_block = out[0]
            .content
            .iter()
            .find_map(|b| {
                if let ContentBlock::ToolSearchToolResult { content, .. } = b {
                    Some(content)
                } else {
                    None
                }
            })
            .expect("tool_search block should survive");
        assert_eq!(
            ts_block.tool_references.len(),
            1,
            "only references to available tools should remain"
        );
        assert_eq!(ts_block.tool_references[0].tool_name, "bash");

        // ServerToolUse must NOT be stripped — it pairs with the tool_search
        // result and orphaning it would 400 the request.
        assert!(out[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ServerToolUse { id, .. } if id == "srv1")));
    }

    #[test]
    fn strip_unavailable_drops_messages_that_become_empty() {
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![
            assistant(vec![tool_use("t1", "removed_tool")]),
            user(vec![tool_result("t1")]),
            assistant(vec![ContentBlock::text("survives")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0].content[0], ContentBlock::Text { text } if text == "survives"));
    }
}

// Re-export git helpers so existing `crate::runtime::executor::{run_git, ensure_gitignore_has_phoenix}`
// imports continue to resolve. Canonical definitions live in `crate::git_ops`.
pub(crate) use crate::git_ops::{ensure_gitignore_has_phoenix, run_git};

/// Rename a task file to `in-progress` status if it isn't already.
///
/// Returns the final filename (unchanged if no rename was needed). The file
/// is renamed in place via `taskmd_core::tasks::update_task`, which is a
/// single `std::fs::rename` on the filename — the body is untouched.
fn promote_task_status_to_in_progress(
    tasks_dir: &std::path::Path,
    task_id: &str,
    current_status: taskmd_core::constants::Status,
    original_filename: &str,
) -> Result<String, String> {
    use taskmd_core::constants::Status;
    use taskmd_core::tasks::{update_task, TaskUpdate};

    if current_status == Status::InProgress {
        return Ok(original_filename.to_string());
    }
    let result = update_task(
        tasks_dir,
        task_id,
        TaskUpdate {
            status: Some(Status::InProgress),
            ..Default::default()
        },
    )
    .map_err(|e| format!("Failed to rename task file to in-progress status: {e}"))?;
    Ok(result.new_filename)
}

/// Global mutex serializing the scan-tasks + write + commit sequence.
/// Task approval is rare; a single mutex is sufficient.
static TASK_APPROVAL_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve the base branch to record on the Work-mode conversation a task
/// approval produces, and single-branch-fetch it (REQ-PROJ-022, best-effort).
///
/// Normally the conversation recorded the base at creation time
/// (`desired_base_branch`). If not (e.g. an older `mode=auto` Managed
/// conversation), fall back to the *main checkout's* HEAD via `repo_root` —
/// **not** `cwd`'s HEAD, which is the early Explore worktree's `task-pending-…`
/// temp branch.
fn resolve_approval_base_branch(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    desired_base_branch: Option<&str>,
) -> Result<String, String> {
    let base_branch = if let Some(b) = desired_base_branch {
        b.to_string()
    } else {
        let b = run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
        if b.is_empty() || b == "HEAD" {
            return Err(
                "Cannot determine the base branch for this approval (the conversation didn't \
                 record one and the repository is on a detached HEAD). Re-create the \
                 conversation with an explicit base branch."
                    .to_string(),
            );
        }
        b
    };
    crate::git_ops::materialize_branch(cwd, &base_branch).map_err(|e| e.to_string())?;
    Ok(base_branch)
}

/// Locate the early Explore worktree at `{repo_root}/.phoenix/worktrees/{conv_id}`
/// and rename its `task-pending-…` temp branch to `task_branch` in place
/// (REQ-PROJ-028). Returns the worktree path.
///
/// `propose_task` is Managed-only and a Managed conversation gets this worktree
/// on its first message, so it always exists by approval time; if it somehow
/// doesn't, this errors with a clear "reject and re-propose" message rather than
/// nesting a new worktree.
fn open_early_worktree_and_rename_branch(
    repo_root: &std::path::Path,
    conv_id: &str,
    task_branch: &str,
) -> Result<std::path::PathBuf, String> {
    let worktree_path = repo_root.join(".phoenix/worktrees").join(conv_id);
    let exists = worktree_path.is_dir()
        && run_git(&worktree_path, &["rev-parse", "--is-inside-work-tree"]).is_ok();
    if !exists {
        return Err(format!(
            "No Explore worktree found at {} for this conversation. \
             Reject the plan and ask the agent to propose again.",
            worktree_path.display()
        ));
    }
    let temp_branch = run_git(&worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| format!("Failed to determine current branch in worktree: {e}"))?
        .trim()
        .to_string();
    tracing::info!(temp_branch = %temp_branch, task_branch, "REQ-PROJ-028: renaming temp branch");
    run_git(&worktree_path, &["branch", "-m", &temp_branch, task_branch])
        .map_err(|e| format!("Failed to rename branch '{temp_branch}' to '{task_branch}': {e}"))?;
    Ok(worktree_path)
}

/// Blocking implementation of taskmd task approval (REQ-PROJ-028).
/// Runs on a blocking thread via `spawn_blocking`.
///
/// `propose_task` is a Managed-only tool and a Managed conversation gets its
/// Explore worktree on its first message, so by the time approval runs `cwd`
/// IS that worktree, sitting on a `task-pending-…` temp branch with the task
/// file already written under `{tasks_dir}/`. Approval renames the temp branch
/// to `task-{id}-{slug}`, promotes the file's status segment to `in-progress`
/// if needed, and commits it on the task branch. There is no "task file lives
/// in the repo-root checkout" fallback — the early worktree always exists; if
/// it somehow doesn't, approval fails with a clear retry message rather than
/// nesting a new worktree.
///
/// A plain-markdown task file (a `.md` name that isn't a taskmd filename) is
/// handled by [`execute_approve_plain_markdown_blocking`] — dispatched below.
///
/// `cwd` is the Explore worktree; `repo_root` is the git repository root, used
/// for the canonical worktree path `{repo_root}/.phoenix/worktrees/{conv_id}`.
fn execute_approve_task_blocking(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    conv_id: &str,
    tasks_dir_name: &str,
    task_file: &str,
    title: &str,
    desired_base_branch: Option<&str>,
) -> Result<TaskApprovalResult, String> {
    // task 13009: a plain-markdown task file (a `.md` file whose name doesn't
    // match the taskmd pattern) takes a separate, simpler approval path — no
    // taskmd id/status/slug, no status-rename, branch uniquified by conversation
    // id. The empty-string legacy shim and all taskmd handling fall through to
    // the code below (a non-`.md`, non-taskmd path produces the taskmd-pattern
    // error there).
    if let Some(filename) = std::path::Path::new(task_file)
        .file_name()
        .and_then(|f| f.to_str())
    {
        if let Some(crate::task_source::TaskSource::PlainMarkdown { stem }) =
            crate::task_source::TaskSource::detect(filename)
        {
            return execute_approve_plain_markdown_blocking(
                cwd,
                repo_root,
                conv_id,
                task_file,
                &stem,
                title,
                desired_base_branch,
            );
        }
    }

    // Serialize approvals so concurrent attempts can't race on the same
    // branch/worktree name.
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if task_file.is_empty() {
        // Backward-compat shim: AwaitingTaskApproval rows persisted before
        // task_file existed deserialise with an empty string.
        return Err(format!(
            "This approval predates the file-based propose_task flow. \
             Reject the plan and ask the agent to propose again — it will \
             draft a task file under {tasks_dir_name}/ this time."
        ));
    }

    let base_branch = resolve_approval_base_branch(cwd, repo_root, desired_base_branch)?;

    // Parse the on-disk task filename — in taskmd 1.0 the filename is the sole
    // source of id/priority/status/slug; Phoenix allocates no ID.
    let rel_path = std::path::Path::new(task_file);
    let original_filename = rel_path
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| format!("task_file has no filename component: '{task_file}'"))?
        .to_string();
    let parsed = taskmd_core::filename::parse_filename(&original_filename).ok_or_else(|| {
        format!(
            "task_file '{original_filename}' does not match the taskmd filename pattern \
             (NNNNN-pX-status--slug.md)"
        )
    })?;
    let task_id = parsed.id.clone();
    let slug = parsed.slug.clone();
    let branch_name = format!("task-{task_id}-{slug}");

    let cwd_filepath = cwd.join(rel_path);
    if !cwd_filepath.exists() {
        return Err(format!(
            "Task file '{task_file}' does not exist under {}. \
             The file must be on disk before approval.",
            cwd.display()
        ));
    }

    // REQ-PROJ-028: `cwd` IS the early Explore worktree, on a `task-pending-…`
    // temp branch, with the task file already under `{tasks_dir}/`. Rename the
    // temp branch, promote the file's status to `in-progress` if needed, then
    // commit it on the task branch.
    let worktree_path = open_early_worktree_and_rename_branch(repo_root, conv_id, &branch_name)?;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    let final_filename = promote_task_status_to_in_progress(
        &worktree_path.join(tasks_dir_name),
        &task_id,
        parsed.status,
        &original_filename,
    )?;

    ensure_gitignore_has_phoenix(&worktree_path)?;
    // If `update_task` renamed the file, the old name is now a deletion — stage
    // it so the commit captures the rename rather than a duplicate task ID.
    // `git add` on an untracked-and-missing path errors harmlessly.
    if final_filename != original_filename {
        let _ = run_git(
            &worktree_path,
            &[
                "add",
                "--",
                &format!("{tasks_dir_name}/{original_filename}"),
            ],
        );
    }
    run_git(
        &worktree_path,
        &["add", "--", &format!("{tasks_dir_name}/{final_filename}")],
    )?;
    let commit_msg = format!("task {task_id}: {title}");
    // Nothing staged — e.g. reusing an existing already-`in-progress` task file
    // that wasn't modified: it's already on the branch, skip the commit.
    if run_git(&worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        if let Err(e) = run_git(&worktree_path, &["commit", "-m", &commit_msg]) {
            return Err(format!("Failed to commit task file in worktree: {e}"));
        }
        tracing::info!(branch = %branch_name, commit_msg = %commit_msg, "Task file committed on task branch");
    } else {
        tracing::info!(branch = %branch_name, "Task file already on the branch unchanged — no commit needed");
    }

    Ok(TaskApprovalResult {
        task_id,
        task_title: title.to_string(),
        branch_name,
        first_task: false,
        task_file: format!("{tasks_dir_name}/{final_filename}"),
        worktree_path: worktree_path_str,
        base_branch,
    })
}

/// Blocking git operations for approving a *plain-markdown* task file (task
/// 13009) — a sibling of [`execute_approve_task_blocking`] for the case where
/// the task file's name is not a taskmd 1.0 filename.
///
/// Differences from the taskmd path:
/// - branch name is `task-{sanitized-stem}-{conv-id-prefix}` — the conv-id
///   prefix is the uniquifier so two conversations proposing files with the
///   same stem don't collide (the approval mutex only serializes);
/// - no `...-ready--` → `...-in-progress--` rename, no `format_filename` call —
///   a plain brief has no status segment;
/// - the file is committed at its own path (e.g. `docs/plan.md`), not under the
///   project's tasks dir.
///
/// Only the REQ-PROJ-028 early-worktree case is handled. `propose_task` is a
/// Managed-only tool, and a Managed conversation gets its Explore worktree on
/// its first message (`ManagedWorktreeOnFirstMessage`), so by the time approval
/// runs the worktree always exists — there is no "task file lives in the
/// repo-root checkout" legacy fallback here (that scenario never existed for
/// plain-markdown task files: they're new in task 13009). If the worktree is
/// somehow missing, the approval fails with a clear "reject and re-propose"
/// message rather than silently recreating one.
///
/// `stem` is the task file's filename with the `.md` extension stripped (the
/// caller has already classified the filename as plain-markdown). `cwd` /
/// `repo_root` have the same meaning as in [`execute_approve_task_blocking`].
fn execute_approve_plain_markdown_blocking(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    conv_id: &str,
    task_file: &str,
    stem: &str,
    title: &str,
    desired_base_branch: Option<&str>,
) -> Result<TaskApprovalResult, String> {
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // base_branch is recorded on the resulting Work-mode conversation; for the
    // early-worktree case it is otherwise unused here (the worktree was already
    // created from it).
    let base_branch = resolve_approval_base_branch(cwd, repo_root, desired_base_branch)?;

    let cwd_filepath = cwd.join(task_file);
    if !cwd_filepath.exists() {
        return Err(format!(
            "Task file '{task_file}' does not exist under {}. \
             The file must be on disk before approval.",
            cwd.display()
        ));
    }

    let (branch_name, task_id) = crate::task_source::TaskSource::PlainMarkdown {
        stem: stem.to_string(),
    }
    .branch_and_id(conv_id);
    let commit_msg = format!("task {task_id}: {title}");

    // REQ-PROJ-028: `cwd` IS the early Explore worktree, on a temp branch, with
    // the task file already in place. Rename the temp branch, then commit the
    // file at its own path on it.
    let worktree_path = open_early_worktree_and_rename_branch(repo_root, conv_id, &branch_name)?;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    ensure_gitignore_has_phoenix(&worktree_path)?;
    run_git(&worktree_path, &["add", "--", task_file])?;
    // If the agent pointed at an existing file that was already on the branch
    // (inherited from base_branch) and didn't modify it — common when
    // `propose_task` targets something like `docs/plan.md` that Explore mode
    // can't edit — there is nothing staged. The file is already on the branch;
    // skip the commit rather than failing with "nothing to commit". `git diff
    // --cached --quiet` exits 0 when the index matches HEAD.
    if run_git(&worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        if let Err(e) = run_git(&worktree_path, &["commit", "-m", &commit_msg]) {
            return Err(format!("Failed to commit task file in worktree: {e}"));
        }
        tracing::info!(branch = %branch_name, worktree = %worktree_path_str, "Plain-markdown task approved — temp branch renamed, task file committed");
    } else {
        tracing::info!(branch = %branch_name, worktree = %worktree_path_str, "Plain-markdown task approved — temp branch renamed; task file already on the branch unchanged, no commit needed");
    }

    Ok(TaskApprovalResult {
        task_id,
        task_title: title.to_string(),
        branch_name,
        first_task: false,
        task_file: task_file.to_string(),
        worktree_path: worktree_path_str,
        base_branch,
    })
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
        crate::llm::LlmErrorKind::UsageLimitReached => crate::db::ErrorKind::UsageLimitReached,
        crate::llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        crate::llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        crate::llm::LlmErrorKind::ServerError => crate::db::ErrorKind::ServerError,
        crate::llm::LlmErrorKind::ServerOverloaded => crate::db::ErrorKind::ServerOverloaded,
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
        LlmErrorKind::UsageLimitReached => {
            // The codex parser always attaches a QuotaDetails to UsageLimitReached
            // errors; fall back to an empty payload only as a defensive measure
            // in case a future caller forgets to populate it.
            let details = error.quota.map_or(
                crate::llm::QuotaDetails {
                    plan_type: None,
                    resets_at: None,
                    limit_id: None,
                    limit_name: None,
                    primary: None,
                    secondary: None,
                    credits: None,
                    promo_message: None,
                },
                |boxed| *boxed,
            );
            LlmOutcome::UsageLimitReached {
                details,
                message: error.message,
            }
        }
        LlmErrorKind::ServerError => LlmOutcome::ServerError {
            status: 500,
            body: error.message,
        },
        LlmErrorKind::ServerOverloaded => LlmOutcome::ServerOverloaded {
            message: error.message,
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
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::UsageLimitReached),
            crate::db::ErrorKind::UsageLimitReached
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ServerOverloaded),
            crate::db::ErrorKind::ServerOverloaded
        );
    }

    #[test]
    fn test_usage_limit_reached_is_terminal_after_mapping() {
        let db_kind = llm_error_to_db_error(LlmErrorKind::UsageLimitReached);
        assert!(
            !db_kind.is_retryable(),
            "UsageLimitReached must NOT be retryable after mapping"
        );
        let db_kind = llm_error_to_db_error(LlmErrorKind::ServerOverloaded);
        assert!(
            !db_kind.is_retryable(),
            "ServerOverloaded must NOT be retryable after mapping"
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

/// Shared git fixture helpers for the executor's worktree-aware test
/// modules. Lives next to those modules (rather than in
/// `runtime::testing`) because every consumer is in this file and the
/// helpers are deliberately tied to the on-disk layout
/// `create_managed_explore_worktree_blocking` produces.
#[cfg(test)]
mod test_git_helpers {
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    pub fn init_repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                // Don't depend on the host's commit-signing setup in tests.
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "init",
                "-q",
            ][..],
        ] {
            let s = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(s.success(), "git {args:?} failed");
        }
        (tmp, root)
    }

    /// Add a worktree at `{repo}/.phoenix/worktrees/{id}` on a fresh
    /// `branch`. Used by tests that need a Work-mode worktree directly,
    /// skipping the Explore->Work promotion path.
    pub fn add_worktree(repo: &Path, id: &str, branch: &str) -> String {
        let wt = repo.join(".phoenix").join("worktrees").join(id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let wt_s = wt.to_string_lossy().to_string();
        let s = std::process::Command::new("git")
            .args(["worktree", "add", "-b", branch, &wt_s])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(s.success(), "git worktree add failed");
        wt_s
    }

    /// Add an Explore worktree at the canonical Phoenix path
    /// `{repo}/.phoenix/worktrees/{conv_id}` on a temp branch, exactly
    /// as `create_managed_explore_worktree_blocking` does in production.
    /// Used by tests that exercise the Explore->Work promotion path.
    pub fn add_explore_worktree(repo: &Path, conv_id: &str, base_branch: &str) -> PathBuf {
        let id_prefix: String = conv_id.chars().take(8).collect();
        let temp_branch = format!("task-pending-{id_prefix}");
        let wt = repo.join(".phoenix").join("worktrees").join(conv_id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let s = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &temp_branch,
                wt.to_str().unwrap(),
                base_branch,
            ])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(s.success(), "git worktree add failed");
        wt
    }

    pub fn branch_exists(repo: &Path, branch: &str) -> bool {
        let o = std::process::Command::new("git")
            .args(["branch", "--list", branch])
            .current_dir(repo)
            .output()
            .unwrap();
        !String::from_utf8_lossy(&o.stdout).trim().is_empty()
    }

    pub fn worktree_list(repo: &Path) -> String {
        String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }
}

/// Task 24696 Phase 3: verify the `Effect::NotifyContextExhausted` handler
/// preserves the worktree and does NOT demote `conv_mode`. The old
/// `cleanup_context_exhausted_worktree` path is gone — worktree handoff to a
/// continuation (REQ-BED-030) or a user-initiated abandon / mark-as-merged
/// are now the only ways a context-exhausted worktree is removed.
///
/// The effect is dispatched through the real `execute_effect` match arm
/// (not a private helper) so the handler's full transition-time behaviour
/// is exercised end-to-end.
#[cfg(test)]
mod context_exhausted_preserves_worktree_tests {
    use super::test_git_helpers::{add_worktree, branch_exists, init_repo};
    use super::*;
    use crate::db::{ConvMode, NonEmptyString};
    use crate::llm::ModelRegistry;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::{ConvContext, Effect};
    use crate::tools::BrowserSessionManager;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc};

    #[allow(clippy::type_complexity)]
    fn build_runtime(
        storage: Arc<InMemoryStorage>,
        conv_id: &str,
        working_dir: PathBuf,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        broadcast::Receiver<SseEvent>,
    ) {
        build_runtime_with_state(storage, conv_id, working_dir, ConvState::Idle)
    }

    #[allow(clippy::type_complexity)]
    fn build_runtime_with_state(
        storage: Arc<InMemoryStorage>,
        conv_id: &str,
        working_dir: PathBuf,
        initial_state: ConvState,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        broadcast::Receiver<SseEvent>,
    ) {
        let context = ConvContext::new(conv_id, working_dir, "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        let broadcast_rx = broadcaster.subscribe();

        let rt = ConversationRuntime::new(
            context,
            initial_state,
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        );
        (rt, broadcast_rx)
    }

    /// Drain the broadcaster for up to `timeout` and return true if a
    /// `StateChange { state: ContextExhausted, .. }` is observed.
    async fn wait_for_context_exhausted_broadcast(
        rx: &mut broadcast::Receiver<SseEvent>,
        timeout: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::StateChange {
                state: ConvState::ContextExhausted { .. },
                ..
            })) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
            {
                return true;
            }
        }
        false
    }

    /// Task 24696 regression: Work-mode conversation reaching
    /// `ContextExhausted` MUST keep its worktree on disk, keep its
    /// `ConvMode::Work` in storage, and broadcast the state change.
    #[tokio::test]
    async fn work_mode_context_exhausted_preserves_worktree_and_mode() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "ctx-work-1";
        let branch = "task-42-fix-bug";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Work {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF042").unwrap(),
            task_title: NonEmptyString::new("Fix bug").unwrap(),
        };
        storage.set_mode(conv_id, original_mode.clone());

        let (mut rt, mut rx) = build_runtime(storage.clone(), conv_id, PathBuf::from(&wt_path));

        // Drive the effect through the real dispatcher so the whole
        // `Effect::NotifyContextExhausted` arm fires, not a private helper.
        let gen = rt
            .execute_effect(Effect::NotifyContextExhausted {
                summary: "out of context".to_string(),
            })
            .await
            .expect("effect dispatch should not error");
        assert!(gen.is_none(), "notify effect generates no chained event");

        // Worktree directory still exists on disk.
        assert!(
            Path::new(&wt_path).exists(),
            "REQ-BED-031: worktree must be preserved on context exhaustion"
        );
        // Branch still exists.
        assert!(branch_exists(&repo_root, branch));
        // conv_mode in storage is untouched (still Work with same fields).
        assert_eq!(
            storage.get_mode(conv_id),
            Some(original_mode),
            "conv_mode must NOT be demoted to Explore anymore"
        );
        // The ContextExhausted StateChange SSE broadcast still fires.
        assert!(
            wait_for_context_exhausted_broadcast(&mut rx, Duration::from_secs(1)).await,
            "StateChange::ContextExhausted must still be broadcast"
        );
    }

    /// Branch-mode twin: same preservation semantics.
    #[tokio::test]
    async fn branch_mode_context_exhausted_preserves_worktree_and_mode() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "ctx-branch-1";
        let branch = "feature/pr-99";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Branch {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new(branch).unwrap(),
        };
        storage.set_mode(conv_id, original_mode.clone());

        let (mut rt, mut rx) = build_runtime(storage.clone(), conv_id, PathBuf::from(&wt_path));

        rt.execute_effect(Effect::NotifyContextExhausted {
            summary: "exhausted".to_string(),
        })
        .await
        .expect("effect dispatch should not error");

        assert!(
            Path::new(&wt_path).exists(),
            "Branch-mode worktree must survive context exhaustion"
        );
        assert!(branch_exists(&repo_root, branch));
        assert_eq!(
            storage.get_mode(conv_id),
            Some(original_mode),
            "Branch mode must NOT demote to Direct"
        );
        assert!(wait_for_context_exhausted_broadcast(&mut rx, Duration::from_secs(1)).await);
    }

    /// REQ-BED-031 regression: the executor's terminal-exit lifecycle hook
    /// (`emit_terminal_lifecycle_event` → `cleanup_worktree_if_present`) must
    /// NOT remove the worktree when the terminal state is `ContextExhausted`.
    ///
    /// The `Effect::NotifyContextExhausted` tests above only exercise the
    /// effect dispatcher; they do not drive the executor-loop exit path
    /// where `cleanup_worktree_if_present` is called. The original Phase 3
    /// commit (e82c1db) removed `cleanup_context_exhausted_worktree` but
    /// missed this sibling cleanup added in 4a94509 for Explore-mode leaks.
    /// As a result, every Work/Branch conversation that hit
    /// `ContextExhausted` had its worktree force-removed on executor exit,
    /// breaking the continuation handoff because the child inherits the
    /// parent's now-missing `worktree_path`.
    #[tokio::test]
    async fn terminal_exit_preserves_worktree_when_context_exhausted() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "term-exit-ctx-1";
        let branch = "task-99-preserve-me";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Work {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF099").unwrap(),
            task_title: NonEmptyString::new("Preserve me").unwrap(),
        };
        storage.set_mode(conv_id, original_mode);

        let (rt, _rx) = build_runtime_with_state(
            storage,
            conv_id,
            PathBuf::from(&wt_path),
            ConvState::ContextExhausted {
                summary: "ran out of context".to_string(),
            },
        );

        // Fire the exact lifecycle hook the executor's `run()` loop calls
        // when it observes a terminal state.
        rt.emit_terminal_lifecycle_event();

        assert!(
            Path::new(&wt_path).exists(),
            "REQ-BED-031: terminal-exit cleanup must NOT remove a \
             ContextExhausted worktree — continuation transfer depends on it"
        );
        assert!(
            branch_exists(&repo_root, branch),
            "branch must also survive — worktree remove --force would have nuked it"
        );
    }

    /// Negative control: the original Explore-mode-leak intent of
    /// `cleanup_worktree_if_present` (commit 4a94509) is preserved.
    /// A non-context-exhausted terminal exit (here `ConvState::Terminal`,
    /// the post-abandon / post-mark-merged sink) still cleans up any
    /// stray worktree at `.phoenix/worktrees/{conv_id}`.
    #[tokio::test]
    async fn terminal_exit_still_cleans_up_non_context_exhausted_terminal() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "term-exit-stray-1";
        let branch = "stray-explore-branch";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let (rt, _rx) = build_runtime_with_state(
            storage,
            conv_id,
            PathBuf::from(&wt_path),
            ConvState::Terminal,
        );

        rt.emit_terminal_lifecycle_event();

        assert!(
            !Path::new(&wt_path).exists(),
            "Non-ContextExhausted terminal exit must still reap stray worktrees \
             (the original 4a94509 intent for Explore-mode leaks)"
        );
    }
}

// ============================================================
// CWD immutability: task 02702
// ============================================================
//
// Verifies that for Managed conversations the Explore worktree is promoted
// in place at approval time (branch renamed, same path returned) so
// conv.cwd never changes across the Explore→Work transition.

#[cfg(test)]
mod cwd_immutability_tests {
    use super::test_git_helpers::{add_explore_worktree, branch_exists, init_repo, worktree_list};
    use super::*;

    /// Core task-02702 regression:
    ///
    /// For a Managed conversation, `execute_approve_task_blocking` must
    /// detect the early Explore worktree and promote it in place (branch
    /// rename only). The returned `worktree_path` must equal the original
    /// Explore worktree path, so `conv.cwd` is unchanged at approval time.
    #[test]
    fn approve_task_returns_same_path_as_explore_worktree() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "test-conv-immutable-cwd";
        let base_branch = "main";

        // Simulate REQ-PROJ-028: create the early Explore worktree
        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        let explore_wt_str = explore_wt.to_string_lossy().to_string();

        // Stage a taskmd-1.0 task file in the worktree (the agent would
        // have created this via the patch tool before calling propose_task).
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_filename = "12345-p2-ready--fix-the-login-bug.md";
        std::fs::write(
            tasks_dir.join(task_filename),
            "# Fix the login bug\n\n1. Investigate\n2. Fix\n3. Test\n",
        )
        .unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            &format!("tasks/{task_filename}"),
            "Fix the login bug",
            Some(base_branch),
        )
        .expect("approve_task_blocking failed");

        // The path must not change: Explore worktree promoted in place.
        assert_eq!(
            result.worktree_path, explore_wt_str,
            "worktree_path must equal original Explore worktree path; \
             a different value means a nested worktree was created"
        );

        // The temp branch must be gone, renamed to the task branch.
        let id_prefix: String = conv_id.chars().take(8).collect();
        let temp_branch = format!("task-pending-{id_prefix}");
        assert!(
            !branch_exists(&repo_root, &temp_branch),
            "temp branch {temp_branch} should have been renamed, not left behind"
        );
        assert!(
            branch_exists(&repo_root, &result.branch_name),
            "task branch {} must exist after approval",
            result.branch_name
        );

        // Only two worktree entries: the main checkout and the one Explore/Work
        // worktree. A nested worktree would show a third entry.
        let wt_list = worktree_list(&repo_root);
        let entry_count = wt_list
            .split("\n\n")
            .filter(|s| !s.trim().is_empty())
            .count();
        assert_eq!(
            entry_count, 2,
            "expected exactly 2 worktree entries (main + promoted worktree), got {entry_count}:\n{wt_list}"
        );

        // No nested .phoenix directory inside the worktree (the pre-fix symptom).
        assert!(
            !explore_wt.join(".phoenix").exists(),
            ".phoenix must not exist inside the worktree; \
             its presence means a nested worktree was created"
        );
    }
}

// ============================================================
// Plain-markdown task files (task 13009)
// ============================================================

#[cfg(test)]
mod plain_markdown_approval_tests {
    use super::test_git_helpers::{add_explore_worktree, branch_exists, init_repo};
    use super::*;

    fn git_show_head(cwd: &std::path::Path) -> String {
        String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["log", "-1", "--name-only", "--pretty=%s"])
                .current_dir(cwd)
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }

    /// A Managed conversation can approve a plain-markdown task file (one whose
    /// name is not a taskmd filename). The temp branch is renamed to
    /// `task-{stem}-{conv-id-prefix}`, the file is committed at its own path,
    /// and no `...-ready--` -> `...-in-progress--` rename happens.
    #[test]
    fn approve_plain_markdown_task_file_in_early_worktree() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "plainconv-abcdef";
        let base_branch = "main";

        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        // The Explore-mode patch tool is restricted to `tasks/`, so a plain
        // task brief lands there too.
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::write(tasks_dir.join("my-plan.md"), "# My plan\n\nDo the thing.\n").unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "tasks/my-plan.md",
            "My plan",
            Some(base_branch),
        )
        .expect("approve_task_blocking failed for plain markdown");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-my-plan-{conv_prefix}"));
        assert_eq!(result.task_id, "my-plan");
        assert_eq!(result.task_title, "My plan");
        assert_eq!(result.worktree_path, explore_wt.to_string_lossy());
        assert!(branch_exists(&repo_root, &result.branch_name));
        assert!(
            !branch_exists(&repo_root, &format!("task-pending-{conv_prefix}")),
            "temp branch should have been renamed"
        );
        // No status-rename: the file keeps its original name.
        assert!(explore_wt.join("tasks/my-plan.md").exists());
        let head = git_show_head(&explore_wt);
        assert!(
            head.contains("task my-plan: My plan"),
            "commit subject: {head}"
        );
        assert!(head.contains("tasks/my-plan.md"), "committed files: {head}");
    }

    /// `propose_task` may point at any markdown file, including one outside the
    /// tasks dir — it is committed at its own path.
    #[test]
    fn approve_plain_markdown_outside_tasks_dir() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "docsconv-99887766";
        let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
        std::fs::create_dir_all(explore_wt.join("docs")).unwrap();
        std::fs::write(explore_wt.join("docs/plan.md"), "# Doc plan\n").unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "docs/plan.md",
            "Doc plan",
            Some("main"),
        )
        .expect("approve failed for docs/plan.md");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-plan-{conv_prefix}"));
        let head = git_show_head(&explore_wt);
        assert!(head.contains("docs/plan.md"), "committed files: {head}");
    }

    /// Two conversations proposing files with the same stem must get distinct
    /// branch names — the conversation-id suffix is the uniquifier.
    #[test]
    fn plain_markdown_branches_distinct_across_conversations() {
        let (_tmp, repo_root) = init_repo();
        for conv_id in ["aaaaaaaa-conv-1", "bbbbbbbb-conv-2"] {
            let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
            std::fs::create_dir_all(explore_wt.join("tasks")).unwrap();
            std::fs::write(explore_wt.join("tasks/feature.md"), "# Feature\n").unwrap();
            let r = execute_approve_task_blocking(
                &explore_wt,
                &repo_root,
                conv_id,
                "tasks",
                "tasks/feature.md",
                "Feature",
                Some("main"),
            )
            .expect("approve failed");
            let prefix: String = conv_id.chars().take(8).collect();
            assert_eq!(r.branch_name, format!("task-feature-{prefix}"));
        }
        assert!(branch_exists(&repo_root, "task-feature-aaaaaaaa"));
        assert!(branch_exists(&repo_root, "task-feature-bbbbbbbb"));
    }

    /// `propose_task` may point at a file that already exists on the base branch
    /// and that the agent didn't (couldn't) modify — approval still succeeds, it
    /// just doesn't create an empty commit; the task branch == the base branch.
    #[test]
    fn approve_plain_markdown_unchanged_existing_file_skips_commit() {
        let (_tmp, repo_root) = init_repo();
        // Put docs/plan.md (and a .gitignore that already lists .phoenix/, so
        // ensure_gitignore_has_phoenix is a no-op in the worktree) on `main`.
        std::fs::create_dir_all(repo_root.join("docs")).unwrap();
        std::fs::write(repo_root.join("docs/plan.md"), "# Existing plan\n").unwrap();
        std::fs::write(repo_root.join(".gitignore"), ".phoenix/\n").unwrap();
        for args in [
            &["add", "docs/plan.md", ".gitignore"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "add plan",
                "-q",
            ][..],
        ] {
            let s = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_root)
                .status()
                .unwrap();
            assert!(s.success(), "git {args:?} failed");
        }
        let conv_id = "existconv-12345678";
        let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
        assert!(explore_wt.join("docs/plan.md").exists());

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "docs/plan.md",
            "Existing plan",
            Some("main"),
        )
        .expect("approve should succeed even with nothing to commit");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-plan-{conv_prefix}"));
        let rev = |r: &str| {
            String::from_utf8_lossy(
                &std::process::Command::new("git")
                    .args(["rev-parse", r])
                    .current_dir(&repo_root)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .trim()
            .to_string()
        };
        assert_eq!(
            rev(&result.branch_name),
            rev("main"),
            "no empty commit should have been created"
        );
    }
}

// ============================================================
// Mode-context refresh on Explore -> Work promotion
// ============================================================
//
// Regression test for task 03002: after a Managed conversation
// approves its task and is promoted to Work mode, the in-memory
// `context.mode_context` must reflect Work (not the stale Explore
// value from runtime startup). The `spawn_agents` Work-parent guard
// reads this field; a stale Explore value rejects legitimate
// `mode: "work"` sub-agent requests from a Work-mode parent.

#[cfg(test)]
mod approve_task_refreshes_mode_context_tests {
    use super::test_git_helpers::{add_explore_worktree, init_repo};
    use super::*;
    use crate::llm::ModelRegistry;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::{ConvContext, ConvState, Effect};
    use crate::system_prompt::ModeContext;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn approve_task_sets_mode_context_to_work() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "mode-ctx-refresh-1";
        let base_branch = "main";

        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_filename = "12345-p2-ready--spawn-work-subagents.md";
        std::fs::write(
            tasks_dir.join(task_filename),
            "# Spawn work subagents\n\n1. Plan\n2. Spawn\n",
        )
        .unwrap();

        let storage = Arc::new(InMemoryStorage::new());
        let mut context = ConvContext::new(conv_id, explore_wt.clone(), "test-model", 200_000);
        context.mode_context = Some(ModeContext::Explore);
        context.desired_base_branch = Some(base_branch.to_string());

        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        let mut rt = ConversationRuntime::new(
            context,
            ConvState::AwaitingTaskApproval {
                task_file: format!("tasks/{task_filename}"),
                title: "Spawn work subagents".to_string(),
                priority: crate::task_source::Priority::P2,
                plan: "Plan and spawn".to_string(),
            },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        );

        rt.execute_effect(Effect::ApproveTask {
            task_file: format!("tasks/{task_filename}"),
            title: "Spawn work subagents".to_string(),
            priority: crate::task_source::Priority::P2,
            plan: "Plan and spawn".to_string(),
        })
        .await
        .expect("approve task effect failed");

        match rt.context.mode_context.as_ref() {
            Some(ModeContext::Work {
                branch_name,
                base_branch: bb,
                worktree_path,
            }) => {
                assert!(
                    !branch_name.is_empty(),
                    "branch_name must be populated post-approval"
                );
                assert_eq!(bb, base_branch, "base_branch must round-trip");
                assert_eq!(
                    worktree_path,
                    &explore_wt.to_string_lossy().to_string(),
                    "worktree_path must equal the in-place-promoted Explore worktree"
                );
            }
            other => panic!(
                "mode_context must be refreshed to Work after approval; got {other:?}. \
                 Stale Explore here is the task-03002 bug: spawn_agents rejects \
                 mode: \"work\" sub-agents because parent_allows_work is false."
            ),
        }
    }
}

// ============================================================
// Steering queue multi-drain detectors (Phase 2)
// ============================================================
//
// These tests exercise the executor-level drain logic in
// `apply_transition_result` for `SteerDrainedUserMessages`. They drive
// synthetic `TransitionResult`s so the detectors are isolated from the
// transition machinery (which is tested separately in
// state_machine/transition.rs).

#[cfg(test)]
mod steer_drain_detector_tests {
    use super::*;
    use crate::llm::ModelRegistry;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::event::SteerEntry;
    use crate::state_machine::state::{
        AssistantMessage, PendingSubAgent, SubAgentMode, ToolCall, ToolInput,
    };
    use crate::state_machine::transition::TransitionResult;
    use crate::state_machine::ConvContext;
    use crate::tools::BrowserSessionManager;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn mk_entry(id: &str, text: &str) -> SteerEntry {
        SteerEntry {
            text: text.to_string(),
            llm_text: None,
            images: vec![],
            message_id: id.to_string(),
            user_agent: None,
            skill_invocation: None,
        }
    }

    fn mk_tool_executing() -> ConvState {
        ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Bash(crate::tools::BashToolInput::run("echo hi")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        }
    }

    fn mk_awaiting_sub_agents() -> ConvState {
        ConvState::AwaitingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "sub-1".to_string(),
                task: "do thing".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
            spawn_tool_id: None,
        }
    }

    #[allow(clippy::type_complexity)]
    fn build_runtime_with_state_and_queue(
        conv_id: &str,
        initial_state: ConvState,
        queue: Vec<SteerEntry>,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        Arc<InMemoryStorage>,
    ) {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new(conv_id, PathBuf::from("/tmp"), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        let rt = ConversationRuntime::new(
            context,
            initial_state,
            storage.clone(),
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
        .with_steering_queue(queue);
        (rt, storage)
    }

    /// Filter generated events down to `SteerDrainedUserMessages` payloads.
    fn extract_steer_drain_entries(events: &[Event]) -> Vec<Vec<SteerEntry>> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::SteerDrainedUserMessages { entries } => Some(entries.clone()),
                _ => None,
            })
            .collect()
    }

    /// Drain-all on entering `Idle`: from `LlmRequesting` → `Idle` with 3 queued
    /// entries, the executor must emit one `SteerDrainedUserMessages` carrying
    /// all 3 entries and clear in-memory `self.steering_queue`. DB queue clear
    /// is the `Effect::ClearSteeringQueueEntries` arm's job, covered by
    /// `clear_steering_queue_entries_preserves_concurrent_enqueue`.
    /// Drain-all on entering Idle is processed INLINE: persists land before
    /// `apply_transition_result` returns. Assertion is via storage rather than
    /// generated_events because the drain event no longer surfaces externally.
    #[tokio::test]
    async fn drain_all_on_entering_idle() {
        let queue = vec![
            mk_entry("s1", "first"),
            mk_entry("s2", "second"),
            mk_entry("s3", "third"),
        ];
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-drain-idle",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let result = TransitionResult::new(ConvState::Idle);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        // Persists ran inline → messages now in storage in FIFO order.
        let msgs = storage.get_all_messages("conv-drain-idle");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2", "s3"]);

        assert!(
            rt.steering_queue.is_empty(),
            "in-memory queue must be empty"
        );
        // Drain transition lands in LlmRequesting (Idle → LlmRequesting via the
        // SteerDrainedUserMessages arm).
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Task 60004: entering Idle with a non-empty steering queue must NOT
    /// broadcast an intermediate `StateChange { Idle }` before the drain's
    /// `StateChange { LlmRequesting }`. The Idle state is still persisted to
    /// the DB; only its SSE broadcast is suppressed.
    #[tokio::test]
    async fn entering_idle_with_queued_steer_suppresses_idle_state_change() {
        let queue = vec![mk_entry("s1", "first")];
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-no-flicker",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let mut rx = rt.broadcast_tx.subscribe();

        // Original transition LlmRequesting -> Idle carries a PersistState
        // effect (the usual turn-end shape).
        let result = TransitionResult::new(ConvState::Idle)
            .with_effect(crate::state_machine::effect::Effect::PersistState);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let mut saw_idle_state_change = false;
        let mut saw_llm_requesting_state_change = false;
        while let Ok(ev) = rx.try_recv() {
            if let SseEvent::StateChange { state, .. } = ev {
                match state {
                    ConvState::Idle => saw_idle_state_change = true,
                    ConvState::LlmRequesting { .. } => {
                        saw_llm_requesting_state_change = true;
                    }
                    _ => {}
                }
            }
        }

        assert!(
            !saw_idle_state_change,
            "intermediate Idle StateChange must be suppressed during inline drain"
        );
        assert!(
            saw_llm_requesting_state_change,
            "drain must still broadcast the authoritative LlmRequesting StateChange"
        );
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain from `ToolExecuting` → `LlmRequesting`: persists run
    /// inline before the (deferred) RequestLlm, so the spawned LLM task reads
    /// a DB that already has the steered messages.
    #[tokio::test]
    async fn drain_all_mid_turn_from_tool_executing() {
        let queue = vec![mk_entry("s1", "one"), mk_entry("s2", "two")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-drain-tool", mk_tool_executing(), queue);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let msgs = storage.get_all_messages("conv-drain-tool");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2"]);
        assert!(rt.steering_queue.is_empty());
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain with RequestLlm in the original effects list: the
    /// executor must defer RequestLlm so persists land before the LLM task
    /// reads the DB. This is a smoke test that the deferred-RequestLlm path
    /// runs without panic, persists land, and final state is LlmRequesting.
    /// Stronger ordering is enforced by `apply_transition_result`'s sequential
    /// awaits (persists before deferred RequestLlm spawn).
    #[tokio::test]
    async fn mid_turn_drain_defers_request_llm_until_after_persists() {
        let queue = vec![mk_entry("s1", "first"), mk_entry("s2", "second")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-defer-req", mk_tool_executing(), queue);

        // Queue an LLM response so dispatch_llm_request can complete cleanly.
        // We don't assert on the response — just that the pipeline doesn't panic
        // and the persists landed before RequestLlm was dispatched.
        rt.llm_client.queue_response(crate::llm::LlmResponse {
            content: vec![],
            end_turn: true,
            usage: crate::llm::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        });

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
            .with_effect(Effect::RequestLlm);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result with deferred RequestLlm must succeed");

        // Persists landed before the LLM task spawn (sequential await ordering).
        let msgs = storage.get_all_messages("conv-defer-req");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2"]);
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain from `AwaitingSubAgents` → `LlmRequesting`.
    #[tokio::test]
    async fn drain_all_mid_turn_from_awaiting_subagents() {
        let queue = vec![mk_entry("s1", "alpha")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-drain-sub", mk_awaiting_sub_agents(), queue);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let msgs = storage.get_all_messages("conv-drain-sub");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message_id, "s1");
        assert!(rt.steering_queue.is_empty());
    }

    /// Entering `Idle` with an empty queue produces no drain event.
    #[tokio::test]
    async fn no_drain_when_queue_empty_idle() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-empty-idle",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        let result = TransitionResult::new(ConvState::Idle);
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "no drain event should be emitted when queue is empty"
        );
    }

    /// Entering `LlmRequesting` from a tool round with an empty queue produces
    /// no drain event.
    #[tokio::test]
    async fn no_drain_when_queue_empty_mid_turn() {
        let (mut rt, _storage) =
            build_runtime_with_state_and_queue("conv-empty-mid", mk_tool_executing(), vec![]);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "no drain when queue is empty"
        );
    }

    /// `LlmRequesting` → `ToolExecuting` (LLM responded with tools) must NOT
    /// trigger a drain — this is mid-conversation but not a hook point. Queue
    /// must be preserved untouched.
    #[tokio::test]
    async fn no_drain_on_intermediate_states() {
        let queue = vec![mk_entry("keep-me", "still here")];
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-intermediate",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let result = TransitionResult::new(mk_tool_executing());
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "transition without an entry hook must not drain"
        );
        assert_eq!(
            rt.steering_queue.len(),
            1,
            "queue must be preserved when no drain hook fires"
        );
        assert_eq!(rt.steering_queue[0].message_id, "keep-me");
        // No persist call happened on this path, so storage's queue is still empty
        // (it was never written to). The point of the assertion above is that the
        // in-memory queue is the live source — it was not modified.
        let _ = storage; // touch to silence unused warning
    }

    /// `Effect::ClearSteeringQueueEntries` removes ONLY the matching message_ids
    /// from storage; concurrently-enqueued entries are preserved. Models the
    /// enqueue-during-drain race.
    #[tokio::test]
    async fn clear_steering_queue_entries_preserves_concurrent_enqueue() {
        use crate::runtime::traits::StateStore;
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-clear-effect",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );
        // Pre-seed storage as if drain took [p1, p2] from in-memory, then a
        // concurrent enqueue persisted [p1, p2, c1] (c1 added by enqueue).
        storage
            .update_steering_queue(
                "conv-clear-effect",
                &[
                    mk_entry("p1", "pending-1"),
                    mk_entry("p2", "pending-2"),
                    mk_entry("c1", "concurrent"),
                ],
            )
            .await
            .expect("seed steering queue");

        // Drain only removes p1 and p2.
        rt.execute_effect(Effect::ClearSteeringQueueEntries {
            message_ids: vec!["p1".to_string(), "p2".to_string()],
        })
        .await
        .expect("ClearSteeringQueueEntries effect must succeed");

        let remaining = storage.get_steering_queue("conv-clear-effect");
        assert_eq!(remaining.len(), 1, "concurrent enqueue must survive drain");
        assert_eq!(remaining[0].message_id, "c1");
    }

    /// `PersistMessage` is idempotent on duplicate `message_id`. Models the
    /// crash-recovery re-drain path: a SteerDrainedUserMessages event re-fires
    /// after a partial drain, but messages already persisted are skipped without
    /// allocating a new sequence_id or producing a duplicate row.
    #[tokio::test]
    async fn persist_message_is_idempotent_on_duplicate_id() {
        use crate::db::{MessageContent, UserContent};
        use crate::runtime::traits::MessageStore;
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-idem",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );
        let dup_id = "dup-msg-1".to_string();
        // Seed: message already exists in storage (simulates partial drain that
        // persisted this entry before a crash).
        storage
            .add_message(
                &dup_id,
                "conv-idem",
                &MessageContent::User(UserContent::new("already-there")),
                None,
                None,
            )
            .await
            .expect("seed message");
        let initial_count = storage.get_all_messages("conv-idem").len();
        assert_eq!(initial_count, 1);

        // Run idempotent PersistMessage with the same message_id — must be a no-op.
        rt.execute_effect(Effect::PersistMessage {
            content: MessageContent::User(UserContent::new("duplicate-attempt")),
            display_data: None,
            usage_data: None,
            message_id: dup_id.clone(),
            idempotent: true,
        })
        .await
        .expect("PersistMessage on duplicate must succeed");

        let after = storage.get_all_messages("conv-idem");
        assert_eq!(
            after.len(),
            1,
            "no duplicate row should be inserted for an existing message_id"
        );
        assert_eq!(after[0].message_id, dup_id);
    }

    /// Sub-agent runtimes never drain a steering queue. Steering is a parent-
    /// only feature; even if a sub-agent's in-memory queue is non-empty (e.g.,
    /// corrupted state on resume), the drain detector must skip it.
    #[tokio::test]
    async fn no_drain_on_sub_agent_runtime() {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::sub_agent(
            "conv-sub-drain",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "parent-conv",
        );
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        let mut rt = ConversationRuntime::new(
            context,
            ConvState::LlmRequesting { attempt: 1 },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
        .with_steering_queue(vec![mk_entry("ignored", "should not drain")]);

        let result = TransitionResult::new(ConvState::Idle);
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "sub-agent runtimes must NOT drain queued steers"
        );
        assert_eq!(
            rt.steering_queue.len(),
            1,
            "sub-agent queue must remain untouched"
        );
    }

    /// Non-idempotent `PersistMessage` does NOT do a `message_exists` precheck.
    /// This is the hot-path guarantee: agent/tool/checkpoint persistence pays
    /// no extra DB query.
    #[tokio::test]
    async fn persist_message_non_idempotent_skips_existence_check() {
        use crate::db::{MessageContent, UserContent};
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-non-idem",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        // Fresh message_id; non-idempotent path should persist normally.
        rt.execute_effect(Effect::PersistMessage {
            content: MessageContent::User(UserContent::new("fresh-message")),
            display_data: None,
            usage_data: None,
            message_id: "new-1".to_string(),
            idempotent: false,
        })
        .await
        .expect("non-idempotent PersistMessage must succeed");

        assert_eq!(storage.get_all_messages("conv-non-idem").len(), 1);
    }

    /// Regression: a tool result carrying typed images must survive the
    /// normal `PersistCheckpoint` round into `ToolContent.images`. Before
    /// this was threaded, `MessageContent::tool(...)` dropped the field, so
    /// read_image (which now relies solely on the typed channel) lost its
    /// image bytes for both the UI and the next LLM turn.
    #[tokio::test]
    async fn persist_checkpoint_preserves_tool_result_images() {
        use crate::db::{MessageContent, ToolContentImage, ToolOutcome, ToolResult};
        use crate::llm::ContentBlock;
        use crate::state_machine::{AssistantMessage, CheckpointData};

        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-img",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        let assistant = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::ToolUse {
                id: "tool-img-1".to_string(),
                name: "read_image".to_string(),
                input: serde_json::json!({"path": "x.png"}),
            }],
            None,
            None,
        );
        let result = ToolResult {
            tool_use_id: "tool-img-1".to_string(),
            outcome: ToolOutcome::Success {
                output: "Image loaded: x.png (3 bytes)".to_string(),
                display_data: None,
                images: vec![ToolContentImage {
                    media_type: "image/png".to_string(),
                    data: "QUJD".to_string(),
                }],
            },
            duration_ms: None,
        };
        let data = CheckpointData::tool_round(assistant, vec![result]).expect("tool_round");

        rt.execute_effect(Effect::PersistCheckpoint { data })
            .await
            .expect("PersistCheckpoint must succeed");

        let msgs = storage.get_all_messages("conv-img");
        let tool_msg = msgs
            .iter()
            .find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == "tool-img-1" => Some(tc),
                _ => None,
            })
            .expect("persisted tool result message");
        assert_eq!(
            tool_msg.images,
            vec![ToolContentImage {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            }],
            "typed images must not be dropped at checkpoint persistence"
        );
    }
}

// ============================================================
// Work-sub-agent cwd-scoping guard (REQ-PROJ-008)
// ============================================================
//
// Distilled from specs/subagents/subagents.allium
// (SpawnRejectedWorkCwdOutsideWorktree). A Work sub-agent's overridden
// `cwd` must stay inside the parent's worktree -- without the guard, the
// sub-agent's runtime would see a different working_dir than the parent
// and writes could escape projects.allium's WriteBlockedOutsideWorktree.

#[cfg(test)]
mod work_subagent_cwd_guard_tests {
    use super::*;
    use crate::db::ToolOutcome;
    use crate::llm::ModelRegistry;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::state::{SpawnAgentsInput, SubAgentMode, SubAgentTask, ToolInput};
    use crate::state_machine::ConvContext;
    use crate::system_prompt::ModeContext;
    use crate::tools::BrowserSessionManager;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    fn runtime_in_work_mode(
        worktree_path: &std::path::Path,
    ) -> ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>> {
        let storage = Arc::new(InMemoryStorage::new());
        let mut context = ConvContext::new(
            "cwd-guard-conv",
            worktree_path.to_path_buf(),
            "test-model",
            200_000,
        );
        context.mode_context = Some(ModeContext::Work {
            branch_name: "task-99999-x".to_string(),
            base_branch: "main".to_string(),
            worktree_path: worktree_path.to_string_lossy().to_string(),
        });
        context.mode = crate::state_machine::state::ModeKind::Managed;

        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
    }

    fn spawn_tool(input: SpawnAgentsInput) -> ToolCall {
        ToolCall::new("tool-spawn-1", ToolInput::SpawnAgents(input))
    }

    fn tool_result_text(result: &ToolResult) -> String {
        match &result.outcome {
            ToolOutcome::Success { output, .. } | ToolOutcome::Error { output, .. } => {
                output.clone()
            }
            ToolOutcome::Cancelled { message } => message.clone(),
        }
    }

    /// Work sub-agent with a `cwd` pointing outside the parent's worktree
    /// is rejected with an error tool result -- the spawn never reaches
    /// the `RuntimeManager`.
    #[tokio::test]
    async fn rejects_work_subagent_cwd_outside_worktree() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "do unsafe writes".to_string(),
                    cwd: Some(outside.path().to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "rejection must surface as a tool error");
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("inside the parent's worktree"),
                    "error message should explain the cwd-scoping rule, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete with error, got {other:?}"),
        }
        assert_eq!(
            rt.active_work_subagents, 0,
            "rejected spawn must not increment active_work_subagents"
        );
    }

    /// A Work sub-agent whose `cwd` is inside the worktree is NOT rejected
    /// by the scoping guard. (It will still fail downstream because no
    /// `spawn_tx` is wired in this test runtime, but that failure is
    /// distinguishable from the scoping rejection.)
    #[tokio::test]
    async fn accepts_work_subagent_cwd_inside_worktree() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let nested = worktree.path().join("sub/dir");
        std::fs::create_dir_all(&nested).expect("nested dir");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "do scoped writes".to_string(),
                    cwd: Some(nested.to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                let msg = tool_result_text(&result);
                assert!(
                    !msg.contains("inside the parent's worktree"),
                    "in-worktree cwd should not trip the scoping guard; got: {msg}"
                );
            }
            Some(Event::SpawnAgentsComplete { .. }) => {
                // Test runtime has no spawn channel wired, so we don't
                // normally reach SpawnAgentsComplete -- but if some future
                // refactor wires it, that's still a "did not reject" pass.
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// Symlinks that escape the worktree are rejected -- the guard
    /// canonicalises both sides before comparing.
    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_work_subagent_cwd_via_escaping_symlink() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");
        let symlink = worktree.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &symlink).expect("create symlink");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "follow the symlink".to_string(),
                    cwd: Some(symlink.to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error());
            }
            other => panic!("expected symlink escape to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn path_is_within_requires_root_to_exist() {
        // Worktree root that doesn't canonicalise -> fail closed. A
        // non-existent root would make the comparison meaningless.
        assert!(!path_is_within(
            "/nonexistent/root/sub",
            "/nonexistent/root"
        ));
    }

    #[test]
    fn path_is_within_accepts_nonexistent_leaf_under_real_root() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let path_in = worktree.path().join("not/yet/created");
        // Deepest existing ancestor of `path_in` is the worktree itself,
        // which canonicalises and starts_with itself.
        assert!(path_is_within(
            path_in.to_str().unwrap(),
            worktree.path().to_str().unwrap()
        ));
    }

    #[test]
    fn path_is_within_rejects_parent_dir_traversal_that_escapes() {
        // `/worktree/../escape` -> canonical /parent_of_worktree, which
        // is not a subpath of the worktree -> rejected. Canonicalise
        // handles the `..` directly (it's a `realpath` call).
        let worktree = TempDir::new().expect("worktree tempdir");
        let bad = format!("{}/../escape", worktree.path().display());
        assert!(!path_is_within(&bad, worktree.path().to_str().unwrap()));
    }

    #[test]
    fn path_is_within_accepts_internal_parent_dir_traversal() {
        // `/worktree/src/../tests` is a legitimate in-worktree path --
        // canonicalisation resolves it to `/worktree/tests`, which is a
        // subpath of the worktree. Don't blanket-reject `..`.
        let worktree = TempDir::new().expect("worktree tempdir");
        std::fs::create_dir_all(worktree.path().join("src")).expect("src dir");
        let inner = format!("{}/src/../tests", worktree.path().display());
        assert!(path_is_within(&inner, worktree.path().to_str().unwrap()));
    }

    #[test]
    fn path_is_within_rejects_relative_paths() {
        let worktree = TempDir::new().expect("worktree tempdir");
        assert!(!path_is_within(
            "sub/dir",
            worktree.path().to_str().unwrap()
        ));
        assert!(!path_is_within("./sub", worktree.path().to_str().unwrap()));
    }

    /// The intermediate-symlink escape: `/worktree/escape` is a symlink
    /// to `/outside`, override cwd is `/worktree/escape/newdir` (does
    /// not exist yet). Without the deepest-existing-ancestor canonical
    /// resolution, the leaf's canonicalisation fails and a lexical
    /// `starts_with` would accept the path. Resolving the deepest
    /// existing ancestor (the symlink itself, which canonicalises to
    /// `/outside`) rejects it.
    #[cfg(unix)]
    #[test]
    fn path_is_within_rejects_intermediate_symlink_escape() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");
        let escape = worktree.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &escape).expect("create symlink");
        let bypass = escape.join("newdir");
        assert!(!path_is_within(
            bypass.to_str().unwrap(),
            worktree.path().to_str().unwrap()
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_parent_dir_traversal_in_override() {
        let worktree = TempDir::new().expect("worktree tempdir");
        // Sibling dir that EXISTS on disk, so canonicalize succeeds for
        // both sides -- the only thing keeping this safe is the `..`
        // rejection up front.
        let outside = TempDir::new().expect("outside tempdir");
        let traversing = format!(
            "{}/../{}",
            worktree.path().display(),
            outside
                .path()
                .file_name()
                .expect("outside dir name")
                .to_string_lossy()
        );

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "traverse out".to_string(),
                    cwd: Some(traversing),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "rejection must surface as a tool error");
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("inside the parent's worktree"),
                    "error message should explain the cwd-scoping rule, got: {msg}"
                );
            }
            other => panic!("expected ..-traversal to be rejected, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tool_output_to_outcome_tests {
    use super::tool_output_to_outcome;
    use crate::db::ToolOutcome;
    use crate::tools::{ToolImage, ToolOutput};

    #[test]
    fn success_output_maps_to_success_outcome() {
        let out = ToolOutput::success("ran clean")
            .with_display(serde_json::json!({ "k": "v" }))
            .with_images(vec![ToolImage {
                media_type: "image/png".to_string(),
                data: "Zm9v".to_string(),
            }]);

        match tool_output_to_outcome(out) {
            ToolOutcome::Success {
                output,
                display_data,
                images,
            } => {
                assert_eq!(output, "ran clean");
                assert_eq!(display_data, Some(serde_json::json!({ "k": "v" })));
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].media_type, "image/png");
                assert_eq!(images[0].data, "Zm9v");
            }
            other => panic!("expected ToolOutcome::Success, got {other:?}"),
        }
    }

    #[test]
    fn error_output_maps_to_error_outcome() {
        // Error-shaped text the executor must NOT misclassify: with the old
        // `success: bool`, only the string distinguished this from a success.
        let out = ToolOutput::error("looks fine but failed");

        match tool_output_to_outcome(out) {
            ToolOutcome::Error { output, .. } => {
                assert_eq!(output, "looks fine but failed");
            }
            other => panic!("expected ToolOutcome::Error, got {other:?}"),
        }
    }
}
