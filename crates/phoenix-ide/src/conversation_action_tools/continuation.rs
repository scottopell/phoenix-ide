use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::continuation_service::{
    ContinuationApplicationService, NativeContinueCompletion, NativeContinueRequest,
};
use crate::runtime::RuntimeManager;
use phoenix_workflow::ClientTurnKey;

pub(super) struct ContinueConversation(pub(super) Arc<RuntimeManager>);

#[derive(Debug)]
struct NonBlankString(String);

impl<'de> Deserialize<'de> for NonBlankString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.trim().is_empty() {
            return Err(serde::de::Error::custom("must not be blank"));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueRequest {
    product_conversation_id: ProductConversationId,
    predecessor_transcript_id: NonBlankString,
    summary_message_id: NonBlankString,
}

impl ContinueConversation {
    async fn run_inner(&self, input: Value, ctx: ToolContext) -> Result<Value, String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let request: ContinueRequest =
            serde_json::from_value(input).map_err(|error| error.to_string())?;
        ctx.source_tool_call()
            .ok_or("Trusted source tool-call identity unavailable")?;
        let first_message_id = ClientTurnKey::new(uuid::Uuid::new_v4().to_string())
            .ok_or_else(|| "generated first-message identity is empty".to_string())?;
        let completion = ContinuationApplicationService::new(self.0.clone())
            .continue_from_coordinator(NativeContinueRequest {
                source_transcript_id: ctx.conversation_id,
                product_conversation_id: request.product_conversation_id,
                predecessor_transcript_id: request.predecessor_transcript_id.0,
                summary_message_id: request.summary_message_id.0,
                first_message_id,
                #[cfg(test)]
                source_gate_wait: None,
            })
            .await;
        match completion {
            NativeContinueCompletion::Established(result) => {
                let response = result?;
                serde_json::to_value(response).map_err(|error| error.to_string())
            }
            NativeContinueCompletion::LocalAuthorityClosed => {
                std::future::pending().await
            }
        }
    }
}

#[async_trait]
impl Tool for ContinueConversation {
    fn name(&self) -> &'static str {
        "continue_conversation"
    }

    fn description(&self) -> String {
        "Current Global Coordinator only. Continue the specified context-exhausted predecessor using its retained generated summary; the successor opening is system-generated, not a user commission. Supply the product conversation id, predecessor transcript id, and exact summary message id. No handoff text, first-message identity, or retry/recovery controls are accepted.".into()
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object", "additionalProperties":false,
            "required":["product_conversation_id", "predecessor_transcript_id", "summary_message_id"],
            "properties": {
                "product_conversation_id":{"type":"string", "minLength":1, "pattern":"\\S"},
                "predecessor_transcript_id":{"type":"string", "minLength":1, "pattern":"\\S"},
                "summary_message_id":{"type":"string", "minLength":1, "pattern":"\\S"}
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        if self.0.local_authority_is_closed() {
            std::future::pending().await
        }
        let result = self.run_inner(input, ctx).await;
        if self.0.local_authority_is_closed() {
            std::future::pending().await
        }
        super::output(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_accepts_only_the_three_narrow_identity_fields() {
        let valid = json!({
            "product_conversation_id":"pc_test",
            "predecessor_transcript_id":"predecessor",
            "summary_message_id":"summary"
        });
        assert!(serde_json::from_value::<ContinueRequest>(valid.clone()).is_ok());
        for key in [
            "handoff", "first_message_id", "user_message", "agent", "retry",
            "recovery", "control", "source_transcript_id", "tool_use_id",
        ] {
            let mut value = valid.clone();
            value[key] = json!("unexpected");
            assert!(serde_json::from_value::<ContinueRequest>(value).is_err(), "{key}");
        }
        for field in [
            "product_conversation_id", "predecessor_transcript_id", "summary_message_id",
        ] {
            let mut value = valid.clone();
            value.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<ContinueRequest>(value).is_err(), "{field}");
        }
        for field in ["product_conversation_id", "predecessor_transcript_id", "summary_message_id"] {
            for blank in ["", "  \t\n"] {
                let mut value = valid.clone();
                value[field] = json!(blank);
                assert!(serde_json::from_value::<ContinueRequest>(value).is_err(), "{field}");
            }
        }
    }
}
