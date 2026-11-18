//! REQ-AUQ-010, REQ-AUQ-011, REQ-AUQ-012: questions the Coordinator posts to the
//! user without stopping its own work.
//!
//! A question is open until the user answers or dismisses it, or the
//! Coordinator withdraws it. Answers and dismissals carry a reply that is
//! delivered back to the Coordinator once; withdrawals deliver nothing.

use phoenix_core::domain::sm_state::UserQuestion;
use sqlx::Row;

use crate::{Database, DbError, DbResult};

/// An open question as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorQuestion {
    pub request_id: String,
    pub coordinator_conversation_id: String,
    pub questions: Vec<UserQuestion>,
    pub asked_at_us: i64,
}

/// How an open question closes. Answers and dismissals carry the reply the
/// Coordinator receives; a withdrawal delivers nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorQuestionClose<'a> {
    Answered { reply: &'a str },
    Dismissed { reply: &'a str },
    Withdrawn,
}

impl<'a> CoordinatorQuestionClose<'a> {
    fn status_and_reply(self) -> (&'static str, Option<&'a str>) {
        match self {
            Self::Answered { reply } => ("answered", Some(reply)),
            Self::Dismissed { reply } => ("dismissed", Some(reply)),
            Self::Withdrawn => ("withdrawn", None),
        }
    }
}

/// Result of trying to settle or withdraw a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorQuestionSettlement {
    Settled,
    NotFound,
    /// Already answered, dismissed or withdrawn; carries the stored status.
    AlreadySettled {
        status: String,
    },
}

/// A reply the Coordinator has not received yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeliveredCoordinatorQuestionReply {
    pub request_id: String,
    pub reply: String,
}

fn decode_question(row: &sqlx::sqlite::SqliteRow) -> DbResult<CoordinatorQuestion> {
    let questions: String = row.try_get("questions")?;
    Ok(CoordinatorQuestion {
        request_id: row.try_get("request_id")?,
        coordinator_conversation_id: row.try_get("coordinator_conversation_id")?,
        questions: serde_json::from_str(&questions)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        asked_at_us: row.try_get("asked_at_us")?,
    })
}

impl Database {
    /// Record a question the Coordinator posted.
    ///
    /// # Errors
    /// Returns an error if the questions cannot be encoded or the insert fails.
    pub async fn post_coordinator_question(
        &self,
        request_id: &str,
        coordinator_conversation_id: &str,
        questions: &[UserQuestion],
        asked_at_us: i64,
    ) -> DbResult<()> {
        let encoded = serde_json::to_string(questions)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "INSERT INTO coordinator_user_questions
                (request_id, coordinator_conversation_id, questions, asked_at_us)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(request_id)
        .bind(coordinator_conversation_id)
        .bind(encoded)
        .bind(asked_at_us)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Open questions, oldest first.
    ///
    /// # Errors
    /// Returns an error if the read fails or a stored question cannot be decoded.
    pub async fn open_coordinator_questions(&self) -> DbResult<Vec<CoordinatorQuestion>> {
        let rows = sqlx::query(
            "SELECT request_id, coordinator_conversation_id, questions, asked_at_us
             FROM coordinator_user_questions WHERE status = 'open'
             ORDER BY asked_at_us, request_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_question).collect()
    }

    /// One open question by request id, or why it is not open.
    ///
    /// # Errors
    /// Returns an error if the read fails or the stored question cannot be decoded.
    pub async fn open_coordinator_question(
        &self,
        request_id: &str,
    ) -> DbResult<Result<CoordinatorQuestion, CoordinatorQuestionSettlement>> {
        let row = sqlx::query(
            "SELECT request_id, coordinator_conversation_id, questions, asked_at_us, status
             FROM coordinator_user_questions WHERE request_id = ?1",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(Err(CoordinatorQuestionSettlement::NotFound));
        };
        let status: String = row.try_get("status")?;
        if status != "open" {
            return Ok(Err(CoordinatorQuestionSettlement::AlreadySettled {
                status,
            }));
        }
        decode_question(&row).map(Ok)
    }

    /// Close an open question. Only the first close wins.
    ///
    /// # Errors
    /// Returns an error if a database operation fails.
    pub async fn settle_coordinator_question(
        &self,
        request_id: &str,
        close: CoordinatorQuestionClose<'_>,
        now_us: i64,
    ) -> DbResult<CoordinatorQuestionSettlement> {
        let (status, reply) = close.status_and_reply();
        let updated = sqlx::query(
            "UPDATE coordinator_user_questions
             SET status = ?2, reply = ?3, settled_at_us = ?4
             WHERE request_id = ?1 AND status = 'open'",
        )
        .bind(request_id)
        .bind(status)
        .bind(reply)
        .bind(now_us)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if updated > 0 {
            return Ok(CoordinatorQuestionSettlement::Settled);
        }
        Ok(self
            .open_coordinator_question(request_id)
            .await?
            .err()
            // An open question would have been updated, so the lookup says
            // why this one is not open.
            .unwrap_or(CoordinatorQuestionSettlement::NotFound))
    }

    /// Replies not yet delivered to the Coordinator, oldest first.
    ///
    /// # Errors
    /// Returns an error if the read fails.
    pub async fn undelivered_coordinator_question_replies(
        &self,
        limit: i64,
    ) -> DbResult<Vec<UndeliveredCoordinatorQuestionReply>> {
        let rows = sqlx::query(
            "SELECT request_id, reply FROM coordinator_user_questions
             WHERE status IN ('answered', 'dismissed') AND delivered_at_us IS NULL
             ORDER BY settled_at_us, request_id LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(UndeliveredCoordinatorQuestionReply {
                    request_id: row.try_get("request_id")?,
                    reply: row.try_get("reply")?,
                })
            })
            .collect()
    }

    /// Record that the Coordinator transcript accepted a reply.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub async fn mark_coordinator_question_reply_delivered(
        &self,
        request_id: &str,
        now_us: i64,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE coordinator_user_questions SET delivered_at_us = ?2
             WHERE request_id = ?1 AND delivered_at_us IS NULL",
        )
        .bind(request_id)
        .bind(now_us)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::sm_state::QuestionOption;

    fn questions() -> Vec<UserQuestion> {
        vec![UserQuestion {
            question: "Merge the cleanup PR?".into(),
            header: "Merge".into(),
            options: vec![
                QuestionOption {
                    label: "Yes".into(),
                    description: None,
                    preview: None,
                },
                QuestionOption {
                    label: "Not yet".into(),
                    description: None,
                    preview: None,
                },
            ],
            multi_select: false,
        }]
    }

    async fn coordinator(db: &Database) -> String {
        db.create_conversation("ember", "ember", "/tmp", true, None, None)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn a_question_stays_open_until_the_first_reply_and_is_delivered_once() {
        let db = Database::open_in_memory().await.unwrap();
        let ember = coordinator(&db).await;
        db.post_coordinator_question("q-1", &ember, &questions(), 10)
            .await
            .unwrap();
        db.post_coordinator_question("q-2", &ember, &questions(), 20)
            .await
            .unwrap();
        let open = db.open_coordinator_questions().await.unwrap();
        assert_eq!(
            open.iter()
                .map(|q| q.request_id.as_str())
                .collect::<Vec<_>>(),
            ["q-1", "q-2"]
        );
        assert_eq!(open[0].questions, questions());

        assert_eq!(
            db.settle_coordinator_question(
                "q-1",
                CoordinatorQuestionClose::Answered { reply: "Yes" },
                30
            )
            .await
            .unwrap(),
            CoordinatorQuestionSettlement::Settled
        );
        assert_eq!(
            db.settle_coordinator_question(
                "q-1",
                CoordinatorQuestionClose::Dismissed { reply: "late" },
                31
            )
            .await
            .unwrap(),
            CoordinatorQuestionSettlement::AlreadySettled {
                status: "answered".into()
            }
        );
        assert_eq!(
            db.settle_coordinator_question(
                "missing",
                CoordinatorQuestionClose::Answered { reply: "Yes" },
                31
            )
            .await
            .unwrap(),
            CoordinatorQuestionSettlement::NotFound
        );
        assert!(db.open_coordinator_question("q-1").await.unwrap().is_err());
        assert!(db.open_coordinator_question("q-2").await.unwrap().is_ok());

        let undelivered = db
            .undelivered_coordinator_question_replies(16)
            .await
            .unwrap();
        assert_eq!(
            undelivered,
            [UndeliveredCoordinatorQuestionReply {
                request_id: "q-1".into(),
                reply: "Yes".into()
            }]
        );
        db.mark_coordinator_question_reply_delivered("q-1", 40)
            .await
            .unwrap();
        assert!(db
            .undelivered_coordinator_question_replies(16)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_withdrawn_question_closes_without_a_reply() {
        let db = Database::open_in_memory().await.unwrap();
        let ember = coordinator(&db).await;
        db.post_coordinator_question("q-1", &ember, &questions(), 10)
            .await
            .unwrap();
        assert_eq!(
            db.settle_coordinator_question("q-1", CoordinatorQuestionClose::Withdrawn, 20)
                .await
                .unwrap(),
            CoordinatorQuestionSettlement::Settled
        );
        assert!(db.open_coordinator_questions().await.unwrap().is_empty());
        assert!(db
            .undelivered_coordinator_question_replies(16)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            db.settle_coordinator_question(
                "q-1",
                CoordinatorQuestionClose::Answered { reply: "Yes" },
                30
            )
            .await
            .unwrap(),
            CoordinatorQuestionSettlement::AlreadySettled {
                status: "withdrawn".into()
            }
        );
    }
}
