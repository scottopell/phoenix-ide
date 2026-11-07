use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_tools::{ToolContext, ToolOutput};
use phoenix_workflow::ClientTurnKey;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::continuation_service::{
    ContinuationApplicationService, NativeContinueCompletion, NativeContinueRequest,
};
use crate::coordinator_tools::instance::{Route, RoutedTool};
use crate::runtime::RuntimeManager;

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
#[allow(
    clippy::struct_field_names,
    reason = "identity field names are the exact deserialized tool-input contract"
)]
pub(super) struct ContinueRequest {
    product_conversation_id: ProductConversationId,
    predecessor_transcript_id: NonBlankString,
    summary_message_id: NonBlankString,
}

pub(crate) struct ContinueConversation(pub(crate) Arc<RuntimeManager>);

impl ContinueConversation {
    async fn run_remote(
        runtime: Arc<RuntimeManager>,
        request: ContinueRequest,
        peer: crate::api::federation::ResolvedPeer,
        ctx: ToolContext,
    ) -> Result<crate::api::ContinueConversationResponse, String> {
        let _operation_authority = runtime
            .acquire_local_authority_pass()
            .map_err(|()| "Local mutation authority is closed".to_string())?;
        let db = runtime.db();
        let source = db
            .get_conversation(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let _source_gate = runtime
            .lock_product_message_admission(source.product_conversation_id.as_str())
            .await;
        db.require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        let _dispatch_authority = runtime
            .acquire_local_authority_pass()
            .map_err(|()| "Local mutation authority is closed".to_string())?;
        crate::api::federation::remote_continue_conversation(
            db,
            &peer,
            request.product_conversation_id,
            request.predecessor_transcript_id.0,
            request.summary_message_id.0,
            &ctx,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn run_local(
        &self,
        request: ContinueRequest,
        ctx: &ToolContext,
    ) -> Result<crate::api::ContinueConversationResponse, String> {
        ctx.source_tool_call()
            .ok_or("Trusted source tool-call identity unavailable")?;
        let first_message_id = ClientTurnKey::new(uuid::Uuid::new_v4().to_string())
            .ok_or_else(|| "generated first-message identity is empty".to_string())?;
        let service = ContinuationApplicationService::new(self.0.clone());
        match service
            .continue_from_coordinator(NativeContinueRequest {
                source_transcript_id: ctx.conversation_id.clone(),
                product_conversation_id: request.product_conversation_id,
                predecessor_transcript_id: request.predecessor_transcript_id.0,
                summary_message_id: request.summary_message_id.0,
                first_message_id,
                #[cfg(test)]
                source_gate_wait: None,
            })
            .await
        {
            NativeContinueCompletion::Established(result) => {
                result.map_err(|error| error.to_string())
            }
            NativeContinueCompletion::LocalAuthorityClosed => std::future::pending().await,
        }
    }
}

#[async_trait]
impl RoutedTool for ContinueConversation {
    fn name(&self) -> &'static str {
        "continue_conversation"
    }

    fn description(&self) -> String {
        "Current Global Coordinator only. Continue the specified context-exhausted predecessor using its retained generated summary; the successor opening is system-generated, not a user commission. Supply the product conversation id, predecessor transcript id, and exact summary message id. No handoff text, first-message identity, or retry/recovery controls are accepted. An optional instance selects an enrolled peer; remote failures never fall back locally.".into()
    }

    fn clearable(&self) -> bool {
        false
    }
    fn nullable_instance(&self) -> bool {
        true
    }
    fn local_authority_is_closed(&self) -> bool {
        self.0.local_authority_is_closed()
    }
    fn route_error(&self, error: String, local: bool) -> ToolOutput {
        if local {
            super::output(Err(error))
        } else {
            ToolOutput::error(error)
        }
    }
    fn resolution_db(&self) -> &crate::db::Database {
        self.0.db()
    }

    fn schema_without_instance(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "product_conversation_id":{"type":"string", "minLength":1, "pattern":"\\S"},
                "predecessor_transcript_id":{"type":"string", "minLength":1, "pattern":"\\S"},
                "summary_message_id":{"type":"string", "minLength":1, "pattern":"\\S"}
            },
            "required": [
                "product_conversation_id",
                "predecessor_transcript_id",
                "summary_message_id"
            ],
            "additionalProperties": false
        })
    }

    async fn authorize(&self, ctx: &ToolContext) -> Result<(), String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())
    }

    fn validate_before_peer_lookup(&self, args: &Value) -> Result<(), String> {
        serde_json::from_value::<ContinueRequest>(args.clone())
            .map(|_| ())
            .map_err(|error| format!("invalid continue_conversation input: {error}"))
    }

    async fn run_routed(
        &self,
        args: Value,
        route: Route<crate::api::federation::ResolvedPeer>,
        ctx: ToolContext,
    ) -> ToolOutput {
        if self.0.local_authority_is_closed() {
            std::future::pending::<()>().await;
        }
        let request: ContinueRequest = match serde_json::from_value(args) {
            Ok(request) => request,
            Err(error) => {
                return self.route_error(error.to_string(), matches!(route, Route::Local))
            }
        };
        let local = matches!(route, Route::Local);
        let result = match route {
            Route::Local => self.run_local(request, &ctx).await,
            Route::Remote(peer) => {
                match tokio::spawn(Self::run_remote(self.0.clone(), request, peer, ctx)).await {
                    Ok(result) => result,
                    Err(error) => Err(error.to_string()),
                }
            }
        };
        if self.0.local_authority_is_closed() {
            std::future::pending::<()>().await;
        }
        if local {
            super::output(
                result.and_then(|value| {
                    serde_json::to_value(value).map_err(|error| error.to_string())
                }),
            )
        } else {
            match result {
                Ok(result) => serde_json::to_string(&result)
                    .map(ToolOutput::success)
                    .unwrap_or_else(|error| ToolOutput::error(error.to_string())),
                Err(error) => ToolOutput::error(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_accepts_only_the_three_narrow_identity_fields() {
        let product = phoenix_core::domain::product_conversation::ProductConversationId::new();
        let value = json!({
            "product_conversation_id": product,
            "predecessor_transcript_id": "predecessor",
            "summary_message_id": "summary"
        });
        let decoded: ContinueRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(decoded.product_conversation_id, product);
        assert_eq!(decoded.predecessor_transcript_id.0, "predecessor");
        assert_eq!(decoded.summary_message_id.0, "summary");

        for extra in [
            "retry_key",
            "control_token",
            "source_transcript_id",
            "instance",
        ] {
            let mut invalid = value.clone();
            invalid[extra] = json!("forbidden");
            assert!(serde_json::from_value::<ContinueRequest>(invalid).is_err());
        }
    }
}
