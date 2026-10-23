use phoenix_workflow::TurnAuthorityId;
use sqlx::{Connection, Row};

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

#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedStandaloneDrain {
    target: StandaloneStopTarget,
    projection: crate::workflow::UserRequestedDrainProjection,
}

impl AcceptedStandaloneDrain {
    #[must_use]
    pub fn target(&self) -> &StandaloneStopTarget {
        &self.target
    }

    #[must_use]
    pub fn projection(&self) -> &crate::workflow::PersistedConversationProjection {
        self.projection.projection()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StandaloneDrainAcceptance {
    Accepted(AcceptedStandaloneDrain),
    NotAccepted,
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
    pub async fn accept_standalone_stop_drain(
        &self,
        target: &StandaloneStopTarget,
        source: &crate::workflow::PersistedConversationProjection,
        projection: &crate::workflow::UserRequestedDrainProjection,
    ) -> DbResult<StandaloneDrainAcceptance> {
        let mut connection = self.pool.acquire().await?;
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        let Some(actual) = exact_standalone_projection_tx(&mut tx, target).await? else {
            return Ok(StandaloneDrainAcceptance::NotAccepted);
        };
        if !same_projection(&actual, source) && !same_projection(&actual, projection.projection()) {
            return Ok(StandaloneDrainAcceptance::NotAccepted);
        }
        let committed = projection.projection();
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3 WHERE id = ?4")
            .bind(serde_json::to_string(&committed.state).map_err(|error| DbError::Serialization(error.to_string()))?)
            .bind(crate::conv_state_kind(&committed.state))
            .bind(committed.state_updated_at.to_rfc3339())
            .bind(target.conversation_id())
            .execute(&mut *tx)
            .await?;
        let result = tx.commit().await;
        drop(connection);
        if let Err(original) = result {
            let mut connection = self.pool.acquire().await?;
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let established = exact_standalone_projection_tx(&mut tx, target)
                .await?
                .is_some_and(|actual| same_projection(&actual, projection.projection()));
            tx.commit().await?;
            if !established {
                return Err(DbError::Sqlx(original));
            }
        }
        Ok(StandaloneDrainAcceptance::Accepted(
            AcceptedStandaloneDrain {
                target: target.clone(),
                projection: projection.clone(),
            },
        ))
    }

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

async fn exact_standalone_projection_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: &StandaloneStopTarget,
) -> DbResult<Option<crate::workflow::PersistedConversationProjection>> {
    let row = sqlx::query(
        "SELECT c.state, c.state_updated_at FROM conversations c
         JOIN steering_execution_occurrences o ON o.conversation_id = c.id
         WHERE c.id = ?1 AND o.message_id = ?2 AND o.source_kind = ?3
           AND NOT EXISTS (SELECT 1 FROM durable_turns t
               WHERE t.conversation_id = c.id AND t.owns_conversation = 1 AND t.terminal_kind IS NULL)",
    )
    .bind(target.conversation_id())
    .bind(target.message_id())
    .bind(target.source().as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(crate::workflow::PersistedConversationProjection {
            state: serde_json::from_str(row.get::<&str, _>("state"))
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            state_updated_at: chrono::DateTime::parse_from_rfc3339(
                row.get::<&str, _>("state_updated_at"),
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?
            .with_timezone(&chrono::Utc),
        })
    })
    .transpose()
}

fn same_projection(
    left: &crate::workflow::PersistedConversationProjection,
    right: &crate::workflow::PersistedConversationProjection,
) -> bool {
    left.state == right.state
        && left.state_updated_at.timestamp_micros() == right.state_updated_at.timestamp_micros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standalone_drain_acceptance_fences_same_message_source_replacement() {
        use crate::workflow::{PersistedConversationProjection, UserRequestedDrainProjection};
        use phoenix_core::domain::{
            db_schema::MessageContent, sm_event::CancelCause, sm_state::ConvState,
        };
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("drain", "drain", "/tmp", true, None, None)
            .await
            .unwrap();
        db.watch_product_conversation(&conversation.product_conversation_id)
            .await
            .unwrap();
        db.add_message_with_seq(
            "input",
            "drain",
            1,
            &MessageContent::user("input"),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO steering_execution_occurrences (conversation_id, message_id, source_kind) VALUES ('drain', 'input', 'steering')")
            .execute(db.pool()).await.unwrap();
        let CapturedStopTarget::Standalone(old_target) =
            db.capture_stop_target("drain").await.unwrap()
        else {
            panic!("expected occurrence");
        };
        let source = PersistedConversationProjection {
            state: ConvState::CancellingSubAgents {
                pending: vec![],
                completed_results: vec![],
                cause: CancelCause::Timeout,
                spawn_tool_id: None,
            },
            state_updated_at: chrono::DateTime::from_timestamp_micros(4).unwrap(),
        };
        db.update_state_and_replay(
            "drain",
            &source.state,
            source.state_updated_at,
            &phoenix_core::domain::provider_replay::ProviderReplayUpdate::Clear,
        )
        .await
        .unwrap();
        let target_projection =
            UserRequestedDrainProjection::new(PersistedConversationProjection {
                state: ConvState::CancellingSubAgents {
                    pending: vec![],
                    completed_results: vec![],
                    cause: CancelCause::UserRequested,
                    spawn_tool_id: None,
                },
                state_updated_at: chrono::DateTime::from_timestamp_micros(5).unwrap(),
            })
            .unwrap();
        sqlx::query("UPDATE steering_execution_occurrences SET source_kind = 'interaction_response' WHERE conversation_id = 'drain'")
            .execute(db.pool()).await.unwrap();
        assert!(matches!(
            db.accept_standalone_stop_drain(&old_target, &source, &target_projection)
                .await
                .unwrap(),
            StandaloneDrainAcceptance::NotAccepted
        ));
        let unchanged = db.get_conversation("drain").await.unwrap();
        assert_eq!(unchanged.state, source.state);
        assert_eq!(
            unchanged.state_updated_at.timestamp_micros(),
            source.state_updated_at.timestamp_micros()
        );
        let CapturedStopTarget::Standalone(target) = db.capture_stop_target("drain").await.unwrap()
        else {
            panic!("expected replacement occurrence");
        };
        assert_ne!(old_target, target);
        let mut stale_source = source.clone();
        stale_source.state_updated_at = chrono::DateTime::from_timestamp_micros(3).unwrap();
        assert!(matches!(
            db.accept_standalone_stop_drain(&target, &stale_source, &target_projection)
                .await
                .unwrap(),
            StandaloneDrainAcceptance::NotAccepted
        ));
        let StandaloneDrainAcceptance::Accepted(receipt) = db
            .accept_standalone_stop_drain(&target, &source, &target_projection)
            .await
            .unwrap()
        else {
            panic!("expected accepted drain");
        };
        assert_eq!(receipt.target(), &target);
        assert_eq!(receipt.projection(), target_projection.projection());
        let durable = db.get_conversation("drain").await.unwrap();
        assert_eq!(durable.state, target_projection.projection().state);
        assert_eq!(
            durable.state_updated_at.timestamp_micros(),
            target_projection
                .projection()
                .state_updated_at
                .timestamp_micros()
        );
        assert!(matches!(
            db.accept_standalone_stop_drain(&target, &source, &target_projection)
                .await
                .unwrap(),
            StandaloneDrainAcceptance::Accepted(_)
        ));
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            0
        );
        let occurrence: (String, String) = sqlx::query_as("SELECT message_id, source_kind FROM steering_execution_occurrences WHERE conversation_id = 'drain'")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(occurrence, ("input".into(), "interaction_response".into()));
        let markers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_cancel_observations WHERE conversation_id = 'drain'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(markers, 0);
    }

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
