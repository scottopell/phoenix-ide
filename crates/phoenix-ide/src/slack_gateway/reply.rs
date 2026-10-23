//! Intentional request-bound publication, independent of transcript tail order.
use async_trait::async_trait;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn tool(db: crate::db::Database, product: String) -> Arc<dyn Tool> {
    Arc::new(Reply { db, product })
}
struct Reply {
    db: crate::db::Database,
    product: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyInput {
    request_ids: Vec<String>,
    text: String,
}
#[async_trait]
impl Tool for Reply {
    fn name(&self) -> &'static str {
        "slack_gateway_reply"
    }
    fn description(&self) -> String {
        "Select the intentional answer for accepted explicit Slack requests in this conversation. Queues one source-thread reply; does not send synchronously. Supply the exact request IDs answered by this text, including original and follow-up when applicable. Never include private transcript dumps. Do not also post the automatic answer with Slack tools. Delivery state is shown in Phoenix.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"request_ids":{"type":"array","items":{"type":"string"},"minItems":1,"uniqueItems":true},"text":{"type":"string","minLength":1,"maxLength":4500}},"required":["request_ids","text"],"additionalProperties":false})
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ReplyInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(format!("Invalid reply: {e}")),
        };
        if input.text.chars().count() > 4500 {
            return ToolOutput::error("Keep the Slack reply under 4500 characters; link the Phoenix conversation for details.");
        }
        let Some(source) = ctx.source_tool_call() else {
            return ToolOutput::error("Exact native tool source is unavailable");
        };
        let executing = match self.db.get_conversation(&ctx.conversation_id).await {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        if executing.product_conversation_id.as_str() != self.product {
            return ToolOutput::error("Reply tool belongs to a different conversation");
        }
        match self
            .db
            .queue_slack_reply(
                &self.product,
                &input.request_ids,
                &ctx.conversation_id,
                &source,
                &input.text,
            )
            .await
        {
            Ok(id) => ToolOutput::success(format!(
                "Reply durably queued: {id}. This is not delivery confirmation."
            )),
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }
}
