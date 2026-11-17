//! `answer_conversation_question`: the Coordinator answers a question a
//! monitored conversation is waiting on (REQ-AUTH-009).
//!
//! The answer goes through the same response path the web client uses, so the
//! worker resumes exactly as if the user had answered, with the persisted
//! message attributed to the Coordinator's tool call.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::coordinator_watch_delivery::render_questions;
use crate::send_chat_service::SendChatApplicationService;
use crate::state_machine::state::{QuestionAnnotation, QuestionRequestId};
use crate::state_machine::ConvState;
use crate::tools::{Tool, ToolContext, ToolOutput};

pub(super) struct AnswerConversationQuestion(pub(super) Arc<SendChatApplicationService>);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerInput {
    transcript_id: String,
    request_id: String,
    answers: HashMap<String, String>,
    #[serde(default)]
    notes: HashMap<String, String>,
}

/// Why an answer was not delivered, phrased for the Coordinator.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    SelfTarget,
    NotMonitored,
    NotHandedOver,
    NotWaiting,
    StaleRequest { current: String, questions: String },
    AnswersMismatch { questions: String },
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Self::SelfTarget => "answer_conversation_question cannot answer this conversation's own question.".into(),
            Self::NotMonitored => "That conversation is not under an active watch. Only monitored conversations route their questions to the Coordinator; the user answers the rest.".into(),
            Self::NotHandedOver => "That question was not handed to the Coordinator: it was asked before the watch began, or its watch event has not been delivered yet. The user answers it.".into(),
            Self::NotWaiting => "That conversation is not waiting on a question. It may already have been answered.".into(),
            Self::StaleRequest { current, questions } => format!(
                "That question is no longer current. The conversation now waits on request ID {current}:{questions}"
            ),
            Self::AnswersMismatch { questions } => format!(
                "Answer every question exactly once, keyed by its full question text:{questions}"
            ),
        }
    }
}

/// Check an answer against the conversation's current wait.
pub(super) fn check_answer(
    state: &ConvState,
    request_id: &str,
    answers: &HashMap<String, String>,
    notes: &HashMap<String, String>,
) -> Result<QuestionRequestId, Refusal> {
    let ConvState::AwaitingUserResponse {
        questions,
        request_authority,
        ..
    } = state
    else {
        return Err(Refusal::NotWaiting);
    };
    let Some(current) = request_authority.request_id() else {
        // Legacy waits carry no identity the Coordinator could name.
        return Err(Refusal::NotWaiting);
    };
    if current.as_str() != request_id {
        return Err(Refusal::StaleRequest {
            current: current.to_string(),
            questions: render_questions(questions),
        });
    }
    let answered_all = answers.len() == questions.len()
        && questions.iter().all(|question| {
            answers
                .get(&question.question)
                .is_some_and(|a| !a.trim().is_empty())
        })
        && notes.keys().all(|key| answers.contains_key(key));
    if !answered_all {
        return Err(Refusal::AnswersMismatch {
            questions: render_questions(questions),
        });
    }
    Ok(current.clone())
}

#[async_trait]
impl Tool for AnswerConversationQuestion {
    fn name(&self) -> &'static str {
        "answer_conversation_question"
    }

    fn description(&self) -> String {
        "Answer the questions a monitored conversation is waiting on, as the user would from the question card. Use the transcript ID and request ID from the watch event. Key each answer by the full question text; give an option label or free text, joining several labels with \", \" where multiple answers are allowed. Answer only what the assignment or your standing Authority settles; otherwise ask the user with post_user_question and pass the user's words on verbatim, with a note that they come from the user. The conversation resumes as soon as the answer is accepted.".into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "transcript_id": {"type": "string", "minLength": 1, "description": "Source transcript from the watch event"},
                "request_id": {"type": "string", "minLength": 1, "description": "Question request ID from the watch event"},
                "answers": {
                    "type": "object",
                    "additionalProperties": {"type": "string", "minLength": 1},
                    "description": "Full question text -> chosen option label or free-text answer"
                },
                "notes": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "Optional context per question, keyed like answers"
                }
            },
            "required": ["transcript_id", "request_id", "answers"],
            "additionalProperties": false
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed = match serde_json::from_value::<AnswerInput>(input) {
            Ok(value) => value,
            Err(error) => return ToolOutput::error(format!("invalid input: {error}")),
        };
        if parsed.transcript_id == ctx.conversation_id {
            return ToolOutput::error(Refusal::SelfTarget.message());
        }
        let db = self.0.db();
        let target = match db.get_conversation(&parsed.transcript_id).await {
            Ok(conversation) => conversation,
            Err(error) => return ToolOutput::error(format!("conversation unavailable: {error}")),
        };
        let monitored = match db.monitored_product_conversations().await {
            Ok(mut monitored) => monitored.remove(target.product_conversation_id.as_str()),
            Err(error) => return ToolOutput::error(format!("watch lookup failed: {error}")),
        };
        let Some(monitored) = monitored else {
            return ToolOutput::error(Refusal::NotMonitored.message());
        };
        let request_id = match check_answer(
            &target.state,
            &parsed.request_id,
            &parsed.answers,
            &parsed.notes,
        ) {
            Ok(request_id) => request_id,
            Err(refusal) => return ToolOutput::error(refusal.message()),
        };
        // Only a question the Coordinator was handed is the Coordinator's to
        // answer; the product list shows every other wait as the user's.
        let handed_over = monitored
            .latest_question
            .is_some_and(|reported| reported.request_id == request_id.as_str());
        if !handed_over {
            return ToolOutput::error(Refusal::NotHandedOver.message());
        }
        let source = match self.0.source_conversation(&ctx.conversation_id).await {
            Ok(source) => source,
            Err(error) => {
                return ToolOutput::error(format!("sender membership unavailable: {error}"))
            }
        };
        let Some(source_call) = ctx.source_tool_call() else {
            return ToolOutput::error("Trusted source tool-call identity unavailable");
        };
        let answer_source = phoenix_core::domain::sm_event::QuestionAnswerSource {
            message_id: uuid::Uuid::new_v4().to_string(),
            origin: super::sender_origin(source, source_call),
        };
        let annotations = (!parsed.notes.is_empty()).then(|| {
            parsed
                .notes
                .into_iter()
                .map(|(question, notes)| {
                    (
                        question,
                        QuestionAnnotation {
                            preview: None,
                            notes: Some(notes),
                        },
                    )
                })
                .collect()
        });
        match self
            .0
            .respond_to_question(
                &parsed.transcript_id,
                Some(request_id),
                parsed.answers,
                annotations,
                Some(answer_source),
            )
            .await
        {
            Ok(()) => ToolOutput::success(
                json!({
                    "transcript_id": parsed.transcript_id,
                    "request_id": parsed.request_id,
                    "status": "answered"
                })
                .to_string(),
            ),
            Err(crate::api::handlers::AppError::Conflict(conflict)) => {
                ToolOutput::error(format!("answer not accepted: {}", conflict.error))
            }
            Err(error) => ToolOutput::error(format!(
                "answer not accepted: {}",
                super::app_error_message(error)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::state::{QuestionOption, QuestionRequestAuthority, UserQuestion};

    fn waiting() -> (ConvState, QuestionRequestId) {
        let request_authority = QuestionRequestAuthority::new();
        let request_id = request_authority.request_id().unwrap().clone();
        let state = ConvState::AwaitingUserResponse {
            questions: vec![
                UserQuestion {
                    question: "Which database?".into(),
                    header: "DB".into(),
                    options: vec![
                        QuestionOption {
                            label: "Postgres".into(),
                            description: None,
                            preview: None,
                        },
                        QuestionOption {
                            label: "SQLite".into(),
                            description: None,
                            preview: None,
                        },
                    ],
                    multi_select: false,
                },
                UserQuestion {
                    question: "Ship today?".into(),
                    header: "Ship".into(),
                    options: Vec::new(),
                    multi_select: false,
                },
            ],
            tool_use_id: "ask".into(),
            request_authority,
        };
        (state, request_id)
    }

    fn answers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(q, a)| ((*q).to_string(), (*a).to_string()))
            .collect()
    }

    #[test]
    fn a_complete_answer_to_the_current_request_is_accepted() {
        let (state, request) = waiting();
        let accepted = check_answer(
            &state,
            request.as_str(),
            &answers(&[("Which database?", "SQLite"), ("Ship today?", "Yes")]),
            &answers(&[("Ship today?", "From the user, verbatim")]),
        );
        assert_eq!(accepted, Ok(request));
    }

    #[test]
    fn a_stale_request_names_the_current_one() {
        let (state, request) = waiting();
        let refusal = check_answer(
            &state,
            "an-older-request",
            &answers(&[("Which database?", "SQLite"), ("Ship today?", "Yes")]),
            &HashMap::new(),
        )
        .unwrap_err();
        let Refusal::StaleRequest { current, questions } = &refusal else {
            panic!("expected stale request, got {refusal:?}");
        };
        assert_eq!(current, request.as_str());
        assert!(questions.contains("1. Which database?\n   - Postgres\n   - SQLite"));
    }

    #[test]
    fn partial_or_misnamed_answers_are_refused() {
        let (state, request) = waiting();
        for bad in [
            answers(&[("Which database?", "SQLite")]),
            answers(&[("Which database?", "SQLite"), ("Ship?", "Yes")]),
            answers(&[("Which database?", "SQLite"), ("Ship today?", "  ")]),
        ] {
            assert!(matches!(
                check_answer(&state, request.as_str(), &bad, &HashMap::new()),
                Err(Refusal::AnswersMismatch { .. })
            ));
        }
        assert!(matches!(
            check_answer(
                &state,
                request.as_str(),
                &answers(&[("Which database?", "SQLite"), ("Ship today?", "Yes")]),
                &answers(&[("Unasked?", "note")]),
            ),
            Err(Refusal::AnswersMismatch { .. })
        ));
    }

    #[test]
    fn a_conversation_that_is_not_waiting_is_refused() {
        assert_eq!(
            check_answer(&ConvState::Idle, "any", &HashMap::new(), &HashMap::new()),
            Err(Refusal::NotWaiting)
        );
    }
}
