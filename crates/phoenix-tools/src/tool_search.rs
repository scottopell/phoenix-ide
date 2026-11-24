//! Local discovery over the permitted catalog frozen for a provider request.

use async_trait::async_trait;
use phoenix_core::domain::tool_discovery::{
    search_tools, tool_search_definition, tool_search_result_text, ToolSearchQuery,
    TOOL_SEARCH_NAME,
};
use serde_json::Value;

use super::{Tool, ToolContext, ToolOutput};

pub struct ToolSearch;

#[async_trait]
impl Tool for ToolSearch {
    fn name(&self) -> &'static str {
        TOOL_SEARCH_NAME
    }

    fn description(&self) -> String {
        tool_search_definition().description
    }

    fn input_schema(&self) -> Value {
        tool_search_definition().input_schema
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let query = match ToolSearchQuery::parse_input(&input) {
            Ok(query) => query,
            Err(error) => return ToolOutput::error(format!("Invalid tool search: {error}")),
        };
        let selected = search_tools(ctx.tool_search_catalog(), &query);
        ToolOutput::success(tool_search_result_text(&selected))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use phoenix_core::domain::llm_types::ToolDefinition;
    use phoenix_core::work_scope::{ResourceAuthority, ResourceScopeKey};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn context() -> ToolContext {
        ToolContext::new_without_filesystem(
            CancellationToken::new(),
            "tool-search-tests".into(),
            Arc::new(crate::BrowserSessionManager::default()),
            Arc::new(crate::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(crate::TmuxRegistry::new()),
            ResourceScopeKey::Unattached("tool-search-tests".into()),
            ResourceAuthority::Work,
        )
    }

    fn documents_tool() -> ToolDefinition {
        ToolDefinition {
            name: "mcp__documents__read_document".into(),
            description: "Read project documents.".into(),
            input_schema: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
            defer_loading: true,
        }
    }

    #[tokio::test]
    async fn discovery_uses_only_this_invocations_catalog_without_filesystem_access() {
        let tool = ToolSearch;
        let original = documents_tool();
        let frozen = context().with_tool_search_catalog(vec![original.clone()]);
        let selected = tool.run(json!({"query":"documents"}), frozen.clone()).await;
        assert!(selected.is_success());
        assert_eq!(
            selected.output(),
            tool_search_result_text(std::slice::from_ref(&original))
        );
        assert!(selected.output().contains(&original.name));
        assert!(selected.images().is_empty());

        let empty = tool.run(json!({"query":"documents"}), context()).await;
        assert!(empty.is_success());
        assert_eq!(empty.output(), tool_search_result_text(&[]));
        assert_ne!(empty.output(), selected.output());
        assert_eq!(frozen.tool_search_catalog().len(), 1);
    }

    #[tokio::test]
    async fn malformed_queries_are_errors_instead_of_discovering_tools() {
        let tool = ToolSearch;
        for input in [json!({}), json!({"query":""}), json!({"query":17})] {
            let result = tool
                .run(
                    input,
                    context().with_tool_search_catalog(vec![documents_tool()]),
                )
                .await;
            assert!(!result.is_success());
            assert!(result.output().contains("Invalid tool search"));
        }
    }

    #[test]
    fn search_is_eager_and_keeps_its_result_evidence() {
        let tool = ToolSearch;
        assert_eq!(tool.name(), TOOL_SEARCH_NAME);
        assert_eq!(tool.input_schema(), tool_search_definition().input_schema);
        assert!(!tool.defer_loading());
        assert!(!tool.clearable());
    }
}
