use crate::{workflow::LocalAuthorityResult, ConvState, Database, DbError, DbResult, Message};
use chrono::{DateTime, Utc};
use phoenix_workflow::TurnCommand;
use sqlx::{Row, SqlitePool};

#[derive(Debug, PartialEq, Eq)]
pub enum QuestionCommitOutcome {
    Committed,
    Rejected,
    NotCommitted(String),
}

pub type QuestionCommitResult = LocalAuthorityResult<QuestionCommitOutcome>;

impl Database {
    /// Whether dismissal still requires a newly accepted explicit message.
    ///
    /// # Errors
    /// Returns an error if the authoritative pause row cannot be read.
    pub async fn question_dismissal_paused(&self, conversation_id: &str) -> DbResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM question_dismissal_pauses WHERE conversation_id = ?1)",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn establish_question_response(
        &self,
        conversation_id: &str,
        request_id: &str,
        message: &Message,
        completed_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> QuestionCommitResult {
        let result = self
            .commit_question_response(
                conversation_id,
                request_id,
                message,
                completed_state,
                state_updated_at,
            )
            .await;
        establish_question_commit(
            &self.pool,
            result,
            request_id,
            message,
            completed_state,
            state_updated_at,
            None,
        )
        .await
    }
}

pub(crate) async fn establish_question_commit(
    pool: &SqlitePool,
    result: DbResult<bool>,
    request_id: &str,
    message: &Message,
    completed_state: &ConvState,
    state_updated_at: DateTime<Utc>,
    terminal: Option<&TurnCommand>,
) -> QuestionCommitResult {
    match result {
        Ok(true) => LocalAuthorityResult::DurableFactEstablished(QuestionCommitOutcome::Committed),
        Ok(false) => LocalAuthorityResult::DurableFactEstablished(QuestionCommitOutcome::Rejected),
        Err(error) => {
            tracing::error!(%error, conversation_id = %message.conversation_id,
                "question commit returned no typed result; classifying once");
            match classify_question_commit(
                pool,
                request_id,
                message,
                completed_state,
                state_updated_at,
                terminal,
            )
            .await
            {
                Ok(true) => {
                    LocalAuthorityResult::DurableFactEstablished(QuestionCommitOutcome::Committed)
                }
                Ok(false) => LocalAuthorityResult::DurableFactEstablished(
                    QuestionCommitOutcome::NotCommitted(error.to_string()),
                ),
                Err(classification_error) => {
                    tracing::error!(%classification_error, "question commit durable fact remains unclassified");
                    LocalAuthorityResult::DurableFactUnclassified
                }
            }
        }
    }
}

async fn classify_question_commit(
    pool: &SqlitePool,
    request_id: &str,
    message: &Message,
    completed_state: &ConvState,
    state_updated_at: DateTime<Utc>,
    terminal: Option<&TurnCommand>,
) -> DbResult<bool> {
    let terminal = terminal
        .map(crate::workflow::direct_turn::terminal_command_parts)
        .transpose()?;
    let turn_id = terminal
        .as_ref()
        .map(|(id, _, _)| i64::try_from(id.0))
        .transpose()
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let generation = terminal
        .as_ref()
        .map(|(_, generation, _)| i64::try_from(*generation))
        .transpose()
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let terminal_sql = terminal
        .as_ref()
        .map(|(_, _, terminal)| crate::workflow::direct_turn::terminal_sql(terminal));
    let row = sqlx::query(
        "SELECT c.state, c.state_kind, c.state_updated_at, m.message_id,
          EXISTS(SELECT 1 FROM question_dismissal_pauses p WHERE p.conversation_id = c.id) AS paused,
          (m.conversation_id = ?1 AND m.sequence_id = ?3 AND m.content = ?4
           AND m.message_type = ?12 AND m.display_data IS ?5 AND m.usage_data IS ?6 AND m.created_at = ?7) AS exact_message,
          (?8 IS NULL OR (dt.conversation_id = ?1 AND dt.generation = ?9 + 1
           AND dt.owns_conversation = 0 AND dt.terminal_kind = ?10
           AND dt.terminal_reason IS ?11)) AS exact_terminal
         FROM conversations c LEFT JOIN messages m ON m.message_id = ?2
         LEFT JOIN durable_turns dt ON dt.turn_id = ?8 WHERE c.id = ?1",
    )
    .bind(&message.conversation_id)
    .bind(&message.message_id)
    .bind(message.sequence_id)
    .bind(
        serde_json::to_string(&message.content.to_stored_json())
            .map_err(|e| DbError::Serialization(e.to_string()))?,
    )
    .bind(
        message
            .display_data
            .as_ref()
            .map(serde_json::Value::to_string),
    )
    .bind(
        message
            .usage_data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| DbError::Serialization(e.to_string()))?,
    )
    .bind(message.created_at.to_rfc3339())
    .bind(turn_id)
    .bind(generation)
    .bind(terminal_sql.map(|(kind, _)| kind))
    .bind(terminal_sql.and_then(|(_, reason)| reason))
    .bind(message.message_type.to_string())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::ConversationNotFound(message.conversation_id.clone()))?;
    let state: ConvState = serde_json::from_str(row.try_get::<&str, _>("state")?)
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let has_message = row.try_get::<Option<String>, _>("message_id")?.is_some();
    if !has_message
        && matches!(&state, ConvState::AwaitingUserResponse { request_id: pending, .. } if pending == request_id)
    {
        return Ok(false);
    }
    if has_message
        && row.try_get::<Option<bool>, _>("exact_message")? == Some(true)
        && row.try_get::<Option<bool>, _>("exact_terminal")? == Some(true)
        && state == *completed_state
        && (!matches!(completed_state, ConvState::Idle) || row.try_get::<bool, _>("paused")?)
        && row.try_get::<String, _>("state_kind")? == crate::conv_state_kind(completed_state)
        && row.try_get::<String, _>("state_updated_at")? == state_updated_at.to_rfc3339()
    {
        return Ok(true);
    }
    Err(DbError::Serialization(
        "question commit evidence does not establish its exact outcome".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn question_commit_response_loss_classifies_exact_answer_and_dismissal() {
        for dismiss in [false, true] {
            let db = Database::open_in_memory().await.unwrap();
            db.create_conversation("question", "question", "/tmp", true, None, None)
                .await
                .unwrap();
            let pending = ConvState::AwaitingUserResponse {
                questions: vec![],
                tool_use_id: "pending".into(),

                request_id: "pending".into(),
            };
            db.update_conversation_state("question", &pending)
                .await
                .unwrap();
            let content = if dismiss {
                crate::MessageContent::system("[ask-user-question-dismissed]")
            } else {
                crate::MessageContent::user("Exact answer\nwith notes")
            };
            let now = Utc::now();
            let message = Message {
                message_id: "answer".into(),
                conversation_id: "question".into(),
                sequence_id: 1,
                message_type: content.message_type(),
                content,
                display_data: dismiss.then(|| serde_json::json!({"hidden": true})),
                usage_data: None,
                created_at: now,
            };
            let completed = if dismiss {
                ConvState::Idle
            } else {
                ConvState::LlmRequesting { attempt: 1 }
            };
            assert!(db
                .commit_question_response("question", "pending", &message, &completed, now)
                .await
                .unwrap());
            let lost_response = Err(DbError::Serialization(
                "injected postcommit response loss".into(),
            ));
            assert!(matches!(
                establish_question_commit(
                    &db.pool,
                    lost_response,
                    "pending",
                    &message,
                    &completed,
                    now,
                    None
                )
                .await,
                LocalAuthorityResult::DurableFactEstablished(QuestionCommitOutcome::Committed)
            ));
            assert_eq!(db.get_messages("question").await.unwrap().len(), 1);
            assert_eq!(
                db.get_conversation("question").await.unwrap().state,
                completed
            );
            assert!(matches!(
                db.establish_question_response("question", "pending", &message, &completed, now)
                    .await,
                LocalAuthorityResult::DurableFactEstablished(QuestionCommitOutcome::Rejected)
            ));
            let mut changed_message = message.clone();
            changed_message.content = crate::MessageContent::user("different answer");
            assert!(classify_question_commit(
                &db.pool,
                "pending",
                &changed_message,
                &completed,
                now,
                None
            )
            .await
            .is_err());
            db.pool.close().await;
            assert!(matches!(
                db.establish_question_response("question", "pending", &message, &completed, now)
                    .await,
                LocalAuthorityResult::DurableFactUnclassified
            ));
        }
    }
}
