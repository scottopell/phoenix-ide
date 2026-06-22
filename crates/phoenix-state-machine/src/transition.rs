//! Pure state transition functions
//!
//! Two entry points:
//! - `transition()`: handles all events (user events + executor events)
//! - `handle_outcome()`: handles executor-produced outcomes via typed channels
//!
//! REQ-BED-001: Pure State Transitions
//! REQ-BED-002: User Message Handling
//! REQ-BED-003: LLM Response Processing
//! REQ-BED-004: Tool Execution Coordination
//! REQ-BED-005: Cancellation Handling
//! REQ-BED-006: Error Recovery

use super::effect::{compute_bash_display_data, CheckpointData};
use super::event::{
    CancelCause, CoreEvent, ParentEvent, ParentOnlyEvent, SubAgentEvent, SubAgentOnlyEvent,
};
use super::outcome::{EffectOutcome, InvalidOutcome, LlmOutcome, PersistOutcome, ToolExecOutcome};
use super::state::{
    AssistantMessage, ContextExhaustionBehavior, ContinuationSummaryRequest, CoreState, ModeKind,
    ParentState, RecoveryKind, RecoveryResumeTarget, SubAgentOutcome, SubAgentResult,
    SubAgentState, TaskApprovalHandoff, TaskApprovalOutcome, ToolCall, ToolInput,
};
use super::{ConvContext, ConvState, Effect, Event};
use phoenix_core::domain::db_schema::{ErrorKind, ToolResult, UsageData};
use phoenix_core::domain::llm_error_kind::LlmAttemptReason;
use phoenix_core::domain::mode_context::ModeContext;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

/// Stable marker persisted as a hidden system message when the user dismisses
/// an `ask_user_question` panel without answering. Recovery uses this to
/// distinguish deliberate dismissal from an interrupted tool turn.
pub const USER_QUESTION_DISMISSED_MARKER: &str = "[ask-user-question-dismissed]";

/// Stable marker persisted as a hidden system message when the user dismisses
/// a persisted error (`DismissError`). Without it, a usage-limit error that
/// occurred after a tool result would persist as `Idle` with a trailing tool
/// result, and the restart recovery heuristic (`should_auto_continue`) would
/// auto-continue the exact turn the user dismissed. The marker becomes the
/// last message, so recovery sees a deliberate dismissal and stays `Idle`.
pub const ERROR_DISMISSED_MARKER: &str = "[error-dismissed]";

/// Statuses a task file may be in for `propose_task` to accept it.
const ACCEPTABLE_PROPOSE_STATUSES: &[taskmd_core::constants::Status] = &[
    taskmd_core::constants::Status::Ready,
    taskmd_core::constants::Status::InProgress,
    taskmd_core::constants::Status::Brainstorming,
];

/// Validated snapshot of a task file at the moment `propose_task` was called.
///
/// `task_file` is normalised to a forward-slash path relative to the
/// conversation cwd; `title`/`priority` come from the [`TaskSource`] (taskmd
/// filenames carry both; plain-markdown files take the body's `# H1` and a
/// `p2` default).
#[derive(Debug)]
struct TaskFileSnapshot {
    task_file: String,
    title: String,
    priority: phoenix_core::task_source::Priority,
    /// Display copy of the brief: the file body trimmed of surrounding
    /// whitespace, for the approval reader.
    plan: String,
    /// The authoritative file bytes, exactly as read from disk (untrimmed).
    /// A fork commits this verbatim, so it must equal the reviewed file
    /// byte-for-byte (REQ-PROJ-033).
    body_raw: String,
}

/// Read and validate a task file referenced by `propose_task`.
///
/// The task file may be either a taskmd 1.0 filename (`NNNNN-pX-status--slug.md`
/// — id/priority/status/slug come from the filename; this form is *required* to
/// live under the project's tasks dir) or any other markdown file anywhere in
/// the worktree (a plain task brief — title from the body's `# H1`, priority
/// defaults to `p2`). See [`phoenix_core::task_source::TaskSource`].
///
/// The state machine treats this read as a deterministic data-load — like
/// reading the conversation cwd off disk — not as an external side effect.
/// All I/O is local to the worktree, synchronous, and bounded.
fn resolve_task_file(
    cwd: &Path,
    tasks_dir_name: &str,
    task_file: &str,
) -> Result<TaskFileSnapshot, String> {
    use phoenix_core::task_source::TaskSource;

    if task_file.is_empty() {
        return Err("task_file is required".to_string());
    }
    let rel_path = Path::new(task_file);
    if rel_path.is_absolute() {
        return Err(format!(
            "task_file must be a relative path (got '{task_file}')"
        ));
    }
    // Reject `..` components: a path like `tasks/../other/foo.md` has
    // `tasks` as its first component but escapes the tree once joined to
    // cwd. PatchTool enforces the same rule.
    if rel_path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "task_file must not contain '..' components (got '{task_file}')"
        ));
    }

    let filename = rel_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("task_file has no filename component: '{task_file}'"))?;
    let source = TaskSource::detect(filename)
        .ok_or_else(|| format!("task_file must be a markdown file (.md) (got '{task_file}')"))?;

    let first_component = rel_path
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str());

    match &source {
        TaskSource::Taskmd { status, .. } => {
            // A taskmd-named file is required to live under the project's tasks
            // dir — that's where `taskmd validate` (and the project's task
            // tooling generally) expects taskmd files. A plain `.md` brief that
            // wants to live elsewhere just must not use the taskmd naming.
            if first_component != Some(tasks_dir_name) {
                return Err(format!(
                    "taskmd-named task files must be under {tasks_dir_name}/ (got '{task_file}'). \
                     Use a non-taskmd `.md` filename if you want it elsewhere."
                ));
            }
            if !ACCEPTABLE_PROPOSE_STATUSES.contains(status) {
                let allowed: Vec<&str> = ACCEPTABLE_PROPOSE_STATUSES
                    .iter()
                    .map(taskmd_core::constants::Status::as_str)
                    .collect();
                return Err(format!(
                    "task file status '{status}' cannot be proposed for approval. \
                     Acceptable statuses: {}.",
                    allowed.join(", ")
                ));
            }
        }
        TaskSource::PlainMarkdown { .. } => {
            // Any markdown file inside the worktree is acceptable as a plain
            // task brief. (Keeping plain-markdown task files *outside* the
            // tasks dir is a convention the agent prompt recommends, not a
            // hard rule — there is nothing structurally wrong with one there.)
        }
    }

    let abs_path = cwd.join(rel_path);
    let meta = abs_path.symlink_metadata().map_err(|e| {
        format!(
            "Failed to read task file '{task_file}': {e}. \
             Create the file in your working directory before calling propose_task."
        )
    })?;
    // The task file must be a plain regular file: a symlink would show the
    // target's contents in the approval reader but `git add <task_file>` would
    // stage the *link*, so the committed plan wouldn't match what was reviewed
    // (and a symlink could also point outside the worktree).
    if !meta.file_type().is_file() {
        return Err(format!(
            "task_file '{task_file}' must be a regular file — not a symlink, directory, \
             or special file."
        ));
    }
    // Belt-and-suspenders against an intermediate symlink in the path that
    // escapes the worktree (the lexical `..` check doesn't catch those):
    // resolve everything and confirm it stays under `cwd`.
    let abs_canon = abs_path
        .canonicalize()
        .map_err(|e| format!("Failed to resolve task file '{task_file}': {e}"))?;
    let cwd_canon = cwd
        .canonicalize()
        .map_err(|e| format!("Failed to resolve the working directory: {e}"))?;
    if !abs_canon.starts_with(&cwd_canon) {
        return Err(format!(
            "task_file '{task_file}' resolves outside your working directory. \
             The task file must be a real file inside the worktree."
        ));
    }
    let body = std::fs::read_to_string(&abs_path)
        .map_err(|e| format!("Failed to read task file '{task_file}': {e}"))?;

    let title = source.title(&body);
    let priority = source.priority();
    let plan = body.trim().to_string();

    Ok(TaskFileSnapshot {
        task_file: task_file.replace('\\', "/"),
        title,
        priority,
        plan,
        body_raw: body,
    })
}

/// Whether `dir` (or any ancestor) is inside a git repository.
///
/// A `.git` entry is matched whether it is a directory (ordinary repo), a file
/// (linked worktree / submodule, where `.git` is a `gitdir:` pointer), so a
/// Direct origin started inside any git checkout is recognised. Like
/// [`resolve_task_file`], this is a deterministic, bounded, local FS read the
/// state machine treats as a data-load, not an external side effect: it gates
/// the fork path (REQ-PROJ-036 — a Direct origin must be git-backed to fork).
fn is_git_repository(dir: &Path) -> bool {
    dir.ancestors().any(|a| a.join(".git").exists())
}

/// The retry-budget ceiling for retryable LLM errors. Surfaced via
/// `SseEvent::LlmAttempt.max_attempts` on every retry-scheduling event
/// so the client can render `(retry K/N <reason>)` (specs/llm-retry-visibility/
/// REQ-LRV-001). `pub` exposure is for the executor's
/// `Effect::ScheduleRetry` handler, which sends the value on the wire.
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Stamp `retry_count = saturating_sub(1)` onto an assistant message's
/// `display_data` (the JSON-blob form `Option<serde_json::Value>`) iff
/// the final attempt count is > 1. No-op for first-try successes so
/// the persisted JSON stays minimal and the UI's
/// `retry_count > 0` check doubles as a presence check. Used by both
/// the `persist_agent_message` helper (no-tool `LlmResponse` path) and
/// the tool-round inline `assistant_message` build
/// (`handle_core_llm_response`'s `ToolExecuting` branch). Specs:
/// `specs/llm-retry-visibility/` REQ-LRV-006.
fn stamp_retry_count(display_data: &mut Option<serde_json::Value>, final_attempt: u32) {
    let retry_count = final_attempt.saturating_sub(1);
    if retry_count == 0 {
        return;
    }
    let display_obj =
        display_data.get_or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(map) = display_obj.as_object_mut() {
        map.insert(
            "retry_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(retry_count)),
        );
    }
}

/// Project a runtime `ErrorKind` onto the three retryable
/// `LlmAttemptReason` variants. `is_auto_retryable()` is the gate for
/// `Effect::ScheduleRetry`; this helper is the wire-side projection of
/// the same predicate. `TimedOut` collapses to `Network` because the
/// upstream `LlmErrorKind` never distinguishes them (timeouts arrive
/// as `LlmErrorKind::Network`); the runtime keeps the kinds separate
/// for non-LLM paths but the `LlmAttempt` reader doesn't care.
fn error_kind_to_attempt_reason(kind: &ErrorKind) -> LlmAttemptReason {
    match kind {
        ErrorKind::RateLimit => LlmAttemptReason::RateLimit,
        // A malformed response is retryable; its transient retry banner reuses
        // the `server_error` reason rather than widening the spec'd
        // `{rate_limit, server_error, network}` wire set.
        ErrorKind::ServerError | ErrorKind::InvalidResponse => LlmAttemptReason::ServerError,
        ErrorKind::Network | ErrorKind::TimedOut => LlmAttemptReason::Network,
        // Non-retryable kinds. `db::ErrorKind::is_auto_retryable` admits
        // exactly the four kinds matched above, and every caller guards on it
        // before reaching here, so a non-retryable kind landing in this arm is
        // a runtime invariant violation. Log it (a wrong `reason` on the wire
        // is a capability gap, not a crash — keep returning a well-formed
        // value rather than panicking the conversation).
        ErrorKind::Auth
        | ErrorKind::UsageLimitReached
        | ErrorKind::ServerOverloaded
        | ErrorKind::InvalidRequest
        | ErrorKind::Cancelled
        | ErrorKind::SubAgentError
        | ErrorKind::ContextExhausted
        | ErrorKind::ContentFilter => {
            tracing::error!(
                ?kind,
                "error_kind_to_attempt_reason reached with a non-retryable kind; \
                 is_auto_retryable() guard should preclude this — defaulting wire reason to network"
            );
            LlmAttemptReason::Network
        }
    }
}

/// Discriminator for a `propose_task` tool call found in an LLM response: the
/// payload either parsed into the typed input or failed serde (carrying the
/// error string). Both cases are intercepted in the same arm of
/// `transition_parent`; this enum keeps the dispatch exhaustive without an
/// `unreachable!()`. See task 13018 follow-up.
enum ProposeTaskCall<'a> {
    Typed(&'a super::state::ProposeTaskInput),
    Malformed(&'a str),
}

/// Sibling of [`ProposeTaskCall`] for `ask_user_question`.
enum AskUserQuestionCall<'a> {
    Typed(&'a super::state::AskUserQuestionInput),
    Malformed(&'a str),
}

/// Result of a state transition
#[derive(Debug)]
pub struct TransitionResult {
    pub new_state: ConvState,
    pub effects: Vec<Effect>,
}

impl TransitionResult {
    #[must_use]
    pub fn new(state: ConvState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    #[must_use]
    pub fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    #[allow(dead_code)] // Builder method
    #[must_use]
    pub fn with_effects(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.effects.extend(effects);
        self
    }
}

/// Errors that can occur during transition.
///
/// Every variant is either payload-free or carries structured data. In
/// particular `InvalidTransition` carries `&'static str` discriminators
/// sourced from [`ConvState::variant_name`] / [`Event::variant_name`]
/// instead of a `format!("{state:?}/{event:?}")` dump — task 24682
/// follow-up. This means `Display`-formatting a `TransitionError`
/// anywhere in the codebase produces output that is always safe to
/// show to humans, never leaks the internal `Debug` shape of
/// `ConvState` or `Event`, and never embeds payload data.
#[derive(Debug, Error)]
pub enum TransitionError {
    #[error("Agent is busy, cannot accept message (cancel current operation first)")]
    AgentBusy,
    #[error("Cancellation in progress")]
    CancellationInProgress,
    #[error("Context window exhausted, please start a new conversation")]
    ContextExhausted,
    #[error("Conversation is awaiting task approval")]
    AwaitingTaskApproval,
    #[error("Conversation is awaiting user response to questions")]
    AwaitingUserResponse,
    #[error("Conversation has reached terminal state (completed or abandoned)")]
    ConversationTerminal,
    #[error("Invalid transition: no arm for state={state} event={event}")]
    InvalidTransition {
        /// Variant name of the `ConvState` that didn't have a matching
        /// transition arm, e.g. `"Idle"`. Populated via
        /// [`ConvState::variant_name`]. Never contains payload data.
        state: &'static str,
        /// Variant name of the `Event` we were trying to apply, e.g.
        /// `"UserCancel"`. Populated via [`Event::variant_name`].
        /// Never contains payload data.
        event: &'static str,
    },
}

/// Synchronously check whether a `UserMessage` event would be accepted by
/// `transition()` for the given parent-conversation state.
///
/// Returns `Ok(())` if the state would accept (and persist + send to LLM) a
/// user message. Returns the same `TransitionError` variant `transition()`
/// would produce, so callers can map it to a typed HTTP error.
///
/// Used by the `/api/conversations/:id/chat` handler to fail-fast with a 409
/// instead of silently queueing the event into a runtime executor that will
/// drop it when the executor wakes and observes a rejecting state. Without
/// this precheck the chat POST returns 200, the optimistic UI transitions to
/// `awaiting_llm`, and the only signal of rejection is a server-side log line
/// (`"Transition rejected"`) — leaving the UI permanently in optimistic
/// `awaiting_llm` while the server stays in the rejecting state.
///
/// The arms here mirror the rejection arms of `transition_core` and
/// `transition_parent` for `UserMessage` events. Drift is caught by
/// `prop_check_user_message_acceptable_matches_transition` in proptests.rs.
///
/// Only valid for parent (top-level) conversations. Sub-agent conversations
/// don't accept chat HTTP traffic.
///
/// # Errors
///
/// Returns [`TransitionError`] when the conversation is in a state that does
/// not accept a new user message.
pub fn check_user_message_acceptable(state: &ConvState) -> Result<(), TransitionError> {
    match state {
        // Idle and Error: transition_core arm (Idle | Error, UserMessage) → LlmRequesting
        ConvState::Idle | ConvState::Error { .. } => Ok(()),

        // transition_core: AgentBusy
        ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::AwaitingSubAgents { .. } => Err(TransitionError::AgentBusy),

        // transition_core: CancellationInProgress
        ConvState::CancellingTool { .. } | ConvState::CancellingSubAgents { .. } => {
            Err(TransitionError::CancellationInProgress)
        }

        // transition_parent: explicit reject arms
        ConvState::AwaitingTaskApproval { .. } => Err(TransitionError::AwaitingTaskApproval),
        ConvState::AwaitingUserResponse { .. } => Err(TransitionError::AwaitingUserResponse),
        ConvState::ContextExhausted { .. } => Err(TransitionError::ContextExhausted),
        ConvState::HandedOff { .. } | ConvState::Terminal => {
            Err(TransitionError::ConversationTerminal)
        }

        // No explicit arm in transition_core/transition_parent — falls through
        // to the catch-all `InvalidTransition`. Enumerated explicitly so
        // adding a new state forces a decision here.
        ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. } => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: "UserMessage",
        }),
    }
}

/// Pure transition function — compatibility wrapper.
///
/// Dispatches to `transition_parent` or `transition_sub_agent` based on
/// `context.is_sub_agent`. `ConvState`/`Event` are converted to the split types,
/// the result is converted back to `ConvState`. This preserves the existing API
/// while the split functions enforce structural type safety.
///
/// REQ-BED-001: This function is pure - given the same inputs, it always
/// produces the same outputs, with no I/O side effects.
///
/// # Errors
///
/// Returns [`TransitionError`] when the event is not valid for the current
/// state.
pub fn transition(
    state: &ConvState,
    context: &ConvContext,
    event: Event,
) -> Result<TransitionResult, TransitionError> {
    if context.is_sub_agent {
        let sub_state = SubAgentState::try_from(state.clone()).map_err(|e| {
            TransitionError::InvalidTransition {
                state: e.from_variant,
                event: event.variant_name(),
            }
        })?;
        let sub_event = match SubAgentEvent::try_from(event) {
            Ok(e) => e,
            Err(e) => {
                // Parent-only events reaching a sub-agent context are invalid.
                // Terminal states absorb; non-terminal states reject.
                if sub_state.is_terminal() {
                    tracing::debug!(
                        event = e.event_variant,
                        state = state.variant_name(),
                        "absorbing parent-only event in terminal sub-agent state"
                    );
                    return Ok(TransitionResult::new(state.clone()));
                }
                return Err(TransitionError::InvalidTransition {
                    state: state.variant_name(),
                    event: e.event_variant,
                });
            }
        };
        let result = transition_sub_agent(&sub_state, context, sub_event)?;
        Ok(result.into_conv_result())
    } else {
        let parent_state = ParentState::try_from(state.clone()).map_err(|e| {
            TransitionError::InvalidTransition {
                state: e.from_variant,
                event: event.variant_name(),
            }
        })?;
        let parent_event = match ParentEvent::try_from(event) {
            Ok(e) => e,
            Err(e) => {
                // Sub-agent-only events reaching a parent context are stale/invalid.
                // Terminal states absorb; non-terminal states reject.
                if parent_state.is_terminal() {
                    tracing::debug!(
                        event = e.event_variant,
                        state = state.variant_name(),
                        "absorbing sub-agent-only event in terminal parent state"
                    );
                    return Ok(TransitionResult::new(state.clone()));
                }
                return Err(TransitionError::InvalidTransition {
                    state: state.variant_name(),
                    event: e.event_variant,
                });
            }
        };
        let result = transition_parent(&parent_state, context, parent_event)?;
        Ok(result.into_conv_result())
    }
}

// ============================================================================
// Split transition functions — CoreState, ParentState, SubAgentState
// ============================================================================

/// Result of a parent state transition
#[derive(Debug)]
pub struct ParentTransitionResult {
    pub new_state: ParentState,
    pub effects: Vec<Effect>,
}

impl ParentTransitionResult {
    fn new(state: ParentState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_conv_result(self) -> TransitionResult {
        TransitionResult {
            new_state: self.new_state.into(),
            effects: self.effects,
        }
    }
}

/// Result of a sub-agent state transition
#[derive(Debug)]
pub struct SubAgentTransitionResult {
    pub new_state: SubAgentState,
    pub effects: Vec<Effect>,
}

impl SubAgentTransitionResult {
    fn new(state: SubAgentState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_conv_result(self) -> TransitionResult {
        TransitionResult {
            new_state: self.new_state.into(),
            effects: self.effects,
        }
    }
}

/// Result of a core state transition
#[derive(Debug)]
pub struct CoreTransitionResult {
    pub new_state: CoreState,
    pub effects: Vec<Effect>,
}

impl CoreTransitionResult {
    fn new(state: CoreState) -> Self {
        Self {
            new_state: state,
            effects: vec![],
        }
    }

    fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    fn into_parent_result(self) -> ParentTransitionResult {
        ParentTransitionResult {
            new_state: ParentState::Core(self.new_state),
            effects: self.effects,
        }
    }

    fn into_sub_agent_result(self) -> SubAgentTransitionResult {
        SubAgentTransitionResult {
            new_state: SubAgentState::Core(self.new_state),
            effects: self.effects,
        }
    }
}

// ============================================================================
// transition_core — shared behavior for both parent and sub-agent
// ============================================================================

/// Core transition function handling behavior shared by both conversation types.
///
/// Routes (state, event) pairs to domain-specific handlers. Each handler is
/// independently testable with explicit inputs and outputs.
///
/// Does NOT handle: `propose_task` interception (parent-only), terminal tools
/// (sub-agent-only), `LlmError` -> `Error` vs `Failed` (diverges by type),
/// `UserCancel` from `LlmRequesting` (parent -> `Idle`, sub-agent -> `Failed`).
///
/// # Errors
///
/// Returns [`TransitionError`] when the core event is not valid for the
/// current core state.
#[allow(clippy::too_many_lines)]
pub fn transition_core(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, &event) {
        // User Message Handling (REQ-BED-002)
        (
            CoreState::Idle | CoreState::Error { .. },
            CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                files,
                message_id,
                user_agent,
                skill_invocation,
            },
        ) => Ok(
            CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::persist_user_message(
                    text.clone(),
                    llm_text.clone(),
                    images.clone(),
                    files.clone(),
                    message_id.clone(),
                    user_agent.clone(),
                    skill_invocation.clone(),
                    false,
                ))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm),
        ),

        (
            CoreState::LlmRequesting { .. }
            | CoreState::ToolExecuting { .. }
            | CoreState::AwaitingSubAgents { .. },
            CoreEvent::UserMessage { .. },
        )
        | (
            CoreState::ToolExecuting { .. } | CoreState::AwaitingSubAgents { .. },
            CoreEvent::SteerDrainedUserMessages { .. },
        ) => Err(TransitionError::AgentBusy),

        (
            CoreState::CancellingTool { .. } | CoreState::CancellingSubAgents { .. },
            CoreEvent::UserMessage { .. } | CoreEvent::SteerDrainedUserMessages { .. },
        ) => Err(TransitionError::CancellationInProgress),

        // LLM Response Processing (REQ-BED-003)
        (CoreState::LlmRequesting { .. }, CoreEvent::LlmResponse { .. }) => {
            handle_core_llm_response(state, context, event)
        }

        // Error Handling and Retry (REQ-BED-006)
        (CoreState::LlmRequesting { .. }, CoreEvent::LlmError { .. })
        | (CoreState::LlmRequesting { .. }, CoreEvent::RetryTimeout { .. }) => {
            handle_core_error_retry(state, event)
        }

        // Tool Execution (REQ-BED-004)
        (CoreState::ToolExecuting { .. }, CoreEvent::ToolComplete { .. })
        | (CoreState::ToolExecuting { .. }, CoreEvent::SpawnAgentsComplete { .. }) => {
            handle_core_tool_complete(state, event)
        }

        // Cancellation (REQ-BED-005)
        (CoreState::AwaitingSubAgents { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::ToolExecuting { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::LlmRequesting { .. }, CoreEvent::UserCancel { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::ToolAborted { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::ToolComplete { .. })
        | (CoreState::CancellingTool { .. }, CoreEvent::SubAgentResult { .. }) => {
            handle_core_cancellation(state, event)
        }

        // Sub-Agent Results (REQ-BED-008)
        (CoreState::AwaitingSubAgents { .. }, CoreEvent::SubAgentResult { .. })
        | (CoreState::CancellingSubAgents { .. }, CoreEvent::SubAgentResult { .. }) => {
            handle_core_sub_agents(state, event)
        }

        // Context Continuation (REQ-BED-019 through REQ-BED-024)
        (CoreState::AwaitingContinuation { .. }, CoreEvent::LlmError { .. })
        | (CoreState::AwaitingContinuation { .. }, CoreEvent::RetryTimeout { .. })
        | (CoreState::Idle, CoreEvent::UserTriggerContinuation) => {
            handle_core_continuation(state, event)
        }

        // Stale LlmResponse after cancel
        (CoreState::Idle, CoreEvent::LlmResponse { .. }) => {
            Ok(CoreTransitionResult::new(CoreState::Idle))
        }

        // Stale UserTriggerContinuation: any non-Idle Core state means the
        // conversation is already in flight (LLM round, tools, sub-agents,
        // continuation summary) or in a sub-agent terminal state. The user's
        // intent ("summarize now") is either being served by the in-flight
        // path or no longer meaningful. Absorbing avoids the SSE-vs-click
        // race surfacing as an error to the user.
        (state, CoreEvent::UserTriggerContinuation) => {
            tracing::debug!(
                state = state.variant_name(),
                "Absorbing stale UserTriggerContinuation"
            );
            Ok(CoreTransitionResult::new(state.clone()))
        }

        // Steering queue drain (REQ-SM-*): persist all entries, then ask LLM.
        // ClearSteeringQueueEntries runs AFTER persist+state so a crash mid-
        // drain leaves the queue intact for re-drain on restart, and removes
        // only the drained ids so a concurrent enqueue is preserved.
        (CoreState::Idle, CoreEvent::SteerDrainedUserMessages { entries }) => {
            if entries.is_empty() {
                return Ok(CoreTransitionResult::new(CoreState::Idle));
            }
            let drained_ids: Vec<String> = entries.iter().map(|e| e.message_id.clone()).collect();
            let mut result = CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 });
            for entry in entries {
                result = result.with_effect(steer_entry_to_persist_effect(entry));
            }
            Ok(result
                .with_effect(Effect::PersistState)
                .with_effect(Effect::ClearSteeringQueueEntries {
                    message_ids: drained_ids,
                })
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm))
        }

        // Mid-turn drain: an LLM request is already in flight (just transitioned
        // from ToolExecuting), so persist entries but do NOT issue a new RequestLlm.
        (CoreState::LlmRequesting { attempt }, CoreEvent::SteerDrainedUserMessages { entries }) => {
            let attempt = *attempt;
            if entries.is_empty() {
                return Ok(CoreTransitionResult::new(CoreState::LlmRequesting {
                    attempt,
                }));
            }
            let drained_ids: Vec<String> = entries.iter().map(|e| e.message_id.clone()).collect();
            let mut result = CoreTransitionResult::new(CoreState::LlmRequesting { attempt });
            for entry in entries {
                result = result.with_effect(steer_entry_to_persist_effect(entry));
            }
            Ok(result.with_effect(Effect::PersistState).with_effect(
                Effect::ClearSteeringQueueEntries {
                    message_ids: drained_ids,
                },
            ))
        }

        // Invalid Transitions
        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Build a `PersistMessage` effect from a queued steering entry.
/// `idempotent: true` because steering-queue re-drain after crash recovery
/// may re-emit this effect with the same `message_id`.
fn steer_entry_to_persist_effect(entry: &crate::event::SteerEntry) -> Effect {
    Effect::persist_user_message(
        entry.text.clone(),
        entry.llm_text.clone(),
        entry.images.clone(),
        entry.files.clone(),
        entry.message_id.clone(),
        entry.user_agent.clone(),
        entry.skill_invocation.clone(),
        true,
    )
}

// ============================================================================
// Domain-specific handlers for transition_core
// ============================================================================

/// Handles `LlmResponse` events when in `LlmRequesting` state.
///
/// By the time we get here, `propose_task` interception, `ask_user_question`
/// interception, context exhaustion check, and sub-agent terminal tool handling
/// have already been done by the parent/sub-agent wrappers. `LlmResponse` here
/// means "normal tool execution or text-only response."
#[allow(clippy::unnecessary_wraps)]
fn handle_core_llm_response(
    state: &CoreState,
    context: &ConvContext,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    let CoreEvent::LlmResponse {
        content,
        tool_calls,
        end_turn: _,
        usage: usage_data,
        request_id,
    } = event
    else {
        unreachable!("handle_core_llm_response called with non-LlmResponse event");
    };
    let CoreState::LlmRequesting { attempt } = state else {
        unreachable!("handle_core_llm_response called in non-LlmRequesting state");
    };
    let final_attempt = *attempt;

    if tool_calls.is_empty() && content.is_empty() {
        tracing::debug!("LLM returned end_turn with empty content — no message to persist");
        return Ok(CoreTransitionResult::new(CoreState::Idle)
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done()));
    }

    if tool_calls.is_empty() {
        return Ok(CoreTransitionResult::new(CoreState::Idle)
            .with_effect(Effect::persist_agent_message(
                content,
                Some(usage_data),
                &context.working_dir,
                request_id,
                final_attempt,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done()));
    }

    // Has tools -> ToolExecuting
    let first = tool_calls[0].clone();
    let rest = tool_calls[1..].to_vec();
    let mut display_data = compute_bash_display_data(&content, &context.working_dir);
    // REQ-LRV-006: same retry_count stamp as the no-tool path
    // (persist_agent_message above). The tool-round flow persists the
    // assistant message via PersistCheckpoint, not persist_agent_message,
    // so we have to inline the stamp here instead of going through the
    // helper.
    stamp_retry_count(&mut display_data, final_attempt);
    let assistant_message =
        AssistantMessage::new(request_id, content, Some(usage_data), display_data);
    // Broadcast (not persist) the assistant message now so the UI's main
    // message list renders the in-flight `tool_use` blocks during execution.
    // Atomic DB persistence still happens later via `PersistCheckpoint`; the
    // UI dedups the duplicate `sse_message` by `message_id`.
    let broadcast_effect = Effect::BroadcastAssistantMessage {
        message: assistant_message.clone(),
    };

    Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
        current_tool: first.clone(),
        remaining_tools: rest,
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message,
    })
    .with_effect(Effect::PersistState)
    .with_effect(broadcast_effect)
    .with_effect(Effect::notify_state_change())
    .with_effect(Effect::execute_tool(first)))
}

/// Handles `ToolComplete` and `SpawnAgentsComplete` events during `ToolExecuting` state.
#[allow(clippy::too_many_lines)]
fn handle_core_tool_complete(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    let CoreState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        pending_sub_agents,
        assistant_message,
    } = state
    else {
        unreachable!("handle_core_tool_complete called in non-ToolExecuting state");
    };

    match event {
        // ToolComplete (more tools remaining) -> next tool
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();

            Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                completed_results: new_results,
                pending_sub_agents: pending_sub_agents.clone(),
                assistant_message: assistant_message.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change())
            .with_effect(Effect::execute_tool(next_tool)))
        }

        // ToolComplete (last tool, no sub-agents) -> LlmRequesting
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in last-tool transition");

            Ok(
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm),
            )
        }

        // ToolComplete (last tool, has sub-agents) -> AwaitingSubAgents
        CoreEvent::ToolComplete {
            tool_use_id,
            result,
        } if tool_use_id == current_tool.id
            && remaining_tools.is_empty()
            && !pending_sub_agents.is_empty() =>
        {
            let mut all_results = completed_results.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect(
                    "tool_use/tool_result count mismatch in last-tool-with-subagents transition",
                );

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: pending_sub_agents.clone(),
                completed_results: vec![],
                spawn_tool_id: None,
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()))
        }

        // SpawnAgentsComplete (more tools) -> accumulate
        CoreEvent::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        } if tool_use_id == current_tool.id && !remaining_tools.is_empty() => {
            let mut new_results = completed_results.clone();
            new_results.push(result);

            let mut new_pending = pending_sub_agents.clone();
            new_pending.extend(spawned);

            let next_tool = remaining_tools[0].clone();
            let new_remaining = remaining_tools[1..].to_vec();

            Ok(CoreTransitionResult::new(CoreState::ToolExecuting {
                current_tool: next_tool.clone(),
                remaining_tools: new_remaining,
                completed_results: new_results,
                pending_sub_agents: new_pending,
                assistant_message: assistant_message.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change())
            .with_effect(Effect::execute_tool(next_tool)))
        }

        // SpawnAgentsComplete (last tool) -> AwaitingSubAgents
        CoreEvent::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        } if tool_use_id == current_tool.id && remaining_tools.is_empty() => {
            let mut all_pending = pending_sub_agents.clone();
            all_pending.extend(spawned);

            let mut all_results = completed_results.clone();
            let spawn_id = result.tool_use_id.clone();
            all_results.push(result);

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in spawn-agents-last transition");

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: all_pending.clone(),
                completed_results: vec![],
                spawn_tool_id: Some(spawn_id),
            })
            .with_effect(Effect::PersistCheckpoint { data: checkpoint })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()))
        }

        // tool_use_id mismatch or unexpected event variant
        CoreEvent::ToolComplete { .. }
        | CoreEvent::SpawnAgentsComplete { .. }
        | CoreEvent::UserMessage { .. }
        | CoreEvent::UserCancel { .. }
        | CoreEvent::LlmResponse { .. }
        | CoreEvent::LlmError { .. }
        | CoreEvent::RetryTimeout { .. }
        | CoreEvent::ToolAborted { .. }
        | CoreEvent::SubAgentResult { .. }
        | CoreEvent::ContinuationResponse { .. }
        | CoreEvent::ContinuationFailed { .. }
        | CoreEvent::UserTriggerContinuation
        | CoreEvent::SteerDrainedUserMessages { .. } => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles cancellation-related events: `UserCancel` from active states,
/// `ToolAborted`/`ToolComplete` during `CancellingTool`, `SubAgentResult` during `CancellingTool`.
#[allow(clippy::too_many_lines)]
fn handle_core_cancellation(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // AwaitingSubAgents + UserCancel -> CancellingSubAgents
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreEvent::UserCancel { cause, .. },
        ) => {
            let ids: Vec<String> = pending.iter().map(|p| p.agent_id.clone()).collect();
            Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                pending: pending.clone(),
                completed_results: completed_results.clone(),
                cause,
                spawn_tool_id: spawn_tool_id.clone(),
            })
            .with_effect(Effect::CancelSubAgents { ids })
            .with_effect(Effect::PersistState))
        }

        // ToolExecuting + UserCancel -> CancellingTool
        (
            CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                pending_sub_agents,
                assistant_message,
            },
            CoreEvent::UserCancel { .. },
        ) => {
            let mut result = CoreTransitionResult::new(CoreState::CancellingTool {
                tool_use_id: current_tool.id.clone(),
                skipped_tools: remaining_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: pending_sub_agents.clone(),
            })
            .with_effect(Effect::AbortTool {
                tool_use_id: current_tool.id.clone(),
            })
            .with_effect(Effect::PersistState);

            if !pending_sub_agents.is_empty() {
                let ids: Vec<String> = pending_sub_agents
                    .iter()
                    .map(|p| p.agent_id.clone())
                    .collect();
                result = result.with_effect(Effect::CancelSubAgents { ids });
            }

            Ok(result)
        }

        // LlmRequesting + UserCancel -> Idle
        (CoreState::LlmRequesting { .. }, CoreEvent::UserCancel { .. }) => {
            Ok(CoreTransitionResult::new(CoreState::Idle)
                .with_effect(Effect::PersistState)
                .with_effect(Effect::AbortLlm)
                .with_effect(Effect::notify_agent_done()))
        }

        // CancellingTool + ToolAborted -> Idle or CancellingSubAgents
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::ToolAborted {
                tool_use_id: aborted_id,
            },
        ) if *tool_use_id == aborted_id => {
            let all_results = build_cancellation_results(
                completed_results,
                tool_use_id,
                "Cancelled by user",
                skipped_tools,
            );

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation transition");

            if pending_sub_agents.is_empty() {
                Ok(CoreTransitionResult::new(CoreState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                    cause: CancelCause::UserRequested,
                    spawn_tool_id: None,
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + ToolComplete -> Idle or CancellingSubAgents
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::ToolComplete {
                tool_use_id: completed_id,
                result: _,
            },
        ) if *tool_use_id == completed_id => {
            let all_results = build_cancellation_results(
                completed_results,
                tool_use_id,
                "Cancelled by user",
                skipped_tools,
            );

            let checkpoint = CheckpointData::tool_round(assistant_message.clone(), all_results)
                .expect("tool_use/tool_result count mismatch in cancellation-complete transition");

            if pending_sub_agents.is_empty() {
                Ok(CoreTransitionResult::new(CoreState::Idle)
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_agent_done()))
            } else {
                Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                    pending: pending_sub_agents.clone(),
                    completed_results: vec![],
                    cause: CancelCause::UserRequested,
                    spawn_tool_id: None,
                })
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState))
            }
        }

        // CancellingTool + SubAgentResult -> absorb
        (
            CoreState::CancellingTool {
                tool_use_id,
                skipped_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            },
            CoreEvent::SubAgentResult { agent_id, .. },
        ) if pending_sub_agents.iter().any(|p| p.agent_id == agent_id) => {
            let new_pending: Vec<_> = pending_sub_agents
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            Ok(CoreTransitionResult::new(CoreState::CancellingTool {
                tool_use_id: tool_use_id.clone(),
                skipped_tools: skipped_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: new_pending,
            })
            .with_effect(Effect::PersistState))
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Builds the tool results list for cancellation transitions, including the
/// cancelled current tool and skipped remaining tools.
fn build_cancellation_results(
    completed_results: &[ToolResult],
    cancelled_tool_id: &str,
    cancel_reason: &str,
    skipped_tools: &[ToolCall],
) -> Vec<ToolResult> {
    let mut all_results = completed_results.to_vec();
    all_results.push(ToolResult::cancelled(
        cancelled_tool_id.to_string(),
        cancel_reason,
    ));
    for tool in skipped_tools {
        all_results.push(ToolResult::cancelled(
            tool.id.clone(),
            "Skipped due to cancellation",
        ));
    }
    all_results
}

/// Map a sub-agent's reported outcome to the outcome recorded when the parent
/// is tearing the sub-agent down (task 61004). A real `Success` always wins
/// (fidelity); a timeout-caused teardown records `TimedOut`; a user-requested
/// cancel keeps the reported outcome (e.g. a `Failure { Cancelled }`).
fn map_teardown_outcome(outcome: SubAgentOutcome, cause: CancelCause) -> SubAgentOutcome {
    match (&outcome, cause) {
        // A real success always wins (fidelity); a user-requested cancel keeps
        // the reported outcome (e.g. Failure { Cancelled }).
        (SubAgentOutcome::Success { .. }, _) | (_, CancelCause::UserRequested) => outcome,
        // A timeout-caused teardown records TimedOut over any non-success outcome.
        (_, CancelCause::Timeout) => SubAgentOutcome::TimedOut,
    }
}

/// Handles `SubAgentResult` events in `AwaitingSubAgents` and `CancellingSubAgents` states.
#[allow(clippy::too_many_lines)]
fn handle_core_sub_agents(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // AwaitingSubAgents + SubAgentResult (more pending)
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() > 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let new_pending: Vec<_> = pending
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome,
            });

            let notify = Effect::notify_state_change();

            Ok(CoreTransitionResult::new(CoreState::AwaitingSubAgents {
                pending: new_pending,
                completed_results: new_results,
                spawn_tool_id: spawn_tool_id.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(notify))
        }

        // AwaitingSubAgents + SubAgentResult (last one) -> LlmRequesting
        (
            CoreState::AwaitingSubAgents {
                pending,
                completed_results,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() == 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome,
            });

            Ok(
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: 1 })
                    .with_effect(Effect::PersistSubAgentResults {
                        results: new_results,
                        spawn_tool_id: spawn_tool_id.clone(),
                    })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm),
            )
        }

        // CancellingSubAgents + SubAgentResult (more pending)
        (
            CoreState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() > 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let new_pending: Vec<_> = pending
                .iter()
                .filter(|p| p.agent_id != agent_id)
                .cloned()
                .collect();
            let recorded = map_teardown_outcome(outcome, *cause);
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome: recorded,
            });

            Ok(CoreTransitionResult::new(CoreState::CancellingSubAgents {
                pending: new_pending,
                completed_results: new_results,
                cause: *cause,
                spawn_tool_id: spawn_tool_id.clone(),
            })
            .with_effect(Effect::PersistState))
        }

        // CancellingSubAgents + SubAgentResult (last one) -> Idle
        (
            CoreState::CancellingSubAgents {
                pending,
                completed_results,
                cause,
                spawn_tool_id,
            },
            CoreEvent::SubAgentResult { agent_id, outcome },
        ) if pending.iter().any(|p| p.agent_id == agent_id) && pending.len() == 1 => {
            let task = pending
                .iter()
                .find(|p| p.agent_id == agent_id)
                .map(|p| p.task.clone())
                .unwrap_or_default();
            let recorded = map_teardown_outcome(outcome, *cause);
            let mut new_results = completed_results.clone();
            new_results.push(SubAgentResult {
                agent_id,
                task,
                outcome: recorded,
            });

            let result = CoreTransitionResult::new(CoreState::Idle);
            // Persist the drained results back onto the originating spawn_agents
            // tool message only when we know its id (AwaitingSubAgents-origin).
            // For the CancellingTool-origin path no spawn id exists, so persisting
            // would fabricate a tool_result with a random id and no matching
            // tool_use — an orphan the provider rejects. Skip persistence there,
            // matching that path's prior behaviour.
            let result = if let Some(id) = spawn_tool_id {
                result.with_effect(Effect::PersistSubAgentResults {
                    results: new_results,
                    spawn_tool_id: Some(id.clone()),
                })
            } else {
                result
            };
            Ok(result
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()))
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles `LlmError` and `RetryTimeout` events during `LlmRequesting` state.
fn handle_core_error_retry(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // Retryable LlmError below max -> retry (shared)
        (
            CoreState::LlmRequesting { attempt },
            CoreEvent::LlmError {
                ref error_kind,
                resets_at,
                ..
            },
        ) if error_kind.is_auto_retryable() && *attempt < MAX_RETRY_ATTEMPTS => {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);
            let reason = error_kind_to_attempt_reason(error_kind);

            Ok(CoreTransitionResult::new(CoreState::LlmRequesting {
                attempt: new_attempt,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::ScheduleRetry {
                delay,
                attempt: new_attempt,
                reason,
                resets_at,
            })
            .with_effect(Effect::notify_state_change()))
        }

        // Non-retryable or exhausted LlmError -> Error (core default)
        (
            CoreState::LlmRequesting { attempt },
            CoreEvent::LlmError {
                message,
                error_kind,
                resets_at,
                ..
            },
        ) => {
            let error_message = if error_kind.is_auto_retryable() {
                format!("Failed after {attempt} attempts: {message}")
            } else {
                message
            };

            Ok(CoreTransitionResult::new(CoreState::Error {
                message: error_message.clone(),
                error_kind,
                // Carry the quota-window reset time so the auto-clear sweep can
                // return the conversation to Idle once the window passes. Only
                // a usage-limit 429 populates this; None for every other error.
                resets_at,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()))
        }

        // RetryTimeout -> Make LLM request
        (
            CoreState::LlmRequesting { attempt },
            CoreEvent::RetryTimeout {
                attempt: retry_attempt,
            },
        ) if *attempt == retry_attempt => {
            Ok(
                CoreTransitionResult::new(CoreState::LlmRequesting { attempt: *attempt })
                    .with_effect(Effect::RequestLlm),
            )
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

/// Handles continuation-related events: `LlmError`/`RetryTimeout` during
/// `AwaitingContinuation`, and `UserTriggerContinuation` from Idle.
fn handle_core_continuation(
    state: &CoreState,
    event: CoreEvent,
) -> Result<CoreTransitionResult, TransitionError> {
    match (state, event) {
        // LlmError during continuation - retry
        (
            CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            CoreEvent::LlmError {
                ref error_kind,
                resets_at,
                ..
            },
        ) if error_kind.is_auto_retryable() && *attempt < MAX_RETRY_ATTEMPTS => {
            let new_attempt = attempt + 1;
            let delay = retry_delay(new_attempt);
            let reason = error_kind_to_attempt_reason(error_kind);

            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: new_attempt,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::ScheduleRetry {
                delay,
                attempt: new_attempt,
                reason,
                resets_at,
            })
            .with_effect(Effect::notify_state_change()))
        }

        // RetryTimeout during continuation
        (
            CoreState::AwaitingContinuation {
                rejected_tool_calls,
                attempt,
            },
            CoreEvent::RetryTimeout {
                attempt: timeout_attempt,
            },
        ) if *attempt == timeout_attempt => {
            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: *attempt,
            })
            .with_effect(Effect::RequestContinuation {
                request: ContinuationSummaryRequest {
                    rejected_tool_calls: rejected_tool_calls.clone(),
                },
            }))
        }

        // UserTriggerContinuation from Idle (REQ-BED-023)
        (CoreState::Idle, CoreEvent::UserTriggerContinuation) => {
            Ok(CoreTransitionResult::new(CoreState::AwaitingContinuation {
                rejected_tool_calls: vec![],
                attempt: 1,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change())
            .with_effect(Effect::RequestContinuation {
                request: ContinuationSummaryRequest {
                    rejected_tool_calls: vec![],
                },
            }))
        }

        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

// ============================================================================
// transition_parent — parent-specific transitions, delegates core
// ============================================================================

/// Parent transition function. Handles parent-only states and events, delegates
/// core state + core event combinations to `transition_core`.
///
/// # Errors
///
/// Returns [`TransitionError`] when the parent event is not valid for the
/// current parent state.
///
/// # Panics
///
/// Panics if internal invariants are violated — e.g. a per-tool result count
/// that does not match the tool-call count, or a core transition that yields a
/// state the parent layer cannot represent. These reflect reducer bugs, not
/// reachable inputs.
#[allow(clippy::too_many_lines)]
pub fn transition_parent(
    state: &ParentState,
    context: &ConvContext,
    event: ParentEvent,
) -> Result<ParentTransitionResult, TransitionError> {
    match (state, event) {
        // ============================================================
        // Parent-only state: AwaitingTaskApproval
        // ============================================================
        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. } | CoreEvent::UserTriggerContinuation),
        ) => Err(TransitionError::AwaitingTaskApproval),

        (
            ParentState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided {
                outcome:
                    TaskApprovalOutcome::Approved {
                        handoff: TaskApprovalHandoff::ContinueInCurrentConversation,
                    },
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
                .with_effect(Effect::ApproveTask {
                    task_file: task_file.clone(),
                    title: title.clone(),
                    priority: *priority,
                    plan: plan.clone(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm),
        ),
        (
            ParentState::AwaitingTaskApproval {
                task_file,
                title,
                priority,
                plan,
            },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided {
                outcome:
                    TaskApprovalOutcome::Approved {
                        handoff: TaskApprovalHandoff::StartFreshWorkConversation,
                    },
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::AwaitingTaskApproval {
                task_file: task_file.clone(),
                title: title.clone(),
                priority: *priority,
                plan: plan.clone(),
            })
            .with_effect(Effect::ApproveTaskFreshHandoff {
                task_file: task_file.clone(),
                title: title.clone(),
                priority: *priority,
                plan: plan.clone(),
            }),
        ),

        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskHandoffComplete { successor_conv_id }),
        ) => Ok(ParentTransitionResult::new(ParentState::HandedOff {
            successor_conv_id: successor_conv_id.clone(),
        })
        .with_effect(Effect::PersistState)
        .with_effect(Effect::NotifyStateChange)
        .with_effect(Effect::notify_agent_done())),

        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::FeedbackProvided { annotations },
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting { attempt: 1 }))
                .with_effect(Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::system(
                        "Plan not approved. The user provided feedback below. \
                         You must call propose_task again with a revised plan \
                         that addresses their feedback.",
                    ),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                    idempotent: false,
                })
                .with_effect(Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::user(annotations),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                    idempotent: false,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm),
        ),

        (
            ParentState::AwaitingTaskApproval { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::Rejected,
            })
            | ParentEvent::Core(CoreEvent::UserCancel { .. }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::Idle))
                .with_effect(Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::system(
                        "Task rejected.",
                    ),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                    idempotent: false,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_agent_done()),
        ),

        // ============================================================
        // Parent-only state: AwaitingUserResponse
        // ============================================================
        (
            ParentState::AwaitingUserResponse { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. } | CoreEvent::UserTriggerContinuation),
        ) => Err(TransitionError::AwaitingUserResponse),

        (
            ParentState::AwaitingUserResponse { .. },
            ParentEvent::Parent(ParentOnlyEvent::UserQuestionDismissed),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::Idle))
                .with_effect(Effect::PersistHiddenSystemMarker {
                    marker: USER_QUESTION_DISMISSED_MARKER,
                    message_id: uuid::Uuid::new_v4().to_string(),
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change()),
        ),

        (
            ParentState::AwaitingUserResponse { questions, .. },
            ParentEvent::Parent(ParentOnlyEvent::UserQuestionResponse {
                answers,
                annotations,
            }),
        ) => {
            let answers_text = questions
                .iter()
                .filter_map(|q| {
                    let a = answers.get(&q.question)?;
                    let q_text = &q.question;
                    let mut parts = vec![format!("\"{}\" = \"{}\"", q_text, a)];
                    let question_data = questions.iter().find(|qq| qq.question == *q_text);
                    if let Some(qd) = question_data {
                        let selected_preview = qd
                            .options
                            .iter()
                            .find(|o| o.label == *a)
                            .and_then(|o| o.preview.as_deref());
                        if let Some(preview) = selected_preview {
                            parts.push(format!("selected preview:\n{preview}"));
                        }
                    }
                    if let Some(ref anns) = annotations {
                        if let Some(ann) = anns.get(q_text.as_str()) {
                            if let Some(ref notes) = ann.notes {
                                parts.push(format!("user notes: {notes}"));
                            }
                        }
                    }
                    Some(parts.join(" "))
                })
                .collect::<Vec<_>>()
                .join("\n");

            let user_text = format!("Here are my answers:\n{answers_text}");

            Ok(
                ParentTransitionResult::new(ParentState::Core(CoreState::LlmRequesting {
                    attempt: 1,
                }))
                .with_effect(Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::user(user_text),
                    display_data: None,
                    usage_data: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                    idempotent: false,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm),
            )
        }

        // ============================================================
        // Parent-only state: AwaitingRecovery (REQ-BED-030)
        // ============================================================
        (
            ParentState::AwaitingRecovery { resume, .. },
            ParentEvent::Parent(ParentOnlyEvent::CredentialBecameAvailable),
        ) => match resume {
            RecoveryResumeTarget::ConversationTurn => Ok(ParentTransitionResult::new(
                ParentState::Core(CoreState::LlmRequesting { attempt: 1 }),
            )
            .with_effect(Effect::PersistState)
            .with_effect(Effect::RequestLlm)),
            RecoveryResumeTarget::ContinuationSummary { request } => Ok(
                ParentTransitionResult::new(ParentState::Core(CoreState::AwaitingContinuation {
                    rejected_tool_calls: request.rejected_tool_calls.clone(),
                    attempt: 1,
                }))
                .with_effect(Effect::PersistState)
                .with_effect(Effect::RequestContinuation {
                    request: request.clone(),
                }),
            ),
        },

        (
            ParentState::AwaitingRecovery { error_kind, .. },
            ParentEvent::Parent(ParentOnlyEvent::CredentialHelperFailed { message }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Core(CoreState::Error {
                message: message.clone(),
                error_kind: error_kind.clone(),
                resets_at: None,
            }))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()),
        ),

        (ParentState::AwaitingRecovery { .. }, ParentEvent::Core(CoreEvent::UserCancel { .. })) => {
            Ok(
                ParentTransitionResult::new(ParentState::Core(CoreState::Idle))
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change()),
            )
        }

        // Error dismissal: Error -> Idle, guarded to user-resumable errors.
        // The resumable-only policy and its rationale live in the bedrock spec
        // (core_status dismissal). Non-resumable errors fall through to
        // InvalidTransition.
        (
            ParentState::Core(CoreState::Error { error_kind, .. }),
            ParentEvent::Parent(ParentOnlyEvent::DismissError),
        ) if error_kind.is_user_resumable() => Ok(ParentTransitionResult::new(ParentState::Core(
            CoreState::Idle,
        ))
        // Persist a hidden dismissal marker as the last message so a restart's
        // recovery heuristic does not auto-continue a turn the user dismissed
        // (see ERROR_DISMISSED_MARKER).
        .with_effect(Effect::PersistHiddenSystemMarker {
            marker: ERROR_DISMISSED_MARKER,
            message_id: uuid::Uuid::new_v4().to_string(),
        })
        .with_effect(Effect::PersistState)
        .with_effect(Effect::notify_state_change())),

        // ============================================================
        // Task resolution: terminal cleanup (mark-merged / abandon) ->
        // Terminal (REQ-BED-029).
        //
        // Reachable from a *stuck* conversation, not just Idle: an Error
        // (e.g. a usage-limit window the user merged around externally) or a
        // ContextExhausted parent whose work was merged externally must still
        // be disposable without first forcing a successful LLM turn. All three
        // converge to Terminal via the same ResolveTask effect. This arm
        // precedes the ContextExhausted catch-all below so TaskResolved is not
        // swallowed as a no-op for that state.
        // ============================================================
        (
            ParentState::Core(CoreState::Idle | CoreState::Error { .. })
            | ParentState::ContextExhausted { .. },
            ParentEvent::Parent(ParentOnlyEvent::TaskResolved {
                system_message,
                repo_root,
            }),
        ) => Ok(
            ParentTransitionResult::new(ParentState::Terminal).with_effect(Effect::ResolveTask {
                system_message,
                repo_root,
            }),
        ),

        // ============================================================
        // Parent-only state: ContextExhausted
        // ============================================================
        (
            ParentState::ContextExhausted { .. },
            ParentEvent::Core(CoreEvent::UserMessage { .. }),
        ) => Err(TransitionError::ContextExhausted),

        (state @ ParentState::ContextExhausted { .. }, _event) => {
            Ok(ParentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Parent-only state: Terminal / handed-off
        // ============================================================
        (ParentState::HandedOff { .. }, ParentEvent::Core(CoreEvent::UserMessage { .. }))
        | (ParentState::Terminal, ParentEvent::Core(CoreEvent::UserMessage { .. })) => {
            Err(TransitionError::ConversationTerminal)
        }

        (state @ ParentState::HandedOff { .. }, _event) => {
            Ok(ParentTransitionResult::new(state.clone()))
        }

        (ParentState::Terminal, _event) => Ok(ParentTransitionResult::new(ParentState::Terminal)),

        // ============================================================
        // Parent-specific LLM response interceptions (before core)
        //
        // Combined into a single match arm to avoid borrow-after-move
        // issues with guards on the same event payload.
        // ============================================================
        (
            ParentState::Core(CoreState::LlmRequesting { attempt }),
            ParentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                usage: usage_data,
                request_id,
                ..
            }),
        ) => {
            let final_attempt = *attempt;
            // REQ-LRV-006: stamp the retry count onto every parent-intercepted
            // assistant message (propose_task / ask_user_question — typed,
            // malformed, and validation-retry branches all persist a
            // checkpointed AssistantMessage). Without this the retry audit
            // trail is missing on exactly these tool replies, unlike the
            // normal no-tool/tool paths. `content` is a parameter (not
            // captured by ref) so each branch can still move its own `content`
            // into `AssistantMessage::new` after calling this.
            let make_display_data = |c: &[phoenix_core::domain::llm_types::ContentBlock]| {
                let mut dd = compute_bash_display_data(c, &context.working_dir);
                stamp_retry_count(&mut dd, final_attempt);
                dd
            };
            // REQ-BED-028: propose_task interception (checked first).
            //
            // Find the propose_task call whether it parsed as typed input or
            // failed serde (ToolInput::Malformed{name: "propose_task"}). The
            // two cases dispatch differently but share the same pre-checks
            // (mode, "must be the only tool") because they are both the LLM
            // calling propose_task — only the payload differs.
            if let Some((tool, call)) = tool_calls.iter().find_map(|t| match &t.input {
                ToolInput::ProposeTask(input) => Some((t, ProposeTaskCall::Typed(input))),
                ToolInput::Malformed { name, error, .. } if name == "propose_task" => {
                    Some((t, ProposeTaskCall::Malformed(error.as_str())))
                }
                ToolInput::Bash(_)
                | ToolInput::Think(_)
                | ToolInput::Patch(_)
                | ToolInput::KeywordSearch(_)
                | ToolInput::ReadImage(_)
                | ToolInput::SpawnAgents(_)
                | ToolInput::SubmitResult(_)
                | ToolInput::SubmitError(_)
                | ToolInput::AskUserQuestion(_)
                | ToolInput::Unknown { .. }
                | ToolInput::Malformed { .. } => None,
            }) {
                if tool_calls.len() > 1 {
                    let msg = "propose_task must be the only tool in response".to_string();
                    let display_data = make_display_data(&content);
                    let assistant_message = AssistantMessage::new(
                        request_id.clone(),
                        content,
                        Some(usage_data),
                        display_data,
                    );
                    let error_results: Vec<ToolResult> = tool_calls
                        .iter()
                        .map(|t| ToolResult::error(t.id.clone(), msg.clone()))
                        .collect();
                    let checkpoint = CheckpointData::tool_round(assistant_message, error_results)
                        .expect("error_results.len() == tool_calls.len()");
                    return Ok(ParentTransitionResult::new(ParentState::Core(
                        CoreState::LlmRequesting { attempt: 1 },
                    ))
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm));
                }

                let input = match call {
                    ProposeTaskCall::Typed(input) => input,
                    ProposeTaskCall::Malformed(err) => {
                        // The payload failed to deserialise into ProposeTaskInput.
                        // Surface the serde error as a tool_result and re-request
                        // the LLM so it can fix the payload, mirroring the
                        // resolve_task_file Err branch below — without this the
                        // typed interception is bypassed and the malformed call
                        // falls through to propose_task's fallback run().
                        let err_msg = format!(
                            "propose_task input failed to parse: {err}. Re-emit the \
                             call with a valid payload (expected `{{\"task_file\": \"<path>\"}}`)."
                        );
                        let display_data = make_display_data(&content);
                        let assistant_message = AssistantMessage::new(
                            request_id.clone(),
                            content,
                            Some(usage_data),
                            display_data,
                        );
                        let tool_result = ToolResult::error(tool.id.clone(), err_msg);
                        let checkpoint =
                            CheckpointData::tool_round(assistant_message, vec![tool_result])
                                .expect(
                                    "propose_task produces exactly one tool_use and one result",
                                );
                        return Ok(ParentTransitionResult::new(ParentState::Core(
                            CoreState::LlmRequesting { attempt: 1 },
                        ))
                        .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change())
                        .with_effect(Effect::RequestLlm));
                    }
                };

                let snapshot = match resolve_task_file(
                    &context.working_dir,
                    &context.tasks_dir_name,
                    &input.task_file,
                ) {
                    Ok(s) => s,
                    Err(err_msg) => {
                        // Validation failed: surface the error as a tool_result and
                        // re-request the LLM so it can fix the file (or pick another)
                        // and retry.
                        let display_data = make_display_data(&content);
                        let assistant_message = AssistantMessage::new(
                            request_id.clone(),
                            content,
                            Some(usage_data),
                            display_data,
                        );
                        let tool_result = ToolResult::error(tool.id.clone(), err_msg);
                        let checkpoint =
                            CheckpointData::tool_round(assistant_message, vec![tool_result])
                                .expect(
                                    "propose_task produces exactly one tool_use and one result",
                                );
                        return Ok(ParentTransitionResult::new(ParentState::Core(
                            CoreState::LlmRequesting { attempt: 1 },
                        ))
                        .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change())
                        .with_effect(Effect::RequestLlm));
                    }
                };

                // Mode-aware resolution (REQ-PROJ-033/036). Explore parks (the
                // in-place Explore->Work gateway); the writing modes record a
                // non-blocking fork and keep running. `ModeKind::Managed` covers
                // both Explore and Work, so the precise mode comes from
                // `mode_context`.
                let is_explore = matches!(context.mode_context, Some(ModeContext::Explore { .. }));
                let fork_eligible = matches!(context.mode, ModeKind::Branch)
                    || matches!(
                        context.mode_context,
                        Some(ModeContext::Work { .. } | ModeContext::Branch { .. })
                    )
                    || (matches!(context.mode, ModeKind::Direct)
                        && is_git_repository(&context.working_dir));

                if is_explore {
                    let tool_result = ToolResult::success(
                        tool.id.clone(),
                        "Plan submitted for review".to_string(),
                    );
                    let display_data = make_display_data(&content);
                    let assistant_message = AssistantMessage::new(
                        request_id.clone(),
                        content,
                        Some(usage_data),
                        display_data,
                    );
                    let checkpoint =
                        CheckpointData::tool_round(assistant_message, vec![tool_result])
                            .expect("propose_task produces exactly one tool_use and one result");

                    return Ok(
                        ParentTransitionResult::new(ParentState::AwaitingTaskApproval {
                            task_file: snapshot.task_file.clone(),
                            title: snapshot.title.clone(),
                            priority: snapshot.priority,
                            plan: snapshot.plan.clone(),
                        })
                        .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change()),
                    );
                }

                if !fork_eligible {
                    // Direct origin outside a git repository: the tool registry
                    // does not offer propose_task here (no default branch to fork
                    // from — REQ-PROJ-036), so this is unreachable in practice.
                    // Surface a tool error rather than panic; record nothing.
                    let err_msg = "propose_task is unavailable: a fork cuts from the \
                         repository's default branch, but this working directory is \
                         not inside a git repository."
                        .to_string();
                    let display_data = make_display_data(&content);
                    let assistant_message = AssistantMessage::new(
                        request_id.clone(),
                        content,
                        Some(usage_data),
                        display_data,
                    );
                    let tool_result = ToolResult::error(tool.id.clone(), err_msg);
                    let checkpoint =
                        CheckpointData::tool_round(assistant_message, vec![tool_result])
                            .expect("propose_task produces exactly one tool_use and one result");
                    return Ok(ParentTransitionResult::new(ParentState::Core(
                        CoreState::LlmRequesting { attempt: 1 },
                    ))
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm));
                }

                // Fork proposal (REQ-PROJ-033). At the continuation threshold the
                // fork does NOT fire: ContextThresholdReachedParent (the check
                // below) parks into awaiting_continuation instead, replaying the
                // propose_task call after continuation. Without this guard a fork
                // would be recorded while the origin is over-budget.
                if should_trigger_continuation(&usage_data, context.context_window) {
                    let tr = handle_context_exhaustion(
                        context,
                        content,
                        tool_calls,
                        usage_data,
                        request_id,
                        final_attempt,
                    );
                    return Ok(ParentTransitionResult {
                        new_state: ParentState::try_from(tr.new_state)
                            .expect("handle_context_exhaustion returns parent-valid state"),
                        effects: tr.effects,
                    });
                }

                let proposal_id = uuid::Uuid::new_v4().to_string();
                // The success ack carries ONLY the proposal_id as a discoverable
                // handle (in display_data, UI-only — never replayed into the LLM
                // transcript, which sees only the output text). The snapshot body
                // is deliberately absent so the shed work cannot leak back into
                // the origin's context on later turns (REQ-PROJ-035). The UI keys
                // the Review affordance off `display_data.fork_proposal_id`.
                let tool_result = ToolResult::success_with_display(
                    tool.id.clone(),
                    "Fork proposal recorded — pending your review; continue your work".to_string(),
                    Some(serde_json::json!({ "fork_proposal_id": proposal_id })),
                );
                let display_data = make_display_data(&content);
                let assistant_message = AssistantMessage::new(
                    request_id.clone(),
                    content,
                    Some(usage_data),
                    display_data,
                );
                let checkpoint = CheckpointData::tool_round(assistant_message, vec![tool_result])
                    .expect("propose_task produces exactly one tool_use and one result");

                // The conversation continues: LlmRequesting, parent_status
                // untouched. One atomic persist (checkpoint + proposal row) via
                // PersistForkProposal — no separate PersistCheckpoint on this arm.
                return Ok(ParentTransitionResult::new(ParentState::Core(
                    CoreState::LlmRequesting { attempt: 1 },
                ))
                .with_effect(Effect::PersistForkProposal {
                    proposal_id,
                    task_file: snapshot.task_file.clone(),
                    title: snapshot.title.clone(),
                    priority: snapshot.priority,
                    body: snapshot.body_raw.clone(),
                    checkpoint,
                })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm));
            }

            // REQ-AUQ-001: ask_user_question interception. Same shape as
            // propose_task: typed input takes the AwaitingUserResponse path,
            // a malformed payload surfaces the serde error to the LLM so it
            // can re-emit. The latter is the structural backstop for the
            // Malformed variant added in task 13018.
            if let Some((tool, call)) = tool_calls.iter().find_map(|t| match &t.input {
                ToolInput::AskUserQuestion(input) => Some((t, AskUserQuestionCall::Typed(input))),
                ToolInput::Malformed { name, error, .. } if name == "ask_user_question" => {
                    Some((t, AskUserQuestionCall::Malformed(error.as_str())))
                }
                ToolInput::Bash(_)
                | ToolInput::Think(_)
                | ToolInput::Patch(_)
                | ToolInput::KeywordSearch(_)
                | ToolInput::ReadImage(_)
                | ToolInput::SpawnAgents(_)
                | ToolInput::SubmitResult(_)
                | ToolInput::SubmitError(_)
                | ToolInput::ProposeTask(_)
                | ToolInput::Unknown { .. }
                | ToolInput::Malformed { .. } => None,
            }) {
                if tool_calls.len() > 1 {
                    let msg = "ask_user_question must be the only tool in response".to_string();
                    let display_data = make_display_data(&content);
                    let assistant_message = AssistantMessage::new(
                        request_id.clone(),
                        content,
                        Some(usage_data),
                        display_data,
                    );
                    let error_results: Vec<ToolResult> = tool_calls
                        .iter()
                        .map(|t| ToolResult::error(t.id.clone(), msg.clone()))
                        .collect();
                    let checkpoint = CheckpointData::tool_round(assistant_message, error_results)
                        .expect("error_results.len() == tool_calls.len()");
                    return Ok(ParentTransitionResult::new(ParentState::Core(
                        CoreState::LlmRequesting { attempt: 1 },
                    ))
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm));
                }

                let input = match call {
                    AskUserQuestionCall::Typed(input) => input,
                    AskUserQuestionCall::Malformed(err) => {
                        let err_msg = format!(
                            "ask_user_question input failed to parse: {err}. Re-emit the \
                             call with a valid `questions` array."
                        );
                        let display_data = make_display_data(&content);
                        let assistant_message = AssistantMessage::new(
                            request_id.clone(),
                            content,
                            Some(usage_data),
                            display_data,
                        );
                        let tool_result = ToolResult::error(tool.id.clone(), err_msg);
                        let checkpoint =
                            CheckpointData::tool_round(assistant_message, vec![tool_result])
                                .expect(
                                "ask_user_question produces exactly one tool_use and one result",
                            );
                        return Ok(ParentTransitionResult::new(ParentState::Core(
                            CoreState::LlmRequesting { attempt: 1 },
                        ))
                        .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::notify_state_change())
                        .with_effect(Effect::RequestLlm));
                    }
                };

                let tool_result = ToolResult::success(
                    tool.id.clone(),
                    "Awaiting user response. See following message for answers.".to_string(),
                );
                let display_data = make_display_data(&content);
                let assistant_message = AssistantMessage::new(
                    request_id.clone(),
                    content,
                    Some(usage_data),
                    display_data,
                );
                let checkpoint = CheckpointData::tool_round(assistant_message, vec![tool_result])
                    .expect("ask_user_question produces exactly one tool_use and one result");

                return Ok(
                    ParentTransitionResult::new(ParentState::AwaitingUserResponse {
                        questions: input.questions.clone(),
                        tool_use_id: tool.id.clone(),
                    })
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change()),
                );
            }

            // REQ-BED-019: Context exhaustion check (after propose_task/ask_user_question)
            if should_trigger_continuation(&usage_data, context.context_window) {
                let tr = handle_context_exhaustion(
                    context,
                    content,
                    tool_calls,
                    usage_data,
                    request_id,
                    final_attempt,
                );
                return Ok(ParentTransitionResult {
                    new_state: ParentState::try_from(tr.new_state)
                        .expect("handle_context_exhaustion returns parent-valid state"),
                    effects: tr.effects,
                });
            }

            // No interception needed — delegate to core
            let core_event = CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn: false,
                usage: usage_data,
                request_id,
            };
            let ParentState::Core(core_state) = state else {
                unreachable!()
            };
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_parent_result())
        }

        // AwaitingRecovery interception for auth errors
        (
            ParentState::Core(CoreState::LlmRequesting { .. }),
            ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                recovery_in_progress: true,
                ..
            }),
        ) if matches!(error_kind, ErrorKind::Auth) => {
            Ok(ParentTransitionResult::new(ParentState::AwaitingRecovery {
                message: message.clone(),
                error_kind: error_kind.clone(),
                recovery_kind: RecoveryKind::Credential,
                resume: RecoveryResumeTarget::ConversationTurn,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()))
        }

        // Intercepted before core delegation — spec rule
        // BackendRejectsContextExhausted (bedrock.allium).
        (
            ParentState::Core(CoreState::LlmRequesting { .. }),
            ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind: ErrorKind::ContextExhausted,
                ..
            }),
        ) => {
            // Stable, human-oriented summary — the raw backend message can
            // carry provider-specific strings and is persisted for the UI
            // banner / clipboard / seed-draft, so it is logged separately
            // rather than interpolated into user-facing text.
            tracing::warn!(
                backend_message = %message,
                "backend rejected request with context_length_exceeded; \
                converging parent to ContextExhausted"
            );
            let summary = "Context limit reached before the turn could complete. \
                Continue to compact and resume, or start a new conversation."
                .to_string();
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: summary.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&summary))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary }))
        }

        // ============================================================
        // Parent-specific continuation transitions
        // ============================================================
        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::ContinuationResponse { summary }),
        ) => Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
            summary: summary.clone(),
        })
        .with_effect(Effect::persist_continuation_message(&summary))
        .with_effect(Effect::PersistState)
        .with_effect(Effect::NotifyContextExhausted { summary })),

        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::ContinuationFailed { error }),
        ) => {
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {error}. \
                Please start a new conversation."
            );
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        (
            ParentState::Core(CoreState::AwaitingContinuation {
                rejected_tool_calls,
                ..
            }),
            ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                recovery_in_progress: true,
                ..
            }),
        ) if matches!(error_kind, ErrorKind::Auth) => {
            Ok(ParentTransitionResult::new(ParentState::AwaitingRecovery {
                message: message.clone(),
                error_kind: error_kind.clone(),
                recovery_kind: RecoveryKind::Credential,
                resume: RecoveryResumeTarget::ContinuationSummary {
                    request: ContinuationSummaryRequest {
                        rejected_tool_calls: rejected_tool_calls.clone(),
                    },
                },
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change()))
        }

        (
            ParentState::Core(CoreState::AwaitingContinuation { .. }),
            ParentEvent::Core(CoreEvent::LlmError {
                ref message,
                ref error_kind,
                ..
            }),
        ) if !error_kind.is_auto_retryable() || {
            // Check if we're at/past max retries
            match state {
                ParentState::Core(CoreState::AwaitingContinuation { attempt, .. }) => {
                    *attempt >= MAX_RETRY_ATTEMPTS
                }
                ParentState::Core(_)
                | ParentState::AwaitingRecovery { .. }
                | ParentState::AwaitingTaskApproval { .. }
                | ParentState::AwaitingUserResponse { .. }
                | ParentState::ContextExhausted { .. }
                | ParentState::HandedOff { .. }
                | ParentState::Terminal => false,
            }
        } =>
        {
            let message = message.clone();
            let fallback = format!(
                "Context limit reached. The continuation summary could not be generated: {message}. \
                Please start a new conversation."
            );
            Ok(ParentTransitionResult::new(ParentState::ContextExhausted {
                summary: fallback.clone(),
            })
            .with_effect(Effect::persist_continuation_message(&fallback))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyContextExhausted { summary: fallback }))
        }

        // Stale TaskApprovalDecided
        (state, ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided { .. })) => {
            tracing::debug!("Absorbing stale TaskApprovalDecided");
            Ok(ParentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Delegate to core
        // ============================================================
        (ParentState::Core(core_state), ParentEvent::Core(core_event)) => {
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_parent_result())
        }

        // Invalid: parent-only events in non-matching states
        (state, event) => Err(TransitionError::InvalidTransition {
            state: state.variant_name(),
            event: event.variant_name(),
        }),
    }
}

// ============================================================================
// transition_sub_agent — sub-agent-specific transitions, delegates core
// ============================================================================

/// Sub-agent transition function. Handles sub-agent-only states and events,
/// intercepts core events with sub-agent-specific behavior, delegates the
/// rest to `transition_core`.
///
/// # Errors
///
/// Returns [`TransitionError`] when the sub-agent event is not valid for the
/// current sub-agent state.
///
/// # Panics
///
/// Panics if internal invariants are violated — e.g. a per-tool result count
/// mismatch, or `is_terminal_tool` disagreeing with the terminal-tool match.
/// These reflect reducer bugs, not reachable inputs.
#[allow(clippy::too_many_lines)]
pub fn transition_sub_agent(
    state: &SubAgentState,
    context: &ConvContext,
    event: SubAgentEvent,
) -> Result<SubAgentTransitionResult, TransitionError> {
    use crate::state::SubAgentOutcome;

    match (state, event) {
        // ============================================================
        // Terminal state absorption (Completed / Failed)
        // ============================================================
        (SubAgentState::Completed { .. } | SubAgentState::Failed { .. }, _event) => {
            Ok(SubAgentTransitionResult::new(state.clone()))
        }

        // ============================================================
        // Grace Turn Exhausted (REQ-BED-026)
        // ============================================================
        (
            _state,
            SubAgentEvent::SubAgent(SubAgentOnlyEvent::GraceTurnExhausted { result: Some(text) }),
        ) => Ok(SubAgentTransitionResult::new(SubAgentState::Completed {
            result: text.clone(),
        })
        .with_effect(Effect::PersistState)
        .with_effect(Effect::NotifyParent {
            outcome: SubAgentOutcome::Success { result: text },
        })),

        (
            _state,
            SubAgentEvent::SubAgent(SubAgentOnlyEvent::GraceTurnExhausted { result: None }),
        ) => {
            let error = "Sub-agent exceeded turn limit with no output".to_string();
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
                error: error.clone(),
                error_kind: ErrorKind::Cancelled,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error,
                    error_kind: ErrorKind::Cancelled,
                },
            }))
        }

        // ============================================================
        // Sub-agent UserCancel during ToolExecuting -> CancellingTool + AbortTool
        // ============================================================
        // A sub-agent's in-flight tool (bash/patch) mutates the worktree it
        // shares with its parent. The sub-agent must abort the tool AND defer
        // notifying the parent until that tool has actually settled — otherwise
        // the parent is told the sub-agent stopped while the tool task is still
        // mid-flight, then resumes and reads/writes the shared worktree
        // concurrently. Route through `CancellingTool` (mirroring the parent's
        // `ToolExecuting + UserCancel` path) and only settle to Failed +
        // NotifyParent once the tool's `ToolAborted`/`ToolComplete` arrives.
        // (A sub-agent cannot itself spawn sub-agents, so `pending_sub_agents`
        // is always empty here; carry it through verbatim for shape parity.)
        (
            SubAgentState::Core(CoreState::ToolExecuting {
                current_tool,
                remaining_tools,
                completed_results,
                assistant_message,
                pending_sub_agents,
            }),
            SubAgentEvent::Core(CoreEvent::UserCancel { reason: _, .. }),
        ) => Ok(
            SubAgentTransitionResult::new(SubAgentState::Core(CoreState::CancellingTool {
                tool_use_id: current_tool.id.clone(),
                skipped_tools: remaining_tools.clone(),
                completed_results: completed_results.clone(),
                assistant_message: assistant_message.clone(),
                pending_sub_agents: pending_sub_agents.clone(),
            }))
            .with_effect(Effect::AbortTool {
                tool_use_id: current_tool.id.clone(),
            })
            .with_effect(Effect::PersistState),
        ),

        // ============================================================
        // Sub-agent CancellingTool + ToolAborted/ToolComplete -> Failed
        // ============================================================
        // The in-flight tool the sub-agent aborted has now settled. A cancelled
        // sub-agent ends terminally Failed (the parent's analogous arm goes to
        // Idle to resume the user's turn — wrong here, where the sub-agent has
        // no turn of its own to resume) and notifies the parent so fan-in
        // accounting stays correct. The tool result is discarded: the round is
        // being cancelled, not checkpointed. Guarded on id match so a stale
        // outcome for a different tool_use does not settle the wrong round.
        (
            SubAgentState::Core(CoreState::CancellingTool { tool_use_id, .. }),
            SubAgentEvent::Core(
                CoreEvent::ToolAborted {
                    tool_use_id: settled_id,
                }
                | CoreEvent::ToolComplete {
                    tool_use_id: settled_id,
                    ..
                },
            ),
        ) if *tool_use_id == settled_id => {
            let error = "Cancelled by parent".to_string();
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
                error: error.clone(),
                error_kind: ErrorKind::Cancelled,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error,
                    error_kind: ErrorKind::Cancelled,
                },
            }))
        }

        // ============================================================
        // Sub-agent UserCancel while already CancellingTool -> absorb
        // ============================================================
        // A second UserCancel can arrive (the per-agent timeout and the parent
        // cancel are independent senders) before the aborting tool settles. Stay
        // in CancellingTool and do NOT notify the parent: notifying now would let
        // the parent resume while the tool task is still aborting, recreating the
        // write-after-cancel race this state exists to prevent. The eventual
        // ToolAborted/ToolComplete settles the round (arm above).
        (
            cancelling @ SubAgentState::Core(CoreState::CancellingTool { .. }),
            SubAgentEvent::Core(CoreEvent::UserCancel { reason: _, .. }),
        ) => Ok(SubAgentTransitionResult::new(cancelling.clone())),

        // ============================================================
        // Sub-agent UserCancel -> Failed (from any other non-terminal core state)
        // ============================================================
        (SubAgentState::Core(_), SubAgentEvent::Core(CoreEvent::UserCancel { reason, .. })) => {
            let error = reason
                .clone()
                .unwrap_or_else(|| "Cancelled by parent".to_string());
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
                error: error.clone(),
                error_kind: ErrorKind::Cancelled,
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error,
                    error_kind: ErrorKind::Cancelled,
                },
            }))
        }

        // ============================================================
        // Sub-agent LLM error handling (non-retryable or exhausted -> Failed)
        // ============================================================
        (
            SubAgentState::Core(CoreState::LlmRequesting { attempt }),
            SubAgentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                ..
            }),
        ) if !error_kind.is_auto_retryable() || *attempt >= MAX_RETRY_ATTEMPTS => {
            let error_message = if error_kind.is_auto_retryable() {
                format!("Failed after {attempt} attempts: {message}")
            } else {
                message
            };
            Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
                error: error_message.clone(),
                error_kind: error_kind.clone(),
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error: error_message,
                    error_kind,
                },
            }))
        }

        // ============================================================
        // Sub-agent LLM response handling (combined to avoid
        // borrow-after-move issues with guards)
        // ============================================================
        (
            SubAgentState::Core(CoreState::LlmRequesting { attempt }),
            SubAgentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                usage: usage_data,
                request_id,
                ..
            }),
        ) => {
            let final_attempt = *attempt;
            // Context exhaustion check first (sub-agent fails immediately)
            if should_trigger_continuation(&usage_data, context.context_window) {
                let tr = handle_context_exhaustion(
                    context,
                    content,
                    tool_calls,
                    usage_data,
                    request_id,
                    final_attempt,
                );
                return Ok(SubAgentTransitionResult {
                    new_state: SubAgentState::try_from(tr.new_state)
                        .expect("sub-agent context exhaustion returns Failed"),
                    effects: tr.effects,
                });
            }

            // Text-only response -> implicit Completed
            if tool_calls.is_empty() {
                let result_text = extract_text_from_content(&content);
                let mut tr = SubAgentTransitionResult::new(SubAgentState::Completed {
                    result: result_text.clone(),
                });
                if !content.is_empty() {
                    tr = tr.with_effect(Effect::persist_agent_message(
                        content,
                        Some(usage_data),
                        &context.working_dir,
                        request_id,
                        final_attempt,
                    ));
                }
                return Ok(tr.with_effect(Effect::PersistState).with_effect(
                    Effect::NotifyParent {
                        outcome: SubAgentOutcome::Success {
                            result: result_text,
                        },
                    },
                ));
            }

            // Terminal tools (submit_result/submit_error)
            if let Some(terminal_tool) = tool_calls.iter().find(|t| t.input.is_terminal_tool()) {
                if tool_calls.len() > 1 {
                    let msg =
                        "submit_result/submit_error must be the only tool in response".to_string();
                    let display_data = compute_bash_display_data(&content, &context.working_dir);
                    let assistant_message = AssistantMessage::new(
                        request_id.clone(),
                        content,
                        Some(usage_data),
                        display_data,
                    );
                    let error_results: Vec<ToolResult> = tool_calls
                        .iter()
                        .map(|t| ToolResult::error(t.id.clone(), msg.clone()))
                        .collect();
                    let checkpoint = CheckpointData::tool_round(assistant_message, error_results)
                        .expect("error_results.len() == tool_calls.len()");
                    return Ok(SubAgentTransitionResult::new(SubAgentState::Core(
                        CoreState::LlmRequesting { attempt: 1 },
                    ))
                    .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                    .with_effect(Effect::PersistState)
                    .with_effect(Effect::notify_state_change())
                    .with_effect(Effect::RequestLlm));
                }

                return match &terminal_tool.input {
                    ToolInput::SubmitResult(input) => {
                        Ok(SubAgentTransitionResult::new(SubAgentState::Completed {
                            result: input.result.clone(),
                        })
                        .with_effect(Effect::persist_agent_message(
                            content,
                            Some(usage_data),
                            &context.working_dir,
                            request_id,
                            final_attempt,
                        ))
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::NotifyParent {
                            outcome: SubAgentOutcome::Success {
                                result: input.result.clone(),
                            },
                        }))
                    }
                    ToolInput::SubmitError(input) => {
                        Ok(SubAgentTransitionResult::new(SubAgentState::Failed {
                            error: input.error.clone(),
                            error_kind: ErrorKind::SubAgentError,
                        })
                        .with_effect(Effect::persist_agent_message(
                            content,
                            Some(usage_data),
                            &context.working_dir,
                            request_id,
                            final_attempt,
                        ))
                        .with_effect(Effect::PersistState)
                        .with_effect(Effect::NotifyParent {
                            outcome: SubAgentOutcome::Failure {
                                error: input.error.clone(),
                                error_kind: ErrorKind::SubAgentError,
                            },
                        }))
                    }
                    ToolInput::Bash(_)
                    | ToolInput::Think(_)
                    | ToolInput::Patch(_)
                    | ToolInput::KeywordSearch(_)
                    | ToolInput::ReadImage(_)
                    | ToolInput::SpawnAgents(_)
                    | ToolInput::ProposeTask(_)
                    | ToolInput::AskUserQuestion(_)
                    | ToolInput::Unknown { .. }
                    | ToolInput::Malformed { .. } => {
                        unreachable!("is_terminal_tool returned true for non-terminal tool")
                    }
                };
            }

            // REQ-PROJ-008 / REQ-PROJ-036 (SubAgentProposeTaskRejected): a
            // sub-agent never gets `propose_task` in its registry, so a sole
            // `propose_task` call here is a stale/replayed payload. Reject it in
            // the state machine — task management belongs to the parent — rather
            // than routing it through the executor's unreachable `run()`
            // fallback. The context-exhaustion check above wins at the
            // threshold, so an over-budget sub-agent fails instead of looping on
            // rejected propose_task errors.
            if tool_calls.len() == 1 && matches!(tool_calls[0].input, ToolInput::ProposeTask(_)) {
                let err_msg =
                    "propose_task is not available to sub-agents — task management is the \
                     parent conversation's job."
                        .to_string();
                let display_data = compute_bash_display_data(&content, &context.working_dir);
                let assistant_message = AssistantMessage::new(
                    request_id.clone(),
                    content,
                    Some(usage_data),
                    display_data,
                );
                let tool_result = ToolResult::error(tool_calls[0].id.clone(), err_msg);
                let checkpoint = CheckpointData::tool_round(assistant_message, vec![tool_result])
                    .expect("propose_task produces exactly one tool_use and one result");
                return Ok(SubAgentTransitionResult::new(SubAgentState::Core(
                    CoreState::LlmRequesting { attempt: 1 },
                ))
                .with_effect(Effect::PersistCheckpoint { data: checkpoint })
                .with_effect(Effect::PersistState)
                .with_effect(Effect::notify_state_change())
                .with_effect(Effect::RequestLlm));
            }

            // Normal tool execution -> delegate to core
            let core_event = CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn: false,
                usage: usage_data,
                request_id,
            };
            let SubAgentState::Core(core_state) = state else {
                unreachable!()
            };
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_sub_agent_result())
        }

        // ============================================================
        // Delegate to core for everything else
        // ============================================================
        (SubAgentState::Core(core_state), SubAgentEvent::Core(core_event)) => {
            let core_result = transition_core(core_state, context, core_event)?;
            Ok(core_result.into_sub_agent_result())
        }
    }
}

// ============================================================================
// handle_outcome — second pure entry point for executor-produced outcomes
// ============================================================================

/// Entry point 2: Executor outcomes (from background tasks via typed channels).
///
/// This is the second layer of defense. Even with typed channels constraining
/// what CAN arrive, this function rejects outcomes that are invalid for the
/// current state. The executor logs and discards `Err` — state unchanged.
///
/// REQ-BED-001: Pure function — given the same inputs, always the same outputs.
///
/// # Errors
///
/// Returns [`InvalidOutcome`] when the outcome is not valid for the current
/// state; the executor logs and discards it, leaving state unchanged.
pub fn handle_outcome(
    state: &ConvState,
    context: &ConvContext,
    outcome: EffectOutcome,
) -> Result<TransitionResult, InvalidOutcome> {
    let event = match outcome {
        EffectOutcome::Llm(llm) => llm_outcome_to_event(llm, state),
        EffectOutcome::Tool(tool) => tool_outcome_to_event(tool),
        EffectOutcome::SubAgent { agent_id, outcome } => {
            Event::SubAgentResult { agent_id, outcome }
        }
        EffectOutcome::Persist(persist) => {
            return handle_persist_outcome(state, persist);
        }
        EffectOutcome::RetryTimeout { attempt } => Event::RetryTimeout { attempt },
    };

    transition(state, context, event).map_err(|e| InvalidOutcome {
        reason: e.to_string(),
    })
}

/// Convert `LlmOutcome` to the equivalent `Event` for delegation to `transition()`.
#[allow(clippy::too_many_lines)] // Pure-data dispatch over a wide LlmOutcome enum
fn llm_outcome_to_event(outcome: LlmOutcome, state: &ConvState) -> Event {
    match outcome {
        LlmOutcome::Response {
            content,
            tool_calls,
            end_turn,
            usage,
            request_id,
        } => Event::LlmResponse {
            content,
            tool_calls,
            end_turn,
            usage,
            request_id,
        },
        LlmOutcome::RateLimited {
            retry_after: _,
            resets_at,
        } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Rate limited".to_string(),
                error_kind: ErrorKind::RateLimit,
                attempt,
                recovery_in_progress: false,
                resets_at,
            }
        }
        LlmOutcome::UsageLimitReached { details, message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::UsageLimitReached,
                attempt,
                recovery_in_progress: false,
                // Carry the quota-window reset time through to
                // `ConvState::Error.resets_at` so the auto-clear sweep returns
                // the conversation to Idle once the window passes. Not used for
                // retry — usage-limit is non-retryable and never reaches
                // `Effect::ScheduleRetry`.
                resets_at: details.resets_at,
            }
        }
        LlmOutcome::ServerError { status, body } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: format!("Server error {status}: {body}"),
                error_kind: ErrorKind::ServerError,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::InvalidResponse { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::InvalidResponse,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::ServerOverloaded { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::ServerOverloaded,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::NetworkError { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Network,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::TokenBudgetExceeded => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Token budget exceeded".to_string(),
                error_kind: ErrorKind::ContextExhausted,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::AuthError {
            message,
            recovery_in_progress,
        } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::Auth,
                attempt,
                recovery_in_progress,
                resets_at: None,
            }
        }
        LlmOutcome::RequestRejected { message } => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message,
                error_kind: ErrorKind::InvalidRequest,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
        LlmOutcome::Cancelled => {
            let attempt = current_attempt(state);
            Event::LlmError {
                message: "Request cancelled".to_string(),
                error_kind: ErrorKind::Cancelled,
                attempt,
                recovery_in_progress: false,
                resets_at: None,
            }
        }
    }
}

/// Convert `ToolExecOutcome` to the equivalent `Event` for delegation to `transition()`.
fn tool_outcome_to_event(outcome: ToolExecOutcome) -> Event {
    match outcome {
        ToolExecOutcome::Completed(result) => Event::ToolComplete {
            tool_use_id: result.tool_use_id.clone(),
            result,
        },
        ToolExecOutcome::Aborted {
            tool_use_id,
            reason: _,
        } => Event::ToolAborted { tool_use_id },
        ToolExecOutcome::Failed { tool_use_id, error } => Event::ToolComplete {
            tool_use_id: tool_use_id.clone(),
            result: ToolResult::error(tool_use_id, error),
        },
    }
}

/// Handle `PersistOutcome` directly — no Event equivalent exists.
/// Persistence failures are logged but don't change state.
fn handle_persist_outcome(
    state: &ConvState,
    outcome: PersistOutcome,
) -> Result<TransitionResult, InvalidOutcome> {
    match outcome {
        PersistOutcome::Ok => Ok(TransitionResult::new(state.clone())),
        PersistOutcome::Failed { error } => Err(InvalidOutcome {
            reason: format!("Persistence failed: {error}"),
        }),
    }
}

/// Extract the current attempt number from state (for LLM error conversion).
fn current_attempt(state: &ConvState) -> u32 {
    match state {
        ConvState::LlmRequesting { attempt }
        | ConvState::SeededLlmRequesting { attempt, .. }
        | ConvState::AwaitingContinuation { attempt, .. } => *attempt,
        ConvState::Idle
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::Error { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::ContextExhausted { .. }
        | ConvState::HandedOff { .. }
        | ConvState::Terminal => 1,
    }
}

// Helper functions

/// Threshold as fraction of context window for triggering continuation (REQ-BED-019)
const CONTINUATION_THRESHOLD: f64 = 0.90;

/// Check if context usage has exceeded the continuation threshold (REQ-BED-019)
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn should_trigger_continuation(usage: &UsageData, context_window: usize) -> bool {
    let used = usage.context_window_used();
    let threshold = (context_window as f64 * CONTINUATION_THRESHOLD) as u64;
    used >= threshold
}

/// Handle context exhaustion based on conversation type (REQ-BED-019, REQ-BED-024)
fn handle_context_exhaustion(
    ctx: &ConvContext,
    blocks: Vec<phoenix_core::domain::llm_types::ContentBlock>,
    tool_calls: Vec<ToolCall>,
    usage_data: UsageData,
    request_id: String,
    final_attempt: u32,
) -> TransitionResult {
    use crate::state::SubAgentOutcome;

    match ctx.context_exhaustion_behavior {
        ContextExhaustionBehavior::ThresholdBasedContinuation => {
            // Normal conversation: trigger continuation flow
            TransitionResult::new(ConvState::AwaitingContinuation {
                rejected_tool_calls: tool_calls.clone(),
                attempt: 1,
            })
            .with_effect(Effect::persist_agent_message(
                blocks,
                Some(usage_data),
                &ctx.working_dir,
                request_id,
                final_attempt,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_state_change())
            .with_effect(Effect::RequestContinuation {
                request: ContinuationSummaryRequest {
                    rejected_tool_calls: tool_calls,
                },
            })
        }
        ContextExhaustionBehavior::IntentionallyUnhandled => {
            // REQ-BED-024: Sub-agent fails immediately
            TransitionResult::new(ConvState::Failed {
                error: "Context window exhausted before result submission".to_string(),
                error_kind: ErrorKind::ContextExhausted,
            })
            .with_effect(Effect::persist_agent_message(
                blocks,
                Some(usage_data),
                &ctx.working_dir,
                request_id,
                final_attempt,
            ))
            .with_effect(Effect::PersistState)
            .with_effect(Effect::NotifyParent {
                outcome: SubAgentOutcome::Failure {
                    error: "Context window exhausted before result submission".to_string(),
                    error_kind: ErrorKind::ContextExhausted,
                },
            })
        }
    }
}

/// Extract concatenated text from content blocks for implicit sub-agent completion.
fn extract_text_from_content(blocks: &[phoenix_core::domain::llm_types::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            phoenix_core::domain::llm_types::ContentBlock::Text { text } => Some(text.as_str()),
            phoenix_core::domain::llm_types::ContentBlock::Image { .. }
            | phoenix_core::domain::llm_types::ContentBlock::ToolUse { .. }
            | phoenix_core::domain::llm_types::ContentBlock::ToolResult { .. }
            | phoenix_core::domain::llm_types::ContentBlock::ServerToolUse { .. }
            | phoenix_core::domain::llm_types::ContentBlock::ToolSearchToolResult { .. }
            | phoenix_core::domain::llm_types::ContentBlock::WebSearchToolResult { .. }
            | phoenix_core::domain::llm_types::ContentBlock::WebFetchToolResult { .. }
            | phoenix_core::domain::llm_types::ContentBlock::CodeExecutionToolResult { .. }
            | phoenix_core::domain::llm_types::ContentBlock::BashCodeExecutionToolResult {
                ..
            }
            | phoenix_core::domain::llm_types::ContentBlock::TextEditorCodeExecutionToolResult {
                ..
            }
            | phoenix_core::domain::llm_types::ContentBlock::McpToolUse { .. }
            | phoenix_core::domain::llm_types::ContentBlock::McpToolResult { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn retry_delay(attempt: u32) -> Duration {
    // Exponential backoff: 1s, 2s, 4s
    Duration::from_secs(1 << (attempt - 1))
}

#[allow(dead_code)] // Conversion utility
#[must_use]
pub fn llm_error_to_db_error(
    kind: phoenix_core::domain::llm_error_kind::LlmErrorKind,
) -> ErrorKind {
    // Explicit match arms — no catch-all. The compiler enforces exhaustiveness.
    match kind {
        phoenix_core::domain::llm_error_kind::LlmErrorKind::Auth => ErrorKind::Auth,
        phoenix_core::domain::llm_error_kind::LlmErrorKind::RateLimit => ErrorKind::RateLimit,
        phoenix_core::domain::llm_error_kind::LlmErrorKind::UsageLimitReached => {
            ErrorKind::UsageLimitReached
        }
        phoenix_core::domain::llm_error_kind::LlmErrorKind::Network => ErrorKind::Network,
        phoenix_core::domain::llm_error_kind::LlmErrorKind::InvalidRequest => {
            ErrorKind::InvalidRequest
        }
        phoenix_core::domain::llm_error_kind::LlmErrorKind::InvalidResponse => {
            ErrorKind::InvalidResponse
        }
        phoenix_core::domain::llm_error_kind::LlmErrorKind::ServerError => ErrorKind::ServerError,
        phoenix_core::domain::llm_error_kind::LlmErrorKind::ServerOverloaded => {
            ErrorKind::ServerOverloaded
        }
        phoenix_core::domain::llm_error_kind::LlmErrorKind::ContentFilter => {
            ErrorKind::ContentFilter
        }
        phoenix_core::domain::llm_error_kind::LlmErrorKind::ContextWindowExceeded => {
            ErrorKind::ContextExhausted
        }
    }
}

// ErrorKind::is_auto_retryable() is now defined in db/schema.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_context() -> ConvContext {
        ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000)
    }

    fn test_tool_call(id: &str) -> ToolCall {
        ToolCall::new(
            id,
            ToolInput::Think(crate::state::ThinkInput {
                thoughts: "inspect".to_string(),
            }),
        )
    }

    // A usage-limit 429's `resets_at` must survive the outcome→event
    // conversion. It was previously hardcoded to None here (only retry
    // scheduling consumed it, and usage-limit is non-retryable); the
    // auto-clear sweep is the second consumer that depends on it.
    #[test]
    fn usage_limit_outcome_threads_resets_at_into_event() {
        let resets = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let details = phoenix_core::domain::quota_details::QuotaDetails {
            plan_type: Some("plus".to_string()),
            resets_at: Some(resets),
            limit_id: None,
            limit_name: None,
            primary: None,
            secondary: None,
            credits: None,
            promo_message: None,
        };
        let event = llm_outcome_to_event(
            LlmOutcome::UsageLimitReached {
                details,
                message: "You've hit your usage limit.".to_string(),
            },
            &ConvState::LlmRequesting { attempt: 1 },
        );
        assert!(
            matches!(
                event,
                Event::LlmError {
                    error_kind: ErrorKind::UsageLimitReached,
                    resets_at: Some(t),
                    ..
                } if t == resets
            ),
            "usage-limit outcome must carry resets_at into the event"
        );
    }

    // The reset time must land in the persisted Error state so the sweep can
    // read it later.
    #[test]
    fn usage_limit_error_persists_resets_at_in_error_state() {
        let resets = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::LlmError {
                message: "You've hit your usage limit.".to_string(),
                error_kind: ErrorKind::UsageLimitReached,
                attempt: 1,
                recovery_in_progress: false,
                resets_at: Some(resets),
            },
        )
        .expect("usage-limit LlmError must transition to Error");
        assert!(
            matches!(
                result.new_state,
                ConvState::Error {
                    error_kind: ErrorKind::UsageLimitReached,
                    resets_at: Some(t),
                    ..
                } if t == resets
            ),
            "usage-limit Error must persist resets_at, got {:?}",
            result.new_state
        );
    }

    #[test]
    fn ordinary_llm_auth_recovery_resumes_main_turn() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::LlmError {
                message: "Waiting for authentication".to_string(),
                error_kind: ErrorKind::Auth,
                attempt: 1,
                recovery_in_progress: true,
                resets_at: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::AwaitingRecovery {
                resume: RecoveryResumeTarget::ConversationTurn,
                ..
            }
        ));

        let resumed = transition(
            &result.new_state,
            &test_context(),
            Event::CredentialBecameAvailable,
        )
        .unwrap();

        assert!(matches!(
            resumed.new_state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
        assert!(resumed
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::RequestLlm)));
    }

    #[test]
    fn continuation_auth_error_enters_recovery_with_continuation_target() {
        let rejected_tool_calls = vec![test_tool_call("tool-1")];
        let result = transition(
            &ConvState::AwaitingContinuation {
                rejected_tool_calls: rejected_tool_calls.clone(),
                attempt: 1,
            },
            &test_context(),
            Event::LlmError {
                message: "Waiting for authentication".to_string(),
                error_kind: ErrorKind::Auth,
                attempt: 1,
                recovery_in_progress: true,
                resets_at: None,
            },
        )
        .unwrap();

        match result.new_state {
            ConvState::AwaitingRecovery {
                resume:
                    RecoveryResumeTarget::ContinuationSummary {
                        request:
                            ContinuationSummaryRequest {
                                rejected_tool_calls: carried,
                            },
                    },
                ..
            } => assert_eq!(carried, rejected_tool_calls),
            other @ (ConvState::Idle
            | ConvState::LlmRequesting { .. }
            | ConvState::SeededLlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::Error { .. }
            | ConvState::AwaitingRecovery { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::ContextExhausted { .. }
            | ConvState::HandedOff { .. }
            | ConvState::Terminal) => {
                panic!("expected continuation recovery target, got {other:?}")
            }
        }
        assert!(!result
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyContextExhausted { .. })));
        assert!(!result.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::Continuation { .. },
                    ..
                }
            )
        }));
    }

    #[test]
    fn credential_success_from_continuation_recovery_retries_continuation() {
        let rejected_tool_calls = vec![test_tool_call("tool-1")];
        let state = ConvState::AwaitingRecovery {
            message: "Waiting for authentication".to_string(),
            error_kind: ErrorKind::Auth,
            recovery_kind: RecoveryKind::Credential,
            resume: RecoveryResumeTarget::ContinuationSummary {
                request: ContinuationSummaryRequest {
                    rejected_tool_calls: rejected_tool_calls.clone(),
                },
            },
        };

        let result = transition(&state, &test_context(), Event::CredentialBecameAvailable).unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::AwaitingContinuation { attempt: 1, .. }
        ));
        assert!(result.effects.iter().any(|effect| matches!(
            effect,
            Effect::RequestContinuation {
                request: ContinuationSummaryRequest { rejected_tool_calls: carried }
            } if *carried == rejected_tool_calls
        )));
    }

    #[test]
    fn credential_failure_from_continuation_recovery_does_not_persist_fallback_summary() {
        let state = ConvState::AwaitingRecovery {
            message: "Waiting for authentication".to_string(),
            error_kind: ErrorKind::Auth,
            recovery_kind: RecoveryKind::Credential,
            resume: RecoveryResumeTarget::ContinuationSummary {
                request: ContinuationSummaryRequest {
                    rejected_tool_calls: vec![test_tool_call("tool-1")],
                },
            },
        };

        let result = transition(
            &state,
            &test_context(),
            Event::CredentialHelperFailed {
                message: "sign-in failed".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::Error {
                message,
                error_kind: ErrorKind::Auth,
                ..
            } if message == "sign-in failed"
        ));
        assert!(!result
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyContextExhausted { .. })));
        assert!(!result.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::Continuation { .. },
                    ..
                }
            )
        }));
    }

    #[test]
    fn non_auth_continuation_failure_still_persists_fallback_summary() {
        let result = transition(
            &ConvState::AwaitingContinuation {
                rejected_tool_calls: vec![],
                attempt: MAX_RETRY_ATTEMPTS,
            },
            &test_context(),
            Event::LlmError {
                message: "invalid continuation request".to_string(),
                error_kind: ErrorKind::InvalidRequest,
                attempt: MAX_RETRY_ATTEMPTS,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::ContextExhausted { .. }
        ));
        assert!(result
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::NotifyContextExhausted { .. })));
        assert!(result.effects.iter().any(|effect| {
            matches!(
                effect,
                Effect::PersistMessage {
                    content: phoenix_core::domain::db_schema::MessageContent::Continuation { .. },
                    ..
                }
            )
        }));
    }

    #[test]
    fn parent_cancel_during_continuation_is_invalid_and_does_not_abort_llm() {
        let state = ConvState::AwaitingContinuation {
            rejected_tool_calls: vec![],
            attempt: 1,
        };

        let err = transition(
            &state,
            &test_context(),
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            },
        )
        .expect_err("continuation generation is not user-cancellable");

        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                state: "AwaitingContinuation",
                event: "UserCancel"
            }
        ));
    }

    // Bug class (task 60008): a parent that exhausts context must never
    // land in ConvState::Error — the /continue recovery precondition
    // (db.rs) gates on ConvState::ContextExhausted, so an Error landing
    // strands the conversation with no recovery path. Both the
    // internal-threshold path and the backend-rejection path
    // (LlmErrorKind::ContextWindowExceeded → ErrorKind::ContextExhausted)
    // must converge on ContextExhausted.
    #[test]
    fn parent_context_exhausted_llm_error_routes_to_context_exhausted() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::LlmError {
                message: "context_length_exceeded".to_string(),
                error_kind: ErrorKind::ContextExhausted,
                attempt: 1,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        // reason: only ContextExhausted is expected; every other ConvState variant is a
        // failure, so an explicit list of the ~16 remaining variants would obscure intent.
        #[allow(clippy::wildcard_enum_match_arm)]
        match &result.new_state {
            ConvState::ContextExhausted { summary } => {
                // Stable summary — the raw backend message must NOT leak
                // into user-facing persisted text.
                assert!(summary.contains("Context limit reached"));
                assert!(!summary.contains("context_length_exceeded"));
            }
            other => panic!("expected ContextExhausted, got {other:?}"),
        }
        // The resulting state must satisfy the /continue precondition.
        assert!(matches!(
            result.new_state,
            ConvState::ContextExhausted { .. }
        ));
    }

    // The interception is scoped to ContextExhausted only — other
    // non-retryable kinds must still reach Error so this fix does not
    // silently swallow auth/invalid-request failures.
    #[test]
    fn parent_other_non_retryable_llm_error_still_routes_to_error() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::LlmError {
                message: "bad request".to_string(),
                error_kind: ErrorKind::InvalidRequest,
                attempt: 1,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::Error {
                error_kind: ErrorKind::InvalidRequest,
                ..
            }
        ));
    }

    // Retries are exhausted in one step for ContextExhausted because it is
    // non-retryable; assert the attempt counter is irrelevant (any attempt
    // value still converges on ContextExhausted, never Error).
    #[test]
    fn parent_context_exhausted_converges_regardless_of_attempt() {
        for attempt in 1..=MAX_RETRY_ATTEMPTS {
            let result = transition(
                &ConvState::LlmRequesting { attempt },
                &test_context(),
                Event::LlmError {
                    message: "ctx".to_string(),
                    error_kind: ErrorKind::ContextExhausted,
                    attempt,
                    recovery_in_progress: false,
                    resets_at: None,
                },
            )
            .unwrap();
            assert!(
                matches!(result.new_state, ConvState::ContextExhausted { .. }),
                "attempt {attempt} should converge on ContextExhausted"
            );
        }
    }

    #[test]
    fn test_idle_to_llm_requesting() {
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
        assert!(!result.effects.is_empty());
    }

    #[test]
    fn test_reject_message_while_busy() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::UserMessage {
                text: "Hello".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        );

        assert!(matches!(result, Err(TransitionError::AgentBusy)));
    }

    #[test]
    fn test_error_recovery() {
        let result = transition(
            &ConvState::Error {
                message: "Previous error".to_string(),
                error_kind: ErrorKind::Network,
                resets_at: None,
            },
            &test_context(),
            Event::UserMessage {
                text: "Try again".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "test-message-id".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .unwrap();

        assert!(matches!(
            result.new_state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
    }

    #[test]
    fn test_cancellation_produces_synthetic_results() {
        use crate::state::{AssistantMessage, ToolCall, ToolInput};
        use phoenix_core::domain::llm_types::ContentBlock;

        // Build an AssistantMessage with 3 tool_use blocks matching the 3 tools
        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![
                ContentBlock::tool_use(
                    "tool-1",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "echo 1"}),
                ),
                ContentBlock::tool_use(
                    "tool-2",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "echo 2"}),
                ),
                ContentBlock::tool_use(
                    "tool-3",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "echo 3"}),
                ),
            ],
            None,
            None,
        );

        let result = transition(
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "tool-1",
                    ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                        "echo 1",
                    )),
                ),
                remaining_tools: vec![
                    ToolCall::new(
                        "tool-2",
                        ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                            "echo 2",
                        )),
                    ),
                    ToolCall::new(
                        "tool-3",
                        ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                            "echo 3",
                        )),
                    ),
                ],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message,
            },
            &test_context(),
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            },
        )
        .unwrap();

        // Phase 1: Should go to CancellingTool with AbortTool effect
        assert!(
            matches!(result.new_state, ConvState::CancellingTool { .. }),
            "Should transition to CancellingTool"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::AbortTool { .. })),
            "Should have AbortTool effect"
        );

        // Phase 2: ToolAborted -> Idle with PersistCheckpoint (atomic)
        let result2 = transition(
            &result.new_state,
            &test_context(),
            Event::ToolAborted {
                tool_use_id: "tool-1".to_string(),
            },
        )
        .unwrap();

        assert!(matches!(result2.new_state, ConvState::Idle));
        assert!(
            result2
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "Should have PersistCheckpoint effect instead of PersistToolResults"
        );
    }

    // ========================================================================
    // Context Exhaustion Tests (REQ-BED-019 through REQ-BED-024)
    // ========================================================================

    #[test]
    fn test_threshold_boundary_below() {
        // 89.9% should NOT trigger continuation
        let usage = UsageData {
            input_tokens: 89_900,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            !should_trigger_continuation(&usage, 100_000),
            "89.9% should not trigger continuation"
        );
    }

    #[test]
    fn test_threshold_boundary_at() {
        // Exactly 90% SHOULD trigger continuation
        let usage = UsageData {
            input_tokens: 90_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "90% should trigger continuation"
        );
    }

    #[test]
    fn test_threshold_boundary_above() {
        // 90.1% should trigger continuation
        let usage = UsageData {
            input_tokens: 90_100,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "90.1% should trigger continuation"
        );
    }

    #[test]
    fn test_threshold_with_output_tokens() {
        // 45k input + 45k output = 90k total >= 90% of 100k
        let usage = UsageData {
            input_tokens: 45_000,
            output_tokens: 45_000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        assert!(
            should_trigger_continuation(&usage, 100_000),
            "Combined tokens should count toward threshold"
        );
    }

    #[test]
    fn test_subagent_context_exhaustion_fails_immediately() {
        use crate::state::ContextExhaustionBehavior;
        use phoenix_core::domain::llm_types::ContentBlock;

        // Create a sub-agent context
        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 100_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
        };

        let result = handle_context_exhaustion(
            &subagent_ctx,
            vec![ContentBlock::text("response")],
            vec![], // no tools
            UsageData {
                input_tokens: 95_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            "test-req-id".to_string(),
            1,
        );

        // Sub-agent should go to Failed, not AwaitingContinuation
        assert!(
            matches!(
                result.new_state,
                ConvState::Failed {
                    error_kind: ErrorKind::ContextExhausted,
                    ..
                }
            ),
            "Sub-agent should fail immediately, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent of failure"
        );

        // Should NOT request continuation
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestContinuation { .. })),
            "Sub-agent should NOT request continuation"
        );
    }

    /// Build a sub-agent `ConvContext` (`is_sub_agent: true`).
    fn sub_agent_context() -> ConvContext {
        use crate::state::ContextExhaustionBehavior;
        ConvContext {
            mode_context: None,
            conversation_id: "subagent-cancel".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 100_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
        }
    }

    /// A sub-agent cancelled mid-tool must abort the in-flight tool (its
    /// bash/patch keeps mutating the shared worktree otherwise) AND defer
    /// notifying the parent until the tool has settled. It routes through
    /// `CancellingTool` (mirroring the parent's `ToolExecuting + UserCancel`
    /// path) rather than failing immediately, so the parent is not told the
    /// sub-agent stopped while the tool task is still mid-flight.
    #[test]
    fn test_subagent_cancel_during_tool_execution_routes_through_cancelling_tool() {
        use crate::state::{AssistantMessage, ToolCall, ToolInput};
        use phoenix_core::domain::llm_types::ContentBlock;

        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::tool_use(
                "sa-tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "echo mutate"}),
            )],
            None,
            None,
        );

        let result = transition(
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "sa-tool-1",
                    ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                        "echo mutate",
                    )),
                ),
                remaining_tools: vec![],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message,
            },
            &sub_agent_context(),
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            },
        )
        .expect("sub-agent ToolExecuting + UserCancel must transition");

        // Routes through CancellingTool (waits for the tool to settle), NOT
        // straight to Failed — so NotifyParent fires only after the tool stops.
        assert!(
            matches!(
                &result.new_state,
                ConvState::CancellingTool { tool_use_id, .. } if tool_use_id == "sa-tool-1"
            ),
            "sub-agent cancel should route through CancellingTool, got {:?}",
            result.new_state
        );

        // AbortTool for the in-flight tool so it stops mutating the worktree.
        let abort = result
            .effects
            .iter()
            .find_map(|e| {
                if let Effect::AbortTool { tool_use_id } = e {
                    Some(tool_use_id.clone())
                } else {
                    None
                }
            })
            .expect("sub-agent ToolExecuting cancel must emit AbortTool");
        assert_eq!(abort, "sa-tool-1", "AbortTool must target the current tool");

        // Crucially, the parent is NOT notified yet — that is deferred until the
        // tool settles. Notifying now would tell the parent the sub-agent
        // stopped while its tool task is still running.
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "NotifyParent must be deferred until the tool settles, not emitted on the cancel step"
        );
    }

    /// Once the aborted tool settles, the cancelled sub-agent fails terminally
    /// and notifies the parent (preserving fan-in). The parent's analogous arm
    /// goes to Idle to resume the user's turn — a sub-agent has no turn to
    /// resume, so it must end Failed instead.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_subagent_cancelling_tool_settles_to_failed_and_notifies_parent() {
        use crate::state::AssistantMessage;
        use phoenix_core::domain::llm_types::ContentBlock;

        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::tool_use(
                "sa-tool-1",
                "bash",
                serde_json::json!({"op": "run", "cmd": "echo mutate"}),
            )],
            None,
            None,
        );

        let cancelling = ConvState::CancellingTool {
            tool_use_id: "sa-tool-1".to_string(),
            skipped_tools: vec![],
            completed_results: vec![],
            assistant_message,
            pending_sub_agents: vec![],
        };

        // ToolAborted (the cancel-branch outcome) settles the round.
        let result = transition(
            &cancelling,
            &sub_agent_context(),
            Event::ToolAborted {
                tool_use_id: "sa-tool-1".to_string(),
            },
        )
        .expect("sub-agent CancellingTool + ToolAborted must transition");

        assert!(
            matches!(
                result.new_state,
                ConvState::Failed {
                    error_kind: ErrorKind::Cancelled,
                    ..
                }
            ),
            "settled sub-agent cancel should fail terminally, got {:?}",
            result.new_state
        );
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::AbortTool { .. })),
            "the tool already settled, so no second AbortTool"
        );
        let notified = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::NotifyParent { .. }));
        assert!(
            notified,
            "settling the cancelled sub-agent must notify the parent for fan-in"
        );

        // A racing ToolComplete (process finished just as cancel landed) settles
        // identically: Failed + NotifyParent, result discarded.
        let result_complete = transition(
            &cancelling,
            &sub_agent_context(),
            Event::ToolComplete {
                tool_use_id: "sa-tool-1".to_string(),
                result: ToolResult::success("sa-tool-1".to_string(), "done".to_string()),
            },
        )
        .expect("sub-agent CancellingTool + ToolComplete must transition");
        assert!(
            matches!(
                result_complete.new_state,
                ConvState::Failed {
                    error_kind: ErrorKind::Cancelled,
                    ..
                }
            ),
            "racing ToolComplete must also settle to Failed, got {:?}",
            result_complete.new_state
        );
        assert!(
            result_complete
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "racing ToolComplete settle must also notify the parent"
        );

        // A SECOND UserCancel arriving before the tool settles must be absorbed:
        // stay in CancellingTool, do NOT notify the parent (else the parent
        // resumes while the tool is still aborting — the race this state prevents).
        let result_recancel = transition(
            &cancelling,
            &sub_agent_context(),
            Event::UserCancel {
                reason: Some("second cancel".to_string()),
                cause: CancelCause::UserRequested,
            },
        )
        .expect("sub-agent CancellingTool + UserCancel must be absorbed");
        assert!(
            matches!(result_recancel.new_state, ConvState::CancellingTool { .. }),
            "a repeated cancel must stay in CancellingTool, got {:?}",
            result_recancel.new_state
        );
        assert!(
            !result_recancel
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "a repeated cancel must NOT notify the parent before the tool settles"
        );
    }

    /// Cancel from a non-tool sub-agent state (e.g. `LlmRequesting`) takes the
    /// general arm: Failed + `NotifyParent`, and crucially NO `AbortTool` (there
    /// is no tool to abort).
    #[test]
    fn test_subagent_cancel_outside_tool_execution_has_no_abort_tool() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &sub_agent_context(),
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            },
        )
        .expect("sub-agent LlmRequesting + UserCancel must transition");

        assert!(
            matches!(
                result.new_state,
                ConvState::Failed {
                    error_kind: ErrorKind::Cancelled,
                    ..
                }
            ),
            "sub-agent cancel should fail, got {:?}",
            result.new_state
        );
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::AbortTool { .. })),
            "no tool is running, so there must be no AbortTool effect"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "sub-agent cancel must notify the parent"
        );
    }

    #[test]
    fn test_parent_context_exhaustion_triggers_continuation() {
        use crate::state::{ToolCall, ToolInput};
        use phoenix_core::domain::llm_types::ContentBlock;

        let parent_ctx = test_context(); // Uses ThresholdBasedContinuation

        let tool_calls = vec![ToolCall::new(
            "tool-1",
            ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                "echo test",
            )),
        )];

        let result = handle_context_exhaustion(
            &parent_ctx,
            vec![ContentBlock::text("response")],
            tool_calls.clone(),
            UsageData {
                input_tokens: 95_000,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            "test-req-id".to_string(),
            1,
        );

        // Parent should go to AwaitingContinuation
        assert!(
            matches!(result.new_state, ConvState::AwaitingContinuation { .. }),
            "Parent should enter AwaitingContinuation, got {:?}",
            result.new_state
        );

        // Should request continuation with rejected tools
        assert!(
            result.effects.iter().any(|e| matches!(
                e,
                Effect::RequestContinuation { request }
                    if request.rejected_tool_calls.len() == 1
            )),
            "Parent should request continuation with rejected tools"
        );

        // Should NOT notify parent (it's not a sub-agent)
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent conversation should NOT notify parent"
        );
    }

    #[test]
    fn test_subagent_text_only_response_is_implicit_completion() {
        use crate::state::ContextExhaustionBehavior;
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
        };

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::text("Here is my analysis of the codebase.")],
                tool_calls: vec![], // No tools — LLM didn't call submit_result
                end_turn: true,
                usage: Usage {
                    input_tokens: 5000,
                    output_tokens: 500,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
                request_id: "test-req-id".to_string(),
            },
        )
        .unwrap();

        // Should go to Completed, NOT Idle
        assert!(
            matches!(result.new_state, ConvState::Completed { .. }),
            "Sub-agent text-only response should go to Completed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent on implicit completion"
        );

        // Should NOT emit notify_agent_done (that's for parent conversations)
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyAgentDone)),
            "Sub-agent should NOT emit agent_done SSE event"
        );
    }

    #[test]
    fn test_parent_text_only_response_still_goes_idle() {
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let parent_ctx = test_context();

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &parent_ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::text("Here is my response.")],
                tool_calls: vec![],
                end_turn: true,
                usage: Usage {
                    input_tokens: 5000,
                    output_tokens: 500,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
                request_id: "test-req-id".to_string(),
            },
        )
        .unwrap();

        // Parent should still go to Idle
        assert!(
            matches!(result.new_state, ConvState::Idle),
            "Parent text-only response should go to Idle, got {:?}",
            result.new_state
        );

        // Should NOT notify parent (it IS the parent)
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent should NOT have NotifyParent effect"
        );
    }

    #[test]
    fn test_subagent_llm_retries_exhausted_notifies_parent() {
        use crate::state::ContextExhaustionBehavior;

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
        };

        // attempt == MAX_RETRY_ATTEMPTS (3), retryable error → retries exhausted
        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &subagent_ctx,
            Event::LlmError {
                message: "Request timeout".to_string(),
                error_kind: ErrorKind::Network, // retryable
                attempt: 3,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        // Sub-agent should go to Failed, NOT Error
        assert!(
            matches!(result.new_state, ConvState::Failed { .. }),
            "Sub-agent with exhausted retries should go to Failed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent when LLM retries are exhausted"
        );
    }

    #[test]
    fn test_subagent_llm_non_retryable_error_notifies_parent() {
        use crate::state::ContextExhaustionBehavior;

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            work_scope_worktree: None,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
        };

        // Non-retryable error at attempt 1 → immediate failure
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmError {
                message: "Invalid API key".to_string(),
                error_kind: ErrorKind::Auth, // non-retryable
                attempt: 1,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        // Sub-agent should go to Failed, NOT Error
        assert!(
            matches!(result.new_state, ConvState::Failed { .. }),
            "Sub-agent with non-retryable error should go to Failed, got {:?}",
            result.new_state
        );

        // Should notify parent
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Sub-agent should notify parent on non-retryable LLM error"
        );
    }

    /// REQ-PROJ-008 / REQ-PROJ-036 (SubAgentProposeTaskRejected): a sub-agent
    /// never has `propose_task` in its registry, but a stale/replayed sole
    /// `propose_task` call must be rejected in the state machine — surfaced as a
    /// tool error and the LLM re-requested — not stalled or routed to the
    /// executor's unreachable `run()` fallback.
    #[test]
    fn test_subagent_sole_propose_task_rejected_not_stalled() {
        use crate::state::{ContextExhaustionBehavior, ProposeTaskInput, ToolInput};
        use phoenix_core::domain::db_schema::ToolOutcome;
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let subagent_ctx = ConvContext {
            mode_context: None,
            conversation_id: "subagent-1".to_string(),
            root_conversation_id: "test-root".to_string(),
            working_dir: PathBuf::from("/tmp"),
            model_id: "test-model".to_string(),
            is_sub_agent: true,
            context_window: 200_000,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            max_turns: 0,
            desired_base_branch: None,
            mode: ModeKind::Managed,
            tasks_dir_name: taskmd_core::constants::DEFAULT_TASKS_DIR_NAME.to_string(),
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            persona: None,
            work_scope_worktree: None,
        };

        let propose_tool = ToolCall::new(
            "tool-propose-1",
            ToolInput::ProposeTask(ProposeTaskInput {
                task_file: "tasks/12345-p1-ready--fix-the-bug.md".to_string(),
            }),
        );

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &subagent_ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool-propose-1".to_string(),
                    name: "propose_task".to_string(),
                    input: serde_json::json!({
                        "task_file": "tasks/12345-p1-ready--fix-the-bug.md"
                    }),
                }],
                tool_calls: vec![propose_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        )
        .expect("sub-agent sole propose_task must produce Ok transition");

        // Re-request the LLM, not stall and not complete/fail.
        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "sub-agent sole propose_task must re-request the LLM, got {:?}",
            result.new_state
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "should have RequestLlm to feed the rejection back"
        );

        // The checkpoint's tool result is an error explaining the parent owns
        // task management — not the executor's "this is a bug" fallback.
        let checkpoint = result
            .effects
            .iter()
            // reason: selecting one Effect variant out of ~22; listing the rest just to
            // map them all to None would obscure the single variant of interest.
            .find_map(|e| {
                #[allow(clippy::wildcard_enum_match_arm)]
                match e {
                    Effect::PersistCheckpoint { data } => Some(data),
                    _ => None,
                }
            })
            .expect("should persist a tool-round checkpoint with the rejection");
        let CheckpointData::ToolRound { tool_results, .. } = checkpoint;
        assert_eq!(tool_results.len(), 1);
        match &tool_results[0].outcome {
            ToolOutcome::Error { output, .. } => {
                assert!(
                    output.contains("parent"),
                    "rejection must explain task management is the parent's job, got: {output}"
                );
            }
            other @ (ToolOutcome::Success { .. } | ToolOutcome::Cancelled { .. }) => {
                panic!("expected an error tool result, got {other:?}")
            }
        }
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::ExecuteTool { .. })),
            "must not dispatch the propose_task tool to the executor"
        );
    }

    #[test]
    fn test_parent_llm_retries_exhausted_still_goes_to_error() {
        let parent_ctx = test_context();

        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &parent_ctx,
            Event::LlmError {
                message: "Request timeout".to_string(),
                error_kind: ErrorKind::Network,
                attempt: 3,
                recovery_in_progress: false,
                resets_at: None,
            },
        )
        .unwrap();

        // Parent should still go to Error (user can retry)
        assert!(
            matches!(result.new_state, ConvState::Error { .. }),
            "Parent with exhausted retries should go to Error, got {:?}",
            result.new_state
        );

        // Should NOT notify parent
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyParent { .. })),
            "Parent should NOT have NotifyParent effect"
        );
    }

    // ========================================================================
    // Ask User Question Tests (REQ-AUQ-001)
    // ========================================================================

    fn make_ask_user_question_tool_call(tool_id: &str) -> ToolCall {
        use crate::state::{AskUserQuestionInput, QuestionOption, ToolInput, UserQuestion};
        ToolCall::new(
            tool_id,
            ToolInput::AskUserQuestion(AskUserQuestionInput {
                questions: vec![UserQuestion {
                    question: "Which library?".to_string(),
                    header: "Dependencies".to_string(),
                    options: vec![
                        QuestionOption {
                            label: "lodash".to_string(),
                            description: None,
                            preview: None,
                        },
                        QuestionOption {
                            label: "ramda".to_string(),
                            description: None,
                            preview: None,
                        },
                    ],
                    multi_select: false,
                }],
                metadata: None,
            }),
        )
    }

    #[test]
    fn test_llm_response_with_ask_user_question_goes_to_awaiting() {
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let ctx = test_context();
        let tool = make_ask_user_question_tool_call("tool-auq-1");

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![
                    ContentBlock::text("Let me ask you something"),
                    ContentBlock::ToolUse {
                        id: "tool-auq-1".to_string(),
                        name: "ask_user_question".to_string(),
                        input: serde_json::json!({}),
                    },
                ],
                tool_calls: vec![tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        )
        .unwrap();

        assert!(
            matches!(result.new_state, ConvState::AwaitingUserResponse { .. }),
            "Should go to AwaitingUserResponse, got {:?}",
            result.new_state
        );

        // Should have PersistCheckpoint + PersistState
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "Should have PersistCheckpoint effect"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistState)),
            "Should have PersistState effect"
        );
    }

    #[test]
    fn test_ask_user_question_must_be_only_tool() {
        use crate::state::ToolInput;
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let ctx = test_context();
        let auq_tool = make_ask_user_question_tool_call("tool-auq-1");
        let bash_tool = ToolCall::new(
            "tool-bash-1",
            ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                "echo test",
            )),
        );

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-auq-1".to_string(),
                        name: "ask_user_question".to_string(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-bash-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"op": "run", "cmd": "echo test"}),
                    },
                ],
                tool_calls: vec![auq_tool, bash_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        );

        let result = result.expect("Should produce Ok transition");
        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "Should transition back to LlmRequesting when ask_user_question mixed with other tools, got {:?}",
            result.new_state
        );
        // All tool calls should have error results in the checkpoint
        let has_checkpoint = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::PersistCheckpoint { .. }));
        assert!(
            has_checkpoint,
            "Should have PersistCheckpoint with error results"
        );
        // Should re-request the LLM
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Should have RequestLlm effect to feed errors back"
        );
    }

    #[test]
    fn test_propose_task_must_be_only_tool() {
        use crate::state::{ProposeTaskInput, ToolInput};
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let ctx = test_context();
        let propose_tool = ToolCall::new(
            "tool-propose-1",
            ToolInput::ProposeTask(ProposeTaskInput {
                task_file: "tasks/12345-p1-ready--fix-the-bug.md".to_string(),
            }),
        );
        let bash_tool = ToolCall::new(
            "tool-bash-1",
            ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                "echo test",
            )),
        );

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-propose-1".to_string(),
                        name: "propose_task".to_string(),
                        input: serde_json::json!({
                            "title": "Fix the bug",
                            "priority": "p1",
                            "plan": "Step 1: Do the thing"
                        }),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-bash-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"op": "run", "cmd": "echo test"}),
                    },
                ],
                tool_calls: vec![propose_tool, bash_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        );

        let result = result.expect("Should produce Ok transition");
        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "Should transition back to LlmRequesting when propose_task mixed with other tools, got {:?}",
            result.new_state
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "Should have PersistCheckpoint with error results"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Should have RequestLlm effect to feed errors back"
        );
    }

    /// Task 13018 follow-up: a `propose_task` whose payload failed to
    /// deserialise (`ToolInput::Malformed{name: "propose_task", ...}`) must
    /// be intercepted in the typed approval flow — the serde error is
    /// surfaced as a `tool_result` and the LLM is re-requested. Without this
    /// interception the malformed call would fall through to the executor
    /// where `propose_task`'s fallback `run()` returns a generic error, hiding
    /// the precise serde diagnostic and skipping the typed approval path.
    #[test]
    fn test_malformed_propose_task_surfaces_serde_error_to_llm() {
        use crate::state::ToolInput;
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let ctx = test_context();
        // Construct via from_name_and_value so we exercise the same path the
        // runtime takes when parsing an LLM tool call with a bad payload.
        let bad_payload = serde_json::json!({"unexpected": "shape"});
        let parsed = ToolInput::from_name_and_value("propose_task", bad_payload.clone());
        assert!(
            matches!(parsed, ToolInput::Malformed { .. }),
            "test setup: expected Malformed, got {parsed:?}"
        );
        let propose_tool = ToolCall::new("tool-propose-1", parsed);

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool-propose-1".to_string(),
                    name: "propose_task".to_string(),
                    input: bad_payload,
                }],
                tool_calls: vec![propose_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        )
        .expect("transition must succeed");

        // The structural backstop: a malformed propose_task neither falls
        // through to ToolExecuting (executor dispatch) nor advances to the
        // approval state — it goes back to LlmRequesting with a tool_result
        // error in the persisted checkpoint.
        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "malformed propose_task must re-request the LLM, got {:?}",
            result.new_state
        );
        assert!(
            !matches!(result.new_state, ConvState::ToolExecuting { .. }),
            "malformed propose_task must not fall through to ToolExecuting"
        );
        assert!(
            !matches!(result.new_state, ConvState::AwaitingTaskApproval { .. }),
            "malformed propose_task must not enter the approval state"
        );

        let has_checkpoint = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::PersistCheckpoint { .. }));
        assert!(
            has_checkpoint,
            "must persist a tool_result checkpoint with the serde error"
        );
        let has_request_llm = result
            .effects
            .iter()
            .any(|e| matches!(e, Effect::RequestLlm));
        assert!(has_request_llm, "must re-request the LLM");
    }

    /// Task 13018 follow-up: same structural backstop for `ask_user_question`.
    #[test]
    fn test_malformed_ask_user_question_surfaces_serde_error_to_llm() {
        use crate::state::ToolInput;
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};

        let ctx = test_context();
        let bad_payload = serde_json::json!({"questions": "not-an-array"});
        let parsed = ToolInput::from_name_and_value("ask_user_question", bad_payload.clone());
        assert!(
            matches!(parsed, ToolInput::Malformed { .. }),
            "test setup: expected Malformed, got {parsed:?}"
        );
        let auq_tool = ToolCall::new("tool-auq-1", parsed);

        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &ctx,
            Event::LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool-auq-1".to_string(),
                    name: "ask_user_question".to_string(),
                    input: bad_payload,
                }],
                tool_calls: vec![auq_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            },
        )
        .expect("transition must succeed");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { .. }),
            "malformed ask_user_question must re-request the LLM, got {:?}",
            result.new_state
        );
        assert!(
            !matches!(result.new_state, ConvState::ToolExecuting { .. }),
            "malformed ask_user_question must not fall through to ToolExecuting"
        );
        assert!(
            !matches!(result.new_state, ConvState::AwaitingUserResponse { .. }),
            "malformed ask_user_question must not enter the awaiting-response state"
        );

        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
            "must persist a tool_result checkpoint with the serde error"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "must re-request the LLM"
        );
    }

    #[test]
    fn test_awaiting_user_response_with_answer_goes_to_llm_requesting() {
        use crate::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let mut answers = std::collections::HashMap::new();
        answers.insert("Which library?".to_string(), "lodash".to_string());

        let result = transition(
            &state,
            &test_context(),
            Event::UserQuestionResponse {
                answers,
                annotations: None,
            },
        )
        .unwrap();

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "Should go to LlmRequesting, got {:?}",
            result.new_state
        );

        // Should have PersistMessage (user answers) + PersistState + RequestLlm
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistMessage { .. })),
            "Should have PersistMessage effect for user answers"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Should have RequestLlm effect"
        );
    }

    #[test]
    fn test_awaiting_user_response_dismisses_without_resuming_llm() {
        use crate::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let result = transition(&state, &test_context(), Event::UserQuestionDismissed).unwrap();

        assert!(
            matches!(result.new_state, ConvState::Idle),
            "Dismiss should return to Idle so the user can type a message, got {:?}",
            result.new_state
        );

        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistMessage { .. })),
            "Dismiss must not persist an implicit answer or proceed instruction"
        );

        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Dismiss must not request the LLM on its own"
        );

        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistState)),
            "Dismiss should persist the Idle state"
        );
    }

    #[test]
    fn test_awaiting_user_response_rejects_user_message() {
        use crate::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let result = transition(
            &state,
            &test_context(),
            Event::UserMessage {
                text: "hello".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "msg-1".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        );

        assert!(
            matches!(result, Err(TransitionError::AwaitingUserResponse)),
            "Should reject user messages with AwaitingUserResponse error, got {result:?}"
        );
    }

    #[test]
    fn test_user_message_after_question_dismissal_resumes_agent() {
        use crate::state::UserQuestion;

        let state = ConvState::AwaitingUserResponse {
            questions: vec![UserQuestion {
                question: "Which library?".to_string(),
                header: "Dependencies".to_string(),
                options: vec![],
                multi_select: false,
            }],
            tool_use_id: "tool-auq-1".to_string(),
        };

        let dismissed = transition(&state, &test_context(), Event::UserQuestionDismissed).unwrap();

        let result = transition(
            &dismissed.new_state,
            &test_context(),
            Event::UserMessage {
                text: "Use lodash.".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "msg-1".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
        )
        .unwrap();

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "Free-form user message after dismiss should resume the agent, got {:?}",
            result.new_state
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistMessage { .. })),
            "Free-form message should be persisted explicitly"
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "Free-form message should request LLM"
        );
    }

    /// Race scenario: SSE-stream connect triggers `should_auto_continue`,
    /// state moves Idle -> `LlmRequesting` before the client receives the
    /// state change. User clicks "trigger continuation" against the stale
    /// Idle UI. The state machine must absorb the event, not surface it as
    /// an `InvalidTransition` error to the user.
    #[test]
    fn user_trigger_continuation_in_llm_requesting_is_absorbed() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 1 },
            &test_context(),
            Event::UserTriggerContinuation,
        )
        .expect("absorb, not error");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "state must not change when absorbing, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.is_empty(),
            "absorb arm must produce no effects, got {} effects",
            result.effects.len()
        );
    }

    #[test]
    fn user_trigger_continuation_in_tool_executing_is_absorbed() {
        use crate::state::{AssistantMessage, ToolCall, ToolInput};

        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run("echo")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        };

        let result = transition(&state, &test_context(), Event::UserTriggerContinuation)
            .expect("absorb, not error");

        assert!(matches!(result.new_state, ConvState::ToolExecuting { .. }));
        assert!(result.effects.is_empty());
    }

    #[test]
    fn user_trigger_continuation_in_awaiting_continuation_is_absorbed() {
        // Already summarizing — clicking again is a redundant intent, not
        // an invalid one.
        let state = ConvState::AwaitingContinuation {
            rejected_tool_calls: vec![],
            attempt: 1,
        };

        let result = transition(&state, &test_context(), Event::UserTriggerContinuation)
            .expect("absorb, not error");

        assert!(matches!(
            result.new_state,
            ConvState::AwaitingContinuation { attempt: 1, .. }
        ));
        assert!(result.effects.is_empty());
    }

    #[test]
    fn check_user_message_acceptable_idle_ok() {
        assert!(check_user_message_acceptable(&ConvState::Idle).is_ok());
    }

    #[test]
    fn check_user_message_acceptable_context_exhausted_returns_typed_error() {
        let state = ConvState::ContextExhausted {
            summary: "summary".to_string(),
        };
        let err = check_user_message_acceptable(&state).expect_err("must reject");
        assert!(matches!(err, TransitionError::ContextExhausted));
    }

    #[test]
    fn check_user_message_acceptable_terminal_returns_typed_error() {
        let err = check_user_message_acceptable(&ConvState::Terminal).expect_err("must reject");
        assert!(matches!(err, TransitionError::ConversationTerminal));
    }

    #[test]
    fn check_user_message_acceptable_busy_returns_agent_busy() {
        let err = check_user_message_acceptable(&ConvState::LlmRequesting { attempt: 1 })
            .expect_err("must reject");
        assert!(matches!(err, TransitionError::AgentBusy));
    }

    #[test]
    fn user_trigger_continuation_from_idle_still_starts_continuation() {
        // Regression guard: the absorb arm must not steal the Idle path,
        // which is the actual user-initiated continuation flow.
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::UserTriggerContinuation,
        )
        .expect("Idle path should succeed");

        assert!(
            matches!(
                result.new_state,
                ConvState::AwaitingContinuation { attempt: 1, .. }
            ),
            "Idle + UserTriggerContinuation must enter AwaitingContinuation, got {:?}",
            result.new_state
        );
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestContinuation { .. })),
            "Idle path must fire RequestContinuation effect"
        );
    }

    // ============================================================================
    // SteerDrainedUserMessages transition tests
    // ============================================================================

    fn mk_steer_entry(id: &str, text: &str) -> crate::event::SteerEntry {
        crate::event::SteerEntry {
            text: text.to_string(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: id.to_string(),
            user_agent: None,
            skill_invocation: None,
        }
    }

    #[test]
    fn steer_drained_from_idle_persists_all_and_transitions() {
        let entries = vec![
            mk_steer_entry("m1", "first"),
            mk_steer_entry("m2", "second"),
            mk_steer_entry("m3", "third"),
        ];

        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::SteerDrainedUserMessages { entries },
        )
        .expect("Idle + SteerDrainedUserMessages must succeed");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
            "must enter LlmRequesting attempt=1, got {:?}",
            result.new_state
        );

        let persist_ids: Vec<&str> = result
            .effects
            .iter()
            // reason: selecting one Effect variant out of ~22; listing the rest just to
            // map them all to None would obscure the single variant of interest.
            .filter_map(|e| {
                #[allow(clippy::wildcard_enum_match_arm)]
                match e {
                    Effect::PersistMessage { message_id, .. } => Some(message_id.as_str()),
                    _ => None,
                }
            })
            .collect();
        assert_eq!(
            persist_ids,
            vec!["m1", "m2", "m3"],
            "must emit PersistMessage effects in input order"
        );

        let persist_state_count = result
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::PersistState))
            .count();
        assert_eq!(persist_state_count, 1, "must emit exactly one PersistState");

        let request_llm_count = result
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::RequestLlm))
            .count();
        assert_eq!(
            request_llm_count, 1,
            "Idle path must issue exactly one RequestLlm"
        );

        let notify_count = result
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::NotifyStateChange))
            .count();
        assert_eq!(
            notify_count, 1,
            "Idle path must emit exactly one state-change notification"
        );

        // Crash-safety ordering: ClearSteeringQueue must come AFTER PersistState
        // (so DB queue is only cleared once messages + state are durable).
        let last_persist_msg_idx = result
            .effects
            .iter()
            .rposition(|e| matches!(e, Effect::PersistMessage { .. }))
            .expect("PersistMessage must be present");
        let persist_state_idx = result
            .effects
            .iter()
            .position(|e| matches!(e, Effect::PersistState))
            .expect("PersistState must be present");
        let clear_idx = result
            .effects
            .iter()
            .position(|e| matches!(e, Effect::ClearSteeringQueueEntries { .. }))
            .expect("ClearSteeringQueueEntries must be present");
        assert!(
            last_persist_msg_idx < persist_state_idx
                && persist_state_idx < clear_idx,
            "ordering must be: all PersistMessage < PersistState < ClearSteeringQueue, \
             got persist_msg={last_persist_msg_idx} persist_state={persist_state_idx} clear={clear_idx}"
        );
    }

    #[test]
    fn steer_drained_from_llm_requesting_persists_no_request_llm() {
        let entries = vec![
            mk_steer_entry("m1", "first"),
            mk_steer_entry("m2", "second"),
        ];

        let result = transition(
            &ConvState::LlmRequesting { attempt: 2 },
            &test_context(),
            Event::SteerDrainedUserMessages { entries },
        )
        .expect("LlmRequesting + SteerDrainedUserMessages must succeed");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 2 }),
            "attempt count must be preserved, got {:?}",
            result.new_state
        );

        let persist_count = result
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::PersistMessage { .. }))
            .count();
        assert_eq!(persist_count, 2, "must persist all entries");

        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistState)),
            "must emit PersistState"
        );

        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::RequestLlm)),
            "mid-turn drain must NOT issue RequestLlm — request already in flight"
        );

        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::NotifyStateChange)),
            "mid-turn drain must NOT emit state-change notification — state unchanged"
        );

        // Crash-safety ordering: ClearSteeringQueue must come AFTER PersistState.
        let last_persist_msg_idx = result
            .effects
            .iter()
            .rposition(|e| matches!(e, Effect::PersistMessage { .. }))
            .expect("PersistMessage must be present");
        let persist_state_idx = result
            .effects
            .iter()
            .position(|e| matches!(e, Effect::PersistState))
            .expect("PersistState must be present");
        let clear_idx = result
            .effects
            .iter()
            .position(|e| matches!(e, Effect::ClearSteeringQueueEntries { .. }))
            .expect("ClearSteeringQueueEntries must be present");
        assert!(
            last_persist_msg_idx < persist_state_idx
                && persist_state_idx < clear_idx,
            "mid-turn ordering must be: all PersistMessage < PersistState < ClearSteeringQueue, \
             got persist_msg={last_persist_msg_idx} persist_state={persist_state_idx} clear={clear_idx}"
        );
    }

    #[test]
    fn steer_drained_from_tool_executing_rejected() {
        use crate::state::{AssistantMessage, ToolCall, ToolInput};

        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run("echo")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        };

        let result = transition(
            &state,
            &test_context(),
            Event::SteerDrainedUserMessages {
                entries: vec![mk_steer_entry("m1", "x")],
            },
        );

        assert!(
            matches!(result, Err(TransitionError::AgentBusy)),
            "ToolExecuting must reject SteerDrainedUserMessages with AgentBusy, got {result:?}"
        );
    }

    #[test]
    fn steer_drained_from_terminal_rejected_or_absorbed() {
        // Terminal state has a catch-all absorbing arm in transition_parent
        // (matches the existing behavior for other events). Confirm it does NOT
        // mutate state and produces no effects.
        let result = transition(
            &ConvState::Terminal,
            &test_context(),
            Event::SteerDrainedUserMessages {
                entries: vec![mk_steer_entry("m1", "x")],
            },
        )
        .expect("Terminal absorbs unknown events as no-op");

        assert!(
            matches!(result.new_state, ConvState::Terminal),
            "Terminal must remain Terminal, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.is_empty(),
            "Terminal absorb must produce no effects, got {} effects",
            result.effects.len()
        );
    }

    #[test]
    fn steer_drained_empty_entries_noop_from_idle() {
        let result = transition(
            &ConvState::Idle,
            &test_context(),
            Event::SteerDrainedUserMessages { entries: vec![] },
        )
        .expect("empty drain must succeed");

        assert!(
            matches!(result.new_state, ConvState::Idle),
            "empty drain from Idle must remain Idle, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.is_empty(),
            "empty drain must produce no effects, got {} effects",
            result.effects.len()
        );
    }

    #[test]
    fn steer_drained_empty_entries_noop_from_llm_requesting() {
        let result = transition(
            &ConvState::LlmRequesting { attempt: 3 },
            &test_context(),
            Event::SteerDrainedUserMessages { entries: vec![] },
        )
        .expect("empty drain must succeed");

        assert!(
            matches!(result.new_state, ConvState::LlmRequesting { attempt: 3 }),
            "empty drain from LlmRequesting must preserve attempt, got {:?}",
            result.new_state
        );
        assert!(
            result.effects.is_empty(),
            "empty drain must produce no effects, got {} effects",
            result.effects.len()
        );
    }

    #[test]
    fn task_resolved_from_error_reaches_terminal_with_resolve_effect() {
        // Terminal cleanup (mark-merged / abandon) must be reachable from a
        // conversation stuck in Error (e.g. a usage-limit window the user
        // merged around externally) — not just from Idle.
        let state = ConvState::Error {
            message: "You've hit your usage limit.".to_string(),
            error_kind: ErrorKind::UsageLimitReached,
            resets_at: None,
        };
        let result = transition(
            &state,
            &test_context(),
            Event::TaskResolved {
                system_message: "Marked as merged.".to_string(),
                repo_root: "/tmp".to_string(),
            },
        )
        .expect("TaskResolved must be accepted from Error");

        assert!(matches!(result.new_state, ConvState::Terminal));
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::ResolveTask { .. })),
            "must emit ResolveTask, got {:?}",
            result.effects
        );
    }

    #[test]
    fn task_resolved_from_context_exhausted_reaches_terminal_with_resolve_effect() {
        let state = ConvState::ContextExhausted {
            summary: "ran out of context".to_string(),
        };
        let result = transition(
            &state,
            &test_context(),
            Event::TaskResolved {
                system_message: "Task abandoned.".to_string(),
                repo_root: "/tmp".to_string(),
            },
        )
        .expect("TaskResolved must be accepted from ContextExhausted");

        assert!(matches!(result.new_state, ConvState::Terminal));
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::ResolveTask { .. })),
            "must emit ResolveTask, got {:?}",
            result.effects
        );
    }

    #[test]
    fn dismiss_error_from_resumable_error_returns_to_idle() {
        // UsageLimitReached is user-resumable — dismissable.
        let state = ConvState::Error {
            message: "boom".to_string(),
            error_kind: ErrorKind::UsageLimitReached,
            resets_at: None,
        };
        let result = transition(&state, &test_context(), Event::DismissError)
            .expect("DismissError must be accepted from a resumable Error");

        assert!(matches!(result.new_state, ConvState::Idle));
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistState)),
            "must persist the idle transition, got {:?}",
            result.effects
        );
    }

    #[test]
    fn dismiss_error_from_non_resumable_error_is_invalid() {
        // A non-resumable error (InvalidRequest) must NOT be dismissable to
        // Idle — that would reopen the resume path the policy denies.
        let state = ConvState::Error {
            message: "bad request".to_string(),
            error_kind: ErrorKind::InvalidRequest,
            resets_at: None,
        };
        let err = transition(&state, &test_context(), Event::DismissError)
            .expect_err("DismissError must be rejected for a non-resumable Error");
        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                event: "DismissError",
                ..
            }
        ));
    }

    #[test]
    fn dismiss_error_from_idle_is_invalid() {
        let err = transition(&ConvState::Idle, &test_context(), Event::DismissError)
            .expect_err("DismissError is only valid from Error");
        assert!(matches!(
            err,
            TransitionError::InvalidTransition {
                event: "DismissError",
                ..
            }
        ));
    }

    // ========================================================================
    // Fork proposal interception (REQ-PROJ-033/036).
    //
    // Explore parks (in-place Explore->Work gateway, unchanged); Work/Branch/
    // Direct-in-a-git-repo record a non-blocking fork and keep running.
    // ========================================================================
    mod fork_proposal {
        use super::*;
        use crate::state::{ProposeTaskInput, ToolInput};
        use phoenix_core::domain::llm_types::{ContentBlock, Usage};
        use tempfile::TempDir;

        /// A temp worktree with `tasks/<file>` written, returned together with
        /// the relative path. The dir handle keeps the files alive for the test.
        fn worktree_with_task() -> (TempDir, String) {
            let tmp = TempDir::new().unwrap();
            std::fs::create_dir(tmp.path().join("tasks")).unwrap();
            let rel = "tasks/12345-p1-ready--fix-the-bug.md".to_string();
            std::fs::write(
                tmp.path().join(&rel),
                "# Fix the bug\n\nplan body for the fork\n",
            )
            .unwrap();
            (tmp, rel)
        }

        fn ctx_for(
            tmp: &TempDir,
            mode: ModeKind,
            mode_context: Option<ModeContext>,
        ) -> ConvContext {
            let mut ctx = ConvContext::new(
                "origin-conv",
                tmp.path().to_path_buf(),
                "test-model",
                200_000,
            );
            ctx.mode = mode;
            ctx.mode_context = mode_context;
            ctx
        }

        fn propose_event(task_file: &str) -> Event {
            let propose_tool = ToolCall::new(
                "tool-propose-1",
                ToolInput::ProposeTask(ProposeTaskInput {
                    task_file: task_file.to_string(),
                }),
            );
            Event::LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool-propose-1".to_string(),
                    name: "propose_task".to_string(),
                    input: serde_json::json!({ "task_file": task_file }),
                }],
                tool_calls: vec![propose_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            }
        }

        fn fork_proposal_effect(
            effects: &[Effect],
        ) -> Option<(
            &String,
            &String,
            &String,
            phoenix_core::task_source::Priority,
            &String,
        )> {
            // reason: selecting one Effect variant out of ~22; listing the rest just to
            // map them all to None would obscure the single variant of interest.
            #[allow(clippy::wildcard_enum_match_arm)]
            effects.iter().find_map(|e| match e {
                Effect::PersistForkProposal {
                    proposal_id,
                    task_file,
                    title,
                    priority,
                    body,
                    ..
                } => Some((proposal_id, task_file, title, *priority, body)),
                _ => None,
            })
        }

        #[test]
        fn explore_valid_file_parks_into_awaiting_task_approval() {
            let (tmp, rel) = worktree_with_task();
            let ctx = ctx_for(
                &tmp,
                ModeKind::Managed,
                Some(ModeContext::Explore {
                    next_taskmd_id_hint: None,
                }),
            );

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(&rel),
            )
            .expect("transition must succeed");

            assert!(
                matches!(result.new_state, ConvState::AwaitingTaskApproval { .. }),
                "Explore must park, got {:?}",
                result.new_state
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
                "Explore parks via PersistCheckpoint"
            );
            assert!(
                fork_proposal_effect(&result.effects).is_none(),
                "Explore must NOT record a fork proposal"
            );
        }

        #[test]
        fn work_valid_file_forks_without_parking() {
            let (tmp, rel) = worktree_with_task();
            let ctx = ctx_for(
                &tmp,
                ModeKind::Managed,
                Some(ModeContext::Work {
                    branch_name: "task-12345".to_string(),
                    base_branch: "main".to_string(),
                    worktree_path: tmp.path().display().to_string(),
                }),
            );

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(&rel),
            )
            .expect("transition must succeed");

            assert!(
                matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
                "Work fork continues running, got {:?}",
                result.new_state
            );
            assert!(
                !matches!(result.new_state, ConvState::AwaitingTaskApproval { .. }),
                "Work fork must NOT park"
            );
            assert!(
                !result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
                "fork path emits PersistForkProposal, not a separate PersistCheckpoint"
            );
            let (proposal_id, task_file, title, priority, body) =
                fork_proposal_effect(&result.effects).expect("must emit PersistForkProposal");
            assert!(!proposal_id.is_empty(), "a proposal_id must be present");
            assert_eq!(task_file, &rel);
            assert_eq!(title, "Fix the bug");
            assert_eq!(priority, phoenix_core::task_source::Priority::P1);
            assert!(body.contains("plan body for the fork"));
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::RequestLlm)),
                "fork continues with a fresh LLM request"
            );
        }

        #[test]
        fn branch_valid_file_takes_the_fork_path() {
            let (tmp, rel) = worktree_with_task();
            let ctx = ctx_for(
                &tmp,
                ModeKind::Branch,
                Some(ModeContext::Branch {
                    branch_name: "feature".to_string(),
                    base_branch: "main".to_string(),
                    worktree_path: tmp.path().display().to_string(),
                }),
            );

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(&rel),
            )
            .expect("transition must succeed");

            assert!(
                matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
                "Branch fork continues running, got {:?}",
                result.new_state
            );
            assert!(
                fork_proposal_effect(&result.effects).is_some(),
                "Branch records a fork proposal"
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::RequestLlm)),
                "Branch fork re-requests the LLM"
            );
        }

        #[test]
        fn direct_in_git_repo_takes_the_fork_path() {
            let (tmp, rel) = worktree_with_task();
            // Make the worktree a git repo so is_git_repository() is satisfied.
            std::fs::create_dir(tmp.path().join(".git")).unwrap();
            let ctx = ctx_for(&tmp, ModeKind::Direct, Some(ModeContext::Direct));

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(&rel),
            )
            .expect("transition must succeed");

            assert!(
                matches!(result.new_state, ConvState::LlmRequesting { attempt: 1 }),
                "Direct-in-git fork continues running, got {:?}",
                result.new_state
            );
            assert!(
                fork_proposal_effect(&result.effects).is_some(),
                "Direct-in-git records a fork proposal"
            );
        }

        /// REQ-PROJ-033: the fork snapshot is the authoritative file BYTES. A
        /// brief with significant leading/trailing whitespace and a trailing
        /// newline must reach `Effect::PersistForkProposal { body }` unaltered —
        /// it is the verbatim source for the fork's committed file, NOT the
        /// trimmed display plan.
        #[test]
        fn fork_body_preserves_raw_file_bytes() {
            let tmp = TempDir::new().unwrap();
            std::fs::create_dir(tmp.path().join("tasks")).unwrap();
            let rel = "tasks/12345-p1-ready--fix-the-bug.md".to_string();
            let raw = "\n\n  # Fix the bug\n\nplan body for the fork\n\n  \n";
            std::fs::write(tmp.path().join(&rel), raw).unwrap();

            let ctx = ctx_for(
                &tmp,
                ModeKind::Managed,
                Some(ModeContext::Work {
                    branch_name: "task-12345".to_string(),
                    base_branch: "main".to_string(),
                    worktree_path: tmp.path().display().to_string(),
                }),
            );

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(&rel),
            )
            .expect("transition must succeed");

            let (_, _, _, _, body) =
                fork_proposal_effect(&result.effects).expect("must emit PersistForkProposal");
            assert_eq!(
                body, raw,
                "fork body must be the raw file bytes, not the trimmed plan"
            );
            assert_ne!(
                body,
                &raw.trim().to_string(),
                "fork body must NOT be the trimmed display plan"
            );
        }

        #[test]
        fn invalid_file_in_fork_mode_records_nothing() {
            let tmp = TempDir::new().unwrap();
            std::fs::create_dir(tmp.path().join("tasks")).unwrap();
            // Closed `done` status — rejected by the shared validation.
            let rel = "tasks/12345-p1-done--closed.md";
            std::fs::write(tmp.path().join(rel), "# closed\n").unwrap();
            let ctx = ctx_for(
                &tmp,
                ModeKind::Managed,
                Some(ModeContext::Work {
                    branch_name: "task-12345".to_string(),
                    base_branch: "main".to_string(),
                    worktree_path: tmp.path().display().to_string(),
                }),
            );

            let result = transition(
                &ConvState::LlmRequesting { attempt: 1 },
                &ctx,
                propose_event(rel),
            )
            .expect("transition must succeed");

            assert!(
                matches!(result.new_state, ConvState::LlmRequesting { .. }),
                "invalid file re-requests the LLM, got {:?}",
                result.new_state
            );
            assert!(
                fork_proposal_effect(&result.effects).is_none(),
                "an invalid file must NOT record a fork proposal"
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
                "the tool error is persisted as a normal checkpoint"
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::RequestLlm)),
                "the LLM is re-requested to fix the file"
            );
        }

        #[test]
        fn propose_task_with_second_tool_still_errors_in_fork_mode() {
            let (tmp, rel) = worktree_with_task();
            let ctx = ctx_for(
                &tmp,
                ModeKind::Managed,
                Some(ModeContext::Work {
                    branch_name: "task-12345".to_string(),
                    base_branch: "main".to_string(),
                    worktree_path: tmp.path().display().to_string(),
                }),
            );
            let propose_tool = ToolCall::new(
                "tool-propose-1",
                ToolInput::ProposeTask(ProposeTaskInput {
                    task_file: rel.clone(),
                }),
            );
            let bash_tool = ToolCall::new(
                "tool-bash-1",
                ToolInput::Bash(phoenix_core::domain::bash_types::BashToolInput::run(
                    "echo hi",
                )),
            );
            let event = Event::LlmResponse {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-propose-1".to_string(),
                        name: "propose_task".to_string(),
                        input: serde_json::json!({ "task_file": rel }),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-bash-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({ "op": "run", "cmd": "echo hi" }),
                    },
                ],
                tool_calls: vec![propose_tool, bash_tool],
                end_turn: false,
                usage: Usage::default(),
                request_id: "test-req-id".to_string(),
            };

            let result = transition(&ConvState::LlmRequesting { attempt: 1 }, &ctx, event)
                .expect("transition must succeed");

            assert!(
                fork_proposal_effect(&result.effects).is_none(),
                "the 'must be the only tool' error pre-empts the fork path"
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::PersistCheckpoint { .. })),
                "the must-be-sole error is persisted as a checkpoint"
            );
            assert!(
                result
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::RequestLlm)),
                "the error is fed back to the LLM"
            );
        }
    }
}

#[cfg(test)]
mod resolve_task_file_tests {
    use super::resolve_task_file;
    use tempfile::TempDir;

    #[test]
    fn taskmd_file_under_tasks_dir_is_accepted() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("tasks")).unwrap();
        std::fs::write(
            tmp.path().join("tasks/12345-p1-ready--fix-login.md"),
            "# Repair login\n\nplan body\n",
        )
        .unwrap();
        let snap = resolve_task_file(tmp.path(), "tasks", "tasks/12345-p1-ready--fix-login.md")
            .expect("taskmd file should resolve");
        assert_eq!(snap.title, "Repair login");
        assert_eq!(snap.priority, phoenix_core::task_source::Priority::P1);
        assert!(snap.plan.contains("plan body"));
    }

    #[test]
    fn taskmd_file_outside_tasks_dir_is_rejected() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("elsewhere")).unwrap();
        std::fs::write(
            tmp.path().join("elsewhere/12345-p1-ready--fix-login.md"),
            "# x\n",
        )
        .unwrap();
        let err = resolve_task_file(
            tmp.path(),
            "tasks",
            "elsewhere/12345-p1-ready--fix-login.md",
        )
        .unwrap_err();
        assert!(err.contains("must be under tasks/"), "got: {err}");
    }

    #[test]
    fn taskmd_file_with_done_status_is_rejected() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("tasks")).unwrap();
        std::fs::write(tmp.path().join("tasks/12345-p1-done--x.md"), "# x\n").unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "tasks/12345-p1-done--x.md").unwrap_err();
        assert!(err.contains("cannot be proposed"), "got: {err}");
    }

    #[test]
    fn plain_markdown_anywhere_is_accepted_with_h1_title_and_p2() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        std::fs::write(
            tmp.path().join("docs/plan.md"),
            "# Migrate the database\n\nstep one\n",
        )
        .unwrap();
        let snap = resolve_task_file(tmp.path(), "tasks", "docs/plan.md")
            .expect("plain markdown should resolve");
        assert_eq!(snap.title, "Migrate the database");
        assert_eq!(snap.priority, phoenix_core::task_source::Priority::P2);
        assert_eq!(snap.task_file, "docs/plan.md");
        assert!(snap.plan.contains("step one"));

        // README.md works too.
        std::fs::write(tmp.path().join("README.md"), "# The readme\n").unwrap();
        let snap = resolve_task_file(tmp.path(), "tasks", "README.md").unwrap();
        assert_eq!(snap.title, "The readme");
    }

    #[test]
    fn non_markdown_file_is_rejected() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "# x\n").unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "notes.txt").unwrap_err();
        assert!(err.contains("must be a markdown file"), "got: {err}");
    }

    #[test]
    fn parent_dir_components_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "../escape.md").unwrap_err();
        assert!(err.contains("'..'"), "got: {err}");
    }

    #[test]
    #[cfg(unix)]
    fn symlink_task_file_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.md"), "# Secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.md"), tmp.path().join("link.md"))
            .unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "link.md").unwrap_err();
        assert!(err.contains("must be a regular file"), "got: {err}");
        // Also a symlink to a file *inside* the worktree (committed plan would
        // be the link, not the target the reader showed).
        std::fs::write(tmp.path().join("real.md"), "# Real\n").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real.md"), tmp.path().join("inner-link.md"))
            .unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "inner-link.md").unwrap_err();
        assert!(err.contains("must be a regular file"), "got: {err}");
    }

    #[test]
    #[cfg(unix)]
    fn intermediate_symlink_escaping_the_worktree_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("plan.md"), "# Outside plan\n").unwrap();
        // `escape/` inside the worktree is a symlink to the outside dir.
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("escape")).unwrap();
        let err = resolve_task_file(tmp.path(), "tasks", "escape/plan.md").unwrap_err();
        assert!(err.contains("resolves outside"), "got: {err}");
    }
}

#[cfg(test)]
mod teardown_tests {
    // ========================================================================
    // task 61004 M1: forced sub-agent teardown via the cancel protocol.
    //
    // These pin the `CancellingSubAgents + SubAgentResult` drain arms and the
    // `map_teardown_outcome` mapping: a real `Success` always wins (fidelity),
    // a `Timeout`-caused teardown relabels non-success outcomes `TimedOut`, and
    // a `UserRequested` teardown keeps the reported outcome verbatim. The cause
    // must also survive a multi-agent drain's self-transition unchanged.
    // ========================================================================

    use super::{transition, ConvState, Effect, Event};
    use crate::event::CancelCause;
    use crate::state::{PendingSubAgent, SubAgentMode, SubAgentOutcome};
    use phoenix_core::domain::db_schema::ErrorKind;
    use std::path::PathBuf;

    fn test_context() -> super::ConvContext {
        super::ConvContext::new("test-conv", PathBuf::from("/tmp"), "test-model", 200_000)
    }

    fn pending(id: &str) -> PendingSubAgent {
        PendingSubAgent {
            agent_id: id.to_string(),
            task: format!("task for {id}"),
            mode: SubAgentMode::Work,
        }
    }

    /// Extract the recorded outcome for `agent_id` from a `PersistSubAgentResults`
    /// effect (emitted on the last-one -> Idle drain).
    fn recorded_in_persist(effects: &[Effect], agent_id: &str) -> SubAgentOutcome {
        for effect in effects {
            if let Effect::PersistSubAgentResults { results, .. } = effect {
                if let Some(r) = results.iter().find(|r| r.agent_id == agent_id) {
                    return r.outcome.clone();
                }
            }
        }
        panic!("no PersistSubAgentResults effect carrying agent {agent_id}: {effects:?}");
    }

    /// Extract the recorded outcome for `agent_id` from the new state's
    /// `completed_results` (the more-pending self-transition arm).
    fn recorded_in_state(state: &ConvState, agent_id: &str) -> SubAgentOutcome {
        let ConvState::CancellingSubAgents {
            completed_results, ..
        } = state
        else {
            panic!("expected CancellingSubAgents, got {}", state.variant_name());
        };
        completed_results
            .iter()
            .find(|r| r.agent_id == agent_id)
            .map_or_else(
                || panic!("agent {agent_id} not in completed_results"),
                |r| r.outcome.clone(),
            )
    }

    /// Test 1 (last-one arm): a real `Success` reported during a `Timeout`-caused
    /// teardown is recorded as-is — fidelity beats the timeout relabel.
    #[test]
    fn real_success_wins_over_timeout_relabel_last_arm() {
        let state = ConvState::CancellingSubAgents {
            pending: vec![pending("a")],
            completed_results: vec![],
            cause: CancelCause::Timeout,
            spawn_tool_id: Some("spawn-1".to_string()),
        };
        let result = transition(
            &state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "a".to_string(),
                outcome: SubAgentOutcome::Success {
                    result: "did the thing".to_string(),
                },
            },
        )
        .unwrap();

        assert!(matches!(result.new_state, ConvState::Idle));
        assert_eq!(
            recorded_in_persist(&result.effects, "a"),
            SubAgentOutcome::Success {
                result: "did the thing".to_string()
            },
            "a real Success must be recorded verbatim even under a Timeout cause"
        );
    }

    /// Test 1 (more-pending arm): same, but with a second agent still pending so
    /// the drain self-transitions `CancellingSubAgents -> CancellingSubAgents`.
    #[test]
    fn real_success_wins_over_timeout_relabel_more_pending_arm() {
        let state = ConvState::CancellingSubAgents {
            pending: vec![pending("a"), pending("b")],
            completed_results: vec![],
            cause: CancelCause::Timeout,
            spawn_tool_id: Some("spawn-1".to_string()),
        };
        let result = transition(
            &state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "a".to_string(),
                outcome: SubAgentOutcome::Success {
                    result: "real".to_string(),
                },
            },
        )
        .unwrap();

        assert_eq!(
            recorded_in_state(&result.new_state, "a"),
            SubAgentOutcome::Success {
                result: "real".to_string()
            },
            "Success recorded verbatim in the self-transition arm too"
        );
    }

    /// Test 2: a non-success outcome under a `Timeout` cause is relabeled
    /// `TimedOut`.
    #[test]
    fn timeout_cause_relabels_non_success_to_timed_out() {
        let state = ConvState::CancellingSubAgents {
            pending: vec![pending("a")],
            completed_results: vec![],
            cause: CancelCause::Timeout,
            spawn_tool_id: Some("spawn-1".to_string()),
        };
        let result = transition(
            &state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "a".to_string(),
                outcome: SubAgentOutcome::Failure {
                    error: "presumed terminated".to_string(),
                    error_kind: ErrorKind::Cancelled,
                },
            },
        )
        .unwrap();

        assert!(matches!(result.new_state, ConvState::Idle));
        assert_eq!(
            recorded_in_persist(&result.effects, "a"),
            SubAgentOutcome::TimedOut,
            "a Failure{{Cancelled}} under a Timeout cause must be relabeled TimedOut"
        );
    }

    /// Test 3: a `UserRequested` cause keeps the reported outcome verbatim — no
    /// relabel.
    #[test]
    fn user_requested_cause_keeps_reported_outcome() {
        let state = ConvState::CancellingSubAgents {
            pending: vec![pending("a")],
            completed_results: vec![],
            cause: CancelCause::UserRequested,
            spawn_tool_id: Some("spawn-1".to_string()),
        };
        let reported = SubAgentOutcome::Failure {
            error: "cancelled by user".to_string(),
            error_kind: ErrorKind::Cancelled,
        };
        let result = transition(
            &state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "a".to_string(),
                outcome: reported.clone(),
            },
        )
        .unwrap();

        assert!(matches!(result.new_state, ConvState::Idle));
        assert_eq!(
            recorded_in_persist(&result.effects, "a"),
            reported,
            "a UserRequested teardown must keep the reported Failure{{Cancelled}} verbatim"
        );
    }

    /// Test 6: the parent's `AwaitingSubAgents` completion timeout reroutes
    /// through the cancel protocol. `AwaitingSubAgents + UserCancel{Timeout}`
    /// lands in `CancellingSubAgents{cause: Timeout}` and emits the real
    /// `Effect::CancelSubAgents` — it does NOT fabricate per-agent `TimedOut`
    /// results directly (those drain later through `CancellingSubAgents`).
    #[test]
    fn timeout_user_cancel_reroutes_to_cancelling_subagents_with_cancel_effect() {
        let state = ConvState::AwaitingSubAgents {
            pending: vec![pending("a"), pending("b")],
            completed_results: vec![],
            spawn_tool_id: None,
        };
        let result = transition(
            &state,
            &test_context(),
            Event::UserCancel {
                reason: None,
                cause: CancelCause::Timeout,
            },
        )
        .unwrap();

        let ConvState::CancellingSubAgents { pending, cause, .. } = &result.new_state else {
            panic!(
                "expected CancellingSubAgents, got {}",
                result.new_state.variant_name()
            );
        };
        assert_eq!(
            *cause,
            CancelCause::Timeout,
            "cause must be stamped Timeout"
        );
        assert_eq!(pending.len(), 2, "all agents stay pending until they drain");

        let cancel = result
            .effects
            .iter()
            .find_map(|e| {
                if let Effect::CancelSubAgents { ids } = e {
                    Some(ids.clone())
                } else {
                    None
                }
            })
            .expect("must emit Effect::CancelSubAgents");
        assert_eq!(cancel.len(), 2, "cancel targets both pending agents");

        // The reroute must NOT directly fabricate per-agent results.
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, Effect::PersistSubAgentResults { .. })),
            "the timeout reroute must not fabricate per-agent results directly"
        );
    }

    /// Cause-on-self-transition probe: a multi-agent drain's more-pending arm
    /// must carry `cause` through unchanged so later agents are still mapped
    /// correctly. Drain agent "a" (non-success) under a Timeout cause; the
    /// resulting `CancellingSubAgents` must still carry `cause: Timeout`, and a
    /// second drain of "b" (non-success) must therefore still relabel `TimedOut`.
    #[test]
    fn cause_survives_multi_agent_self_transition() {
        let state = ConvState::CancellingSubAgents {
            pending: vec![pending("a"), pending("b")],
            completed_results: vec![],
            cause: CancelCause::Timeout,
            spawn_tool_id: Some("spawn-1".to_string()),
        };
        let first = transition(
            &state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "a".to_string(),
                outcome: SubAgentOutcome::Failure {
                    error: "x".to_string(),
                    error_kind: ErrorKind::Cancelled,
                },
            },
        )
        .unwrap();

        let ConvState::CancellingSubAgents { cause, .. } = &first.new_state else {
            panic!(
                "expected CancellingSubAgents, got {}",
                first.new_state.variant_name()
            );
        };
        assert_eq!(
            *cause,
            CancelCause::Timeout,
            "cause must survive the self-transition"
        );
        assert_eq!(
            recorded_in_state(&first.new_state, "a"),
            SubAgentOutcome::TimedOut
        );

        // Drain the last agent; the carried cause must still relabel.
        let second = transition(
            &first.new_state,
            &test_context(),
            Event::SubAgentResult {
                agent_id: "b".to_string(),
                outcome: SubAgentOutcome::Failure {
                    error: "y".to_string(),
                    error_kind: ErrorKind::Cancelled,
                },
            },
        )
        .unwrap();
        assert!(matches!(second.new_state, ConvState::Idle));
        assert_eq!(
            recorded_in_persist(&second.effects, "b"),
            SubAgentOutcome::TimedOut,
            "the carried Timeout cause must relabel the last agent too"
        );
    }
}
