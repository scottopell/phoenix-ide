//! REQ-AUQ-011: deliver the user's reply to a Coordinator question.
//!
//! The reply is stored first and delivered to the current Coordinator
//! transcript by a retrying pass, so an answer given while the Coordinator is
//! busy, between transcripts, or before restart is never lost. The question's
//! request id doubles as the message id, so a repeated pass cannot duplicate it.

use std::collections::HashMap;
use std::sync::Arc;

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::sm_state::{render_question_answers, QuestionAnnotation, UserQuestion};

use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
};

/// The Coordinator-facing text for an answered question.
pub(crate) fn answer_reply(
    request_id: &str,
    questions: &[UserQuestion],
    answers: &HashMap<String, String>,
    annotations: Option<&HashMap<String, QuestionAnnotation>>,
) -> String {
    format!(
        "Answer to your question (request ID {request_id}):\n{}",
        render_question_answers(questions, answers, annotations)
    )
}

/// The Coordinator-facing text for a dismissed question.
pub(crate) fn dismissal_reply(request_id: &str) -> String {
    format!(
        "The user dismissed your question (request ID {request_id}) without answering. Do not ask it again unless something material changes."
    )
}

/// Deliver stored replies to the current Coordinator transcript.
pub(crate) async fn deliver_pass(runtime: &Arc<RuntimeManager>) {
    let db = runtime.db();
    let replies = match db.undelivered_coordinator_question_replies(16).await {
        Ok(replies) => replies,
        Err(error) => {
            tracing::warn!(%error, "coordinator question reply discovery failed; retrying");
            return;
        }
    };
    if replies.is_empty() {
        return;
    }
    let target = match db.coordinator_watch_target().await {
        Ok(Some(target)) => target,
        Ok(None) => {
            tracing::debug!("coordinator question replies awaiting coordinator transcript");
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "coordinator question reply target lookup failed; retrying");
            return;
        }
    };
    let service = SendChatApplicationService::new(db.clone(), runtime.clone());
    for reply in replies {
        let request = SendChatRequest {
            conversation_id: target.clone(),
            origin: InputOrigin::UserApi,
            text: reply.reply,
            message_id: reply.request_id.clone(),
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            expansion_policy: MessageExpansionPolicy::LiteralText,
        };
        match service.send(request).await {
            Ok(
                SendChatOutcome::Delivered
                | SendChatOutcome::AlreadyPersisted
                | SendChatOutcome::QueuedAsSteering,
            ) => {
                let now = chrono::Utc::now().timestamp_micros();
                if let Err(error) = db
                    .mark_coordinator_question_reply_delivered(&reply.request_id, now)
                    .await
                {
                    tracing::warn!(request_id = %reply.request_id, %error, "coordinator question reply delivered but not marked; the next pass is idempotent");
                }
            }
            Ok(SendChatOutcome::Rejected { message, code }) => {
                tracing::warn!(request_id = %reply.request_id, code, %message, "coordinator question reply not accepted; retrying");
            }
            Err(error) => {
                tracing::warn!(request_id = %reply.request_id, %error, "coordinator question reply delivery failed; retrying");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::sm_state::QuestionOption;

    #[test]
    fn replies_name_the_request_and_render_like_card_answers() {
        let questions = vec![UserQuestion {
            question: "Merge now?".into(),
            header: "Merge".into(),
            options: vec![QuestionOption {
                label: "Yes".into(),
                description: None,
                preview: None,
            }],
            multi_select: false,
        }];
        let answers = HashMap::from([("Merge now?".to_string(), "Yes".to_string())]);
        let notes = HashMap::from([(
            "Merge now?".to_string(),
            QuestionAnnotation {
                preview: None,
                notes: Some("after CI".into()),
            },
        )]);
        assert_eq!(
            answer_reply("q-1", &questions, &answers, Some(&notes)),
            "Answer to your question (request ID q-1):\n\"Merge now?\" = \"Yes\" user notes: after CI"
        );
        assert!(dismissal_reply("q-1").contains("request ID q-1"));
    }
}
