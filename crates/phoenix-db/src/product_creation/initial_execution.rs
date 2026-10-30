use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductCreationInitialExecution {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    AwaitingUserResponse,
    AwaitingTaskApproval,
    Unavailable,
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
        let state: Option<String> = sqlx::query_scalar(
            "SELECT CASE
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
              ELSE COALESCE((SELECT e.terminal_kind FROM coordinator_watch_events e
                WHERE e.source_transcript_id = j.published_conversation_id
                  AND e.source_occurrence_kind = 'steering'
                  AND e.source_occurrence_id = j.request_id
                ORDER BY e.source_generation DESC, e.occurred_at_us DESC, e.event_id DESC LIMIT 1), 'unavailable')
             END
             FROM product_creation_jobs j
             LEFT JOIN conversations c ON c.id = j.published_conversation_id
             WHERE j.request_id = ?1 AND j.published_conversation_id IS NOT NULL",
        )
        .bind(request_id)
        .fetch_optional(connection)
        .await?;
        Ok(match state.as_deref() {
            Some("queued") => ProductCreationInitialExecution::Queued,
            Some("running") => ProductCreationInitialExecution::Running,
            Some("completed") => ProductCreationInitialExecution::Completed,
            Some("failed") => ProductCreationInitialExecution::Failed,
            Some("cancelled") => ProductCreationInitialExecution::Cancelled,
            Some("awaiting_user_response") => ProductCreationInitialExecution::AwaitingUserResponse,
            Some("awaiting_task_approval") => ProductCreationInitialExecution::AwaitingTaskApproval,
            _ => ProductCreationInitialExecution::Unavailable,
        })
    }
}
