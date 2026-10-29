use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::conversation_goal::GoalVersion;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::prepared::GoalPreparationIntent;
use phoenix_db::conversation_goals::user_commands::GoalUserResult;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::GoalCommandService;
use crate::runtime::RuntimeManager;
use crate::tools::{Tool, ToolContext, ToolOutput};

#[derive(Clone, Copy)]
enum Operation {
    Read,
    Prepare,
    Apply,
}

struct GoalTool {
    service: GoalCommandService,
    operation: Operation,
}

pub(super) fn tools(runtime: Arc<RuntimeManager>) -> Vec<Arc<dyn Tool>> {
    let service = GoalCommandService::new(runtime);
    [Operation::Read, Operation::Prepare, Operation::Apply]
        .into_iter()
        .map(|operation| {
            Arc::new(GoalTool {
                service: service.clone(),
                operation,
            }) as Arc<dyn Tool>
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    target: String,
    #[serde(default)]
    after_version: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareInput {
    target: String,
    action: GoalPreparationIntent,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    action_reference: Uuid,
}

fn target(value: &str) -> Result<ProductConversationId, String> {
    let id = value.strip_prefix("@conv:").filter(|id| !id.is_empty() && !id.chars().any(char::is_whitespace) && !id.contains('#'))
        .ok_or_else(|| "Use a stable @conv:<product-conversation-id> target returned by conversation discovery; transcript IDs and bare IDs are refused".to_string())?;
    ProductConversationId::parse(id.to_string())
        .map_err(|error| format!("Invalid stable Goal target: {error}"))
}

fn target_schema() -> Value {
    json!({"type":"string","pattern":"^@conv:[^\\s#]+$","description":"Stable ordinary conversation target returned by discovery. Not @transcript or a runtime UUID."})
}
fn text_schema() -> Value {
    json!({"type":"string","minLength":1,"pattern":"\\S"})
}

#[async_trait]
impl Tool for GoalTool {
    fn name(&self) -> &str {
        match self.operation {
            Operation::Read => "read_goal",
            Operation::Prepare => "prepare_goal",
            Operation::Apply => "apply_goal",
        }
    }
    fn description(&self) -> String {
        match self.operation {
            Operation::Read => "Read an ordinary conversation's saved Goal and up to 100 ordered history entries. Coordinator only. Uses stable @conv targets and follows the current transcript. after_version is only a history cursor. Goal disposition is not active-execution state; this never dispatches work.",
            Operation::Prepare => "Prepare an immutable Goal action against the observed saved Goal. Coordinator only. create refuses an existing Goal; replace is an explicit new Goal identity retaining old history; state updates disposition. Returns a server-created action_reference and frozen expected version/Goal identity. Review this observation, then pass only action_reference to apply_goal. Never invent UUIDs or versions. A conflict requires deliberate reconsideration, NOT reread-and-overwrite/reprepare. No automatic scheduling, Start/Stop, Suspended disposition, or Suspended reactivation.",
            Operation::Apply => "Apply or reconcile a previously prepared Goal action by its server-issued action_reference. Coordinator only. Never accepts target, version, authority, or replacement content. Exact retries replay the immutable accepted or refused receipt, including across restart/Coordinator continuation. On conflict or busy refusal do NOT blindly prepare another action; read Goal/history and reconsider explicitly. Does not dispatch or schedule work, Start/Stop, or reactivate Suspended Goals.",
        }.to_owned()
    }
    fn input_schema(&self) -> Value {
        match self.operation {
            Operation::Read => {
                json!({"type":"object","properties":{"target":target_schema(),"after_version":{"type":"integer","minimum":0}},"required":["target"],"additionalProperties":false})
            }
            Operation::Apply => {
                json!({"type":"object","properties":{"action_reference":{"type":"string","format":"uuid","description":"Copy the opaque reference returned by prepare_goal; do not generate one."}},"required":["action_reference"],"additionalProperties":false})
            }
            Operation::Prepare => {
                let objective = |operation: &str| json!({"type":"object","properties":{"operation":{"const":operation},"objective":text_schema()},"required":["operation","objective"],"additionalProperties":false});
                let reason = |state: &str| json!({"type":"object","properties":{"state":{"const":state},"reason":text_schema()},"required":["state","reason"],"additionalProperties":false});
                json!({"type":"object","properties":{"target":target_schema(),"action":{"oneOf":[objective("create"),objective("replace"),{"type":"object","properties":{"operation":{"const":"state"},"disposition":{"oneOf":[{"type":"object","properties":{"state":{"const":"underway"}},"required":["state"],"additionalProperties":false},reason("blocked"),reason("complete"),reason("exhausted")]}},"required":["operation","disposition"],"additionalProperties":false}]}},"required":["target","action"],"additionalProperties":false})
            }
        }
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        match self.execute(input, &ctx).await {
            Ok(output) => output,
            Err(error) => ToolOutput::error(error),
        }
    }
}

impl GoalTool {
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        let bad_input = |error| {
            format!("Invalid {} input: {error}. Read the tool schema; authority and versions are server-derived.", self.name())
        };
        let refusal = |error: crate::api::handlers::AppError| {
            use crate::api::handlers::AppError;
            let message = match error {
                AppError::Conflict(response) => response.error,
                other @ (AppError::BadRequest(_)
                | AppError::TypedBadRequest { .. }
                | AppError::NotFound(_)
                | AppError::Forbidden(_)
                | AppError::Internal(_)
                | AppError::TranscriptSnapshotChanged
                | AppError::TypedInternal { .. }
                | AppError::UnprocessableEntity(_)) => super::app_error_message(other),
            };
            format!("{} refused: {}. Verify current Coordinator authority and an ordinary open @conv target. Read Goal/history before explicitly reconsidering; do not blind-retry with a fresh preparation.", self.name(), message)
        };
        self.service
            .authorize_coordinator(&ctx.conversation_id)
            .await
            .map_err(refusal)?;
        let output = match self.operation {
            Operation::Read => {
                let args: ReadInput = serde_json::from_value(input).map_err(bad_input)?;
                let owner = target(&args.target)?;
                let after = GoalVersion::parse(args.after_version.unwrap_or(0))
                    .map_err(|error| error.to_string())?;
                let (current, history) = self
                    .service
                    .read_coordinator(&ctx.conversation_id, &owner, after)
                    .await
                    .map_err(refusal)?;
                json!({"target":args.target,"current":current,"history":history,"history_page_size":100,"execution_state_included":false})
            }
            Operation::Prepare => {
                let args: PrepareInput = serde_json::from_value(input).map_err(bad_input)?;
                let owner = target(&args.target)?;
                let source = ctx.source_tool_call().ok_or(
                    "Missing server-bound assistant/tool invocation; cannot prepare a Goal action",
                )?;
                let turn_id = ctx.direct_turn_id().ok_or("Goal preparation requires server-bound durable turn provenance; caller-supplied execution IDs are refused")?;
                let action = self
                    .service
                    .prepare_coordinator(
                        &ctx.conversation_id,
                        &source,
                        &owner,
                        args.action,
                        turn_id,
                    )
                    .await
                    .map_err(refusal)?;
                json!({"action_reference":action.reference,"target":args.target,"observed_goal_id":action.observed_goal_id,"expected_version":action.expected_version,"action":action.intent,"next_tool":"apply_goal","note":"Frozen observation only; Goal unchanged. Review before applying. Replacement creates a new Goal ID and preserves history."})
            }
            Operation::Apply => {
                let args: ApplyInput = serde_json::from_value(input).map_err(bad_input)?;
                let receipt = self
                    .service
                    .apply_prepared(&ctx.conversation_id, args.action_reference)
                    .await
                    .map_err(refusal)?;
                let accepted = matches!(receipt.result, GoalUserResult::Accepted { .. });
                let value = json!({"action_reference":args.action_reference,"receipt":receipt,"next_action":if accepted {"No dispatch performed. Use read_goal to inspect history."} else {"Retained refusal: same reference will return the same refusal. Read current Goal/history and explicitly reconsider; do not automatically reprepare."}});
                return if accepted {
                    Ok(ToolOutput::success(
                        serde_json::to_string(&value).map_err(|error| error.to_string())?,
                    ))
                } else {
                    Ok(ToolOutput::error(
                        serde_json::to_string(&value).map_err(|error| error.to_string())?,
                    ))
                };
            }
        };
        Ok(ToolOutput::success(
            serde_json::to_string(&output).map_err(|error| error.to_string())?,
        ))
    }
}

#[cfg(test)]
mod tests;
