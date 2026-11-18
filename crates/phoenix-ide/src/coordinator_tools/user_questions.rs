//! REQ-AUQ-010, REQ-AUQ-012: the Coordinator asks the user without stopping.
//!
//! `post_user_question` stores the question and returns at once; the
//! Coordinator keeps coordinating, and the user's reply arrives later as an
//! ordinary user message (see `coordinator_question_delivery`). Ordinary
//! conversations keep the blocking `ask_user_question`.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state_machine::state::AskUserQuestionInput;
use crate::tools::{AskUserQuestionTool, Tool, ToolContext, ToolOutput};
use phoenix_db::{CoordinatorQuestionClose, CoordinatorQuestionSettlement};

pub(super) struct PostUserQuestion(pub(super) crate::db::Database);
pub(super) struct WithdrawUserQuestion(pub(super) crate::db::Database);

#[async_trait]
impl Tool for PostUserQuestion {
    fn name(&self) -> &'static str {
        "post_user_question"
    }

    fn description(&self) -> String {
        "Ask the user 1-4 structured questions without stopping your own work. The question appears on your page and notifies the user; this call returns at once with a request ID. Keep coordinating everything that does not depend on the answer, and do not act on what you asked until the reply arrives as a user message beginning \"Answer to your question (request ID …)\" or reports that the user dismissed it. Withdraw a question you settled another way with withdraw_user_question. Provide 2-4 options per question; the user can also type a custom answer.".into()
    }

    fn input_schema(&self) -> Value {
        AskUserQuestionTool.input_schema()
    }

    // REQ-AUQ-008: question tools load on demand.
    fn defer_loading(&self) -> bool {
        true
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let mut parsed = match serde_json::from_value::<AskUserQuestionInput>(input) {
            Ok(value) => value,
            Err(error) => {
                return ToolOutput::error(format!(
                    "post_user_question input failed to parse: {error}. Re-emit the call with a valid `questions` array."
                ))
            }
        };
        if let Err(error) = parsed.validate_and_normalize() {
            return ToolOutput::error(error);
        }
        let request_id = crate::state_machine::state::QuestionRequestId::new().to_string();
        if let Err(error) = self
            .0
            .post_coordinator_question(
                &request_id,
                &ctx.conversation_id,
                &parsed.questions,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
        {
            return ToolOutput::error(format!("question not posted: {error}"));
        }
        ToolOutput::success(
            json!({
                "request_id": request_id,
                "status": "posted",
                "next": "Keep working on anything that does not depend on the answer. The reply arrives as a user message naming this request ID."
            })
            .to_string(),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawInput {
    request_id: String,
}

#[async_trait]
impl Tool for WithdrawUserQuestion {
    fn name(&self) -> &'static str {
        "withdraw_user_question"
    }

    fn description(&self) -> String {
        "Withdraw an open question you posted with post_user_question, when you settled it another way or it no longer matters. The question disappears from the user's view; no reply will arrive. Fails if the user already answered or dismissed it — then read their reply instead.".into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "request_id": {"type": "string", "minLength": 1}
            },
            "required": ["request_id"],
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value, _ctx: ToolContext) -> ToolOutput {
        let parsed = match serde_json::from_value::<WithdrawInput>(input) {
            Ok(value) => value,
            Err(error) => return ToolOutput::error(format!("invalid input: {error}")),
        };
        match self
            .0
            .settle_coordinator_question(
                &parsed.request_id,
                CoordinatorQuestionClose::Withdrawn,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
        {
            Ok(CoordinatorQuestionSettlement::Settled) => ToolOutput::success(
                json!({"request_id": parsed.request_id, "status": "withdrawn"}).to_string(),
            ),
            Ok(CoordinatorQuestionSettlement::NotFound) => ToolOutput::error(format!(
                "No question with request ID {}.",
                parsed.request_id
            )),
            Ok(CoordinatorQuestionSettlement::AlreadySettled { status }) => {
                ToolOutput::error(format!(
                    "Question {} is already {status}; it cannot be withdrawn.",
                    parsed.request_id
                ))
            }
            Err(error) => ToolOutput::error(format!("withdraw failed: {error}")),
        }
    }
}
