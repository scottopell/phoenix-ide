use chrono::Utc;
use phoenix_core::domain::product_conversation::ProductConversationId;
use serde::Serialize;
use sqlx::{Row, Sqlite, Transaction};

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, Serialize)]
pub struct WatchSnapshot {
    pub product_conversation_id: ProductConversationId,
    pub current_transcript_id: String,
    pub current_state: phoenix_core::domain::sm_state::ConvState,
    pub enrolled_at: String,
}

#[derive(Debug, Clone)]
pub struct PendingWatchEvent {
    pub event_id: String,
    pub product_conversation_id: ProductConversationId,
    pub source_transcript_id: String,
    pub source_turn_id: i64,
    pub source_generation: i64,
    pub terminal_kind: String,
    pub terminal_reason: Option<String>,
    pub occurred_at: String,
}

fn decode_snapshot(row: &sqlx::sqlite::SqliteRow) -> DbResult<WatchSnapshot> {
    let id: String = row.try_get("source_product_conversation_id")?;
    let state: String = row.try_get("state")?;
    Ok(WatchSnapshot {
        product_conversation_id: ProductConversationId::parse(id)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        current_transcript_id: row.try_get("transcript_id")?,
        current_state: serde_json::from_str(&state)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        enrolled_at: row.try_get("enrolled_at")?,
    })
}

const WATCH_SNAPSHOT: &str = "SELECT w.source_product_conversation_id, w.enrolled_at,
    c.id AS transcript_id, c.state
    FROM coordinator_watches w
    JOIN product_conversations p ON p.id = w.source_product_conversation_id
    JOIN conversations c ON c.product_conversation_id = p.id
      AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
    WHERE w.ended_at IS NULL AND p.ordinary_lifecycle = 'open'
      AND (?1 IS NULL OR w.source_product_conversation_id = ?1)
    ORDER BY w.enrolled_at, w.id";

impl Database {
    /// Enroll an open ordinary product conversation and return its current snapshot.
    ///
    /// # Errors
    /// Returns an error if the product conversation is unavailable, a database operation
    /// fails, or the persisted snapshot cannot be decoded.
    pub async fn watch_product_conversation(
        &self,
        product_id: &ProductConversationId,
    ) -> DbResult<WatchSnapshot> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let eligible: bool = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM product_conversations p
             WHERE p.id = ?1 AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open')",
        )
        .bind(product_id.as_str())
        .fetch_one(&mut *tx)
        .await?
            != 0;
        if !eligible {
            return Err(DbError::ProductConversationUnavailable(product_id.clone()));
        }
        sqlx::query(
            "INSERT INTO coordinator_watches(source_product_conversation_id, enrolled_at)
                     VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        )
        .bind(product_id.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(WATCH_SNAPSHOT)
            .bind(product_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
        let snapshot = decode_snapshot(&row)?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// End an active watch, suppressing its undelivered events.
    ///
    /// # Errors
    /// Returns an error if a database operation fails.
    pub async fn unwatch_product_conversation(
        &self,
        product_id: &ProductConversationId,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let ended = sqlx::query(
            "UPDATE coordinator_watches SET ended_at = ?2
                                   WHERE source_product_conversation_id = ?1 AND ended_at IS NULL",
        )
        .bind(product_id.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 0;
        sqlx::query("UPDATE coordinator_watch_events SET delivery_state = 'suppressed'
                     WHERE delivery_state = 'pending' AND watch_id IN
                       (SELECT id FROM coordinator_watches WHERE source_product_conversation_id = ?1 AND ended_at IS NOT NULL)")
            .bind(product_id.as_str()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(ended)
    }

    /// List active watches with their current transcript snapshots.
    ///
    /// # Errors
    /// Returns an error if a database operation fails or a persisted snapshot cannot be decoded.
    pub async fn list_coordinator_watches(&self) -> DbResult<Vec<WatchSnapshot>> {
        let rows = sqlx::query(WATCH_SNAPSHOT)
            .bind(Option::<&str>::None)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(decode_snapshot).collect()
    }

    /// List eligible undelivered watch events up to `limit`.
    ///
    /// # Errors
    /// Returns an error if a database operation fails or a persisted product ID is invalid.
    pub async fn pending_coordinator_watch_events(
        &self,
        limit: i64,
    ) -> DbResult<Vec<PendingWatchEvent>> {
        let rows = sqlx::query("SELECT e.event_id, w.source_product_conversation_id,
                  e.source_transcript_id, e.source_turn_id, e.source_generation, e.terminal_kind, e.terminal_reason, e.occurred_at
             FROM coordinator_watch_events e JOIN coordinator_watches w ON w.id = e.watch_id
             JOIN product_conversations p ON p.id = w.source_product_conversation_id
             WHERE e.delivery_state = 'pending' AND e.continuation_state = 'none' AND w.ended_at IS NULL
               AND p.ordinary_lifecycle = 'open'
               AND NOT EXISTS (SELECT 1 FROM close_obligations o
                               WHERE o.product_conversation_id = p.id AND o.phase != 'completed')
             ORDER BY e.occurred_at, e.event_id LIMIT ?1")
            .bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                let product_id: String = row.try_get("source_product_conversation_id")?;
                Ok(PendingWatchEvent {
                    event_id: row.try_get("event_id")?,
                    product_conversation_id: ProductConversationId::parse(product_id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    source_transcript_id: row.try_get("source_transcript_id")?,
                    source_turn_id: row.try_get("source_turn_id")?,
                    source_generation: row.try_get("source_generation")?,
                    terminal_kind: row.try_get("terminal_kind")?,
                    terminal_reason: row.try_get("terminal_reason")?,
                    occurred_at: row.try_get("occurred_at")?,
                })
            })
            .collect()
    }

    /// Find the active coordinator transcript that receives watch events.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub async fn coordinator_watch_target(&self) -> DbResult<Option<String>> {
        sqlx::query_scalar("SELECT c.id FROM product_conversations p
                 JOIN conversations head ON head.product_conversation_id = p.id AND head.coordinator_head = 1
                 JOIN conversations c ON c.product_conversation_id = p.id
                    AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
                 WHERE p.kind = 'coordinator'")
            .fetch_optional(&self.pool).await.map_err(DbError::Sqlx)
    }
}

pub(crate) async fn record_terminal_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    turn_id: u64,
    generation: u64,
    transcript_id: &str,
    terminal_kind: &str,
    reason: Option<&str>,
    context_exhausted: bool,
) -> DbResult<()> {
    let category = match terminal_kind {
        "Completed" => "completed",
        "Failed" => "failed",
        "Cancelled" => "cancelled",
        _ => return Ok(()),
    };
    if category == "completed" && context_exhausted {
        return Ok(());
    }
    sqlx::query("INSERT INTO coordinator_watch_events
        (event_id, watch_id, source_turn_id, source_generation, source_transcript_id,
         terminal_kind, terminal_reason, occurred_at, continuation_state)
        SELECT ?1, w.id, ?2, ?3, c.id, ?4, ?5, ?6,
          CASE WHEN ?8 AND ?4 = 'failed' AND p.auto_continue_on_context_exhaustion = 1
               THEN 'awaiting' ELSE 'none' END
        FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
          JOIN coordinator_watches w ON w.source_product_conversation_id = p.id AND w.ended_at IS NULL
        WHERE c.id = ?7 AND c.parent_conversation_id IS NULL AND p.kind = 'ordinary'
          AND p.ordinary_lifecycle = 'open'
          AND (?4 != 'completed' OR c.state_kind IN ('idle', 'terminal'))
          AND c.state_kind NOT IN ('awaiting_continuation', 'handed_off')
          AND NOT EXISTS (SELECT 1 FROM close_obligations o
                          WHERE o.product_conversation_id = p.id AND o.phase != 'completed')
        ON CONFLICT(source_turn_id, source_generation, watch_id) DO NOTHING")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(i64::try_from(turn_id).map_err(|_| DbError::Serialization("turn id overflow".into()))?)
        .bind(i64::try_from(generation).map_err(|_| DbError::Serialization("generation overflow".into()))?)
        .bind(category).bind(reason.or((category == "failed").then_some("turn failed")))
        .bind(Utc::now().to_rfc3339())
        .bind(transcript_id).bind(context_exhausted).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::AcceptAuthoritativeTurn;
    use phoenix_core::domain::db_schema::InputOrigin;
    use phoenix_core::domain::sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    };
    use phoenix_workflow::{
        AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
        TurnOutcome,
    };

    async fn source_turn(db: &Database, conversation: &str, key: &str) -> u64 {
        let payload = PreparedDirectTurnPayload::from_parts(
            SubmittedDirectTurnIdentity {
                message_id: key.into(),
                origin: InputOrigin::UserApi,
                text: "source input".into(),
                images: vec![],
                files: vec![],
                user_agent: None,
                skill_invocation: None,
                expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
            },
            PreparedDirectTurnDelivery {
                text: "source input".into(),
                llm_text: None,
                images: vec![],
                files: vec![],
                user_agent: None,
                skill_invocation: None,
            },
        );
        let result = db
            .workflow_repository()
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(key).unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority(conversation.into()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = result.outcome else {
            panic!("turn was not created")
        };
        turn_id.0
    }

    #[tokio::test]
    async fn enrollment_is_future_only_and_unwatch_suppresses_unaccepted_events() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let id = source.product_conversation_id.clone();
        let historical = source_turn(&db, &source.id, "historical").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(&mut tx, historical, 0, &source.id, "Completed", None, false)
            .await
            .unwrap();
        sqlx::query("UPDATE durable_turns SET generation = 1, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = ?1")
            .bind(i64::try_from(historical).unwrap()).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let snapshot = db.watch_product_conversation(&id).await.unwrap();
        assert_eq!(snapshot.current_transcript_id, source.id);
        assert_eq!(db.list_coordinator_watches().await.unwrap().len(), 1);
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        let turn = source_turn(&db, &source.id, "current").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("model failed"),
            false,
        )
        .await
        .unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("model failed"),
            false,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let events = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].terminal_reason.as_deref(), Some("model failed"));
        assert!(db.unwatch_product_conversation(&id).await.unwrap());
        assert!(!db.unwatch_product_conversation(&id).await.unwrap());
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        assert!(db.list_coordinator_watches().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn context_exhaustion_releases_original_failure_or_suppresses_on_handoff() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-auto", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = source.product_conversation_id.clone();
        db.set_auto_continue_on_context_exhaustion(
            &source.product_conversation_id,
            phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .unwrap();
        db.watch_product_conversation(&product).await.unwrap();
        let turn = source_turn(&db, &source.id, "exhausted").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("context exhausted"),
            true,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        let (id, kind, state): (String, String, String) = sqlx::query_as(
            "SELECT event_id, terminal_kind, continuation_state FROM coordinator_watch_events",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((kind.as_str(), state.as_str()), ("failed", "awaiting"));
        db.update_conversation_state(
            &source.id,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "source summary".into(),
            },
        )
        .await
        .unwrap();
        let summary = db
            .add_message(
                "watch-auto-summary",
                &source.id,
                &crate::MessageContent::continuation("source summary"),
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO automatic_continuation_admissions
            (predecessor_conversation_id, product_conversation_id, summary_message_id,
             operation_id, first_message_id, admitted_at_unix_micros, updated_at_unix_micros)
            VALUES (?1, ?2, ?3, 'watch-operation', 'watch-auto-first', 1, 1)",
        )
        .bind(&source.id)
        .bind(product.as_str())
        .bind(summary.message_id)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        // Failure of the admitted continuation releases the original source occurrence.
        sqlx::query("UPDATE automatic_continuation_admissions SET phase = 'failed', last_error = 'continuation stopped'
                    WHERE predecessor_conversation_id = ?1")
            .bind(&source.id).execute(db.pool()).await.unwrap();
        let delivered = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].event_id, id);
        assert_eq!(delivered[0].source_turn_id, i64::try_from(turn).unwrap());
        assert_eq!(delivered[0].source_generation, 0);
        assert_eq!(delivered[0].terminal_kind, "failed");
        assert_eq!(
            delivered[0].terminal_reason.as_deref(),
            Some("context exhausted")
        );
        assert_eq!(delivered[0].source_transcript_id, source.id);
    }

    #[tokio::test]
    async fn successful_auto_handoff_suppresses_original_failure() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-success", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = source.product_conversation_id.clone();
        db.set_auto_continue_on_context_exhaustion(
            &product,
            phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .unwrap();
        db.watch_product_conversation(&product).await.unwrap();
        let turn = source_turn(&db, &source.id, "exhausted-success").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(&mut tx, turn, 0, &source.id, "Completed", None, true)
            .await
            .unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("context exhausted"),
            true,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE durable_turns SET generation = 1, terminal_kind = 'Failed', terminal_reason = 'context exhausted', owns_conversation = 0 WHERE turn_id = ?1")
            .bind(i64::try_from(turn).unwrap()).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        db.update_conversation_state(
            &source.id,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "source summary".into(),
            },
        )
        .await
        .unwrap();
        let summary = db
            .add_message(
                "watch-success-summary",
                &source.id,
                &crate::MessageContent::continuation("source summary"),
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO automatic_continuation_admissions
            (predecessor_conversation_id, product_conversation_id, summary_message_id,
             operation_id, first_message_id, admitted_at_unix_micros, updated_at_unix_micros)
            VALUES (?1, ?2, ?3, 'watch-operation', 'watch-success-first', 1, 1)",
        )
        .bind(&source.id)
        .bind(product.as_str())
        .bind(summary.message_id)
        .execute(db.pool())
        .await
        .unwrap();
        let (outcome, _) = db
            .continue_conversation_with_intent(
                &source.id,
                crate::NewContinuationDispatchIntent::generated_predecessor_context(
                    ClientTurnKey::new("watch-success-first").unwrap(),
                    "source summary".into(),
                ),
            )
            .await
            .unwrap();
        let successor = expect_created(outcome);
        db.add_message(
            "watch-success-first",
            &successor.id,
            &crate::MessageContent::continuation("source summary"),
            None,
            None,
        )
        .await
        .unwrap();
        let (state, continuation): (String, String) = sqlx::query_as(
            "SELECT delivery_state, continuation_state FROM coordinator_watch_events",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            (state.as_str(), continuation.as_str()),
            ("suppressed", "suppressed")
        );
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
    }

    fn expect_created(outcome: crate::ContinueOutcome) -> crate::Conversation {
        match outcome {
            crate::ContinueOutcome::Created(conversation) => conversation,
            other @ (crate::ContinueOutcome::AlreadyContinued(_)
            | crate::ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected successor, got {other:?}")
            }
        }
    }

    async fn cancel_close_attempt(db: &Database) {
        for phase in [
            "awaiting_stop_work_confirmation",
            "settling_active_work",
            "cancel_requested_during_settlement",
        ] {
            sqlx::query(
                "UPDATE close_obligations SET phase = ?1 WHERE attempt_id = 'watch-close-attempt'",
            )
            .bind(phase)
            .execute(db.pool())
            .await
            .unwrap();
        }
        sqlx::query("UPDATE close_obligations SET phase = 'completed', close_outcome = 'cancelled', completed_at = ?1 WHERE attempt_id = 'watch-close-attempt'")
            .bind(Utc::now().to_rfc3339()).execute(db.pool()).await.unwrap();
    }

    #[tokio::test]
    async fn requested_close_fences_delivery_but_cancel_keeps_watch_open() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-close", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = source.product_conversation_id.clone();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        db.watch_product_conversation(&product).await.unwrap();
        let turn = source_turn(&db, &source.id, "before-close").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("source failed"),
            false,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE durable_turns SET generation = 1, terminal_kind = 'Failed', terminal_reason = 'source failed', owns_conversation = 0 WHERE turn_id = ?1")
            .bind(i64::try_from(turn).unwrap()).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            1
        );
        db.begin_close_foundation(
            &source.product_conversation_id,
            &phoenix_core::domain::close::TranscriptConversationId::parse(source.id.clone())
                .unwrap(),
            "watch-close-attempt",
        )
        .await
        .unwrap();
        assert_eq!(db.list_coordinator_watches().await.unwrap().len(), 1);
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        let (prior_id, state): (String, String) =
            sqlx::query_as("SELECT event_id, delivery_state FROM coordinator_watch_events")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(state, "pending");
        let steering = "INSERT INTO steering_messages (message_id, conversation_id, ordinal, text, origin_kind, origin_subscription_event_id)
                         VALUES (?1, ?2, ?3, 'terminal event', 'subscription_event', ?4)";
        assert!(sqlx::query(steering)
            .bind("blocked-close")
            .bind(&coordinator.id)
            .bind(0)
            .bind(&prior_id)
            .execute(db.pool())
            .await
            .is_err());
        cancel_close_attempt(&db).await;
        assert_eq!(db.list_coordinator_watches().await.unwrap().len(), 1);
        let exposed = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(exposed.len(), 1);
        assert_eq!(exposed[0].event_id, prior_id);
        assert_eq!(exposed[0].source_turn_id, i64::try_from(turn).unwrap());
        let later = source_turn(&db, &source.id, "after-cancel").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(
            &mut tx,
            later,
            0,
            &source.id,
            "Failed",
            Some("later failure"),
            false,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let events = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, prior_id);
        let later_event = events
            .iter()
            .find(|event| event.source_turn_id == i64::try_from(later).unwrap())
            .unwrap();
        sqlx::query(steering)
            .bind("after-cancel")
            .bind(&coordinator.id)
            .bind(0)
            .bind(&later_event.event_id)
            .execute(db.pool())
            .await
            .unwrap();
        let remaining = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_id, prior_id);
    }

    #[tokio::test]
    async fn acceptance_is_coordinator_only_atomic_and_at_most_once() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = source.product_conversation_id.clone();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        db.watch_product_conversation(&product).await.unwrap();
        let turn = source_turn(&db, &source.id, "watched").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(&mut tx, turn, 0, &source.id, "Completed", None, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let event_id = db.pending_coordinator_watch_events(16).await.unwrap()[0]
            .event_id
            .clone();
        let admission = "INSERT INTO steering_messages (message_id, conversation_id, ordinal, text, origin_kind, origin_subscription_event_id)
                         VALUES (?1, ?2, ?3, 'terminal event', 'subscription_event', ?4)";
        assert!(sqlx::query(admission)
            .bind("wrong-target")
            .bind(&source.id)
            .bind(0)
            .bind(&event_id)
            .execute(db.pool())
            .await
            .is_err());
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            1
        );
        sqlx::query(admission)
            .bind("accepted")
            .bind(&coordinator.id)
            .bind(0)
            .bind(&event_id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        assert!(sqlx::query(admission)
            .bind("duplicate")
            .bind(&coordinator.id)
            .bind(1)
            .bind(&event_id)
            .execute(db.pool())
            .await
            .is_err());
        let accepted: (String, String) = sqlx::query_as("SELECT delivery_state, accepted_transcript_id FROM coordinator_watch_events WHERE event_id = ?1")
            .bind(&event_id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(accepted, ("accepted".into(), coordinator.id));
        db.unwatch_product_conversation(&product).await.unwrap();
        let persisted: String = sqlx::query_scalar(
            "SELECT delivery_state FROM coordinator_watch_events WHERE event_id = ?1",
        )
        .bind(event_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(persisted, "accepted");
    }
}
