use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Deserializer};

fn deserialize_present_instance<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

use serde_json::{json, Value};

use crate::automatic_continuation_settings::{native, Target};
use crate::runtime::RuntimeManager;

#[derive(Clone, Copy)]
enum Mode {
    Read,
    Set,
}

pub(super) struct AutomaticContinuationTool {
    runtime: Arc<RuntimeManager>,
    mode: Mode,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TargetInput {
    Ordinary {
        product_conversation_id: ProductConversationId,
    },
    Coordinator {},
}

impl From<TargetInput> for Target {
    fn from(input: TargetInput) -> Self {
        match input {
            TargetInput::Ordinary {
                product_conversation_id,
            } => Self::Ordinary(product_conversation_id),
            TargetInput::Coordinator {} => Self::Coordinator,
        }
    }
}

impl From<TargetInput> for crate::api::federation::RemoteAutomaticContinuationTarget {
    fn from(input: TargetInput) -> Self {
        match input {
            TargetInput::Ordinary {
                product_conversation_id,
            } => Self::Ordinary {
                product_conversation_id,
            },
            TargetInput::Coordinator {} => Self::Coordinator {},
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    target: TargetInput,
    #[serde(default, deserialize_with = "deserialize_present_instance")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRequest {
    target: TargetInput,
    #[serde(default, deserialize_with = "deserialize_present_instance")]
    instance: Option<String>,
    auto_continue_on_context_exhaustion: bool,
}

impl AutomaticContinuationTool {
    pub(super) fn read(runtime: Arc<RuntimeManager>) -> Self {
        Self {
            runtime,
            mode: Mode::Read,
        }
    }

    pub(super) fn set(runtime: Arc<RuntimeManager>) -> Self {
        Self {
            runtime,
            mode: Mode::Set,
        }
    }

    async fn run_inner(&self, input: Value, ctx: ToolContext) -> Result<Value, String> {
        self.runtime
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        ctx.source_tool_call()
            .ok_or("Trusted source tool-call identity unavailable")?;
        let (target, instance, operation) = match self.mode {
            Mode::Read => {
                let request: ReadRequest =
                    serde_json::from_value(input).map_err(|error| error.to_string())?;
                (request.target, request.instance, native::Operation::Read)
            }
            Mode::Set => {
                let request: SetRequest =
                    serde_json::from_value(input).map_err(|error| error.to_string())?;
                (
                    request.target,
                    request.instance,
                    native::Operation::Set(request.auto_continue_on_context_exhaustion.into()),
                )
            }
        };
        if let Some(selector) = instance.filter(|selector| selector != "local") {
            let result = remote_from_coordinator(
                self.runtime.clone(),
                ctx.conversation_id.clone(),
                selector,
                target,
                operation,
            )
            .await?;
            return serde_json::to_value(result).map_err(|error| error.to_string());
        }
        match native::from_coordinator(
            self.runtime.clone(),
            ctx.conversation_id,
            target.into(),
            operation,
        )
        .await
        {
            native::Completion::Established(result) => {
                serde_json::to_value(result?).map_err(|error| error.to_string())
            }
            native::Completion::LocalAuthorityClosed => std::future::pending().await,
        }
    }
}

async fn remote_from_coordinator(
    runtime: Arc<RuntimeManager>,
    source_transcript_id: String,
    selector: String,
    target: TargetInput,
    operation: native::Operation,
) -> Result<native::PreferenceView, String> {
    tokio::spawn(remote_from_coordinator_inner(
        runtime,
        source_transcript_id,
        selector,
        target,
        operation,
    ))
    .await
    .map_err(|error| error.to_string())?
}

async fn remote_from_coordinator_inner(
    runtime: Arc<RuntimeManager>,
    source_transcript_id: String,
    selector: String,
    target: TargetInput,
    operation: native::Operation,
) -> Result<native::PreferenceView, String> {
    let _operation_authority = runtime
        .acquire_local_authority_pass()
        .map_err(|()| "Local mutation authority is closed".to_string())?;
    let source = runtime
        .db()
        .get_conversation(&source_transcript_id)
        .await
        .map_err(|error| error.to_string())?;
    let _source_gate = runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    runtime
        .db()
        .require_current_coordinator_transcript(&source_transcript_id)
        .await
        .map_err(|error| error.to_string())?;
    let _dispatch_authority = runtime
        .acquire_local_authority_pass()
        .map_err(|()| "Local mutation authority is closed".to_string())?;
    match operation {
        native::Operation::Read => {
            crate::api::federation::remote_read_automatic_continuation(
                runtime.db(),
                &selector,
                target.into(),
            )
            .await
        }
        native::Operation::Set(value) => {
            crate::api::federation::remote_set_automatic_continuation(
                runtime.db(),
                &selector,
                target.into(),
                value.is_enabled(),
            )
            .await
        }
    }
    .map_err(|error| error.to_string())
}

#[async_trait]
impl Tool for AutomaticContinuationTool {
    fn name(&self) -> &'static str {
        match self.mode {
            Mode::Read => "read_automatic_continuation",
            Mode::Set => "set_automatic_continuation",
        }
    }

    fn description(&self) -> String {
        let action = match self.mode {
            Mode::Read => "Read",
            Mode::Set => "Set",
        };
        format!("Current Global Coordinator only. Optional instance: omitted or exact local routes locally; otherwise select one saved peer by UUID or unique name. {action} the persisted automatic context-continuation preference for an exact ordinary product conversation id or the explicit Coordinator singleton. Returns preference only, not admission or execution status. Setting is prospective: enabling does not wake exhausted work; disabling does not revoke admitted work. No implicit Goal enablement, retry, recovery, rearm, scheduler, or stop/pause bypass.")
    }

    fn input_schema(&self) -> Value {
        let mut schema = json!({
            "type":"object", "additionalProperties":false,
            "required":["target"],
            "properties":{
                "instance":{"type":"string", "minLength":1},
                "target":{"anyOf":[
                    {"type":"object", "additionalProperties":false,
                        "required":["kind", "product_conversation_id"],
                        "properties":{
                            "kind":{"type":"string", "enum":["ordinary"]},
                            "product_conversation_id":{"type":"string", "minLength":1, "pattern":"\\S"}
                        }},
                    {"type":"object", "additionalProperties":false,
                        "required":["kind"],
                        "properties":{"kind":{"type":"string", "enum":["coordinator"]}}}
                ]}
            }
        });
        if matches!(self.mode, Mode::Set) {
            schema["required"] = json!(["target", "auto_continue_on_context_exhaustion"]);
            schema["properties"]["auto_continue_on_context_exhaustion"] = json!({"type":"boolean"});
        }
        schema
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        if self.runtime.local_authority_is_closed() {
            std::future::pending::<()>().await;
        }
        let result = self.run_inner(input, ctx).await;
        if self.runtime.local_authority_is_closed() {
            std::future::pending::<()>().await;
        }
        super::output(result)
    }
}

#[cfg(test)]
mod tests;
