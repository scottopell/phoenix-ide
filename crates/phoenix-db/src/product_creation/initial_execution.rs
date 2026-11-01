use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProductCreationInitialExecution {
    Queued,
    Running,
    Completed,
    Failed { reason: String },
    Cancelled,
    AwaitingUserResponse,
    AwaitingTaskApproval,
    Unavailable,
}

pub(crate) async fn record_initial_execution_receipt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation_id: &str,
    message_id: &str,
    terminal: &phoenix_workflow::TurnTerminal,
) -> DbResult<()> {
    let (terminal_kind, reason) = match terminal {
        phoenix_workflow::TurnTerminal::Completed => ("Completed", None),
        phoenix_workflow::TurnTerminal::Cancelled => ("Cancelled", None),
        phoenix_workflow::TurnTerminal::Failed { reason } => ("Failed", Some(reason.as_str())),
    };
    sqlx::query(
        "INSERT INTO product_creation_initial_execution_receipts
            (request_id, terminal_kind, failure_reason)
         SELECT j.request_id, ?3, ?4
         FROM product_creation_jobs j
         WHERE j.published_conversation_id = ?1 AND j.request_id = ?2
         ON CONFLICT(request_id) DO NOTHING",
    )
    .bind(conversation_id)
    .bind(message_id)
    .bind(terminal_kind)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl Database {
    /// Read exact initial-objective evidence independently of creation publication.
    pub async fn product_creation_initial_execution(
        &self,
        request_id: &str,
    ) -> DbResult<ProductCreationInitialExecution> {
        let mut connection = self.pool.acquire().await?;
        Self::product_creation_initial_execution_on(&mut connection, request_id).await
    }

    pub(super) async fn product_creation_initial_execution_on(
        connection: &mut SqliteConnection,
        request_id: &str,
    ) -> DbResult<ProductCreationInitialExecution> {
        let row = sqlx::query(
            "WITH selected_watch_event AS (
                SELECT e.terminal_kind, e.terminal_reason
                FROM coordinator_watch_events e
                JOIN product_creation_jobs j
                  ON e.source_transcript_id = j.published_conversation_id
                 AND e.source_occurrence_id = j.request_id
                WHERE j.request_id = ?1
                  AND e.source_occurrence_kind = 'steering'
                ORDER BY e.source_generation DESC, e.occurred_at_us DESC, e.event_id DESC
                LIMIT 1
             )
             SELECT CASE
               WHEN r.request_id IS NOT NULL THEN r.terminal_kind
               WHEN EXISTS (SELECT 1 FROM steering_messages q
                 WHERE q.conversation_id = j.published_conversation_id AND q.message_id = j.request_id)
                 THEN 'queued'
               WHEN EXISTS (SELECT 1 FROM steering_execution_occurrences o
                 WHERE o.conversation_id = j.published_conversation_id AND o.message_id = j.request_id)
                 THEN CASE
                   WHEN c.id IS NULL THEN 'unavailable'
                   WHEN c.state_kind = 'awaiting_user_response' THEN 'awaiting_user_response'
                   WHEN c.state_kind = 'awaiting_task_approval' THEN 'awaiting_task_approval'
                   ELSE 'running' END
               ELSE COALESCE((SELECT terminal_kind FROM selected_watch_event), 'unavailable')
             END AS terminal_kind,
             CASE
               WHEN r.request_id IS NOT NULL THEN r.failure_reason
               WHEN NOT EXISTS (SELECT 1 FROM steering_messages q
                 WHERE q.conversation_id = j.published_conversation_id AND q.message_id = j.request_id)
                AND NOT EXISTS (SELECT 1 FROM steering_execution_occurrences o
                 WHERE o.conversation_id = j.published_conversation_id AND o.message_id = j.request_id)
                 THEN (SELECT terminal_reason FROM selected_watch_event)
               ELSE NULL
             END AS failure_reason
             FROM product_creation_jobs j
             LEFT JOIN product_creation_initial_execution_receipts r ON r.request_id = j.request_id
             LEFT JOIN conversations c ON c.id = j.published_conversation_id
             WHERE j.request_id = ?1 AND j.published_conversation_id IS NOT NULL",
        )
        .bind(request_id)
        .fetch_optional(connection)
        .await?;
        let Some(row) = row else {
            return Ok(ProductCreationInitialExecution::Unavailable);
        };
        let state: String = row.try_get("terminal_kind")?;
        let reason: Option<String> = row.try_get("failure_reason")?;
        Ok(match state.as_str() {
            "queued" => ProductCreationInitialExecution::Queued,
            "running" => ProductCreationInitialExecution::Running,
            "completed" | "Completed" => ProductCreationInitialExecution::Completed,
            "failed" | "Failed" => ProductCreationInitialExecution::Failed {
                reason: reason.ok_or_else(|| {
                    sqlx::Error::Decode("failed initial execution is missing failure reason".into())
                })?,
            },
            "cancelled" | "Cancelled" => ProductCreationInitialExecution::Cancelled,
            "awaiting_user_response" => ProductCreationInitialExecution::AwaitingUserResponse,
            "awaiting_task_approval" => ProductCreationInitialExecution::AwaitingTaskApproval,
            _ => ProductCreationInitialExecution::Unavailable,
        })
    }
}
