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
        "Select the intentional answer for accepted explicit Slack requests in this conversation. Queues one source-thread reply; does not send synchronously. Supply the exact request IDs answered by this text, including original and follow-up when applicable. Never include private transcript dumps. Use native questions for clarifications; the gateway supplies a generic Phoenix handoff. Do not also post the automatic answer with Slack tools. Delivery state is shown in Phoenix.".into()
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
        let channel_submission: bool = match sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_requests r JOIN slack_gateway_bindings b USING(binding_id) WHERE b.product_id=?1 AND r.trigger_kind='inbox')")
            .bind(&self.product).fetch_one(self.db.pool()).await {
                Ok(value) => value,
                Err(error) => return ToolOutput::error(error.to_string()),
            };
        let text = if channel_submission {
            let Ok(Some(config)) = super::config(&self.db).await else {
                return ToolOutput::error("Submission reply requires a configured Phoenix URL");
            };
            channel_answer(&input.text, &config.phoenix_url, &self.product)
        } else {
            input.text
        };
        if text.chars().count() > 4500 {
            return ToolOutput::error("Shorten the answer to leave room for its Phoenix conversation link (4500 characters total).");
        }
        match self
            .db
            .queue_slack_reply(
                &self.product,
                &input.request_ids,
                &ctx.conversation_id,
                &source,
                &text,
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

fn channel_answer(text: &str, origin: &str, product: &str) -> String {
    let url = format!("{}/c/{product}", origin.trim_end_matches('/'));
    if text.contains(&url) {
        text.to_owned()
    } else {
        format!("{}\n\nContinue in Phoenix: {url}", text.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn submission_result_has_host_selected_phoenix_handoff() {
        // REQ-SG-020
        let answer = channel_answer("Brief findings", "https://phoenix.example/", "product");
        assert_eq!(
            answer,
            "Brief findings\n\nContinue in Phoenix: https://phoenix.example/c/product"
        );
        assert_eq!(
            channel_answer(&answer, "https://phoenix.example", "product"),
            answer
        );
    }
}
