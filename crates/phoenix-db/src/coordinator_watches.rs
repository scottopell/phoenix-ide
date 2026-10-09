mod mandatory_close;
use mandatory_close::decode_route;
pub use mandatory_close::{
    append_mandatory_close_failure_event_tx, CloseFailureStop, MandatoryCloseFailureSubject,
    WatchEventRoute,
};

use chrono::Utc;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use serde::Serialize;
use sqlx::{Row, Sqlite, Transaction};

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, Serialize)]
pub struct WatchSnapshot {
    pub product_conversation_id: ProductConversationId,
    pub current_transcript_id: String,
    pub current_state: phoenix_core::domain::sm_state::ConvState,
    pub enrolled_at_us: i64,
    pub display_name: String,
    pub transcript_slug: Option<String>,
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchOutcome {
    Completed,
    Failed { reason: String },
    CleanupFailed { reason: String },
    Cancelled,
    AwaitingUserResponse,
    AwaitingTaskApproval,
}

impl WatchOutcome {
    fn decode(kind: &str, reason: Option<String>) -> DbResult<Self> {
        match (kind, reason) {
            ("completed", None) => Ok(Self::Completed),
            ("failed", Some(reason)) => Ok(Self::Failed { reason }),
            ("cleanup_failed", Some(reason)) => Ok(Self::CleanupFailed { reason }),
            ("cancelled", None) => Ok(Self::Cancelled),
            ("awaiting_user_response", Some(reason)) if reason == "question_request" => {
                Ok(Self::AwaitingUserResponse)
            }
            ("awaiting_task_approval", Some(reason)) if reason == "task_approval_wait" => {
                Ok(Self::AwaitingTaskApproval)
            }
            _ => Err(DbError::Serialization(
                "invalid watch outcome/reason pair".into(),
            )),
        }
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed { .. } => "failed",
            Self::CleanupFailed { .. } => "cleanup_failed",
            Self::Cancelled => "cancelled",
            Self::AwaitingUserResponse => "awaiting_user_response",
            Self::AwaitingTaskApproval => "awaiting_task_approval",
        }
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed { reason } | Self::CleanupFailed { reason } => Some(reason),
            Self::AwaitingUserResponse => Some("question_request"),
            Self::AwaitingTaskApproval => Some("task_approval_wait"),
            Self::Completed | Self::Cancelled => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingWatchEvent {
    pub route: WatchEventRoute,
    pub event_id: String,
    pub product_conversation_id: ProductConversationId,
    pub source_transcript_id: String,
    pub source_occurrence_kind: String,
    pub source_occurrence_id: String,
    pub source_generation: i64,
    pub outcome: WatchOutcome,
    pub occurred_at_us: i64,
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
        enrolled_at_us: row.try_get("enrolled_at_us")?,
        display_name: row.try_get("display_name")?,
        transcript_slug: row.try_get("transcript_slug")?,
        project_path: row.try_get("project_path")?,
    })
}

const WATCH_SNAPSHOT: &str = "SELECT w.source_product_conversation_id, w.enrolled_at_us,
    c.id AS transcript_id, c.state,
    COALESCE(NULLIF(root.chain_name, ''), NULLIF(root.title, ''), NULLIF(root.slug, ''), 'Untitled conversation') AS display_name,
    c.slug AS transcript_slug, project.canonical_path AS project_path
    FROM coordinator_watches w
    JOIN product_conversations p ON p.id = w.source_product_conversation_id
    JOIN conversations c ON c.product_conversation_id = p.id
      AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
    JOIN conversations root ON root.product_conversation_id = p.id
      AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL
      AND NOT EXISTS (SELECT 1 FROM conversations predecessor
        WHERE predecessor.product_conversation_id = root.product_conversation_id
          AND predecessor.continued_in_conv_id = root.id)
    LEFT JOIN projects project ON project.id = c.project_id
    WHERE w.ended_at_us IS NULL AND p.ordinary_lifecycle = 'open'
      AND (?1 IS NULL OR w.source_product_conversation_id = ?1)
    ORDER BY w.enrolled_at_us, w.id";

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
            "INSERT INTO coordinator_watches(source_product_conversation_id, enrolled_at_us)
                     VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        )
        .bind(product_id.as_str())
        .bind(Utc::now().timestamp_micros())
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
            "UPDATE coordinator_watches SET ended_at_us = ?2
                                   WHERE source_product_conversation_id = ?1 AND ended_at_us IS NULL",
        )
        .bind(product_id.as_str())
        .bind(Utc::now().timestamp_micros())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 0;
        sqlx::query("UPDATE coordinator_watch_events SET delivery_state = 'suppressed'
                     WHERE delivery_state = 'pending' AND watch_id IN
                       (SELECT id FROM coordinator_watches WHERE source_product_conversation_id = ?1 AND ended_at_us IS NOT NULL)")
            .bind(product_id.as_str()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(ended)
    }

    /// Suppress a pending occurrence whose source no longer permits delivery.
    /// # Errors
    /// Returns a database error if suppression fails.
    pub async fn suppress_stale_watch_event(&self, event_id: &str) -> DbResult<()> {
        sqlx::query("UPDATE coordinator_watch_events SET delivery_state = 'suppressed' WHERE event_id = ?1 AND route_kind = 'subscription' AND delivery_state = 'pending' AND NOT EXISTS (SELECT 1 FROM coordinator_watches w JOIN product_conversations p ON p.id = w.source_product_conversation_id WHERE w.id = coordinator_watch_events.watch_id AND w.ended_at_us IS NULL AND p.ordinary_lifecycle = 'open')")
            .bind(event_id).execute(self.pool()).await?;
        Ok(())
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
        let rows = sqlx::query("SELECT e.event_id, e.route_kind,
                  COALESCE(w.source_product_conversation_id, f.source_product_conversation_id) AS source_product_conversation_id,
                  e.source_transcript_id, e.source_occurrence_kind, e.source_occurrence_id,
                  e.source_generation, e.terminal_kind, e.terminal_reason, e.occurred_at_us,
                  f.authority_kind, f.scope, f.resource_kind, f.identity_kind, f.identity_codec, f.identity_value,
                  f.detail, f.cleanup_run_ordinal, f.stop_certainty, f.confirmed_at_us AS stop_confirmed_at_unix_us
             FROM coordinator_watch_events e
             LEFT JOIN coordinator_watches w ON w.id = e.watch_id
             LEFT JOIN product_conversations p ON p.id = w.source_product_conversation_id
             LEFT JOIN close_cleanup_failures f ON f.failure_occurrence_id = e.mandatory_failure_occurrence_id
             WHERE e.delivery_state = 'pending' AND e.continuation_state = 'none'
               AND (e.route_kind = 'mandatory_close_failure' OR
                    (e.route_kind = 'subscription' AND w.ended_at_us IS NULL
                     AND p.ordinary_lifecycle = 'open'
                     AND NOT EXISTS (SELECT 1 FROM close_obligations o
                                     WHERE o.product_conversation_id = p.id AND o.phase != 'completed')))
             ORDER BY e.occurred_at_us, e.event_id LIMIT ?1")
            .bind(limit).fetch_all(&self.pool).await?;
        let mut events = rows
            .into_iter()
            .map(|row| {
                let product_id: String = row.try_get("source_product_conversation_id")?;
                Ok(PendingWatchEvent {
                    route: decode_route(&row)?,
                    event_id: row.try_get("event_id")?,
                    product_conversation_id: ProductConversationId::parse(product_id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                    source_transcript_id: row.try_get("source_transcript_id")?,
                    source_occurrence_kind: row.try_get("source_occurrence_kind")?,
                    source_occurrence_id: row.try_get("source_occurrence_id")?,
                    source_generation: row.try_get("source_generation")?,
                    outcome: WatchOutcome::decode(
                        row.try_get("terminal_kind")?,
                        row.try_get("terminal_reason")?,
                    )?,
                    occurred_at_us: row.try_get("occurred_at_us")?,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        for event in &mut events {
            if let WatchEventRoute::MandatoryCloseFailure {
                remaining_resources,
                ..
            } = &mut event.route
            {
                *remaining_resources =
                    mandatory_close::remaining_resources(&self.pool, &event.source_occurrence_id)
                        .await?;
            }
        }
        Ok(events)
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
    record_watch_event_tx(
        tx,
        "direct_turn",
        &turn_id.to_string(),
        generation,
        transcript_id,
        terminal_kind,
        reason,
        context_exhausted,
    )
    .await
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MessageExecutionSource {
    Steering,
    Wake,
    SeededFork,
    InteractionResponse,
}

impl MessageExecutionSource {
    pub(crate) fn from_db(value: &str) -> DbResult<Self> {
        match value {
            "steering" => Ok(Self::Steering),
            "wake" => Ok(Self::Wake),
            "seeded_fork" => Ok(Self::SeededFork),
            "interaction_response" => Ok(Self::InteractionResponse),
            _ => Err(DbError::Serialization(format!(
                "invalid message execution source: {value}"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Steering => "steering",
            Self::Wake => "wake",
            Self::SeededFork => "seeded_fork",
            Self::InteractionResponse => "interaction_response",
        }
    }
}

pub(crate) async fn record_steering_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    source_kind: MessageExecutionSource,
    message_id: &str,
    transcript_id: &str,
    category: &str,
    reason: Option<&str>,
) -> DbResult<()> {
    record_watch_event_tx(
        tx,
        source_kind.as_str(),
        message_id,
        0,
        transcript_id,
        category,
        reason,
        reason == Some("context exhausted"),
    )
    .await
}

pub(crate) async fn record_wait_entry_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    state: &ConvState,
) -> DbResult<()> {
    let (kind, outcome, identified_request) = match state {
        ConvState::AwaitingUserResponse {
            request_authority, ..
        } => (
            "question_request",
            "awaiting_user_response",
            request_authority.request_id(),
        ),
        ConvState::AwaitingTaskApproval { .. } => {
            ("task_approval_wait", "awaiting_task_approval", None)
        }
        _ => return Ok(()),
    };
    let previous: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
        .bind(transcript_id)
        .fetch_one(&mut **tx)
        .await?;
    let previous: ConvState = serde_json::from_str(&previous)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let same_occurrence = match (identified_request, &previous) {
        (
            Some(request_id),
            ConvState::AwaitingUserResponse {
                request_authority, ..
            },
        ) => request_authority.request_id() == Some(request_id),
        (None, _) => &previous == state,
        _ => false,
    };
    if same_occurrence {
        return Ok(());
    }
    let occurrence_id =
        identified_request.map_or_else(|| uuid::Uuid::new_v4().to_string(), ToString::to_string);
    let watch_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT w.id FROM conversations c JOIN coordinator_watches w
         ON w.source_product_conversation_id = c.product_conversation_id
         WHERE c.id = ?1 AND w.ended_at_us IS NULL",
    )
    .bind(transcript_id)
    .fetch_all(&mut **tx)
    .await?;
    for watch_id in watch_ids {
        sqlx::query("INSERT INTO coordinator_watch_events
            (event_id, watch_id, source_occurrence_kind, source_occurrence_id,
             source_generation, source_transcript_id, terminal_kind, terminal_reason, occurred_at_us)
            VALUES (?1, ?2, ?6, ?3, 0, ?4, ?7, ?6, ?5)
            ON CONFLICT(source_occurrence_kind, source_occurrence_id, source_generation, watch_id) DO NOTHING")
            .bind(uuid::Uuid::new_v4().to_string()).bind(watch_id)
            .bind(&occurrence_id).bind(transcript_id)
            .bind(chrono::Utc::now().timestamp_micros()).bind(kind).bind(outcome)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

pub(crate) async fn record_summary_failure_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    failure: &phoenix_core::domain::sm_state::RecoverableContinuationFailure,
) -> DbResult<()> {
    record_watch_event_tx(
        tx,
        "continuation_summary",
        &failure.request.operation_id,
        u64::from(failure.request.attempt),
        transcript_id,
        "Failed",
        Some("continuation summary failed"),
        false,
    )
    .await
}

pub(crate) async fn record_creation_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    job_id: &str,
    generation: u64,
    transcript_id: &str,
    terminal_kind: &str,
    reason: Option<&str>,
) -> DbResult<()> {
    record_watch_event_tx(
        tx,
        "creation",
        job_id,
        generation,
        transcript_id,
        terminal_kind,
        reason,
        reason == Some("context exhausted"),
    )
    .await
}

#[allow(clippy::too_many_arguments)] // One transaction binds the complete source occurrence and terminal fact.
async fn record_watch_event_tx(
    tx: &mut Transaction<'_, Sqlite>,
    occurrence_kind: &str,
    occurrence_id: &str,
    generation: u64,
    transcript_id: &str,
    terminal_kind: &str,
    reason: Option<&str>,
    context_exhausted: bool,
) -> DbResult<()> {
    let cancelled =
        sqlx::query("DELETE FROM execution_cancel_observations WHERE conversation_id = ?1")
            .bind(transcript_id)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            > 0;
    let category = match if cancelled {
        "Cancelled"
    } else {
        terminal_kind
    } {
        "Completed" => "completed",
        "Failed" => "failed",
        "Cancelled" => "cancelled",
        _ => return Ok(()),
    };
    if category == "completed" && context_exhausted {
        return Ok(());
    }
    sqlx::query("INSERT INTO coordinator_watch_events
        (event_id, watch_id, source_occurrence_kind, source_occurrence_id,
         source_generation, source_transcript_id, terminal_kind, terminal_reason,
         occurred_at_us, continuation_state)
        SELECT ?1, w.id, ?2, ?3, ?4, c.id, ?5, ?6, ?7,
          CASE WHEN ?9 AND ?5 = 'failed' AND EXISTS
               (SELECT 1 FROM automatic_continuation_admissions a WHERE a.predecessor_conversation_id = c.id
                AND a.phase NOT IN ('failed', 'superseded'))
               THEN 'awaiting' ELSE 'none' END
        FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
          JOIN coordinator_watches w ON w.source_product_conversation_id = p.id AND w.ended_at_us IS NULL
        WHERE c.id = ?8 AND c.parent_conversation_id IS NULL AND p.kind = 'ordinary'
          AND p.ordinary_lifecycle = 'open'
          AND (?5 != 'completed' OR c.state_kind IN ('idle', 'terminal'))
          AND c.state_kind NOT IN ('awaiting_continuation', 'handed_off')
        ON CONFLICT(source_occurrence_kind, source_occurrence_id, source_generation, watch_id)
        DO NOTHING")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(occurrence_kind)
        .bind(occurrence_id)
        .bind(i64::try_from(generation).map_err(|_| DbError::Serialization("generation overflow".into()))?)
        .bind(category).bind(if context_exhausted && category == "failed" { Some("context exhausted") } else { reason.or((category == "failed").then_some("turn failed")) })
        .bind(Utc::now().timestamp_micros())
        .bind(transcript_id).bind(context_exhausted).execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_preserves_mandatory_cleanup_failure_alongside_waits() {
        assert_eq!(
            WatchOutcome::decode("cleanup_failed", Some("resource preserved".into())).unwrap(),
            WatchOutcome::CleanupFailed {
                reason: "resource preserved".into()
            }
        );
        assert!(WatchOutcome::decode("cleanup_failed", None).is_err());
        assert_eq!(
            WatchOutcome::decode("awaiting_user_response", Some("question_request".into()))
                .unwrap(),
            WatchOutcome::AwaitingUserResponse
        );
    }

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
    async fn durable_question_wait_is_emitted_once_per_request_without_settling_turn() {
        use phoenix_core::domain::sm_state::{QuestionRequestAuthority, UserQuestion};
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-wait", "watch-wait", "/tmp", true, None, None)
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        let turn_id = source_turn(&db, &source.id, "active-question-turn").await;
        let waiting = |authority| ConvState::AwaitingUserResponse {
            tool_use_id: "provider-reused-tool-id".into(),
            request_authority: authority,
            questions: vec![UserQuestion {
                question: "Choose".into(),
                header: "Choice".into(),
                options: vec![],
                multi_select: false,
            }],
        };
        let first = waiting(QuestionRequestAuthority::new());
        db.update_conversation_state(&source.id, &first)
            .await
            .unwrap();
        db.update_conversation_state(&source.id, &first)
            .await
            .unwrap();
        let restored: ConvState =
            serde_json::from_str(&serde_json::to_string(&first).unwrap()).unwrap();
        db.update_conversation_state(&source.id, &restored)
            .await
            .unwrap();
        let events = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(
            events.len(),
            1,
            "one durable request must produce one wait event"
        );
        assert_eq!(events[0].outcome.label(), "awaiting_user_response");
        assert_eq!(events[0].outcome.reason(), Some("question_request"));
        db.update_conversation_state(&source.id, &waiting(QuestionRequestAuthority::new()))
            .await
            .unwrap();
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            2
        );
        let terminal: Option<String> =
            sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id = ?1")
                .bind(i64::try_from(turn_id).unwrap())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(
            terminal.is_none(),
            "waiting must not settle the active turn"
        );
    }

    #[tokio::test]
    async fn task_and_legacy_wait_entries_deduplicate_without_replaying_enrollment() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("fallback-waits", "fallback-waits", "/tmp", true, None, None)
            .await
            .unwrap();
        let approval = ConvState::AwaitingTaskApproval {
            task_file: "tasks/12345-p1-ready--example.md".into(),
            title: "Example".into(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "Review".into(),
        };
        let legacy: ConvState = serde_json::from_value(serde_json::json!({
            "type": "awaiting_user_response", "tool_use_id": "reused", "questions": []
        }))
        .unwrap();
        db.update_conversation_state(&source.id, &approval)
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        db.update_conversation_state(&source.id, &approval)
            .await
            .unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        db.update_conversation_state(&source.id, &ConvState::Idle)
            .await
            .unwrap();
        let turn_id = source_turn(&db, &source.id, "fallback-turn").await;
        for (index, wait) in [&approval, &legacy].into_iter().enumerate() {
            db.update_conversation_state(&source.id, &ConvState::Idle)
                .await
                .unwrap();
            db.update_conversation_state(&source.id, wait)
                .await
                .unwrap();
            let restored: ConvState =
                serde_json::from_str(&serde_json::to_string(wait).unwrap()).unwrap();
            db.update_conversation_state(&source.id, &restored)
                .await
                .unwrap();
            assert_eq!(
                db.pending_coordinator_watch_events(16).await.unwrap().len(),
                index * 2 + 1
            );
            db.update_conversation_state(&source.id, &ConvState::Idle)
                .await
                .unwrap();
            db.update_conversation_state(&source.id, wait)
                .await
                .unwrap();
            let events = db.pending_coordinator_watch_events(16).await.unwrap();
            assert_eq!(events.len(), index * 2 + 2);
            assert_ne!(
                events[index * 2].source_occurrence_id,
                events[index * 2 + 1].source_occurrence_id
            );
        }
        let terminal: Option<String> =
            sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id = ?1")
                .bind(i64::try_from(turn_id).unwrap())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(terminal.is_none());
    }

    #[tokio::test]
    async fn enrolling_during_a_question_wait_does_not_replay_it() {
        use phoenix_core::domain::sm_state::{QuestionRequestAuthority, UserQuestion};
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation(
                "pending-before-watch",
                "pending-before-watch",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let waiting = ConvState::AwaitingUserResponse {
            tool_use_id: "tool".into(),
            request_authority: QuestionRequestAuthority::new(),
            questions: vec![UserQuestion {
                question: "Choose".into(),
                header: "Choice".into(),
                options: vec![],
                multi_select: false,
            }],
        };
        db.update_conversation_state(&source.id, &waiting)
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        db.update_conversation_state(&source.id, &waiting)
            .await
            .unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn recorded_cancel_overrides_idle_completion_without_inventing_actor() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-cancel", "watch-cancel", "/tmp", true, None, None)
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        sqlx::query("INSERT INTO execution_cancel_observations(conversation_id) VALUES (?1)")
            .bind(&source.id)
            .execute(db.pool())
            .await
            .unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        record_creation_event_tx(&mut tx, "cancel-source", 0, &source.id, "Completed", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let events = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome.label(), "cancelled");
        assert_eq!(events[0].outcome.reason(), None);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_cancel_observations")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
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
        assert_eq!(events[0].outcome.reason(), Some("model failed"));
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
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            1
        );
        let (id, kind, state): (String, String, String) = sqlx::query_as(
            "SELECT event_id, terminal_kind, continuation_state FROM coordinator_watch_events",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((kind.as_str(), state.as_str()), ("failed", "none"));
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
        assert_eq!(delivered[0].source_occurrence_kind, "direct_turn");
        assert_eq!(delivered[0].source_occurrence_id, turn.to_string());
        assert_eq!(delivered[0].source_generation, 0);
        assert_eq!(delivered[0].outcome.label(), "failed");
        assert_eq!(delivered[0].outcome.reason(), Some("context exhausted"));
        assert_eq!(delivered[0].source_transcript_id, source.id);
    }

    #[tokio::test]
    async fn watch_timestamps_reject_negative_values() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-timestamp-check", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = source.product_conversation_id.clone();
        assert!(sqlx::query(
            "INSERT INTO coordinator_watches(source_product_conversation_id, enrolled_at_us)
             VALUES (?1, -1)",
        )
        .bind(product.as_str())
        .execute(db.pool())
        .await
        .is_err());
        db.watch_product_conversation(&product).await.unwrap();
        assert!(sqlx::query(
            "UPDATE coordinator_watches SET ended_at_us = -1
             WHERE source_product_conversation_id = ?1",
        )
        .bind(product.as_str())
        .execute(db.pool())
        .await
        .is_err());
        let turn = source_turn(&db, &source.id, "timestamp-check-turn").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(
            &mut tx,
            turn,
            0,
            &source.id,
            "Failed",
            Some("failed"),
            false,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(sqlx::query(
            "UPDATE coordinator_watch_events SET occurred_at_us = -1
             WHERE source_occurrence_id = ?1",
        )
        .bind(turn.to_string())
        .execute(db.pool())
        .await
        .is_err());
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
    #[allow(clippy::too_many_lines)] // Exercises request, cancellation, and subsequent execution in one lifecycle.
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
        assert_eq!(exposed[0].source_occurrence_kind, "direct_turn");
        assert_eq!(exposed[0].source_occurrence_id, turn.to_string());
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
            .find(|event| {
                event.source_occurrence_kind == "direct_turn"
                    && event.source_occurrence_id == later.to_string()
            })
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
    async fn event_terminalized_while_close_is_pending_is_exposed_after_cancellation() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("watch-during-close", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        let turn = source_turn(&db, &source.id, "during-close").await;
        db.begin_close_foundation(
            &source.product_conversation_id,
            &phoenix_core::domain::close::TranscriptConversationId::parse(source.id.clone())
                .unwrap(),
            "watch-close-attempt",
        )
        .await
        .unwrap();

        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(&mut tx, turn, 0, &source.id, "Cancelled", None, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(db
            .pending_coordinator_watch_events(16)
            .await
            .unwrap()
            .is_empty());
        let persisted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM coordinator_watch_events WHERE source_occurrence_id = ?1",
        )
        .bind(turn.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(persisted, 1);

        cancel_close_attempt(&db).await;
        let exposed = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(exposed.len(), 1);
        assert_eq!(exposed[0].source_occurrence_kind, "direct_turn");
        assert_eq!(exposed[0].source_occurrence_id, turn.to_string());
        assert_eq!(exposed[0].outcome.label(), "cancelled");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn mandatory_failure_loads_ordered_resources_until_acceptance() {
        use crate::{
            CloseCleanupFailureAuthority, CloseCleanupFailureResource,
            CloseCleanupResourceDisposition as Disposition,
            TerminalizeInitialCloseCleanupFailureRequest,
        };
        use phoenix_core::domain::close::{
            CloseAttemptId, CloseRunOrdinal, CloseStopCertainty, LossItemIdentity, OpaqueIdentity,
            RetiredResourceIdentity, RetiredResourceKind, RetirementFailureReason,
            TranscriptConversationId,
        };

        let db = Database::open_in_memory().await.unwrap();
        let ordinary = db
            .create_conversation("ordinary", "ordinary", "/tmp", true, None, None)
            .await
            .unwrap();
        db.watch_product_conversation(&ordinary.product_conversation_id)
            .await
            .unwrap();
        let turn = source_turn(&db, &ordinary.id, "ordinary-turn").await;
        let mut tx = db.pool().begin().await.unwrap();
        record_terminal_event_tx(&mut tx, turn, 0, &ordinary.id, "Completed", None, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let ordinary_event_id = db.pending_coordinator_watch_events(16).await.unwrap()[0]
            .event_id
            .clone();

        let source = db
            .create_conversation("close-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope = source.attached_work_scope_id.clone().unwrap();
        let other_scope = phoenix_core::work_scope::WorkScopeId::parse("other-scope").unwrap();
        sqlx::query("INSERT INTO work_scopes (id, authority_kind, lifecycle, environment_kind, cwd, created_at, updated_at)
            SELECT ?1, authority_kind, lifecycle, 'unowned_cwd', '/tmp/child', created_at, updated_at FROM work_scopes WHERE id = ?2")
            .bind(other_scope.as_str()).bind(scope.as_str()).execute(db.pool()).await.unwrap();
        let mut latest = source.clone();
        latest.id = "close-latest".into();
        latest.slug = Some("close-latest".into());
        latest.attached_work_scope_id = Some(other_scope.clone());
        let mut tx = db.pool().begin().await.unwrap();
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO product_continuation_reservations (predecessor_conversation_id, successor_conversation_id, product_conversation_id) VALUES (?1, ?2, ?3)")
            .bind(&source.id).bind(&latest.id).bind(source.product_conversation_id.as_str()).execute(&mut *tx).await.unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(&latest.id)
            .bind(&source.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        crate::insert_conversation_tx(&mut tx, &latest)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM product_continuation_reservations WHERE predecessor_conversation_id = ?1",
        )
        .bind(&source.id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        db.begin_close_foundation(
            &source.product_conversation_id,
            &TranscriptConversationId::parse(latest.id.clone()).unwrap(),
            "watch-failure",
        )
        .await
        .unwrap();
        let resources = [
            (
                &scope,
                RetiredResourceKind::BashProcessGroup,
                "epoch:failed",
                Disposition::Failed,
            ),
            (
                &other_scope,
                RetiredResourceKind::BrowserSession,
                "epoch:residual",
                Disposition::Residual,
            ),
            (
                &scope,
                RetiredResourceKind::PtySession,
                "epoch:unattempted",
                Disposition::Unattempted,
            ),
            (
                &other_scope,
                RetiredResourceKind::EquivalentLiveResource,
                "epoch:unknown",
                Disposition::Unknown,
            ),
        ]
        .into_iter()
        .map(
            |(scope, kind, identity, disposition)| CloseCleanupFailureResource {
                scope: scope.clone(),
                resource: RetiredResourceIdentity::parse(
                    kind,
                    LossItemIdentity::Opaque(OpaqueIdentity::parse(identity).unwrap()),
                )
                .unwrap(),
                disposition,
            },
        )
        .collect::<Vec<_>>();
        let request = TerminalizeInitialCloseCleanupFailureRequest {
            failure_occurrence_id: "close-failure:1:watch-failure".into(),
            attempt_id: CloseAttemptId::parse("watch-failure").unwrap(),
            source_product_conversation_id: source.product_conversation_id.clone(),
            authority: CloseCleanupFailureAuthority::ObservedProcessResource {
                scope,
                resource: resources[0].resource.clone(),
            },
            remaining_resources: resources.clone(),
            reason: RetirementFailureReason::ResidualProcessAlive,
            detail: "shutdown failed".into(),
            stop_certainty: CloseStopCertainty::ShutdownUncertain,
            occurred_at_us: 1,
        };
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        db.suppress_stale_watch_event(&request.failure_occurrence_id)
            .await
            .unwrap();
        assert!(!db
            .unwatch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap());
        let expected = resources
            .into_iter()
            .map(Into::into)
            .collect::<Vec<mandatory_close::CloseFailureRemainingResource>>();
        for _ in 0..2 {
            let pending = db.pending_coordinator_watch_events(16).await.unwrap();
            assert_eq!(pending.len(), 2);
            assert_eq!(pending[0].event_id, request.failure_occurrence_id);
            assert_eq!(pending[1].event_id, ordinary_event_id);
            assert!(matches!(pending[1].route, WatchEventRoute::Subscription));
            let WatchEventRoute::MandatoryCloseFailure {
                run_ordinal,
                remaining_resources,
                stop,
                ..
            } = &pending[0].route
            else {
                panic!("expected mandatory route");
            };
            assert_eq!(*run_ordinal, CloseRunOrdinal::INITIAL);
            assert_eq!(*remaining_resources, expected);
            assert!(matches!(stop, CloseFailureStop::ShutdownUncertain));
            let limited = db.pending_coordinator_watch_events(1).await.unwrap();
            assert_eq!(limited.len(), 1);
            assert_eq!(limited[0].event_id, request.failure_occurrence_id);
        }
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let admission = "INSERT INTO steering_messages (message_id, conversation_id, ordinal, text, origin_kind, origin_subscription_event_id)
                         VALUES (?1, ?2, ?3, 'failure with resources', 'subscription_event', ?4)";
        assert!(sqlx::query(admission)
            .bind("wrong-target")
            .bind(&ordinary.id)
            .bind(0)
            .bind(&request.failure_occurrence_id)
            .execute(db.pool())
            .await
            .is_err());
        assert_eq!(
            db.pending_coordinator_watch_events(16).await.unwrap().len(),
            2
        );
        sqlx::query(admission)
            .bind("accepted-failure")
            .bind(&coordinator.id)
            .bind(0)
            .bind(&request.failure_occurrence_id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(sqlx::query(admission)
            .bind("duplicate-failure")
            .bind(&coordinator.id)
            .bind(1)
            .bind(&request.failure_occurrence_id)
            .execute(db.pool())
            .await
            .is_err());
        db.terminalize_initial_close_cleanup_failure(&request)
            .await
            .unwrap();
        let pending = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_id, ordinary_event_id);
        assert_eq!(
            mandatory_close::remaining_resources(db.pool(), &request.failure_occurrence_id)
                .await
                .unwrap(),
            expected
        );
        let accepted: (String, String) = sqlx::query_as("SELECT delivery_state, accepted_transcript_id FROM coordinator_watch_events WHERE event_id = ?1")
            .bind(&request.failure_occurrence_id).fetch_one(db.pool()).await.unwrap();
        assert_eq!(accepted, ("accepted".into(), coordinator.id));
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
