use crate::api::global_read::GlobalReadService;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::tools::{Tool, ToolContext, ToolOutput};

pub(crate) fn tools(service: GlobalReadService) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchConversations(service.clone())),
        Arc::new(ReadConversation(service.clone())),
        Arc::new(ListOpenWork(service.clone())),
        Arc::new(ResolveReference(service)),
    ]
}

struct SearchConversations(GlobalReadService);
struct ReadConversation(GlobalReadService);
struct ListOpenWork(GlobalReadService);
struct ResolveReference(GlobalReadService);

#[async_trait]
impl Tool for SearchConversations {
    fn name(&self) -> &'static str {
        "search_conversations"
    }
    fn description(&self) -> String {
        "Search all Phoenix conversation messages. Results include stable conversation/message references and app-local citation links.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let query = input.get("query").and_then(Value::as_str).unwrap_or("");
        result(self.0.search(query).await)
    }
}

#[async_trait]
impl Tool for ReadConversation {
    fn name(&self) -> &'static str {
        "read_conversation"
    }
    fn description(&self) -> String {
        "Read one source conversation transcript in bounded pages. Pass a conversation id, @conv reference, or app-local conversation link. Use cursor when the result says more content is available.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"conversation_id":{"type":"string"},"cursor":{"type":"integer","minimum":0}},"required":["conversation_id"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let conversation = input
            .get("conversation_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let cursor = input
            .get("cursor")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        result(self.0.read_conversation(conversation, cursor).await)
    }
}

#[async_trait]
impl Tool for ListOpenWork {
    fn name(&self) -> &'static str {
        "list_open_work"
    }
    fn description(&self) -> String {
        "Read the deterministic current fleet/open-work projection grouped by project, including stable references and explainable inclusion signals.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"offset":{"type":"integer","minimum":0}}})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        result(self.0.open_work_page(offset).await)
    }
}

#[async_trait]
impl Tool for ResolveReference {
    fn name(&self) -> &'static str {
        "resolve_reference"
    }
    fn description(&self) -> String {
        "Resolve @conv, @chain, @work, and app-local conversation/chain references to durable source metadata.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"reference":{"type":"string"}},"required":["reference"]})
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let reference = input.get("reference").and_then(Value::as_str).unwrap_or("");
        match self.0.resolve_reference(reference).await {
            Ok(value) => match serde_json::to_string_pretty(&value) {
                Ok(value) => ToolOutput::success(value),
                Err(error) => ToolOutput::error(format!("failed to encode reference: {error}")),
            },
            Err(error) => ToolOutput::error(format!("{error:?}")),
        }
    }
}

fn result(value: Result<String, String>) -> ToolOutput {
    match value {
        Ok(value) => ToolOutput::success(value),
        Err(error) => ToolOutput::error(error),
    }
}
