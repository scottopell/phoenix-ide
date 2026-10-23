use phoenix_workflow::TurnAuthorityId;
use sqlx::Row;

use crate::{Database, DbError, DbResult, MessageExecutionSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneStopTarget {
    conversation_id: String,
    message_id: String,
    source: MessageExecutionSource,
}

impl StandaloneStopTarget {
    #[must_use]
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    #[must_use]
    pub fn message_id(&self) -> &str {
        &self.message_id
    }

    #[must_use]
    pub fn source(&self) -> MessageExecutionSource {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturedStopTarget {
    DirectTurn {
        turn_id: TurnAuthorityId,
        generation: u64,
    },
    Standalone(StandaloneStopTarget),
    Boundary,
}

impl Database {
    pub async fn capture_stop_target(&self, conversation_id: &str) -> DbResult<CapturedStopTarget> {
        let mut tx = self.pool.begin().await?;
        let owner = sqlx::query(
            "SELECT turn_id, generation, disposition FROM durable_turns
             WHERE conversation_id = ?1 AND owns_conversation = 1 AND terminal_kind IS NULL
             ORDER BY turn_id DESC LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await?;
        let target = if let Some(owner) = owner {
            if owner.get::<&str, _>("disposition") == "Runtime" {
                CapturedStopTarget::DirectTurn {
                    turn_id: TurnAuthorityId(
                        u64::try_from(owner.get::<i64, _>("turn_id"))
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                    ),
                    generation: u64::try_from(owner.get::<i64, _>("generation"))
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                }
            } else {
                CapturedStopTarget::Boundary
            }
        } else {
            let occurrence = sqlx::query(
                "SELECT message_id, source_kind FROM steering_execution_occurrences
                 WHERE conversation_id = ?1
                   AND source_kind IN ('steering', 'seeded_fork', 'interaction_response')",
            )
            .bind(conversation_id)
            .fetch_optional(&mut *tx)
            .await?;
            match occurrence {
                Some(row) => CapturedStopTarget::Standalone(StandaloneStopTarget {
                    conversation_id: conversation_id.to_owned(),
                    message_id: row.get("message_id"),
                    source: MessageExecutionSource::from_db(row.get("source_kind"))?,
                }),
                None => CapturedStopTarget::Boundary,
            }
        };
        tx.commit().await?;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_snapshot_captures_only_supported_exact_occurrence_sources() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("standalone", "Stop", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message_with_seq(
            "exact-input",
            "standalone",
            1,
            &phoenix_core::domain::db_schema::MessageContent::User(
                phoenix_core::domain::db_schema::UserContent::new("input"),
            ),
            None,
            None,
        )
        .await
        .unwrap();
        for source in ["steering", "seeded_fork", "interaction_response"] {
            sqlx::query("INSERT INTO steering_execution_occurrences (conversation_id, message_id, source_kind) VALUES ('standalone', 'exact-input', ?1) ON CONFLICT(conversation_id) DO UPDATE SET source_kind = excluded.source_kind")
                .bind(source).execute(db.pool()).await.unwrap();
            let CapturedStopTarget::Standalone(target) =
                db.capture_stop_target("standalone").await.unwrap()
            else {
                panic!("supported occurrence must be captured");
            };
            assert_eq!(target.conversation_id(), "standalone");
            assert_eq!(target.message_id(), "exact-input");
            assert_eq!(
                target.source(),
                MessageExecutionSource::from_db(source).unwrap()
            );
        }
        sqlx::query("UPDATE steering_execution_occurrences SET source_kind = 'wake' WHERE conversation_id = 'standalone'")
            .execute(db.pool()).await.unwrap();
        assert_eq!(
            db.capture_stop_target("standalone").await.unwrap(),
            CapturedStopTarget::Boundary
        );
        assert_eq!(
            db.capture_stop_target("missing").await.unwrap(),
            CapturedStopTarget::Boundary
        );
    }
}
