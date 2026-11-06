use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::conversation_goal::GoalVersion;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::prepared::GoalPreparationIntent;
use phoenix_db::conversation_goals::user_commands::GoalUserResult;
use phoenix_db::conversation_goals::CoordinatorGoalWaitRequest;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::GoalCommandService;
use crate::runtime::RuntimeManager;
use crate::tools::{Tool, ToolContext, ToolOutput};

#[derive(Clone, Copy)]
enum Operation {
    Prepare,
    Apply,
}

struct GoalTool {
    service: GoalCommandService,
    runtime: Arc<RuntimeManager>,
    operation: Operation,
}

struct ReadGoalTool {
    service: GoalCommandService,
    runtime: Arc<RuntimeManager>,
}

pub(super) fn tools(runtime: &Arc<RuntimeManager>) -> Vec<Arc<dyn Tool>> {
    let service = GoalCommandService::new(Arc::clone(runtime));
    let mut tools = vec![super::instance::RouteTool::new(ReadGoalTool {
        service: service.clone(),
        runtime: Arc::clone(runtime),
    })];
    tools.extend(
        [Operation::Prepare, Operation::Apply]
            .into_iter()
            .map(|operation| {
                Arc::new(GoalTool {
                    service: service.clone(),
                    operation,
                    runtime: Arc::clone(runtime),
                }) as Arc<dyn Tool>
            }),
    );
    tools
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    target: String,
    #[serde(default)]
    after_version: Option<i64>,
    #[serde(default)]
    wait: Option<GoalWaitInput>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoalWaitInput {
    minimum_version: i64,
    timeout_seconds: u64,
}

impl GoalWaitInput {
    fn into_request(self) -> Result<CoordinatorGoalWaitRequest, String> {
        let minimum_version =
            GoalVersion::parse(self.minimum_version).map_err(|error| error.to_string())?;
        CoordinatorGoalWaitRequest::new(
            minimum_version,
            std::time::Duration::from_secs(self.timeout_seconds),
        )
        .map_err(|error| error.to_string())
    }
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
            Operation::Prepare => "prepare_goal",
            Operation::Apply => "apply_goal",
        }
    }
    fn description(&self) -> String {
        match self.operation {
            Operation::Prepare => "Prepare an immutable Goal action against the observed saved Goal. Coordinator only. create refuses an existing Goal; replace is an explicit new Goal identity retaining old history; state updates disposition. Applying create/replace requires the target's persisted autoContinue preference to be explicitly enabled; preparation does not enable it. Returns a server-created action_reference and frozen expected version/Goal identity. Review this observation, then pass only action_reference to apply_goal. Never invent UUIDs or versions. A conflict requires deliberate reconsideration, NOT reread-and-overwrite/reprepare. No automatic scheduling, Start/Stop, Suspended disposition, or Suspended reactivation.",
            Operation::Apply => "Apply or reconcile a previously prepared Goal action by its server-issued action_reference. Coordinator only. Never accepts target, version, authority, or replacement content. Exact retries replay the immutable accepted or refused receipt, including across restart/Coordinator continuation. New create/replace requires the target's persisted autoContinue preference to be explicitly enabled; refusal does not enable it or dispatch work. On conflict or busy refusal do NOT blindly prepare another action; read Goal/history and reconsider explicitly. Does not dispatch or schedule work, Start/Stop, or reactivate Suspended Goals.",
        }.to_owned()
    }
    fn input_schema(&self) -> Value {
        match self.operation {
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

#[async_trait]
impl super::instance::RoutedTool for ReadGoalTool {
    fn name(&self) -> &'static str {
        "read_goal"
    }
    fn nullable_instance(&self) -> bool {
        true
    }
    fn description(&self) -> String {
        "Read an ordinary conversation's saved Goal and up to 100 ordered history entries. Coordinator only. Uses stable @conv targets and follows the current transcript. after_version is only a history cursor. Omit instance or set it to exact \"local\" for this instance; otherwise a saved-peer ID or unique display name reads exactly that peer and never falls back locally. Optional wait blocks for minimum_version for 1-25 seconds and returns an authoritative satisfied or timed_out snapshot. Goal disposition is not active-execution state; this never dispatches work.".into()
    }
    fn clearable(&self) -> bool {
        true
    }
    fn schema_without_instance(&self) -> Value {
        json!({"type":"object","properties":{"target":target_schema(),"after_version":{"type":"integer","minimum":0,"description":"History cursor only; entries at or before this Goal version are omitted."},"wait":{"type":"object","properties":{"minimum_version":{"type":"integer","minimum":0},"timeout_seconds":{"type":"integer","minimum":1,"maximum":25}},"required":["minimum_version","timeout_seconds"],"additionalProperties":false}},"required":["target"],"additionalProperties":false})
    }
    fn db(&self) -> &crate::db::Database {
        self.runtime.db()
    }
    async fn authorize(&self, ctx: &ToolContext) -> Result<(), String> {
        self.service
            .authorize_coordinator(&ctx.conversation_id)
            .await
            .map_err(|error| self.refusal(error))
    }
    async fn run_routed(
        &self,
        input: Value,
        route: super::instance::Route<crate::api::federation::ResolvedPeer>,
        ctx: ToolContext,
    ) -> ToolOutput {
        match self.read(input, &ctx, route).await {
            Ok(value) => ToolOutput::success(value.to_string()),
            Err(error) => ToolOutput::error(error),
        }
    }
}

impl ReadGoalTool {
    fn bad_input(&self, error: &serde_json::Error) -> String {
        format!("Invalid read_goal input: {error}. Read the tool schema; authority and versions are server-derived.")
    }

    fn refusal(&self, error: crate::api::handlers::AppError) -> String {
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
        format!("read_goal refused: {message}. Verify current Coordinator authority and an ordinary open @conv target.")
    }

    async fn read(
        &self,
        input: Value,
        ctx: &ToolContext,
        route: super::instance::Route<crate::api::federation::ResolvedPeer>,
    ) -> Result<Value, String> {
        let args: ReadInput =
            serde_json::from_value(input).map_err(|error| self.bad_input(&error))?;
        self.service
            .authorize_coordinator(&ctx.conversation_id)
            .await
            .map_err(|error| self.refusal(error))?;
        let owner = target(&args.target)?;
        let after = GoalVersion::parse(args.after_version.unwrap_or(0))
            .map_err(|error| error.to_string())?;
        match (route, args.wait) {
            (super::instance::Route::Local, None) => {
                let (current, history) = self
                    .service
                    .read_coordinator(&ctx.conversation_id, &owner, after)
                    .await
                    .map_err(|error| self.refusal(error))?;
                Ok(
                    json!({"target":args.target,"current":current,"history":history,"history_page_size":100,"execution_state_included":false}),
                )
            }
            (super::instance::Route::Local, Some(wait)) => {
                let wait = wait.into_request()?;
                let result = self
                    .service
                    .wait_coordinator(&ctx.conversation_id, &owner, after, wait)
                    .await
                    .map_err(|error| self.refusal(error))?;
                Ok(
                    json!({"target":args.target,"exact_transcript_id":result.destination_transcript_id(),"current":result.current(),"history":result.history(),"history_page_size":100,"execution_state_included":false,"wait_status":result.status()}),
                )
            }
            (super::instance::Route::Remote(peer), wait) => {
                let wait = wait.map(GoalWaitInput::into_request).transpose()?;
                let response = crate::api::federation::read_remote_goal(
                    self.runtime.db(),
                    &peer,
                    &owner,
                    after,
                    wait,
                )
                .await
                .map_err(|error| format!("read_goal remote read failed: {error}"))?;
                serde_json::to_value(response).map_err(|error| error.to_string())
            }
        }
    }
}

impl GoalTool {
    fn bad_input(&self, error: &serde_json::Error) -> String {
        format!(
            "Invalid {} input: {error}. Read the tool schema; authority and versions are server-derived.",
            self.name()
        )
    }

    fn refusal(&self, error: crate::api::handlers::AppError) -> String {
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
        format!(
            "{} refused: {}. Verify current Coordinator authority and an ordinary open @conv target. Read Goal/history before explicitly reconsidering; do not blind-retry with a fresh preparation.",
            self.name(),
            message
        )
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, String> {
        self.service
            .authorize_coordinator(&ctx.conversation_id)
            .await
            .map_err(|error| self.refusal(error))?;
        let output = match self.operation {
            Operation::Prepare => {
                let args: PrepareInput =
                    serde_json::from_value(input).map_err(|error| self.bad_input(&error))?;
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
                    .map_err(|error| self.refusal(error))?;
                json!({"action_reference":action.reference,"target":args.target,"observed_goal_id":action.observed_goal_id,"expected_version":action.expected_version,"action":action.intent,"next_tool":"apply_goal","note":"Frozen observation only; Goal unchanged. Review before applying. Replacement creates a new Goal ID and preserves history."})
            }
            Operation::Apply => {
                let args: ApplyInput =
                    serde_json::from_value(input).map_err(|error| self.bad_input(&error))?;
                let receipt = self
                    .service
                    .apply_prepared(&ctx.conversation_id, args.action_reference)
                    .await
                    .map_err(|error| self.refusal(error))?;
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
