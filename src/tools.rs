//! Tool implementations for Phoenix IDE
//!
//! REQ-BASH-010, REQ-BT-012: Stateless Tools with Context Injection

mod bash;
pub mod bash_check;
pub mod browser;
mod keyword_search;
pub mod mcp;
pub mod patch;
mod propose_plan;
mod read_file;
mod read_image;
mod search;
mod subagent;
mod think;

pub use bash::BashTool;
pub use browser::{
    BrowserClearConsoleLogsTool, BrowserClickTool, BrowserError, BrowserEvalTool,
    BrowserKeyPressTool, BrowserNavigateTool, BrowserRecentConsoleLogsTool, BrowserResizeTool,
    BrowserSessionManager, BrowserTakeScreenshotTool, BrowserTypeTool, BrowserWaitForSelectorTool,
};
pub use keyword_search::KeywordSearchTool;
pub use patch::PatchTool;
pub use propose_plan::ProposePlanTool;
pub use read_file::ReadFileTool;
pub use read_image::ReadImageTool;
pub use search::SearchTool;
pub use subagent::{SpawnAgentsTool, SubmitErrorTool, SubmitResultTool};
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
}

impl ToolContext {
    /// Create a new tool context
    pub fn new(
        cancel: CancellationToken,
        conversation_id: String,
        working_dir: PathBuf,
        browser_sessions: Arc<BrowserSessionManager>,
        llm_registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            cancel,
            conversation_id,
            working_dir,
            browser_sessions,
            llm_registry,
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

impl ToolRegistry {
    /// Create tool registry for Explore mode WITHOUT sandbox.
    /// REQ-PROJ-002, REQ-PROJ-013: Restricted tool set — no bash, no patch.
    pub fn explore_no_sandbox() -> Self {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ThinkTool),
            Arc::new(ReadFileTool),
            Arc::new(SearchTool),
            Arc::new(KeywordSearchTool),
            Arc::new(ReadImageTool),
            Arc::new(ProposePlanTool),
            Arc::new(SpawnAgentsTool),
            // Browser tools
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
        ];
        Self { tools }
    }

    /// Create tool registry for Explore mode WITH sandbox.
    /// REQ-PROJ-013: All tools available, bash sandboxed read-only.
    /// Adds `propose_plan` (Explore-only gateway to Work mode).
    pub fn explore_with_sandbox() -> Self {
        let mut registry = Self::new_with_options(false);
        registry.tools.push(Arc::new(ProposePlanTool));
        registry
    }

    /// Create standard tool registry (parent conversations — legacy, will be removed)
    #[cfg(test)] // Only used in tests now; production uses mode-aware constructors
    pub fn standard() -> Self {
        Self::new_with_options(false)
    }

    /// Create tool registry for Standalone mode (non-git directories).
    /// Full tool suite, same as standard/Work mode.
    pub fn standalone() -> Self {
        Self::new_with_options(false)
    }

    /// Create tool registry for sub-agents (different tool set)
    pub fn for_subagent() -> Self {
        Self::new_with_options(true)
    }

    /// Create tool registry with options
    fn new_with_options(is_sub_agent: bool) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ThinkTool),
            Arc::new(BashTool),
            Arc::new(PatchTool::default()),
            Arc::new(KeywordSearchTool),
            Arc::new(ReadImageTool),
            // Browser tools
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
        ];

        if is_sub_agent {
            // Sub-agents get completion tools, no spawning
            tools.push(Arc::new(SubmitResultTool));
            tools.push(Arc::new(SubmitErrorTool));
        } else {
            // Parent conversations can spawn sub-agents
            tools.push(Arc::new(SpawnAgentsTool));
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
                defer_loading: false,
            })
            .collect()
    }

    /// Return an error for a tool that is not available in the current mode.
    /// REQ-BED-017: Clear, actionable error when tools are unavailable due to mode.
    #[allow(dead_code)]
    pub fn blocked_tool_error(tool_name: &str) -> ToolOutput {
        ToolOutput::error(format!(
            "The '{tool_name}' tool is not available in Explore mode. \
             Use propose_plan to propose work that requires write access."
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

    #[test]
    fn test_browser_tools_registered() {
        let registry = ToolRegistry::standard();
        let defs = registry.definitions();
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();

        assert!(
            names.contains(&"browser_navigate"),
            "Missing browser_navigate"
        );
        assert!(names.contains(&"browser_eval"), "Missing browser_eval");
        assert!(
            names.contains(&"browser_take_screenshot"),
            "Missing browser_take_screenshot"
        );
        assert!(
            names.contains(&"browser_recent_console_logs"),
            "Missing browser_recent_console_logs"
        );
        assert!(
            names.contains(&"browser_clear_console_logs"),
            "Missing browser_clear_console_logs"
        );
        assert!(names.contains(&"browser_resize"), "Missing browser_resize");
        // browser_inject/remove_react_devtools removed — hook is auto-injected
        // by BrowserSession::launch_and_init()
    }
}
