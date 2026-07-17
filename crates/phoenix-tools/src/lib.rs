//! Tool implementations for Phoenix IDE
//!
//! REQ-BASH-010, REQ-BT-012: Stateless Tools with Context Injection

mod ask_user_question;
pub mod bash;
pub mod bash_check;
pub mod browser;
mod commission_review;
mod keyword_search;
pub mod mcp;
pub mod patch;
pub mod process_inspection;
mod propose_task;
mod read_file;
mod read_image;
mod search;
mod skill;
mod subagent;
mod terminal_command_history;
mod terminal_last_command;
mod think;
pub mod tmux;
pub mod work_scope_inventory;

pub use ask_user_question::AskUserQuestionTool;
pub use bash::{
    BashHandleError, BashHandleRegistry, BashLifecycleEvent, BashLifecycleSink, BashOp, BashTool,
    BashToolInput, SandboxedBashTool, WorkScopeHandles as BashWorkScopeHandles,
};
pub use browser::{
    BrowserClearConsoleLogsTool, BrowserClickTool, BrowserError, BrowserEvalTool,
    BrowserKeyPressTool, BrowserNavigateTool, BrowserProfileTool, BrowserRecentConsoleLogsTool,
    BrowserResizeTool, BrowserSessionManager, BrowserTakeScreenshotTool, BrowserTypeTool,
    BrowserWaitForSelectorTool,
};
pub use commission_review::CommissionReviewTool;
pub use keyword_search::KeywordSearchTool;
pub use patch::PatchTool;
pub use propose_task::ProposeTaskTool;
pub use read_file::ReadFileTool;
pub use read_image::ReadImageTool;
pub use search::SearchTool;
pub use skill::SkillTool;
pub use subagent::{SpawnAgentsTool, SubmitErrorTool, SubmitResultTool};
pub use terminal_command_history::TerminalCommandHistoryTool;
pub use terminal_last_command::TerminalLastCommandTool;
pub use think::ThinkTool;
pub use tmux::{
    TmuxError, TmuxLifecycleEvent, TmuxLifecycleSink, TmuxRegistry, TmuxRunTool, TmuxServer,
    TmuxTool,
};

use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub use browser::BrowserSession;
use phoenix_core::domain::sm_state::ExploreBashCapability;
use phoenix_core::llm_service::LlmSelector;
use phoenix_core::platform::PlatformCapability;
use phoenix_core::work_scope::WorkScope;

/// Test-only `LlmSelector` that offers no completion service.
///
/// Tool tests construct a [`ToolContext`] purely to drive non-LLM tool paths
/// (bash, patch, search, terminal, …); they never reach a real model. This
/// fake stands in for the concrete `ModelRegistry` (which lives in phoenix-ide
/// and is invisible across the crate boundary), keeping these tests
/// self-contained. Both methods return `None`, so any tool that *does* try to
/// select a model gets the same "no model configured" path an empty registry
/// would produce.
#[cfg(test)]
pub(crate) struct NoLlm;

#[cfg(test)]
impl LlmSelector for NoLlm {
    fn get(
        &self,
        _model_id: &str,
    ) -> Option<std::sync::Arc<dyn phoenix_core::llm_service::CompletionService>> {
        None
    }

    fn default_service(
        &self,
    ) -> Option<std::sync::Arc<dyn phoenix_core::llm_service::CompletionService>> {
        None
    }
}

/// Typed image data for LLM consumption.
#[derive(Debug, Clone)]
pub struct ToolImage {
    pub media_type: String,
    pub data: String, // base64-encoded
}

#[derive(Debug, Clone)]
pub struct ToolLlmUsage {
    pub model: String,
    pub usage: phoenix_core::domain::llm_types::Usage,
}

/// Result from tool execution.
///
/// An enum, not a `struct { success: bool, output: String, .. }`: those two
/// fields were independently settable, so a tool returning `success: true`
/// with error-shaped `output` was structurally indistinguishable from a real
/// success — the bug class behind P1 incident 08545 (conversation-stuck),
/// which inferred outcome state from `output` string content. The sibling
/// persisted type `db::ToolOutcome` (in phoenix-ide) already made this
/// distinction structural; this is the producer-side counterpart every
/// `Tool::run()` returns.
///
/// There is deliberately no `Cancelled` variant. Cancellation is detected by
/// the executor via the cancellation token, never returned by `Tool::run()`;
/// a `Cancelled` variant here would just relocate the wrong-state problem
/// into this type.
#[derive(Debug, Clone)]
pub enum ToolOutput {
    Success {
        output: String,
        /// Typed images for LLM consumption (sent as image content blocks,
        /// not text).
        images: Vec<ToolImage>,
        display_data: Option<Value>,
        llm_usage: Option<Box<ToolLlmUsage>>,
    },
    Error {
        output: String,
        images: Vec<ToolImage>,
        display_data: Option<Value>,
        llm_usage: Option<Box<ToolLlmUsage>>,
    },
}

impl ToolOutput {
    pub fn success(output: impl Into<String>) -> Self {
        Self::Success {
            output: output.into(),
            images: vec![],
            display_data: None,
            llm_usage: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            output: message.into(),
            images: vec![],
            display_data: None,
            llm_usage: None,
        }
    }

    #[must_use]
    pub fn with_display(mut self, data: Value) -> Self {
        match &mut self {
            Self::Success { display_data, .. } | Self::Error { display_data, .. } => {
                *display_data = Some(data);
            }
        }
        self
    }

    /// Attach tool-spent LLM usage for runtime accounting.
    #[must_use]
    pub fn with_llm_usage(mut self, usage: ToolLlmUsage) -> Self {
        match &mut self {
            Self::Success { llm_usage, .. } | Self::Error { llm_usage, .. } => {
                *llm_usage = Some(Box::new(usage));
            }
        }
        self
    }

    pub fn take_llm_usage(&mut self) -> Option<ToolLlmUsage> {
        match self {
            Self::Success { llm_usage, .. } | Self::Error { llm_usage, .. } => {
                llm_usage.take().map(|usage| *usage)
            }
        }
    }

    #[must_use]
    pub fn with_images(mut self, imgs: Vec<ToolImage>) -> Self {
        match &mut self {
            Self::Success { images, .. } | Self::Error { images, .. } => *images = imgs,
        }
        self
    }

    /// Whether the tool reported success.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// The tool's textual output — success payload or error message.
    ///
    /// `dead_code`: production code destructures the enum directly (see
    /// `tool_output_to_outcome` in the executor); these accessors exist for
    /// test assertions that only need to read one field.
    #[allow(dead_code)]
    #[must_use]
    pub fn output(&self) -> &str {
        match self {
            Self::Success { output, .. } | Self::Error { output, .. } => output,
        }
    }

    /// Typed images for LLM consumption.
    #[allow(dead_code)]
    #[must_use]
    pub fn images(&self) -> &[ToolImage] {
        match self {
            Self::Success { images, .. } | Self::Error { images, .. } => images,
        }
    }

    /// Free-form UI hinting payload, if the tool attached one.
    #[allow(dead_code)]
    #[must_use]
    pub fn display_data(&self) -> Option<&Value> {
        match self {
            Self::Success { display_data, .. } | Self::Error { display_data, .. } => {
                display_data.as_ref()
            }
        }
    }
}

/// All context needed for a tool invocation.
///
/// Created fresh for each tool call with validated conversation context.
/// Tools should be stateless and derive all context from this struct.
///
/// REQ-BASH-010, REQ-BT-012: Stateless Tools with Context Injection
#[derive(Clone)]
pub struct ToolContext {
    /// Cancellation signal for long-running operations
    pub cancel: CancellationToken,

    /// The conversation this tool is executing within
    pub conversation_id: String,

    /// Root conversation for trace correlation across sub-agents.
    pub root_conversation_id: String,

    /// Working directory for file operations
    pub working_dir: PathBuf,

    /// Browser session manager (access via `browser()` method)
    browser_sessions: Arc<BrowserSessionManager>,

    /// Per-process bash handle registry (access via `bash_handles()` method).
    /// Owns the per-`WorkScope` handle tables, ring buffers, tombstones,
    /// and live-handle cap enforcement (REQ-BASH-005, REQ-BASH-006,
    /// REQ-BASH-014). Reached by tools through `bash_handles()` /
    /// `bash_handle_registry()`.
    bash_handles: Arc<BashHandleRegistry>,

    /// LLM selector for tools that need model access. Holds the narrow
    /// base-crate [`LlmSelector`] trait object rather than the concrete
    /// `ModelRegistry`, so tools depend only on the completion capability.
    llm_selector: Arc<dyn LlmSelector>,

    /// Active PTY terminal sessions — used by the terminal-command tools
    /// (`terminal_last_command`, `terminal_command_history`).
    pub terminals: phoenix_terminal::ActiveTerminals,

    /// Per-process tmux server registry (access via `tmux()` method).
    /// Owns the per-conversation tmux server entries (`Arc<RwLock<TmuxServer>>`)
    /// and resolves the deterministic socket path. REQ-TMUX-001 /
    /// REQ-TMUX-013.
    tmux_registry: Arc<TmuxRegistry>,

    /// The worktree path for this conversation, if in Work/Branch/Explore
    /// mode. `None` for Direct-mode conversations. Used by `tmux()` to
    /// key the socket to the worktree rather than the conversation ID so
    /// the session survives context-exhaustion continuations (task 03001).
    pub worktree_path: Option<PathBuf>,

    /// Durable owner for work-affine resources created by this tool call.
    /// Worktree-backed conversations scope to the worktree path so resources
    /// survive context-exhaustion continuations; Direct conversations fall
    /// back to the conversation id.
    pub work_scope: WorkScope,
}

impl ToolContext {
    /// Create a new tool context
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cancel: CancellationToken,
        conversation_id: String,
        working_dir: PathBuf,
        browser_sessions: Arc<BrowserSessionManager>,
        bash_handles: Arc<BashHandleRegistry>,
        llm_selector: Arc<dyn LlmSelector>,
        terminals: phoenix_terminal::ActiveTerminals,
        tmux_registry: Arc<TmuxRegistry>,
        worktree_path: Option<PathBuf>,
    ) -> Self {
        let root_conversation_id = conversation_id.clone();
        let work_scope = WorkScope::resolve(&conversation_id, worktree_path.as_deref());
        Self {
            cancel,
            conversation_id,
            root_conversation_id,
            working_dir,
            browser_sessions,
            bash_handles,
            llm_selector,
            terminals,
            tmux_registry,
            worktree_path,
            work_scope,
        }
    }

    #[must_use]
    pub fn with_root_conversation_id(mut self, root_conversation_id: String) -> Self {
        self.root_conversation_id = root_conversation_id;
        self
    }

    /// Get or create the browser session for this conversation's
    /// `WorkScope`.
    ///
    /// Lazily initializes Chrome on first call. Subsequent calls — including
    /// from a continuation that resolves to the same scope — return the
    /// existing session, so the open tabs / cookies / dev-tools state of the
    /// predecessor are inherited (REQ-BROWSER-WS-001).
    ///
    /// REQ-BT-010: Implicit Session Model
    ///
    /// # Errors
    /// Returns [`BrowserError`] when Chrome cannot be launched or the
    /// session fails to initialize on first use.
    pub async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        self.browser_sessions.get_session(&self.work_scope).await
    }

    /// Get the per-`WorkScope` bash handle table.
    ///
    /// Lazily creates the scope entry on first call; subsequent calls that
    /// resolve to the same `WorkScope` — including from a continuation that
    /// inherits the same worktree — return the same `Arc<RwLock<...>>`, so a
    /// continuation chain on one worktree shares one handle table
    /// (REQ-BASH-WS-001). Returns a `Result` for shape-parity with
    /// [`Self::browser`] — `get_or_create` is currently infallible, but the
    /// surface accepts future failure modes (e.g. registry resource
    /// exhaustion) without reshaping every callsite.
    ///
    /// REQ-BASH-014: Stateless Tool with Per-`WorkScope` Handle Registry.
    ///
    /// # Errors
    /// Returns [`BashHandleError`] to keep shape-parity with [`Self::browser`].
    /// `get_or_create` is currently infallible, so this presently always
    /// returns `Ok`.
    pub async fn bash_handles(&self) -> Result<Arc<RwLock<BashWorkScopeHandles>>, BashHandleError> {
        Ok(self.bash_handles.get_or_create(&self.work_scope).await)
    }

    /// Direct access to the registry (used by the hard-delete cascade
    /// integration and by the shutdown kill-tree pass).
    #[must_use]
    pub fn bash_handle_registry(&self) -> &Arc<BashHandleRegistry> {
        &self.bash_handles
    }

    /// Get the LLM selector
    #[must_use]
    pub fn llm_selector(&self) -> &Arc<dyn LlmSelector> {
        &self.llm_selector
    }

    /// Resolve the conversation's tmux server, lazily spawning it on
    /// first use and reusing it on subsequent calls. Returns the same
    /// `Arc<RwLock<TmuxServer>>` across repeated calls in one
    /// conversation; concurrent first-use callers share a single spawn
    /// (the registry's per-conversation write lock serialises them).
    ///
    /// REQ-TMUX-013.
    ///
    /// # Errors
    /// - [`TmuxError::BinaryUnavailable`] when `which("tmux")` failed at
    ///   registry init.
    /// - Other variants when the probe / spawn / mkdir steps fail.
    pub async fn tmux(&self) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
        // `working_dir` is the conversation's CWD — used by tmux's
        // `new-session -c` when a fresh server is spawned so the pane
        // shell starts in the conversation's project. Ignored on
        // re-attach (see `ensure_live` doc).
        //
        // `worktree_path` controls socket keying: worktree-scoped for
        // Work/Branch/Explore, conv-scoped for Direct (task 03001). The
        // ToolContext computes `work_scope` from this at construction time;
        // tmux ownership is keyed off the scope rather than re-deciding
        // the worktree-vs-conversation fallback at each callsite.
        self.tmux_registry
            .ensure_live(&self.work_scope, &self.working_dir)
            .await
    }

    /// Direct access to the registry (used by the hard-delete cascade
    /// integration in task 02696 and by the terminal attach path).
    #[allow(dead_code)] // Wired up in task 02696.
    #[must_use]
    pub fn tmux_registry(&self) -> &Arc<TmuxRegistry> {
        &self.tmux_registry
    }
}

/// Trait for tools that can be executed by the agent
///
/// REQ-BASH-010, REQ-BT-012: Tools are stateless - all context via `ToolContext`
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description for LLM (phoenix-native).
    fn description(&self) -> String;

    /// Tool description in a non-default language. The default impl falls
    /// back to `description()` (phoenix-native). Tools that have curated
    /// shorter prose for an alternate language override this to switch on
    /// `language`. See `phoenix_core::llm_language::tool_description_override` for
    /// the centralized override table.
    fn description_for_language(
        &self,
        language: phoenix_core::llm_language::LlmLanguage,
    ) -> String {
        phoenix_core::llm_language::tool_description_override(self.name(), language)
            .map_or_else(|| self.description(), str::to_string)
    }

    /// JSON schema for tool input
    fn input_schema(&self) -> Value;

    /// Whether this tool's full definition should be deferred (lazy-loaded on demand).
    /// Deferred tools send only name + description to the LLM initially, reducing
    /// prompt size when there are many tools. Override to `true` for rarely-used
    /// built-in tools (REQ-AUQ-008).
    fn defer_loading(&self) -> bool {
        false
    }

    /// Whether a stale result from this tool may be cleared from the model's
    /// view once a conversation approaches the context window (REQ-STR-002).
    ///
    /// True only when the tool *reads re-queryable state*: the agent can
    /// re-invoke it to re-obtain what it needs about the current state, so
    /// dropping an old result loses only a stale snapshot the agent has
    /// already acted on, not irreplaceable information. The test is not
    /// byte-reproducibility — in a mutated workspace a re-read yields current
    /// content, not the old snapshot — but re-queryability.
    ///
    /// Defaults to `false` so a newly added tool is never silently cleared. A
    /// tool whose result is the sole record of something not re-queryable (a
    /// human answer, a state-changing effect) must leave this false.
    fn clearable(&self) -> bool {
        false
    }

    /// Execute the tool with all context provided via `ToolContext`
    ///
    /// Tools that spawn long-running subprocesses should monitor
    /// ctx.cancel and terminate gracefully when cancelled.
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput;
}

/// Collection of tools available to conversations
///
/// Stateless - tools are singletons, all per-call context via `ToolContext`
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

// =============================================================================
// Named base tool sets — composed by the registry constructors below.
//
// Rationale: before this refactor the ToolRegistry constructors each assembled
// their own Vec of tools. `read_file` was present in `explore_no_sandbox()` and
// `for_subagent_explore()` but absent from `new_with_options()`, which powers
// both Direct and Work modes. The drift was only catchable at runtime via
// "Unknown tool: read_file" from the LLM.
//
// The sets here are the single source of truth. Each mode-specific constructor
// is a straight-line composition of these sets, so adding a new read-only tool
// happens in exactly one place and every mode picks it up. Drift is caught by
// `registry_mode_matrix` in the tests module.
// =============================================================================

/// Read-only information tools available in every mode.
/// Reading files, searching, thinking, reading images — nothing that mutates
/// on-disk or remote state.
fn read_only_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ThinkTool),
        Arc::new(ReadFileTool),
        Arc::new(SearchTool),
        Arc::new(KeywordSearchTool),
        Arc::new(ReadImageTool),
    ]
}

/// Shell and file-mutating tools.
/// Present in Direct, Work, sandboxed Explore, and Work sub-agents. Absent
/// from Explore-no-sandbox and Explore sub-agents (which only read).
///
/// `TmuxTool` is registered alongside bash because it serves the same
/// "run a command in this conversation" purpose with a complementary
/// persistence model (REQ-TMUX-003 / REQ-TMUX-009). When the tmux
/// binary is unavailable the tool's first invocation returns
/// `tmux_binary_unavailable` rather than failing at registration.
fn write_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BashTool),
        Arc::new(PatchTool::default()),
        Arc::new(TmuxRunTool),
        Arc::new(TmuxTool),
    ]
}

/// Headless-browser tools. Available in every conversation mode.
fn browser_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BrowserNavigateTool),
        Arc::new(BrowserEvalTool),
        Arc::new(BrowserTakeScreenshotTool),
        Arc::new(BrowserRecentConsoleLogsTool),
        Arc::new(BrowserClearConsoleLogsTool),
        Arc::new(BrowserResizeTool),
        Arc::new(BrowserWaitForSelectorTool),
        Arc::new(BrowserClickTool),
        Arc::new(BrowserTypeTool),
        Arc::new(BrowserKeyPressTool),
        Arc::new(BrowserProfileTool),
    ]
}

/// Coordination tools only available to parent conversations — sub-agents are
/// not allowed to spawn more sub-agents, ask the user, or invoke skills
/// (REQ-PROJ-008, REQ-AUQ-006).
///
/// `agents` is the working-directory's discovered named-agent catalog (sorted
/// by name), captured into `SpawnAgentsTool` so its `agent_type` enum reflects
/// what's available (REQ-AG-004). Empty when none are discovered.
fn parent_coordination_tools(agents: Vec<phoenix_agents::AgentDefinition>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SpawnAgentsTool::with_agents(agents)),
        Arc::new(AskUserQuestionTool),
        Arc::new(SkillTool),
    ]
}

fn explore_coordination_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(AskUserQuestionTool), Arc::new(SkillTool)]
}

/// Sub-agent terminal tools — how a sub-agent reports its result or error
/// back to the parent. Only available to sub-agents.
fn sub_agent_terminal_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(SubmitResultTool), Arc::new(SubmitErrorTool)]
}

/// Terminal-integration tools present only in parent Direct/Work modes
/// (sub-agents don't own a PTY).
///
/// Historical note: until `99c5df1` these were the single `ReadTerminalTool`
/// (which returned the tail of the xterm buffer). That was replaced by a
/// two-tool command-record model backed by OSC 133 shell-integration
/// markers. Both tools live here because they share the same scope:
/// read-only access to the parent conversation's PTY.
fn parent_terminal_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TerminalLastCommandTool),
        Arc::new(TerminalCommandHistoryTool),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExploreToolPolicy {
    bash: ExploreBashCapability,
}

impl ExploreToolPolicy {
    #[must_use]
    pub fn from_platform(platform: &PlatformCapability) -> Self {
        Self {
            bash: if platform.has_sandbox() {
                ExploreBashCapability::Sandboxed
            } else {
                ExploreBashCapability::Unavailable
            },
        }
    }

    #[must_use]
    pub const fn bash(self) -> ExploreBashCapability {
        self.bash
    }

    #[must_use]
    pub const fn has_sandboxed_bash(self) -> bool {
        matches!(self.bash, ExploreBashCapability::Sandboxed)
    }

    #[must_use]
    pub const fn allow_top_level_spawn_agents(self) -> bool {
        self.has_sandboxed_bash()
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn explore(
        tasks_dir_name: &str,
        agents: Vec<phoenix_agents::AgentDefinition>,
        policy: ExploreToolPolicy,
    ) -> Self {
        match policy.bash() {
            ExploreBashCapability::Sandboxed => {
                Self::explore_with_sandbox(tasks_dir_name, agents, policy)
            }
            ExploreBashCapability::Unavailable => Self::explore_no_sandbox(tasks_dir_name, agents),
        }
    }

    /// Minimal read-only registry for the global Coordinator. The host supplies
    /// the fleet/history tools so their global scope cannot be model-selected.
    /// Filesystem, browser, shell, MCP, and lifecycle tools are absent.
    #[must_use]
    pub fn coordinator(mut global_read_tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ThinkTool)];
        tools.append(&mut global_read_tools);
        Self { tools }
    }

    /// Create tool registry for Explore mode WITHOUT sandbox.
    ///
    /// REQ-PROJ-002, REQ-PROJ-013: Restricted tool set — no bash, no
    /// general patch. A scoped `patch` is included so the agent can draft
    /// task files under the project's tasks dir before calling
    /// `propose_task`; writes outside that dir are rejected at runtime.
    #[must_use]
    pub fn explore_no_sandbox(
        tasks_dir_name: &str,
        agents: Vec<phoenix_agents::AgentDefinition>,
    ) -> Self {
        let mut tools = read_only_tools();
        tools.extend(browser_tools());
        tools.extend(parent_coordination_tools(agents));
        tools.push(Arc::new(PatchTool::for_task_proposal_drafts(
            tasks_dir_name,
        )));
        tools.push(Arc::new(ProposeTaskTool));
        Self { tools }
    }

    /// Create tool registry for Explore mode WITH sandbox.
    /// REQ-PROJ-013: read-only/planning tools plus browser tools and
    /// OS-sandboxed bash.
    #[must_use]
    pub fn explore_with_sandbox(
        tasks_dir_name: &str,
        agents: Vec<phoenix_agents::AgentDefinition>,
        policy: ExploreToolPolicy,
    ) -> Self {
        debug_assert!(policy.has_sandboxed_bash());
        let mut tools = read_only_tools();
        tools.extend(browser_tools());
        tools.push(Arc::new(SandboxedBashTool));
        if policy.allow_top_level_spawn_agents() {
            tools.extend(parent_coordination_tools(agents));
        } else {
            tools.extend(explore_coordination_tools());
        }
        tools.push(Arc::new(PatchTool::for_task_proposal_drafts(
            tasks_dir_name,
        )));
        tools.push(Arc::new(ProposeTaskTool));
        Self { tools }
    }

    /// Create standard tool registry (parent conversations — legacy, will be removed)
    #[cfg(test)] // Only used in tests now; production uses mode-aware constructors
    #[must_use]
    pub fn standard() -> Self {
        Self::new_with_options(false, Vec::new())
    }

    /// Create tool registry for Direct mode.
    /// Full tool suite -- same as Work mode.
    #[must_use]
    pub fn direct(agents: Vec<phoenix_agents::AgentDefinition>) -> Self {
        Self::new_with_options(false, agents)
    }

    /// Add `propose_task` to a writing-mode registry (Work, Branch, or
    /// Direct-in-a-git-repo), where it serves as the non-blocking fork
    /// proposal (REQ-PROJ-033/036). Unlike Explore, the writing modes keep
    /// their full unrestricted `patch`; `propose_task` is added on top.
    #[must_use]
    pub fn with_propose_task(mut self) -> Self {
        self.tools.push(Arc::new(ProposeTaskTool));
        self
    }

    /// Add `commission_review` where Phoenix can infer a git-backed review target
    /// and gate execution through parent approval.
    #[must_use]
    pub fn with_commission_review(mut self) -> Self {
        self.tools.push(Arc::new(CommissionReviewTool));
        self
    }

    /// Tool registry for Explore-mode sub-agents using the OS-enforced
    /// read-only bash sandbox.
    #[must_use]
    pub fn for_subagent_explore_with_sandbox() -> Self {
        let mut tools = read_only_tools();
        tools.push(Arc::new(SandboxedBashTool));
        tools.extend(browser_tools());
        tools.extend(sub_agent_terminal_tools());
        Self { tools }
    }

    #[must_use]
    pub fn for_subagent_explore(policy: ExploreToolPolicy) -> Self {
        match policy.bash() {
            ExploreBashCapability::Sandboxed => Self::for_subagent_explore_with_sandbox(),
            ExploreBashCapability::Unavailable => Self::for_subagent_explore_no_sandbox(),
        }
    }

    /// Tool registry for Explore-mode sub-agents without sandbox support.
    #[must_use]
    pub fn for_subagent_explore_no_sandbox() -> Self {
        let mut tools = read_only_tools();
        tools.extend(browser_tools());
        tools.extend(sub_agent_terminal_tools());
        Self { tools }
    }

    /// Tool registry for Work-mode sub-agents (REQ-PROJ-008).
    /// Everything Explore has PLUS bash and patch. No spawn, no `ask_user`, no
    /// skill, no `propose_task`. Work mode is a writing mode, so unrestricted
    /// bash is expected here (unlike read-only Explore).
    #[must_use]
    pub fn for_subagent_work() -> Self {
        let mut tools = read_only_tools();
        tools.push(Arc::new(BashTool));
        tools.extend(browser_tools());
        tools.extend(sub_agent_terminal_tools());
        tools.push(Arc::new(PatchTool::default()));
        Self { tools }
    }

    /// Create tool registry for sub-agents (different tool set)
    #[deprecated(note = "Use for_subagent_explore() or for_subagent_work() instead")]
    #[must_use]
    pub fn for_subagent() -> Self {
        Self::for_subagent_explore_no_sandbox()
    }

    /// Create tool registry with options.
    ///
    /// `agents` feeds the parent `spawn_agents` tool's `agent_type` enum; it is
    /// unused for sub-agents (which cannot spawn).
    fn new_with_options(is_sub_agent: bool, agents: Vec<phoenix_agents::AgentDefinition>) -> Self {
        let mut tools = read_only_tools();
        tools.extend(write_tools());
        tools.extend(browser_tools());

        if is_sub_agent {
            // Sub-agents get completion tools, no spawning, no ask_user_question (REQ-AUQ-006)
            tools.extend(sub_agent_terminal_tools());
        } else {
            // Parent conversations can read the terminal, spawn sub-agents,
            // ask user questions, and invoke skills.
            tools.extend(parent_terminal_tools());
            tools.extend(parent_coordination_tools(agents));
        }

        Self { tools }
    }

    /// Get all tool definitions for LLM (phoenix-native).
    #[allow(dead_code)] // Convenience over `definitions_for_language(..::default())`.
    #[must_use]
    pub fn definitions(&self) -> Vec<phoenix_core::domain::llm_types::ToolDefinition> {
        self.definitions_for_language(phoenix_core::llm_language::LlmLanguage::default())
    }

    /// Get all tool definitions for LLM in the requested language.
    /// Tools without a translated description fall back to phoenix-native.
    #[must_use]
    pub fn definitions_for_language(
        &self,
        language: phoenix_core::llm_language::LlmLanguage,
    ) -> Vec<phoenix_core::domain::llm_types::ToolDefinition> {
        self.tools
            .iter()
            .map(|t| phoenix_core::domain::llm_types::ToolDefinition {
                name: t.name().to_string(),
                description: t.description_for_language(language),
                input_schema: t.input_schema(),
                defer_loading: t.defer_loading(),
            })
            .collect()
    }

    /// Return an error for a tool that is not available in the current mode.
    /// REQ-BED-017: Clear, actionable error when tools are unavailable due to mode.
    #[allow(dead_code)]
    #[must_use]
    pub fn blocked_tool_error(tool_name: &str) -> ToolOutput {
        ToolOutput::error(format!(
            "The '{tool_name}' tool is not available in Explore mode. \
             Use propose_task to propose work that requires write access."
        ))
    }

    /// Find a tool by name, returning a cloned `Arc` so callers can use it
    /// after releasing any lock on the registry.
    ///
    /// This and `Tool::run` are lower-level primitives. The conversation
    /// runtime reaches tool execution only through the gated
    /// `ToolExecutor::execute` (specs/permissions/), which consumes a
    /// `CheckedToolCall`; there is intentionally no name+input execute helper
    /// on the registry that would offer an ungated shortcut.
    #[must_use]
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Names of tools whose stale results may be cleared from the model-bound
    /// history (`Tool::clearable()`). Derived from the live tool set so the set
    /// cannot drift from the actual capabilities
    /// (`specs/stale-tool-results`, REQ-STR-002).
    #[must_use]
    pub fn clearable_tool_names(&self) -> std::collections::HashSet<String> {
        self.tools
            .iter()
            .filter(|t| t.clearable())
            .map(|t| t.name().to_string())
            .collect()
    }
}

// Legacy constructors — kept for any downstream callers during migration.
// No call sites remain in production code; remove once confirmed dead.
#[allow(dead_code, deprecated)]
impl ToolRegistry {
    /// Legacy constructor - use mode-aware constructors instead
    #[deprecated(note = "Use ToolRegistry::explore_*() or standard() instead")]
    pub fn new(_working_dir: PathBuf, _llm_selector: Arc<dyn LlmSelector>) -> Self {
        Self::new_with_options(false, Vec::new())
    }

    /// Legacy constructor - use `for_subagent()` instead
    #[deprecated(note = "Use ToolRegistry::for_subagent() instead")]
    pub fn new_for_subagent(_working_dir: PathBuf, _llm_selector: Arc<dyn LlmSelector>) -> Self {
        Self::for_subagent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The whole point of `ToolOutput` being an enum: `::success` carries any
    /// string — including error-shaped text — and is still structurally a
    /// success. There is no field to flip and no constructor that yields a
    /// `Success` reporting `is_success() == false`. The contradictory state
    /// the old `success: bool` + `output: String` pair allowed (P1 incident
    /// 08545) is unrepresentable.
    #[test]
    fn success_constructor_is_always_a_success() {
        let out = ToolOutput::success("Error: command failed with exit code 1");
        assert!(out.is_success());
        assert!(matches!(out, ToolOutput::Success { .. }));
    }

    #[test]
    fn error_constructor_is_always_an_error() {
        let out = ToolOutput::error("ok, completed successfully");
        assert!(!out.is_success());
        assert!(matches!(out, ToolOutput::Error { .. }));
    }

    #[test]
    fn builders_preserve_the_variant() {
        let img = || ToolImage {
            media_type: "image/png".to_string(),
            data: "Zm9v".to_string(),
        };

        let s = ToolOutput::success("done")
            .with_display(serde_json::json!({ "k": "v" }))
            .with_images(vec![img()]);
        assert!(s.is_success());
        assert_eq!(s.output(), "done");
        assert_eq!(s.display_data(), Some(&serde_json::json!({ "k": "v" })));
        assert_eq!(s.images().len(), 1);

        let e = ToolOutput::error("boom")
            .with_display(serde_json::json!({ "k": "v" }))
            .with_images(vec![img()]);
        assert!(!e.is_success());
        assert_eq!(e.output(), "boom");
        assert!(matches!(e, ToolOutput::Error { .. }));
    }

    fn names(registry: &ToolRegistry) -> BTreeSet<String> {
        registry
            .definitions()
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }

    fn sandbox_policy() -> ExploreToolPolicy {
        ExploreToolPolicy {
            bash: ExploreBashCapability::Sandboxed,
        }
    }

    fn no_sandbox_policy() -> ExploreToolPolicy {
        ExploreToolPolicy {
            bash: ExploreBashCapability::Unavailable,
        }
    }

    #[test]
    fn test_browser_tools_registered() {
        let names = names(&ToolRegistry::standard());
        for expected in [
            "browser_navigate",
            "browser_eval",
            "browser_take_screenshot",
            "browser_recent_console_logs",
            "browser_clear_console_logs",
            "browser_resize",
            "browser_profile",
        ] {
            assert!(names.contains(expected), "Missing {expected}");
        }
    }

    /// Read-only tools (`read_file`, `search`, `keyword_search`, `read_image`,
    /// `think`) must be present in every registry. Drift here caused the
    /// original "Unknown tool: `read_file`" infinite loop in Direct mode — the
    /// mock provider emitted a `read_file` call that the registry didn't
    /// recognise, which fed back into the LLM unbounded.
    ///
    /// This test is the guardrail. Adding a new read-only tool to
    /// `read_only_tools()` in tools.rs will automatically propagate it to
    /// every mode and keep this test passing; forgetting to add it to a
    /// specific constructor will fail this test.
    #[test]
    fn registry_mode_matrix_read_only_tools_everywhere() {
        let read_only_expected: BTreeSet<&str> = [
            "think",
            "read_file",
            "search",
            "keyword_search",
            "read_image",
        ]
        .into_iter()
        .collect();

        let registries: Vec<(&str, ToolRegistry)> = vec![
            ("direct", ToolRegistry::direct(Vec::new())),
            (
                "explore_no_sandbox",
                ToolRegistry::explore("tasks", Vec::new(), no_sandbox_policy()),
            ),
            (
                "explore_with_sandbox",
                ToolRegistry::explore("tasks", Vec::new(), sandbox_policy()),
            ),
            (
                "subagent_explore",
                ToolRegistry::for_subagent_explore(sandbox_policy()),
            ),
            ("subagent_work", ToolRegistry::for_subagent_work()),
        ];

        for (label, registry) in &registries {
            let present = names(registry);
            for tool in &read_only_expected {
                assert!(
                    present.contains(*tool),
                    "{label} registry is missing read-only tool `{tool}`"
                );
            }
        }
    }

    /// Per-mode capability matrix. If a constructor starts handing out the
    /// wrong capability set — e.g. giving sub-agents `spawn_agents`, or
    /// Explore-no-sandbox a `bash` — this test fails loudly instead of
    /// surfacing as a runtime transition error.
    ///
    /// Note on terminal tools: `terminal_last_command` and
    /// `terminal_command_history` replaced the older single
    /// `read_terminal` tool (commit `99c5df1`). They're the parent-mode
    /// terminal capability now and must only appear in Direct/Work —
    /// never in Explore (sandboxed or not) or in sub-agents.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn registry_mode_matrix_capability_boundaries() {
        const PARENT_TERMINAL_TOOLS: &[&str] =
            &["terminal_last_command", "terminal_command_history"];

        // Direct: full suite, no propose_task, no sub-agent submission tools.
        let direct = names(&ToolRegistry::direct(Vec::new()));
        assert!(direct.contains("bash"));
        assert!(direct.contains("patch"));
        assert!(direct.contains("tmux_run"));
        assert!(direct.contains("tmux"));
        assert!(!direct.contains("commission_review"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(direct.contains(*tool), "Direct missing {tool}");
        }
        assert!(direct.contains("spawn_agents"));
        assert!(direct.contains("ask_user_question"));
        assert!(!direct.contains("propose_task"));
        assert!(!direct.contains("submit_result"));
        assert!(!direct.contains("submit_error"));

        // Direct + with_propose_task: the writing-mode fork seam (Work/Branch
        // always, Direct-in-a-git-repo conditionally — REQ-PROJ-036). Adds
        // propose_task on top of the full suite; the base `direct()` stays
        // propose_task-free above.
        let direct_fork = names(
            &ToolRegistry::direct(Vec::new())
                .with_propose_task()
                .with_commission_review(),
        );
        assert!(direct_fork.contains("propose_task"));
        assert!(direct_fork.contains("commission_review"));
        assert!(direct_fork.contains("bash"));
        assert!(direct_fork.contains("patch"));

        // Explore (sandbox): read-only/planning tools + sandboxed bash.
        let work = names(&ToolRegistry::explore(
            "tasks",
            Vec::new(),
            sandbox_policy(),
        ));
        assert!(work.contains("bash"));
        assert!(work.contains("browser_wait_for_selector"));
        assert!(work.contains("patch"));
        assert!(!work.contains("tmux_run"));
        assert!(!work.contains("tmux"));
        assert!(work.contains("propose_task"));
        assert!(!work.contains("commission_review"));
        assert!(work.contains("spawn_agents"));
        assert!(work.contains("ask_user_question"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !work.contains(*tool),
                "Explore sandbox should not have {tool}"
            );
        }

        // Explore (no sandbox): read-only + propose_task + scoped patch
        // (limited to the configured tasks dir at runtime; `"tasks"` here
        // is the fixture name). No bash, no tmux, no terminal — the agent
        // only sees what's in the repo here.
        let explore = names(&ToolRegistry::explore(
            "tasks",
            Vec::new(),
            no_sandbox_policy(),
        ));
        assert!(explore.contains("propose_task"));
        assert!(explore.contains("ask_user_question"));
        assert!(explore.contains("browser_wait_for_selector"));
        assert!(explore.contains("spawn_agents"));
        assert!(explore.contains("patch"));
        assert!(!explore.contains("bash"));
        assert!(!explore.contains("tmux_run"));
        assert!(!explore.contains("tmux"));
        assert!(!explore.contains("commission_review"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !explore.contains(*tool),
                "Explore-no-sandbox should not have {tool}"
            );
        }

        // Sub-agent Explore with sandbox: read-only + sandboxed bash + submit.
        // no ask_user, no propose_task, no parent-terminal tools.
        let sub_explore = names(&ToolRegistry::for_subagent_explore(sandbox_policy()));
        assert!(sub_explore.contains("bash"));
        assert!(sub_explore.contains("browser_wait_for_selector"));
        assert!(
            !sub_explore.contains("tmux_run"),
            "sub-agent explore must not have tmux_run (task 94001)"
        );
        assert!(
            !sub_explore.contains("tmux"),
            "sub-agent explore must not have tmux (task 03001)"
        );
        assert!(sub_explore.contains("submit_result"));
        assert!(sub_explore.contains("submit_error"));
        assert!(!sub_explore.contains("patch"));
        assert!(!sub_explore.contains("spawn_agents"));
        assert!(!sub_explore.contains("ask_user_question"));
        assert!(!sub_explore.contains("propose_task"));
        assert!(!sub_explore.contains("commission_review"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !sub_explore.contains(*tool),
                "Sub-agent should not have parent terminal tool {tool}"
            );
        }

        let sub_explore_no_sandbox =
            names(&ToolRegistry::for_subagent_explore(no_sandbox_policy()));
        assert!(!sub_explore_no_sandbox.contains("bash"));
        assert!(sub_explore_no_sandbox.contains("submit_result"));

        // Sub-agent Work: Explore + patch.
        let sub_work = names(&ToolRegistry::for_subagent_work());
        assert!(sub_work.contains("bash"));
        assert!(sub_work.contains("patch"));
        assert!(
            !sub_work.contains("tmux_run"),
            "sub-agent work must not have tmux_run (task 94001)"
        );
        assert!(
            !sub_work.contains("tmux"),
            "sub-agent work must not have tmux (task 03001)"
        );
        assert!(sub_work.contains("submit_result"));
        assert!(!sub_work.contains("spawn_agents"));
        assert!(!sub_work.contains("propose_task"));
        assert!(!sub_work.contains("commission_review"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !sub_work.contains(*tool),
                "Sub-agent should not have parent terminal tool {tool}"
            );
        }
    }

    /// REQ-BASH-010: the bash tool's input schema must reach every registry
    /// that exposes bash, including sub-agent registries, with the `op`
    /// discriminator + per-op value fields. Anthropic's tool-use API
    /// rejects top-level `oneOf` / `allOf` / `anyOf` in `input_schema`, so
    /// per-operation field requirements are validated at runtime; the
    /// schema-level surface stays clean with `op` as the only required
    /// field.
    #[test]
    fn bash_input_schema_flows_through_to_subagent_registries() {
        for (label, registry) in [
            ("direct", ToolRegistry::direct(Vec::new())),
            (
                "subagent_explore",
                ToolRegistry::for_subagent_explore(sandbox_policy()),
            ),
            ("subagent_work", ToolRegistry::for_subagent_work()),
        ] {
            let bash = registry
                .find_tool("bash")
                .unwrap_or_else(|| panic!("{label} registry missing bash"));
            let schema = bash.input_schema();

            // The Anthropic API rejects top-level oneOf/allOf/anyOf.
            assert!(
                schema.get("oneOf").is_none(),
                "{label}: bash schema must not use top-level oneOf (Anthropic API rejects)"
            );
            assert!(
                schema.get("allOf").is_none(),
                "{label}: bash schema must not use top-level allOf"
            );
            assert!(
                schema.get("anyOf").is_none(),
                "{label}: bash schema must not use top-level anyOf"
            );

            let description = schema
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_else(|| panic!("{label}: bash schema missing description"));
            assert!(
                description.contains("op"),
                "{label}: bash schema description should reference the `op` discriminator"
            );

            let properties = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .unwrap_or_else(|| panic!("{label}: bash schema missing properties"));
            // Discriminator + per-op value fields.
            for key in &["op", "cmd", "handle"] {
                assert!(
                    properties.contains_key(*key),
                    "{label}: bash schema missing field `{key}`"
                );
            }
            assert!(properties.contains_key("wait_seconds"));
            assert!(properties.contains_key("signal"));
            assert!(properties.contains_key("lines"));
            assert!(properties.contains_key("since"));

            // `op` is the only schema-required field.
            let required = schema
                .get("required")
                .and_then(|r| r.as_array())
                .unwrap_or_else(|| panic!("{label}: bash schema missing `required`"));
            let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
            assert_eq!(
                names,
                vec!["op"],
                "{label}: only `op` should be schema-required"
            );

            // Deprecated `mode` is dropped from the schema (parser still
            // tolerates it for in-flight history).
            assert!(
                !properties.contains_key("mode"),
                "{label}: deprecated `mode` should no longer appear in the schema"
            );
        }
    }
}
