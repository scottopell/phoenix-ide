use std::collections::HashMap;

use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{QueryBuilder, Row, Sqlite};

use super::WatchOutcome;
use crate::{Database, DbError, DbResult};

/// The event's routing provenance, not its delivery payload or delivery eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEventProvenance {
    Subscription,
    MandatoryCloseFailure,
}

#[derive(Debug, Clone)]
pub struct WatchEventPresentation {
    pub event_id: String,
    pub product_conversation_id: ProductConversationId,
    /// Current root-transcript display name; None means source metadata is unavailable.
    pub display_name: Option<String>,
    pub source_transcript_id: String,
    pub source_occurrence_kind: String,
    pub source_occurrence_id: String,
    pub source_generation: i64,
    pub outcome: WatchOutcome,
    pub occurred_at_us: i64,
    pub provenance: WatchEventProvenance,
    pub goal_capture: Option<phoenix_core::domain::watch_goal::WatchGoalCapture>,
}

const EVENT_IDS_PER_QUERY: usize = 400;

impl Database {
    /// Read presentation facts for exact durable event IDs, across all delivery states.
    /// Missing event IDs are absent from the result; missing source metadata is None.
    ///
    /// # Errors
    /// Returns an error if a query fails or persisted event facts cannot be decoded.
    pub async fn coordinator_watch_event_presentations(
        &self,
        event_ids: &[String],
    ) -> DbResult<HashMap<String, WatchEventPresentation>> {
        let mut events = HashMap::new();
        for ids in event_ids.chunks(EVENT_IDS_PER_QUERY) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT e.event_id, e.route_kind,
                    COALESCE(w.source_product_conversation_id, e.mandatory_source_product_id) AS product_id,
                    e.source_transcript_id, e.source_occurrence_kind, e.source_occurrence_id,
                    e.source_generation, e.terminal_kind, e.terminal_reason, e.occurred_at_us,
                    e.goal_capture_kind, gc.kind AS capture_kind, gc.owner_id AS capture_owner_id,
                    gc.owner_version AS capture_owner_version, gc.goal_id AS capture_goal_id,
                    gc.objective AS capture_objective, gc.state AS capture_state,
                    gc.reason AS capture_reason, gc.usage_kind AS capture_usage_kind,
                    (SELECT COALESCE(NULLIF(root.title, ''),
                                     NULLIF(root.slug, ''), 'Untitled conversation')
                     FROM conversations root
                     WHERE root.product_conversation_id = p.id AND root.runtime_role = 'user'
                       AND root.parent_conversation_id IS NULL
                       AND NOT EXISTS (SELECT 1 FROM conversations predecessor
                           WHERE predecessor.product_conversation_id = root.product_conversation_id
                             AND predecessor.continued_in_conv_id = root.id)
                     ORDER BY root.id LIMIT 1) AS display_name
                 FROM coordinator_watch_events e
                 LEFT JOIN coordinator_watch_goal_captures gc ON gc.event_id=e.event_id
                 LEFT JOIN coordinator_watches w ON w.id = e.watch_id
                 LEFT JOIN product_conversations p
                   ON p.id = COALESCE(w.source_product_conversation_id, e.mandatory_source_product_id)
                 WHERE e.event_id IN (",
            );
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            for row in query.build().fetch_all(self.pool()).await? {
                let provenance = match row.try_get::<&str, _>("route_kind")? {
                    "subscription" => WatchEventProvenance::Subscription,
                    "mandatory_close_failure" => WatchEventProvenance::MandatoryCloseFailure,
                    _ => return Err(DbError::Serialization("invalid watch event route".into())),
                };
                let event = WatchEventPresentation {
                    event_id: row.try_get("event_id")?,
                    product_conversation_id: ProductConversationId::parse(
                        row.try_get::<String, _>("product_id")?,
                    )
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                    display_name: row.try_get("display_name")?,
                    source_transcript_id: row.try_get("source_transcript_id")?,
                    source_occurrence_kind: row.try_get("source_occurrence_kind")?,
                    source_occurrence_id: row.try_get("source_occurrence_id")?,
                    source_generation: row.try_get("source_generation")?,
                    outcome: WatchOutcome::decode(
                        row.try_get("terminal_kind")?,
                        row.try_get("terminal_reason")?,
                    )?,
                    occurred_at_us: row.try_get("occurred_at_us")?,
                    provenance,
                    goal_capture: super::goal_facts::decode(&row)?,
                };
                events.insert(event.event_id.clone(), event);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator_watches::record_watch_event_tx;

    async fn source(db: &Database) -> (String, ProductConversationId) {
        let source = db
            .create_conversation(
                "presentation-source",
                "presentation-source",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET title = 'Source title' WHERE id = ?1")
            .bind(&source.id)
            .execute(db.pool())
            .await
            .unwrap();
        (source.id, source.product_conversation_id)
    }

    async fn completed_event(db: &Database, transcript_id: &str, occurrence: &str) -> String {
        let mut tx = db.pool().begin().await.unwrap();
        record_watch_event_tx(
            &mut tx,
            "creation",
            occurrence,
            7,
            transcript_id,
            "Completed",
            None,
            false,
            crate::conversation_goals::control::GoalControlInstallation::Disabled,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sqlx::query_scalar(
            "SELECT event_id FROM coordinator_watch_events WHERE source_occurrence_id = ?1",
        )
        .bind(occurrence)
        .fetch_one(db.pool())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn presentation_survives_delivery_unwatch_and_history_without_reinterpreting_outcome() {
        let db = Database::open_in_memory().await.unwrap();
        let (transcript, product) = source(&db).await;
        let accepted = completed_event(&db, &transcript, "accepted-occurrence").await;
        let suppressed = completed_event(&db, &transcript, "suppressed-occurrence").await;
        let ids = vec![accepted.clone(), suppressed.clone(), "missing-event".into()];
        let pending = db
            .coordinator_watch_event_presentations(&ids)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
        sqlx::query("UPDATE coordinator_watch_events SET delivery_state = 'accepted', accepted_transcript_id = ?2 WHERE event_id = ?1")
            .bind(&accepted).bind(&transcript).execute(db.pool()).await.unwrap();
        db.unwatch_product_conversation(&product).await.unwrap();
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1",
        )
        .bind(product.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        let historical = db
            .coordinator_watch_event_presentations(&ids)
            .await
            .unwrap();
        assert_eq!(historical.len(), 2);
        for id in [&accepted, &suppressed] {
            let event = &historical[id];
            assert_eq!(event.event_id, *id);
            assert_eq!(event.product_conversation_id, product);
            assert_eq!(event.display_name.as_deref(), Some("Source title"));
            assert_eq!(event.source_transcript_id, transcript);
            assert_eq!(event.source_occurrence_kind, "creation");
            assert_eq!(event.source_occurrence_id, pending[id].source_occurrence_id);
            assert_eq!(event.source_generation, 7);
            assert_eq!(event.outcome, WatchOutcome::Completed);
            assert_eq!(event.occurred_at_us, pending[id].occurred_at_us);
            assert_eq!(event.provenance, WatchEventProvenance::Subscription);
        }
    }

    #[tokio::test]
    async fn missing_product_metadata_preserves_event_identity() {
        let db = Database::open_in_memory().await.unwrap();
        let (transcript, product) = source(&db).await;
        let id = completed_event(&db, &transcript, "removed-source").await;
        db.unwatch_product_conversation(&product).await.unwrap();
        sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
            .bind(product.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        let events = db
            .coordinator_watch_event_presentations(std::slice::from_ref(&id))
            .await
            .unwrap();
        let event = &events[&id];
        assert_eq!(event.display_name, None);
        assert_eq!(event.product_conversation_id, product);
        assert_eq!(event.source_transcript_id, transcript);
        assert_eq!(event.outcome, WatchOutcome::Completed);
    }

    #[tokio::test]
    async fn projection_preserves_failure_reason_and_cancelled_fact_without_writing() {
        let db = Database::open_in_memory().await.unwrap();
        let (transcript, _) = source(&db).await;
        let mut tx = db.pool().begin().await.unwrap();
        for (occurrence, kind, reason) in [
            ("failed-occurrence", "Failed", Some("exact durable reason")),
            ("cancelled-occurrence", "Cancelled", None),
        ] {
            record_watch_event_tx(
                &mut tx,
                "creation",
                occurrence,
                0,
                &transcript,
                kind,
                reason,
                false,
                crate::conversation_goals::control::GoalControlInstallation::Disabled,
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        let before: Vec<(String, String, Option<String>, String, i64)> = sqlx::query_as(
            "SELECT event_id, terminal_kind, terminal_reason, delivery_state, occurred_at_us
             FROM coordinator_watch_events ORDER BY event_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        let ids: Vec<String> = before.iter().map(|row| row.0.clone()).collect();
        let events = db
            .coordinator_watch_event_presentations(&ids)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        for event in events.values() {
            match event.source_occurrence_id.as_str() {
                "failed-occurrence" => assert_eq!(
                    event.outcome,
                    WatchOutcome::Failed {
                        reason: "exact durable reason".into(),
                    }
                ),
                "cancelled-occurrence" => assert_eq!(event.outcome, WatchOutcome::Cancelled),
                other => panic!("unexpected occurrence {other}"),
            }
        }
        let after: Vec<(String, String, Option<String>, String, i64)> = sqlx::query_as(
            "SELECT event_id, terminal_kind, terminal_reason, delivery_state, occurred_at_us
             FROM coordinator_watch_events ORDER BY event_id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn lookup_chunks_ids_and_never_returns_unrequested_events() {
        let db = Database::open_in_memory().await.unwrap();
        let (transcript, _) = source(&db).await;
        let mut ids = Vec::new();
        for n in 0..=EVENT_IDS_PER_QUERY {
            ids.push(completed_event(&db, &transcript, &format!("occurrence-{n}")).await);
        }
        let unrelated = completed_event(&db, &transcript, "unrequested").await;
        ids.push(ids[0].clone());
        ids.push("missing".into());
        let events = db
            .coordinator_watch_event_presentations(&ids)
            .await
            .unwrap();
        assert_eq!(events.len(), EVENT_IDS_PER_QUERY + 1);
        assert!(!events.contains_key(&unrelated));
        assert!(!events.contains_key("missing"));
        for id in &ids[..=EVENT_IDS_PER_QUERY] {
            assert!(events.contains_key(id));
        }
        assert!(db
            .coordinator_watch_event_presentations(&[])
            .await
            .unwrap()
            .is_empty());
    }
}
