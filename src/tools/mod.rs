//! Tool implementations for Phoenix IDE

mod bash;
pub mod patch;
mod think;
mod keyword_search;

pub use bash::BashTool;
pub use patch::PatchTool;
pub use think::ThinkTool;
pub use keyword_search::KeywordSearchTool;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::llm::ModelRegistry;

/// Result from tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_data: Option<Value>,
}

impl ToolOutput {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            display_data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            output: message.into(),
            display_data: None,
        }
    }

    pub fn with_display(mut self, data: Value) -> Self {
        self.display_data = Some(data);
        self
    }
}

/// Trait for tools that can be executed by the agent
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;
    
    /// Tool description for LLM
    fn description(&self) -> String;
    
    /// JSON schema for tool input
    fn input_schema(&self) -> Value;
    
    /// Execute the tool
    async fn run(&self, input: Value) -> ToolOutput;
}

/// Collection of tools available to a conversation
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create standard tool registry for a conversation
    pub fn new(working_dir: PathBuf, llm_registry: Arc<ModelRegistry>) -> Self {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ThinkTool),
            Arc::new(BashTool::new(working_dir.clone())),
            Arc::new(PatchTool::new(working_dir.clone())),
            Arc::new(KeywordSearchTool::new(working_dir, llm_registry)),
        ];
        Self { tools }
    }

    /// Create tool registry for sub-agents (limited tools)
    pub fn new_for_subagent(working_dir: PathBuf, llm_registry: Arc<ModelRegistry>) -> Self {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ThinkTool),
            Arc::new(BashTool::new(working_dir.clone())),
            Arc::new(PatchTool::new(working_dir.clone())),
            Arc::new(KeywordSearchTool::new(working_dir, llm_registry)),
            // Note: sub-agents would get submit_result tool here
        ];
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
            })
            .collect()
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: Value) -> Option<ToolOutput> {
        for tool in &self.tools {
            if tool.name() == name {
                return Some(tool.run(input).await);
            }
        }
        None
    }
}
