//! Tool implementations for Phoenix IDE
//!
//! REQ-BASH-010, REQ-BT-012: Stateless Tools with Context Injection

mod ask_user_question;
mod bash;
pub mod bash_check;
pub mod browser;
mod keyword_search;
pub mod mcp;
pub mod patch;
mod propose_task;
mod read_file;
mod read_image;
mod search;
mod skill;
mod subagent;
mod terminal_command_history;
mod terminal_last_command;
mod think;

pub use ask_user_question::AskUserQuestionTool;
pub use bash::BashTool;
pub use browser::{
    BrowserClearConsoleLogsTool, BrowserClickTool, BrowserError, BrowserEvalTool,
    BrowserKeyPressTool, BrowserNavigateTool, BrowserRecentConsoleLogsTool, BrowserResizeTool,
    BrowserSessionManager, BrowserTakeScreenshotTool, BrowserTypeTool, BrowserWaitForSelectorTool,
};
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

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::llm::ModelRegistry;
pub use browser::session::BrowserSession;

/// Typed image data for LLM consumption.
#[derive(Debug, Clone)]
pub struct ToolImage {
    pub media_type: String,
    pub data: String, // base64-encoded
}

/// Result from tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    pub output: String,
    /// Typed images for LLM consumption (sent as image content blocks, not text).
    #[serde(skip)]
    pub images: Vec<ToolImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_data: Option<Value>,
}

impl ToolOutput {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            images: vec![],
            display_data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            output: message.into(),
            images: vec![],
            display_data: None,
        }
    }

    pub fn with_display(mut self, data: Value) -> Self {
        self.display_data = Some(data);
        self
    }

    /// Attach typed images for LLM consumption.
    pub fn with_images(mut self, images: Vec<ToolImage>) -> Self {
        self.images = images;
        self
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

    /// Working directory for file operations
    pub working_dir: PathBuf,

    /// Browser session manager (access via `browser()` method)
    browser_sessions: Arc<BrowserSessionManager>,

    /// LLM registry for tools that need model access
    llm_registry: Arc<ModelRegistry>,

    /// Active PTY terminal sessions — used by the terminal-command tools
    /// (`terminal_last_command`, `terminal_command_history`).
    pub terminals: crate::terminal::ActiveTerminals,
}

impl ToolContext {
    /// Create a new tool context
    pub fn new(
        cancel: CancellationToken,
        conversation_id: String,
        working_dir: PathBuf,
        browser_sessions: Arc<BrowserSessionManager>,
        llm_registry: Arc<ModelRegistry>,
        terminals: crate::terminal::ActiveTerminals,
    ) -> Self {
        Self {
            cancel,
            conversation_id,
            working_dir,
            browser_sessions,
            llm_registry,
            terminals,
        }
    }

    /// Get or create the browser session for this conversation.
    ///
    /// Lazily initializes Chrome on first call. Subsequent calls return
    /// the existing session. Conversation ID is derived internally.
    ///
    /// REQ-BT-010: Implicit Session Model
    pub async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        self.browser_sessions
            .get_session(&self.conversation_id)
            .await
    }

    /// Get the LLM registry
    pub fn llm_registry(&self) -> &Arc<ModelRegistry> {
        &self.llm_registry
    }
}

/// Trait for tools that can be executed by the agent
///
/// REQ-BASH-010, REQ-BT-012: Tools are stateless - all context via `ToolContext`
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description for LLM
    fn description(&self) -> String;

    /// JSON schema for tool input
    fn input_schema(&self) -> Value;

    /// Whether this tool's full definition should be deferred (lazy-loaded on demand).
    /// Deferred tools send only name + description to the LLM initially, reducing
    /// prompt size when there are many tools. Override to `true` for rarely-used
    /// built-in tools (REQ-AUQ-008).
    fn defer_loading(&self) -> bool {
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
fn write_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(BashTool), Arc::new(PatchTool::default())]
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
    ]
}

/// Coordination tools only available to parent conversations — sub-agents are
/// not allowed to spawn more sub-agents, ask the user, or invoke skills
/// (REQ-PROJ-008, REQ-AUQ-006).
fn parent_coordination_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SpawnAgentsTool),
        Arc::new(AskUserQuestionTool),
        Arc::new(SkillTool),
    ]
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

impl ToolRegistry {
    /// Create tool registry for Explore mode WITHOUT sandbox.
    /// REQ-PROJ-002, REQ-PROJ-013: Restricted tool set — no bash, no patch.
    pub fn explore_no_sandbox() -> Self {
        let mut tools = read_only_tools();
        tools.extend(browser_tools());
        tools.extend(parent_coordination_tools());
        tools.push(Arc::new(ProposeTaskTool));
        Self { tools }
    }

    /// Create tool registry for Explore mode WITH sandbox.
    /// REQ-PROJ-013: Full tool suite, bash sandboxed read-only at runtime.
    /// Adds `propose_task` (Explore-only gateway to Work mode).
    pub fn explore_with_sandbox() -> Self {
        let mut registry = Self::new_with_options(false);
        registry.tools.push(Arc::new(ProposeTaskTool));
        registry
    }

    /// Create standard tool registry (parent conversations — legacy, will be removed)
    #[cfg(test)] // Only used in tests now; production uses mode-aware constructors
    pub fn standard() -> Self {
        Self::new_with_options(false)
    }

    /// Create tool registry for Direct mode.
    /// Full tool suite -- same as Work mode.
    pub fn direct() -> Self {
        Self::new_with_options(false)
    }

    /// Tool registry for Explore-mode sub-agents (REQ-PROJ-008).
    /// Read-only tools + bash + `submit_result`/`submit_error`. No patch, no
    /// spawn, no `ask_user`, no skill, no `propose_task`.
    // TODO: read-only bash enforcement not yet implemented --
    // uses regular bash. See REQ-BASH-008 for the planned sandbox approach.
    pub fn for_subagent_explore() -> Self {
        let mut tools = read_only_tools();
        tools.push(Arc::new(BashTool));
        tools.extend(browser_tools());
        tools.extend(sub_agent_terminal_tools());
        Self { tools }
    }

    /// Tool registry for Work-mode sub-agents (REQ-PROJ-008).
    /// Everything Explore has PLUS patch. No spawn, no `ask_user`, no skill, no `propose_task`.
    pub fn for_subagent_work() -> Self {
        let mut registry = Self::for_subagent_explore();
        registry.tools.push(Arc::new(PatchTool::default()));
        registry
    }

    /// Create tool registry for sub-agents (different tool set)
    #[deprecated(note = "Use for_subagent_explore() or for_subagent_work() instead")]
    pub fn for_subagent() -> Self {
        Self::for_subagent_explore()
    }

    /// Create tool registry with options
    fn new_with_options(is_sub_agent: bool) -> Self {
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
            tools.extend(parent_coordination_tools());
        }

        Self { tools }
    }

    /// Get all tool definitions for LLM
    pub fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.tools
            .iter()
            .map(|t| crate::llm::ToolDefinition {
                name: t.name().to_string(),
                description: t.description(),
                input_schema: t.input_schema(),
                defer_loading: t.defer_loading(),
            })
            .collect()
    }

    /// Return an error for a tool that is not available in the current mode.
    /// REQ-BED-017: Clear, actionable error when tools are unavailable due to mode.
    #[allow(dead_code)]
    pub fn blocked_tool_error(tool_name: &str) -> ToolOutput {
        ToolOutput::error(format!(
            "The '{tool_name}' tool is not available in Explore mode. \
             Use propose_task to propose work that requires write access."
        ))
    }

    /// Find a tool by name, returning a cloned `Arc` so callers can use it
    /// after releasing any lock on the registry.
    pub fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    /// Execute a tool by name with context
    #[allow(dead_code)] // Used in tests; production goes through find_tool() + run()
    pub async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput> {
        for tool in &self.tools {
            if tool.name() == name {
                return Some(tool.run(input, ctx).await);
            }
        }
        None
    }
}

// Legacy constructors — kept for any downstream callers during migration.
// No call sites remain in production code; remove once confirmed dead.
#[allow(dead_code, deprecated)]
impl ToolRegistry {
    /// Legacy constructor - use mode-aware constructors instead
    #[deprecated(note = "Use ToolRegistry::explore_*() or standard() instead")]
    pub fn new(_working_dir: PathBuf, _llm_registry: Arc<ModelRegistry>) -> Self {
        Self::new_with_options(false)
    }

    /// Legacy constructor - use `for_subagent()` instead
    #[deprecated(note = "Use ToolRegistry::for_subagent() instead")]
    pub fn new_for_subagent(_working_dir: PathBuf, _llm_registry: Arc<ModelRegistry>) -> Self {
        Self::for_subagent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn names(registry: &ToolRegistry) -> BTreeSet<String> {
        registry
            .definitions()
            .iter()
            .map(|d| d.name.clone())
            .collect()
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
        ] {
            assert!(names.contains(expected), "Missing {expected}");
        }
    }

    /// Read-only tools (`read_file`, `search`, `keyword_search`, `read_image`,
    /// `think`) must be present in every registry. Drift here caused the
    /// original "Unknown tool: read_file" infinite loop in Direct mode — the
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
            ("direct", ToolRegistry::direct()),
            ("explore_no_sandbox", ToolRegistry::explore_no_sandbox()),
            ("explore_with_sandbox", ToolRegistry::explore_with_sandbox()),
            ("subagent_explore", ToolRegistry::for_subagent_explore()),
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
    fn registry_mode_matrix_capability_boundaries() {
        const PARENT_TERMINAL_TOOLS: &[&str] =
            &["terminal_last_command", "terminal_command_history"];

        // Direct: full suite, no propose_task, no sub-agent submission tools.
        let direct = names(&ToolRegistry::direct());
        assert!(direct.contains("bash"));
        assert!(direct.contains("patch"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(direct.contains(*tool), "Direct missing {tool}");
        }
        assert!(direct.contains("spawn_agents"));
        assert!(direct.contains("ask_user_question"));
        assert!(!direct.contains("propose_task"));
        assert!(!direct.contains("submit_result"));
        assert!(!direct.contains("submit_error"));

        // Explore (sandbox): full suite + propose_task.
        let work = names(&ToolRegistry::explore_with_sandbox());
        assert!(work.contains("bash"));
        assert!(work.contains("patch"));
        assert!(work.contains("propose_task"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(work.contains(*tool), "Work missing {tool}");
        }

        // Explore (no sandbox): read-only + propose_task, no bash/patch,
        // no terminal (the agent only sees what's in the repo here).
        let explore = names(&ToolRegistry::explore_no_sandbox());
        assert!(explore.contains("propose_task"));
        assert!(explore.contains("ask_user_question"));
        assert!(!explore.contains("bash"));
        assert!(!explore.contains("patch"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !explore.contains(*tool),
                "Explore-no-sandbox should not have {tool}"
            );
        }

        // Sub-agent Explore: read-only + bash + submit. No patch, no spawn,
        // no ask_user, no propose_task, no parent-terminal tools.
        let sub_explore = names(&ToolRegistry::for_subagent_explore());
        assert!(sub_explore.contains("bash"));
        assert!(sub_explore.contains("submit_result"));
        assert!(sub_explore.contains("submit_error"));
        assert!(!sub_explore.contains("patch"));
        assert!(!sub_explore.contains("spawn_agents"));
        assert!(!sub_explore.contains("ask_user_question"));
        assert!(!sub_explore.contains("propose_task"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !sub_explore.contains(*tool),
                "Sub-agent should not have parent terminal tool {tool}"
            );
        }

        // Sub-agent Work: Explore + patch.
        let sub_work = names(&ToolRegistry::for_subagent_work());
        assert!(sub_work.contains("bash"));
        assert!(sub_work.contains("patch"));
        assert!(sub_work.contains("submit_result"));
        assert!(!sub_work.contains("spawn_agents"));
        assert!(!sub_work.contains("propose_task"));
        for tool in PARENT_TERMINAL_TOOLS {
            assert!(
                !sub_work.contains(*tool),
                "Sub-agent should not have parent terminal tool {tool}"
            );
        }
    }
}
