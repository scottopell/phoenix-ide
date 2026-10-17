use chrono::{DateTime, Utc};
use phoenix_core::domain::product_conversation::{
    ContinuationOpeningAuthority, OrdinaryProductConversationLifecycle, ProductConversation,
    ProductConversationId, ProductConversationKind,
};
use phoenix_core::work_scope::RuntimeRole;
use serde::Serialize;
use sqlx::{Connection, Row, SqliteConnection};
use tracing::Instrument;

use crate::{CloseProjection, Database, DbError, DbResult, MessageContent, MessageType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProductConversation {
    pub product_conversation_id: ProductConversationId,
    pub requested_transcript_row_id: String,
}

#[derive(Debug, Clone)]
pub struct ProductConversationAggregate {
    pub product_conversation: ProductConversation,
    pub root: ProductConversationTranscriptRow,
    pub latest_transcript_row_id: String,
    pub updated_at: DateTime<Utc>,
    pub segments: Vec<ProductConversationSegment>,
    pub source: Option<ProductConversationSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationCloseAvailability {
    Available,
    Unavailable(ProductConversationCloseUnavailableReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationCloseUnavailableReason {
    ActiveCloseAttempt,
    AwaitingTaskApproval,
    AwaitingContinuation,
    HandedOffWithoutContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationListLifecycle {
    Open {
        close_availability: ProductConversationCloseAvailability,
    },
    History,
}

#[derive(Debug, Clone)]
pub struct ProductConversationListProjection {
    pub product_conversation_id: ProductConversationId,
    pub lifecycle: ProductConversationListLifecycle,
    pub root_transcript_row_id: String,
    pub root_slug: Option<String>,
    pub root_title: Option<String>,
    pub latest_transcript_row_id: String,
    pub latest_state: crate::ConvState,
    pub latest_continued_in_conv_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrdinaryProductConversationCitation {
    pub product_conversation_id: ProductConversationId,
    pub root_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationOpeningEdge {
    pub product_conversation_id: ProductConversationId,
    pub predecessor_transcript_row_id: String,
    pub successor_transcript_row_id: String,
    pub continuation_message_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationOpeningPresentation {
    Boundary,
    SeparateMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationOpeningRead {
    None,
    Pending {
        edge: ContinuationOpeningEdge,
        client_turn_key: phoenix_workflow::ClientTurnKey,
        opening_authority: ContinuationOpeningAuthority,
    },
    Accepted {
        edge: ContinuationOpeningEdge,
        accepted_message_id: String,
        opening_authority: ContinuationOpeningAuthority,
        presentation: ContinuationOpeningPresentation,
    },
}

impl ContinuationOpeningRead {
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContinuationOpeningUpdateRead {
    Current(ContinuationOpeningRead),
    Superseded {
        content: MessageContent,
        transcript_generation: i64,
        display_data: Option<serde_json::Value>,
        opening: ContinuationOpeningRead,
    },
}

#[derive(Debug, Clone)]
pub struct ProductConversationSnapshotRead {
    pub aggregate: ProductConversationAggregate,
    pub latest_opening: ContinuationOpeningRead,
    pub close: Option<CloseProjection>,
    pub messages: Vec<(i64, crate::Message)>,
    pub requested_transcript_row_id: String,
}

#[derive(Debug, Clone)]
pub struct CoordinatorConversationSnapshotRead {
    pub aggregate: ProductConversationAggregate,
    pub latest_opening: ContinuationOpeningRead,
    pub messages: Vec<(i64, crate::Message)>,
    pub requested_transcript_row_id: String,
}

/// A per-segment upper bound captured with an aggregate page cursor.
///
/// Bounds keep pagination on the original message snapshot while later tail
/// appends remain visible to a fresh read. Topology remains validated separately.
#[derive(Debug, Clone, Serialize)]
pub struct ProductConversationSegmentCeiling {
    pub transcript_row_id: String,
    pub tail_sequence_id: i64,
    pub tail_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProductConversationTranscriptRow {
    pub conversation: crate::Conversation,
    pub segment_ordinal: i64,
    /// Durable append watermark for this transcript segment.
    pub tail_sequence_id: i64,
    pub tail_message_id: Option<String>,
    pub work_identity: Option<ProductConversationWorkIdentity>,
}

#[derive(Debug, Clone)]
pub struct ProductConversationWorkIdentity {
    pub worktree_path: String,
    pub branch_name: Option<String>,
    pub base_branch: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProductConversationSegment {
    pub transcript_row: ProductConversationTranscriptRow,
    pub handoff: Option<ProductConversationHandoff>,
}

#[derive(Debug, Clone)]
pub enum ProductConversationHandoff {
    Completed {
        predecessor_transcript_row_id: String,
        successor_transcript_row_id: String,
        continuation_message_id: String,
        accepted_successor_message_id: String,
        opening_authority: ContinuationOpeningAuthority,
        summary: String,
        accepted_is_duplicate_summary: bool,
    },
    Historical {
        predecessor_transcript_row_id: String,
        successor_transcript_row_id: String,
        continuation_message_id: String,
        summary: String,
    },
}

impl ProductConversationHandoff {
    #[must_use]
    pub fn continuation_message_id(&self) -> &str {
        match self {
            Self::Completed {
                continuation_message_id,
                ..
            }
            | Self::Historical {
                continuation_message_id,
                ..
            } => continuation_message_id,
        }
    }

    #[must_use]
    pub fn accepted_successor_message_id(&self) -> Option<&str> {
        match self {
            Self::Completed {
                accepted_successor_message_id,
                ..
            } => Some(accepted_successor_message_id),
            Self::Historical { .. } => None,
        }
    }

    #[must_use]
    pub fn accepted_is_duplicate_summary(&self) -> bool {
        matches!(
            self,
            Self::Completed {
                accepted_is_duplicate_summary: true,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductConversationSource {
    pub source_product_conversation_id: ProductConversationId,
    pub source_conversation_id: String,
    pub relation_kind: ProductConversationSourceKind,
    pub relation_key: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationSourceKind {
    ApprovedTask,
}

impl ProductConversationSourceKind {
    fn from_db(value: &str) -> Option<Self> {
        match value {
            "approved_task" => Some(Self::ApprovedTask),
            _ => None,
        }
    }
}

fn work_identity_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<Option<ProductConversationWorkIdentity>, sqlx::Error> {
    let attached_work_scope_id: Option<String> = row.try_get("work_scope_id")?;
    let environment_kind: Option<String> = row.try_get("environment_kind")?;
    let worktree_path: Option<String> = row.try_get("worktree_path")?;
    let branch_name: Option<String> = row.try_get("branch_name")?;
    let base_branch: Option<String> = row.try_get("base_branch")?;
    match (
        attached_work_scope_id,
        environment_kind.as_deref(),
        worktree_path,
        branch_name,
        base_branch,
    ) {
        (
            Some(_),
            Some("allocated_worktree"),
            Some(worktree_path),
            branch_name,
            Some(base_branch),
        ) => Ok(Some(ProductConversationWorkIdentity {
            worktree_path,
            branch_name,
            base_branch,
            task_id: row.try_get("objective_task_id")?,
            task_title: row.try_get("objective_task_title")?,
        })),
        _ => Ok(None),
    }
}

fn message_content_from_row(
    row: &sqlx::sqlite::SqliteRow,
    message_type_column: &str,
    content_column: &str,
) -> DbResult<MessageContent> {
    let message_type: String = row.try_get(message_type_column)?;
    let message_type = serde_json::from_value(serde_json::Value::String(message_type))
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let content = serde_json::from_str(&row.try_get::<String, _>(content_column)?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    MessageContent::from_stored_json(message_type, content).map_err(DbError::Serialization)
}

fn list_projection_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> DbResult<ProductConversationListProjection> {
    let product_conversation_id = row
        .try_get::<String, _>("product_conversation_id")?
        .parse::<ProductConversationId>()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let lifecycle = row.try_get::<String, _>("ordinary_lifecycle")?;
    let lifecycle =
        OrdinaryProductConversationLifecycle::from_db_str(&lifecycle).ok_or_else(|| {
            DbError::Serialization(format!("unknown ordinary lifecycle: {lifecycle}"))
        })?;
    let latest_state = serde_json::from_str(&row.try_get::<String, _>("latest_state")?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let close_availability = if row.try_get::<bool, _>("has_active_close_attempt")? {
        ProductConversationCloseAvailability::Unavailable(
            ProductConversationCloseUnavailableReason::ActiveCloseAttempt,
        )
    } else if row.try_get::<bool, _>("has_awaiting_task_approval")? {
        ProductConversationCloseAvailability::Unavailable(
            ProductConversationCloseUnavailableReason::AwaitingTaskApproval,
        )
    } else if row.try_get::<bool, _>("has_awaiting_continuation")? {
        ProductConversationCloseAvailability::Unavailable(
            ProductConversationCloseUnavailableReason::AwaitingContinuation,
        )
    } else if matches!(&latest_state, crate::ConvState::HandedOff { .. }) {
        ProductConversationCloseAvailability::Unavailable(
            ProductConversationCloseUnavailableReason::HandedOffWithoutContinuation,
        )
    } else {
        ProductConversationCloseAvailability::Available
    };
    let lifecycle = match lifecycle {
        OrdinaryProductConversationLifecycle::Open => {
            ProductConversationListLifecycle::Open { close_availability }
        }
        OrdinaryProductConversationLifecycle::History => ProductConversationListLifecycle::History,
    };
    Ok(ProductConversationListProjection {
        product_conversation_id,
        lifecycle,
        root_transcript_row_id: row.try_get("root_transcript_row_id")?,
        root_slug: row.try_get("root_slug")?,
        root_title: row.try_get("root_title")?,
        latest_transcript_row_id: row.try_get("latest_transcript_row_id")?,
        latest_state,
        latest_continued_in_conv_id: row.try_get("latest_continued_in_conv_id")?,
        updated_at: row
            .try_get::<String, _>("updated_at")?
            .parse::<DateTime<Utc>>()
            .map_err(|error| DbError::Serialization(error.to_string()))?,
    })
}

fn continuation_summary(row: &sqlx::sqlite::SqliteRow, content_column: &str) -> DbResult<String> {
    let content = serde_json::from_str(&row.try_get::<String, _>(content_column)?)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let MessageContent::Continuation(content) =
        MessageContent::from_stored_json(MessageType::Continuation, content)
            .map_err(DbError::Serialization)?
    else {
        return Err(DbError::Serialization(
            "continuation handoff references a non-continuation message".to_string(),
        ));
    };
    Ok(content.summary)
}

#[cfg(test)]
const ORIGINAL_PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL: &str = "WITH RECURSIVE transcript(id, ordinal) AS (
     SELECT root.id, 0 FROM conversations root WHERE root.product_conversation_id = ?1 AND root.runtime_role = (SELECT CASE kind WHEN 'coordinator' THEN 'coordinator' ELSE 'user' END FROM product_conversations WHERE id = ?1) AND root.parent_conversation_id IS NULL
     AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id)
     UNION ALL SELECT successor.id, transcript.ordinal + 1 FROM transcript JOIN conversations predecessor ON predecessor.id = transcript.id JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
     WHERE successor.product_conversation_id = ?1 AND successor.runtime_role = (SELECT CASE kind WHEN 'coordinator' THEN 'coordinator' ELSE 'user' END FROM product_conversations WHERE id = ?1) AND successor.parent_conversation_id IS NULL
 ), snapshot_ceiling AS (
     SELECT json_extract(value, '$.transcript_row_id') AS transcript_row_id,
            json_extract(value, '$.tail_sequence_id') AS tail_sequence_id,
            json_extract(value, '$.tail_message_id') AS tail_message_id
     FROM json_each(?2)
 ) SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at, origin_kind, origin_product_conversation_id, origin_transcript_id, origin_subscription_event_id, origin_source_message_id, origin_source_tool_use_id, transcript.ordinal
 FROM transcript
 CROSS JOIN messages INDEXED BY messages_conversation_sequence
   ON messages.conversation_id = transcript.id
 WHERE (?2 IS NULL OR EXISTS (SELECT 1 FROM snapshot_ceiling ceiling WHERE ceiling.transcript_row_id = transcript.id AND (messages.sequence_id < ceiling.tail_sequence_id OR (messages.sequence_id = ceiling.tail_sequence_id AND messages.message_id <= ceiling.tail_message_id))))
   AND (?3 IS NULL OR transcript.ordinal < ?3 OR (transcript.ordinal = ?3 AND (messages.sequence_id < ?4 OR (messages.sequence_id = ?4 AND messages.message_id < ?5))))
 ORDER BY transcript.ordinal DESC, messages.sequence_id DESC, messages.message_id DESC LIMIT ?6";

const PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL: &str = "WITH RECURSIVE transcript(id, ordinal) AS (
     SELECT root.id, 0 FROM conversations root WHERE root.product_conversation_id = ?1 AND root.runtime_role = (SELECT CASE kind WHEN 'coordinator' THEN 'coordinator' ELSE 'user' END FROM product_conversations WHERE id = ?1) AND root.parent_conversation_id IS NULL
     AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id)
     UNION ALL SELECT successor.id, transcript.ordinal + 1 FROM transcript JOIN conversations predecessor ON predecessor.id = transcript.id JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
     WHERE successor.product_conversation_id = ?1 AND successor.runtime_role = (SELECT CASE kind WHEN 'coordinator' THEN 'coordinator' ELSE 'user' END FROM product_conversations WHERE id = ?1) AND successor.parent_conversation_id IS NULL
 ), snapshot_ceiling AS MATERIALIZED (
     SELECT json_extract(value, '$.transcript_row_id') AS transcript_row_id,
            json_extract(value, '$.tail_sequence_id') AS tail_sequence_id,
            json_extract(value, '$.tail_message_id') AS tail_message_id
     FROM json_each(?2)
 ), selected AS MATERIALIZED (
     SELECT messages.message_id, messages.conversation_id, messages.sequence_id, transcript.ordinal
     FROM transcript
     CROSS JOIN messages INDEXED BY messages_conversation_sequence
       ON messages.conversation_id = transcript.id
     WHERE (?2 IS NULL OR EXISTS (SELECT 1 FROM snapshot_ceiling ceiling WHERE ceiling.transcript_row_id = transcript.id AND (messages.sequence_id < ceiling.tail_sequence_id OR (messages.sequence_id = ceiling.tail_sequence_id AND messages.message_id <= ceiling.tail_message_id))))
       AND (?3 IS NULL OR transcript.ordinal < ?3 OR (transcript.ordinal = ?3 AND (messages.sequence_id < ?4 OR (messages.sequence_id = ?4 AND messages.message_id < ?5))))
     ORDER BY transcript.ordinal DESC, messages.sequence_id DESC, messages.message_id DESC LIMIT ?6
 ) SELECT m.message_id, m.conversation_id, m.sequence_id, m.message_type, m.content, m.display_data, m.usage_data, m.created_at, m.origin_kind, m.origin_product_conversation_id, m.origin_transcript_id, m.origin_subscription_event_id, m.origin_source_message_id, m.origin_source_tool_use_id, selected.ordinal
 FROM selected JOIN messages m ON m.message_id = selected.message_id
 ORDER BY selected.ordinal DESC, selected.sequence_id DESC, selected.message_id DESC";

impl Database {
    /// Returns the accepted opening message ID for a completed continuation edge.
    ///
    /// # Errors
    /// Returns an error if the provenance query fails.
    pub async fn accepted_continuation_handoff_message_id(
        &self,
        conv_id: &str,
    ) -> DbResult<Option<String>> {
        sqlx::query_scalar(
            "SELECT accepted_successor_message_id FROM completed_continuation_handoffs
             WHERE successor_conversation_id = ?1",
        )
        .bind(conv_id)
        .fetch_optional(self.pool())
        .await
        .map_err(DbError::from)
    }

    /// Resolves one ordinary aggregate reference.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the reference is absent or excluded,
    /// and a database or decode error when persisted aggregate data is invalid.
    pub async fn resolve_ordinary_product_conversation(
        &self,
        reference: &str,
    ) -> DbResult<ResolvedProductConversation> {
        let row = sqlx::query(
            "SELECT c.product_conversation_id, c.id
             FROM conversations c
             JOIN product_conversations p ON p.id = c.product_conversation_id
             WHERE p.kind = 'ordinary'
               AND c.runtime_role = 'user'
               AND c.parent_conversation_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM product_creation_jobs creation
                   WHERE creation.product_conversation_id = p.id AND creation.status <> 'published')
               AND (p.id = ?1 OR c.id = ?1 OR c.slug = ?1)
             ORDER BY CASE WHEN p.id = ?1 THEN 0 WHEN c.id = ?1 THEN 1 ELSE 2 END, c.id ASC
             LIMIT 1",
        )
        .bind(reference)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(DbError::ConversationNotFound(reference.to_string()));
        };
        let product_conversation_id = row
            .try_get::<String, _>("product_conversation_id")?
            .parse::<ProductConversationId>()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        Ok(ResolvedProductConversation {
            product_conversation_id,
            requested_transcript_row_id: row.try_get("id")?,
        })
    }

    /// Sets the user-visible title on the canonical root transcript row for one ordinary aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the reference is absent or excluded,
    /// [`DbError::ProductConversationUnavailable`] when the aggregate is in History,
    /// and a database or decode error when persisted aggregate data is invalid.
    pub async fn set_ordinary_product_conversation_title(
        &self,
        reference: &str,
        title: &str,
    ) -> DbResult<ProductConversationId> {
        self.mutate_ordinary_product_conversation_title_authority(reference, title)
            .await
    }

    /// Sets the compatibility chain-name override without replacing the aggregate title fallback.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the reference is absent or excluded,
    /// [`DbError::ProductConversationUnavailable`] when the aggregate is in History,
    /// and a database or decode error when persisted aggregate data is invalid.
    pub async fn set_ordinary_product_conversation_legacy_title(
        &self,
        reference: &str,
        title: &str,
    ) -> DbResult<ProductConversationId> {
        self.mutate_ordinary_product_conversation_legacy_title_authority(reference, Some(title))
            .await
    }

    /// Clears the legacy name override while preserving the aggregate title.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the reference is absent or excluded,
    /// [`DbError::ProductConversationUnavailable`] when the aggregate is in History,
    /// and a database or decode error when persisted aggregate data is invalid.
    pub async fn clear_ordinary_product_conversation_legacy_title(
        &self,
        reference: &str,
    ) -> DbResult<ProductConversationId> {
        self.mutate_ordinary_product_conversation_legacy_title_authority(reference, None)
            .await
    }

    async fn mutate_ordinary_product_conversation_legacy_title_authority(
        &self,
        reference: &str,
        title: Option<&str>,
    ) -> DbResult<ProductConversationId> {
        let mut connection = self.pool.acquire().await?;
        let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
        let now = chrono::Utc::now();
        let result = async {
            let resolved =
                Self::resolve_ordinary_product_conversation_on(&mut transaction, reference).await?;
            crate::close_foundation::require_product_conversation_admission_tx(
                &mut transaction,
                &resolved.requested_transcript_row_id,
            )
            .await?;
            let result = sqlx::query(
                "UPDATE conversations
                 SET chain_name = ?1, updated_at = ?2
                 WHERE product_conversation_id = ?3
                   AND user_initiated = 1
                   AND runtime_role = 'user'
                   AND parent_conversation_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations predecessor
                       WHERE predecessor.product_conversation_id = conversations.product_conversation_id
                         AND predecessor.continued_in_conv_id = conversations.id
                   )",
            )
            .bind(title)
            .bind(now.to_rfc3339())
            .bind(resolved.product_conversation_id.as_str())
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DbError::ConversationNotFound(reference.to_string()));
            }
            Ok::<ProductConversationId, DbError>(resolved.product_conversation_id)
        }
        .await;
        match result {
            Ok(product_conversation_id) => {
                transaction.commit().await?;
                Ok(product_conversation_id)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    async fn mutate_ordinary_product_conversation_title_authority(
        &self,
        reference: &str,
        title: &str,
    ) -> DbResult<ProductConversationId> {
        let mut connection = self.pool.acquire().await?;
        let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
        let now = chrono::Utc::now();
        let result = async {
            let resolved =
                Self::resolve_ordinary_product_conversation_on(&mut transaction, reference).await?;
            crate::close_foundation::require_product_conversation_admission_tx(
                &mut transaction,
                &resolved.requested_transcript_row_id,
            )
            .await?;
            let row = sqlx::query(
                "SELECT root.id
                 FROM conversations root
                 WHERE root.product_conversation_id = ?1
                   AND root.user_initiated = 1
                   AND root.runtime_role = 'user'
                   AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations predecessor
                       WHERE predecessor.product_conversation_id = root.product_conversation_id
                         AND predecessor.continued_in_conv_id = root.id
                   )
                 ORDER BY root.id ASC
                 LIMIT 1",
            )
            .bind(resolved.product_conversation_id.as_str())
            .fetch_optional(&mut *transaction)
            .await?;
            let Some(row) = row else {
                return Err(DbError::ConversationNotFound(reference.to_string()));
            };
            let root_id: String = row.try_get("id")?;
            let result = sqlx::query(
                "UPDATE conversations
                 SET title = ?1, chain_name = NULL, updated_at = ?2
                 WHERE id = ?3",
            )
            .bind(title)
            .bind(now.to_rfc3339())
            .bind(&root_id)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() == 0 {
                return Err(DbError::ConversationNotFound(reference.to_string()));
            }
            Ok::<ProductConversationId, DbError>(resolved.product_conversation_id)
        }
        .await;
        match result {
            Ok(product_conversation_id) => {
                transaction.commit().await?;
                Ok(product_conversation_id)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// Lists the one-row sidebar projection for every ordinary product conversation.
    /// This deliberately does not hydrate each aggregate's transcript or handoff history.
    ///
    /// # Errors
    ///
    /// Returns a database or decode error when a projection cannot be read.
    pub async fn list_ordinary_product_conversation_projections(
        &self,
    ) -> DbResult<Vec<ProductConversationListProjection>> {
        let rows = sqlx::query(
            "WITH RECURSIVE transcript(product_conversation_id, id, ordinal) AS (
                 SELECT root.product_conversation_id, root.id, 0
                 FROM conversations root
                 JOIN product_conversations product ON product.id = root.product_conversation_id
                 WHERE product.kind = 'ordinary'
                   AND root.user_initiated = 1
                   AND root.runtime_role = 'user'
                   AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM product_creation_jobs creation
                       WHERE creation.product_conversation_id = root.product_conversation_id
                         AND creation.status <> 'published'
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM conversation_creation_jobs creation
                       WHERE creation.conversation_id = root.id
                         AND creation.status <> 'ready'
                   )
                   AND NOT (root.archived = 1 AND EXISTS (
                       SELECT 1 FROM conversation_creation_jobs job
                       WHERE job.conversation_id = root.id AND job.status = 'deletion_pending'
                   ))
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations predecessor
                       WHERE predecessor.product_conversation_id = root.product_conversation_id
                         AND predecessor.continued_in_conv_id = root.id
                   )
                 UNION ALL
                 SELECT transcript.product_conversation_id, successor.id, transcript.ordinal + 1
                 FROM transcript
                 JOIN conversations predecessor ON predecessor.id = transcript.id
                 JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
                 WHERE successor.runtime_role = 'user'
                   AND successor.parent_conversation_id IS NULL
             ), ranked AS (
                 SELECT transcript.*,
                        ROW_NUMBER() OVER (PARTITION BY product_conversation_id ORDER BY ordinal) AS root_rank,
                        ROW_NUMBER() OVER (PARTITION BY product_conversation_id ORDER BY ordinal DESC) AS latest_rank
                 FROM transcript
             ), activity AS (
                 SELECT transcript.product_conversation_id, MAX(conversation.updated_at) AS updated_at,
                        MAX(conversation.state_kind = 'awaiting_task_approval') AS has_awaiting_task_approval,
                        MAX(conversation.state_kind = 'awaiting_continuation') AS has_awaiting_continuation
                 FROM transcript JOIN conversations conversation ON conversation.id = transcript.id
                 GROUP BY transcript.product_conversation_id
             )
             SELECT product.id AS product_conversation_id, product.ordinary_lifecycle,
                    root.id AS root_transcript_row_id, root.slug AS root_slug,
                    COALESCE(root.chain_name, root.title) AS root_title,
                    latest.id AS latest_transcript_row_id, latest.state AS latest_state,
                    latest.continued_in_conv_id AS latest_continued_in_conv_id,
                    activity.updated_at, activity.has_awaiting_task_approval,
                    activity.has_awaiting_continuation,
                    EXISTS (
                        SELECT 1 FROM close_obligations obligation
                        WHERE obligation.product_conversation_id = product.id
                          AND (obligation.phase <> 'completed' OR obligation.close_outcome = 'close_incomplete')
                    ) AS has_active_close_attempt
             FROM product_conversations product
             JOIN ranked root_ranked ON root_ranked.product_conversation_id = product.id AND root_ranked.root_rank = 1
             JOIN ranked latest_ranked ON latest_ranked.product_conversation_id = product.id AND latest_ranked.latest_rank = 1
             JOIN conversations root ON root.id = root_ranked.id
             JOIN conversations latest ON latest.id = latest_ranked.id
             JOIN activity ON activity.product_conversation_id = product.id
             WHERE product.kind = 'ordinary'
             ORDER BY activity.updated_at DESC, product.id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(list_projection_from_row).collect()
    }

    /// Reads citation metadata without hydrating the aggregate transcript.
    ///
    /// # Errors
    ///
    /// Returns a database or identity decode error when citation metadata cannot be read.
    pub async fn ordinary_product_conversation_citation(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<OrdinaryProductConversationCitation>> {
        let row = sqlx::query(
            "SELECT product.id AS product_conversation_id, \
                    COALESCE(root.chain_name, root.title, root.slug, root.id) AS root_title \
             FROM product_conversations product \
             JOIN conversations root ON root.product_conversation_id = product.id \
             WHERE product.id = ?1 AND product.kind = 'ordinary' \
               AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL \
               AND NOT EXISTS ( \
                   SELECT 1 FROM conversations predecessor \
                   WHERE predecessor.product_conversation_id = product.id \
                     AND predecessor.continued_in_conv_id = root.id \
               ) \
             LIMIT 1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(OrdinaryProductConversationCitation {
                product_conversation_id: ProductConversationId::parse(
                    row.try_get::<String, _>("product_conversation_id")?,
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?,
                root_title: row.try_get("root_title")?,
            })
        })
        .transpose()
    }

    /// Reads the persisted kind for a live `ProductConversation` identity.
    ///
    /// # Errors
    ///
    /// Returns a database or decode error when the persisted kind cannot be read.
    pub async fn product_conversation_kind(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<ProductConversationKind>> {
        let kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM product_conversations WHERE id = ?1")
                .bind(product_conversation_id.as_str())
                .fetch_optional(&self.pool)
                .await?;
        kind.map(|kind| {
            ProductConversationKind::from_db_str(&kind).ok_or_else(|| {
                DbError::Serialization(format!("unknown product conversation kind: {kind}"))
            })
        })
        .transpose()
    }

    /// Reads one ordinary aggregate from its durable product identity.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the aggregate is absent or not ordinary,
    /// and a database or decode error when its persisted topology is invalid.
    #[allow(clippy::too_many_lines)]
    pub async fn get_ordinary_product_conversation(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<ProductConversationAggregate> {
        let product =
            sqlx::query("SELECT kind, ordinary_lifecycle FROM product_conversations p WHERE id = ?1
                AND NOT EXISTS (SELECT 1 FROM product_creation_jobs creation
                    WHERE creation.product_conversation_id = p.id AND creation.status <> 'published')")
                .bind(product_conversation_id.as_str())
                .fetch_optional(&self.pool)
                .await?;
        let Some(product) = product else {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        };
        let kind: String = product.try_get("kind")?;
        if ProductConversationKind::from_db_str(&kind) != Some(ProductConversationKind::Ordinary) {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        }
        let lifecycle: String = product.try_get("ordinary_lifecycle")?;
        let lifecycle =
            OrdinaryProductConversationLifecycle::from_db_str(&lifecycle).ok_or_else(|| {
                DbError::Serialization(format!("unknown ordinary lifecycle: {lifecycle}"))
            })?;

        let rows = sqlx::query(
            "WITH RECURSIVE transcript(id, ordinal) AS (
                 SELECT root.id, 0
                 FROM conversations root
                 WHERE root.product_conversation_id = ?1
                   AND root.runtime_role = 'user'
                   AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations predecessor
                       WHERE predecessor.product_conversation_id = root.product_conversation_id
                         AND predecessor.continued_in_conv_id = root.id
                   )
                 UNION ALL
                 SELECT successor.id, transcript.ordinal + 1
                 FROM transcript
                 JOIN conversations predecessor ON predecessor.id = transcript.id
                 JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
                 WHERE successor.product_conversation_id = ?1
                   AND successor.runtime_role = 'user'
                   AND successor.parent_conversation_id IS NULL
             )
             SELECT transcript.id, transcript.ordinal,
                    COALESCE((
                        SELECT MAX(message.sequence_id)
                        FROM messages message
                        WHERE message.conversation_id = transcript.id
                    ), 0) AS tail_sequence_id,
                    (SELECT message.message_id
                     FROM messages message
                     WHERE message.conversation_id = transcript.id
                     ORDER BY message.sequence_id DESC, message.message_id DESC
                     LIMIT 1) AS tail_message_id,
                    conversation.work_scope_id, environment.environment_kind,
                    environment.worktree_path, environment.branch_name, environment.base_branch,
                    COALESCE(objective.task_id, conversation.cm_task_id) AS objective_task_id,
                    COALESCE(objective.task_title, conversation.cm_task_title) AS objective_task_title
             FROM transcript
             JOIN conversations conversation ON conversation.id = transcript.id
             LEFT JOIN work_scope_environments environment
               ON environment.work_scope_id = conversation.work_scope_id
             LEFT JOIN work_scope_approved_task_authorities authority
               ON authority.work_scope_id = conversation.work_scope_id
             LEFT JOIN conversation_approved_task_objectives objective
               ON objective.conversation_id = authority.objective_conversation_id
             ORDER BY ordinal",
        )
        .bind(product_conversation_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        let mut transcript_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let ordinal: i64 = row.try_get("ordinal")?;
            transcript_rows.push(ProductConversationTranscriptRow {
                conversation: self.get_conversation(&id).await?,
                segment_ordinal: ordinal,
                tail_sequence_id: row.try_get("tail_sequence_id")?,
                tail_message_id: row.try_get("tail_message_id")?,
                work_identity: work_identity_from_row(&row)?,
            });
        }
        let Some(root) = transcript_rows.first().cloned() else {
            return Err(DbError::Serialization(format!(
                "ordinary ProductConversation {product_conversation_id} has no parent transcript root"
            )));
        };
        let mut segments = Vec::with_capacity(transcript_rows.len());
        for transcript_row in transcript_rows {
            debug_assert_eq!(transcript_row.conversation.runtime_role, RuntimeRole::User);
            let handoff = match transcript_row.conversation.continued_in_conv_id.as_deref() {
                Some(successor_transcript_row_id) => {
                    self.continuation_handoff(
                        &transcript_row.conversation.id,
                        successor_transcript_row_id,
                    )
                    .await?
                }
                None => None,
            };
            segments.push(ProductConversationSegment {
                transcript_row,
                handoff,
            });
        }
        let latest = segments
            .last()
            .ok_or_else(|| DbError::Serialization("aggregate has no segments".to_string()))?;
        let updated_at = segments
            .iter()
            .map(|segment| segment.transcript_row.conversation.updated_at)
            .max()
            .ok_or_else(|| {
                DbError::Serialization("aggregate has no updated timestamp".to_string())
            })?;
        let source = self
            .product_conversation_source(product_conversation_id)
            .await?;
        Ok(ProductConversationAggregate {
            product_conversation: ProductConversation::ordinary(
                product_conversation_id.clone(),
                lifecycle,
            ),
            root,
            latest_transcript_row_id: latest.transcript_row.conversation.id.clone(),
            updated_at,
            segments,
            source,
        })
    }

    /// Reads topology, cursor watermarks, and its bounded message page from one `SQLite` snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] when the reference is absent or excluded,
    /// and a database or decode error when the snapshot cannot be projected.
    pub async fn read_ordinary_product_conversation_snapshot(
        &self,
        reference: &str,
        before: Option<(i64, i64, String)>,
        segment_ceilings: Option<&[ProductConversationSegmentCeiling]>,
        limit: usize,
    ) -> DbResult<ProductConversationSnapshotRead> {
        let mut connection = self
            .pool
            .acquire()
            .instrument(tracing::info_span!("product_conversation.pool_wait"))
            .await?;
        let mut tx = connection
            .begin()
            .instrument(tracing::info_span!("product_conversation.begin_read"))
            .await?;
        let result = async {
            let resolved = Self::resolve_ordinary_product_conversation_on(&mut tx, reference)
                .instrument(tracing::info_span!("product_conversation.resolve"))
                .await?;
            tracing::Span::current().record(
                "product.reference",
                resolved.product_conversation_id.as_str(),
            );
            let aggregate = Self::get_ordinary_product_conversation_on(
                &mut tx,
                &resolved.product_conversation_id,
            )
            .instrument(tracing::info_span!("product_conversation.aggregate"))
            .await?;
            let latest_opening =
                Self::read_continuation_opening_on(&mut tx, &aggregate.latest_transcript_row_id)
                    .await?;
            let close = Self::get_active_close_projection_for_product_on(
                &mut tx,
                aggregate.product_conversation.id(),
            )
            .instrument(tracing::info_span!("product_conversation.close"))
            .await?;
            let messages = Self::get_product_conversation_messages_page_on(
                &mut tx,
                aggregate.product_conversation.id(),
                before,
                segment_ceilings,
                limit,
            )
            .instrument(tracing::info_span!("product_conversation.message_page"))
            .await?;
            Ok(ProductConversationSnapshotRead {
                aggregate,
                latest_opening,
                close,
                messages,
                requested_transcript_row_id: resolved.requested_transcript_row_id,
            })
        }
        .instrument(tracing::info_span!(
            "product_conversation.snapshot_read",
            "product.reference" = tracing::field::Empty,
        ))
        .await;
        tx.rollback()
            .instrument(tracing::info_span!("product_conversation.rollback_read"))
            .await?;
        result
    }

    /// Reads bounded Global history without ordinary lifecycle or Close authority.
    ///
    /// # Errors
    /// Returns an error for absent/non-Coordinator references or invalid persisted topology.
    pub async fn read_coordinator_product_conversation_snapshot(
        &self,
        reference: &str,
        before: Option<(i64, i64, String)>,
        segment_ceilings: Option<&[ProductConversationSegmentCeiling]>,
        limit: usize,
    ) -> DbResult<CoordinatorConversationSnapshotRead> {
        let mut connection = self.pool.acquire().await?;
        let mut tx = connection.begin().await?;
        let result = async {
            let resolved = Self::resolve_product_conversation_on(
                &mut tx,
                reference,
                ProductConversationKind::Coordinator,
            )
            .await?;
            let aggregate = Self::get_product_conversation_on(
                &mut tx,
                &resolved.product_conversation_id,
                ProductConversationKind::Coordinator,
            )
            .await?;
            let latest_opening =
                Self::read_continuation_opening_on(&mut tx, &aggregate.latest_transcript_row_id)
                    .await?;
            let messages = Self::get_product_conversation_messages_page_on(
                &mut tx,
                aggregate.product_conversation.id(),
                before,
                segment_ceilings,
                limit,
            )
            .await?;
            Ok(CoordinatorConversationSnapshotRead {
                aggregate,
                latest_opening,
                messages,
                requested_transcript_row_id: resolved.requested_transcript_row_id,
            })
        }
        .await;
        tx.rollback().await?;
        result
    }

    async fn resolve_ordinary_product_conversation_on(
        connection: &mut SqliteConnection,
        reference: &str,
    ) -> DbResult<ResolvedProductConversation> {
        Self::resolve_product_conversation_on(
            connection,
            reference,
            ProductConversationKind::Ordinary,
        )
        .await
    }

    async fn resolve_product_conversation_on(
        connection: &mut SqliteConnection,
        reference: &str,
        kind: ProductConversationKind,
    ) -> DbResult<ResolvedProductConversation> {
        let row = sqlx::query(
            "SELECT c.product_conversation_id, c.id
             FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
             WHERE p.kind = ?2 AND c.runtime_role = ?3 AND c.parent_conversation_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM product_creation_jobs creation
                   WHERE creation.product_conversation_id = p.id AND creation.status <> 'published')
               AND (p.id = ?1 OR c.id = ?1 OR c.slug = ?1)
             ORDER BY CASE WHEN p.id = ?1 THEN 0 WHEN c.id = ?1 THEN 1 ELSE 2 END,
                      c.created_at ASC, c.id ASC LIMIT 1",
        )
        .bind(reference)
        .bind(kind.as_str())
        .bind(match kind {
            ProductConversationKind::Ordinary => "user",
            ProductConversationKind::Coordinator => "coordinator",
        })
        .fetch_optional(&mut *connection)
        .await?;
        let Some(row) = row else {
            return Err(DbError::ConversationNotFound(reference.to_string()));
        };
        Ok(ResolvedProductConversation {
            product_conversation_id: row
                .try_get::<String, _>("product_conversation_id")?
                .parse::<ProductConversationId>()
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            requested_transcript_row_id: row.try_get("id")?,
        })
    }

    async fn get_ordinary_product_conversation_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<ProductConversationAggregate> {
        Self::get_product_conversation_on(
            connection,
            product_conversation_id,
            ProductConversationKind::Ordinary,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn get_product_conversation_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
        kind: ProductConversationKind,
    ) -> DbResult<ProductConversationAggregate> {
        let product =
            sqlx::query("SELECT kind, ordinary_lifecycle FROM product_conversations p WHERE id = ?1
                AND NOT EXISTS (SELECT 1 FROM product_creation_jobs creation
                    WHERE creation.product_conversation_id = p.id AND creation.status <> 'published')")
                .bind(product_conversation_id.as_str())
                .fetch_optional(&mut *connection)
                .await?;
        let Some(product) = product else {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        };
        if ProductConversationKind::from_db_str(&product.try_get::<String, _>("kind")?)
            != Some(kind)
        {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        }
        let product_conversation = match kind {
            ProductConversationKind::Ordinary => {
                let lifecycle = product.try_get::<String, _>("ordinary_lifecycle")?;
                let lifecycle = OrdinaryProductConversationLifecycle::from_db_str(&lifecycle)
                    .ok_or_else(|| {
                        DbError::Serialization(format!("unknown ordinary lifecycle: {lifecycle}"))
                    })?;
                ProductConversation::ordinary(product_conversation_id.clone(), lifecycle)
            }
            ProductConversationKind::Coordinator => {
                ProductConversation::coordinator(product_conversation_id.clone())
            }
        };
        let runtime_role = match kind {
            ProductConversationKind::Ordinary => "user",
            ProductConversationKind::Coordinator => "coordinator",
        };
        let rows = sqlx::query(
            "WITH RECURSIVE transcript(id, ordinal) AS (
                 SELECT root.id, 0 FROM conversations root WHERE root.product_conversation_id = ?1
                   AND root.runtime_role = ?2 AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id)
                 UNION ALL
                 SELECT successor.id, transcript.ordinal + 1 FROM transcript
                 JOIN conversations predecessor ON predecessor.id = transcript.id
                 JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
                 WHERE successor.product_conversation_id = ?1 AND successor.runtime_role = ?2 AND successor.parent_conversation_id IS NULL
             ) SELECT transcript.id, transcript.ordinal,
                    COALESCE((SELECT MAX(message.sequence_id) FROM messages message WHERE message.conversation_id = transcript.id), 0) AS tail_sequence_id,
                    (SELECT message.message_id FROM messages message
                     WHERE message.conversation_id = transcript.id
                     ORDER BY message.sequence_id DESC, message.message_id DESC LIMIT 1) AS tail_message_id,
                    conversation.work_scope_id, environment.environment_kind,
                    environment.worktree_path, environment.branch_name, environment.base_branch,
                    COALESCE(objective.task_id, conversation.cm_task_id) AS objective_task_id,
                    COALESCE(objective.task_title, conversation.cm_task_title) AS objective_task_title
             FROM transcript
             JOIN conversations conversation ON conversation.id = transcript.id
             LEFT JOIN work_scope_environments environment
               ON environment.work_scope_id = conversation.work_scope_id
             LEFT JOIN work_scope_approved_task_authorities authority
               ON authority.work_scope_id = conversation.work_scope_id
             LEFT JOIN conversation_approved_task_objectives objective
               ON objective.conversation_id = authority.objective_conversation_id
             ORDER BY ordinal",
        ).bind(product_conversation_id.as_str()).bind(runtime_role).fetch_all(&mut *connection).await?;
        let mut transcript_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            transcript_rows.push(ProductConversationTranscriptRow {
                conversation: Self::get_conversation_on(connection, &id).await?,
                segment_ordinal: row.try_get("ordinal")?,
                tail_sequence_id: row.try_get("tail_sequence_id")?,
                tail_message_id: row.try_get("tail_message_id")?,
                work_identity: work_identity_from_row(&row)?,
            });
        }
        let Some(root) = transcript_rows.first().cloned() else {
            return Err(DbError::Serialization(format!(
                "ordinary ProductConversation {product_conversation_id} has no parent transcript root"
            )));
        };
        let mut segments = Vec::with_capacity(transcript_rows.len());
        for transcript_row in transcript_rows {
            let handoff = match transcript_row.conversation.continued_in_conv_id.as_deref() {
                Some(successor) => {
                    Self::continuation_handoff_on(
                        connection,
                        &transcript_row.conversation.id,
                        successor,
                    )
                    .await?
                }
                None => None,
            };
            segments.push(ProductConversationSegment {
                transcript_row,
                handoff,
            });
        }
        let latest = segments
            .last()
            .ok_or_else(|| DbError::Serialization("aggregate has no segments".to_string()))?;
        let updated_at = segments
            .iter()
            .map(|segment| segment.transcript_row.conversation.updated_at)
            .max()
            .ok_or_else(|| {
                DbError::Serialization("aggregate has no updated timestamp".to_string())
            })?;
        let source =
            Self::product_conversation_source_on(connection, product_conversation_id).await?;
        Ok(ProductConversationAggregate {
            product_conversation,
            root,
            latest_transcript_row_id: latest.transcript_row.conversation.id.clone(),
            updated_at,
            segments,
            source,
        })
    }

    async fn get_conversation_on(
        connection: &mut SqliteConnection,
        id: &str,
    ) -> DbResult<crate::Conversation> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier, c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id, c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id WHERE c.id = ?1",
        ).bind(id).try_map(super::parse_conversation_row).fetch_one(&mut *connection).await.map_err(|error| {
            if matches!(error, sqlx::Error::RowNotFound) { DbError::ConversationNotFound(id.to_string()) } else { DbError::Sqlx(error) }
        })
    }

    /// Returns the persisted boundary for a continuation edge.
    ///
    /// Edges recorded after migration 074 retain their acceptance-time successor
    /// message. Older edges have no such relation, but their persisted predecessor
    /// continuation remains the historical boundary; no successor message is inferred.
    /// Reads one transcript's opening authority in a consistent read transaction.
    ///
    /// # Errors
    /// Invalid persisted edges and unreadable receipts are errors, never absence.
    pub async fn read_continuation_opening(
        &self,
        successor: &str,
    ) -> DbResult<ContinuationOpeningRead> {
        let mut tx = self.pool.begin().await?;
        let result = Self::read_continuation_opening_on(&mut tx, successor).await;
        tx.rollback().await?;
        result
    }

    /// Reads an opening observation only for the exact committed receipt message.
    ///
    /// # Errors
    /// Read and provenance failures remain errors.
    pub async fn read_message_continuation_opening(
        &self,
        message_id: &str,
    ) -> DbResult<Option<ContinuationOpeningRead>> {
        let mut tx = self.pool.begin().await?;
        let result = async {
            let successor: Option<String> = sqlx::query_scalar(
                "SELECT successor_conversation_id FROM completed_continuation_handoffs
                 WHERE accepted_successor_message_id = ?1",
            )
            .bind(message_id)
            .fetch_optional(&mut *tx)
            .await?;
            match successor {
                Some(successor) => Self::read_continuation_opening_on(&mut tx, &successor)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
        .await;
        tx.rollback().await?;
        result
    }

    /// Reads a current opening observation or a coherent committed replacement.
    ///
    /// # Errors
    /// Missing/future generation, wrong current content, or invalid receipts require reconciliation.
    pub async fn read_message_continuation_opening_at_update(
        &self,
        message_id: &str,
        generation: Option<i64>,
        content: &MessageContent,
    ) -> DbResult<Option<ContinuationOpeningUpdateRead>> {
        let mut tx = self.pool.begin().await?;
        let result = async {
            let row = sqlx::query(
                "SELECT handoff.successor_conversation_id, conversation.transcript_generation,
                        message.*
                 FROM completed_continuation_handoffs handoff
                 JOIN messages message ON message.message_id = handoff.accepted_successor_message_id
                 JOIN conversations conversation ON conversation.id = message.conversation_id
                 WHERE handoff.accepted_successor_message_id = ?1",
            )
            .bind(message_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Ok(None);
            };
            let actual_generation: i64 = row.try_get("transcript_generation")?;
            let actual_content = message_content_from_row(&row, "message_type", "content")?;
            let successor: String = row.try_get("successor_conversation_id")?;
            let mut message = super::parse_message_row(row)?;
            message.content = actual_content;
            super::hydrate_attachments_conn(&mut tx, std::slice::from_mut(&mut message)).await?;
            let actual_content = message.content;
            let Some(generation) = generation else {
                return Err(DbError::TranscriptSnapshotChanged);
            };
            if generation < 0
                || generation > actual_generation
                || (generation == actual_generation && content != &actual_content)
            {
                return Err(DbError::TranscriptSnapshotChanged);
            }
            let opening = Self::read_continuation_opening_on(&mut tx, &successor).await?;
            Ok(Some(if generation == actual_generation {
                ContinuationOpeningUpdateRead::Current(opening)
            } else {
                ContinuationOpeningUpdateRead::Superseded {
                    content: actual_content,
                    transcript_generation: actual_generation,
                    display_data: message.display_data,
                    opening,
                }
            }))
        }
        .await;
        tx.rollback().await?;
        result
    }

    /// Fences an opening read against the transcript selected for SSE init.
    ///
    /// # Errors
    /// Concurrent append/mutation or invalid authority cannot produce a snapshot.
    pub async fn read_continuation_opening_at_snapshot(
        &self,
        successor: &str,
        generation: i64,
        tail: Option<i64>,
    ) -> DbResult<ContinuationOpeningRead> {
        let mut tx = self.pool.begin().await?;
        let result = async {
            let actual: (i64, Option<i64>) = sqlx::query_as(
                "SELECT transcript_generation,
                        (SELECT MAX(sequence_id) FROM messages WHERE conversation_id = conversations.id)
                 FROM conversations WHERE id = ?1",
            ).bind(successor).fetch_one(&mut *tx).await?;
            if actual != (generation, tail) {
                return Err(DbError::TranscriptSnapshotChanged);
            }
            Self::read_continuation_opening_on(&mut tx, successor).await
        }.await;
        tx.rollback().await?;
        result
    }

    #[allow(clippy::too_many_lines)]
    async fn read_continuation_opening_on(
        connection: &mut SqliteConnection,
        successor: &str,
    ) -> DbResult<ContinuationOpeningRead> {
        let pending = sqlx::query(
            "SELECT parent_conversation_id, message_id, opening_authority
             FROM continuation_dispatch_intents WHERE successor_conversation_id = ?1",
        )
        .bind(successor)
        .fetch_optional(&mut *connection)
        .await?;
        let completed = sqlx::query(
            "SELECT predecessor_conversation_id, continuation_message_id,
                    accepted_successor_message_id, opening_authority
             FROM completed_continuation_handoffs WHERE successor_conversation_id = ?1",
        )
        .bind(successor)
        .fetch_optional(&mut *connection)
        .await?;
        let predecessor: String = if let Some(row) = &completed {
            row.try_get("predecessor_conversation_id")?
        } else if let Some(row) = &pending {
            row.try_get("parent_conversation_id")?
        } else {
            return Ok(ContinuationOpeningRead::None);
        };
        let product_id: Option<String> = sqlx::query_scalar(
            "SELECT child.product_conversation_id FROM conversations child
             JOIN conversations parent ON parent.id = ?1
             WHERE child.id = ?2 AND parent.continued_in_conv_id = child.id
               AND child.product_conversation_id = parent.product_conversation_id",
        )
        .bind(&predecessor)
        .bind(successor)
        .fetch_optional(&mut *connection)
        .await?;
        let product_conversation_id = product_id.ok_or_else(|| {
            DbError::Serialization(
                "opening receipt/intent has invalid continuation topology".into(),
            )
        })?;
        let product_conversation_id = ProductConversationId::parse(&product_conversation_id)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let authority = |row: &sqlx::sqlite::SqliteRow| -> DbResult<ContinuationOpeningAuthority> {
            ContinuationOpeningAuthority::from_db_str(
                &row.try_get::<String, _>("opening_authority")?,
            )
            .ok_or_else(|| DbError::Serialization("unknown opening authority".into()))
        };
        if let Some(receipt) = completed {
            let continuation_message_id: String = receipt.try_get("continuation_message_id")?;
            let accepted_message_id: String = receipt.try_get("accepted_successor_message_id")?;
            let opening_authority = authority(&receipt)?;
            if let Some(intent) = pending {
                let key: String = intent.try_get("message_id")?;
                if intent.try_get::<String, _>("parent_conversation_id")? != predecessor
                    || authority(&intent)? != opening_authority
                    || (accepted_message_id != key
                        && accepted_message_id != format!("{successor}:{key}"))
                {
                    return Err(DbError::Serialization(
                        "opening intent conflicts with completed receipt".into(),
                    ));
                }
            }
            let handoff =
                Self::continuation_handoff_on(connection, &predecessor, successor).await?;
            let Some(ProductConversationHandoff::Completed {
                accepted_is_duplicate_summary,
                ..
            }) = handoff
            else {
                return Err(DbError::Serialization(
                    "completed opening lacks exact persisted messages".into(),
                ));
            };
            return Ok(ContinuationOpeningRead::Accepted {
                edge: ContinuationOpeningEdge {
                    product_conversation_id,
                    predecessor_transcript_row_id: predecessor,
                    successor_transcript_row_id: successor.into(),
                    continuation_message_id,
                },
                accepted_message_id,
                opening_authority,
                presentation: if accepted_is_duplicate_summary {
                    ContinuationOpeningPresentation::Boundary
                } else {
                    ContinuationOpeningPresentation::SeparateMessage
                },
            });
        }
        let intent = pending.expect("pending or completed selected predecessor");
        let key: String = intent.try_get("message_id")?;
        let client_turn_key = phoenix_workflow::ClientTurnKey::try_from(key)
            .map_err(|error| DbError::Serialization(error.into()))?;
        let continuation_message_id: String = sqlx::query_scalar(
            "SELECT message_id FROM messages WHERE conversation_id = ?1 AND message_type = 'continuation'
             ORDER BY sequence_id DESC, message_id DESC LIMIT 1",
        ).bind(&predecessor).fetch_optional(&mut *connection).await?
            .ok_or_else(|| DbError::Serialization("pending opening lacks persisted boundary".into()))?;
        Ok(ContinuationOpeningRead::Pending {
            edge: ContinuationOpeningEdge {
                product_conversation_id,
                predecessor_transcript_row_id: predecessor,
                successor_transcript_row_id: successor.into(),
                continuation_message_id,
            },
            client_turn_key,
            opening_authority: authority(&intent)?,
        })
    }

    async fn continuation_handoff_on(
        connection: &mut SqliteConnection,
        predecessor: &str,
        successor: &str,
    ) -> DbResult<Option<ProductConversationHandoff>> {
        let completed = sqlx::query(
            "SELECT handoff.continuation_message_id, handoff.accepted_successor_message_id,
                    handoff.opening_authority,
                    continuation.content AS continuation_content,
                    accepted.message_type AS accepted_message_type,
                    accepted.content AS accepted_content
             FROM completed_continuation_handoffs handoff
             JOIN messages continuation
               ON continuation.message_id = handoff.continuation_message_id
              AND continuation.conversation_id = handoff.predecessor_conversation_id
             JOIN messages accepted
               ON accepted.message_id = handoff.accepted_successor_message_id
              AND accepted.conversation_id = handoff.successor_conversation_id
             WHERE handoff.predecessor_conversation_id = ?1
               AND handoff.successor_conversation_id = ?2",
        )
        .bind(predecessor)
        .bind(successor)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some(row) = completed {
            let summary = continuation_summary(&row, "continuation_content")?;
            let accepted_content =
                message_content_from_row(&row, "accepted_message_type", "accepted_content")?;
            let opening_authority = ContinuationOpeningAuthority::from_db_str(
                &row.try_get::<String, _>("opening_authority")?,
            )
            .ok_or_else(|| {
                DbError::Serialization("unknown continuation opening authority".to_string())
            })?;
            let accepted_is_duplicate_summary = opening_authority
                == ContinuationOpeningAuthority::GeneratedPredecessorContext
                && match accepted_content {
                    MessageContent::User(user) => user.text == summary,
                    MessageContent::Continuation(continuation) => continuation.summary == summary,
                    MessageContent::Agent(_)
                    | MessageContent::Tool(_)
                    | MessageContent::System(_)
                    | MessageContent::Error(_)
                    | MessageContent::Skill(_) => false,
                };
            return Ok(Some(ProductConversationHandoff::Completed {
                predecessor_transcript_row_id: predecessor.to_string(),
                successor_transcript_row_id: successor.to_string(),
                continuation_message_id: row.try_get("continuation_message_id")?,
                accepted_successor_message_id: row.try_get("accepted_successor_message_id")?,
                opening_authority,
                summary,
                accepted_is_duplicate_summary,
            }));
        }

        let historical = sqlx::query(
            "SELECT message_id AS continuation_message_id, content AS continuation_content
             FROM messages
             WHERE conversation_id = ?1 AND message_type = 'continuation'
             ORDER BY sequence_id DESC, message_id DESC
             LIMIT 1",
        )
        .bind(predecessor)
        .fetch_optional(&mut *connection)
        .await?;
        historical
            .map(|row| {
                Ok(ProductConversationHandoff::Historical {
                    predecessor_transcript_row_id: predecessor.to_string(),
                    successor_transcript_row_id: successor.to_string(),
                    continuation_message_id: row.try_get("continuation_message_id")?,
                    summary: continuation_summary(&row, "continuation_content")?,
                })
            })
            .transpose()
    }

    async fn continuation_handoff(
        &self,
        predecessor_transcript_row_id: &str,
        successor_transcript_row_id: &str,
    ) -> DbResult<Option<ProductConversationHandoff>> {
        let mut connection = self.pool.acquire().await?;
        Self::continuation_handoff_on(
            &mut connection,
            predecessor_transcript_row_id,
            successor_transcript_row_id,
        )
        .await
    }

    async fn product_conversation_source_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<ProductConversationSource>> {
        let row = sqlx::query("SELECT source_product_conversation_id, source_conversation_id, relation_kind, relation_key FROM product_conversation_sources WHERE target_product_conversation_id = ?1")
            .bind(product_conversation_id.as_str()).fetch_optional(&mut *connection).await?;
        row.map(|row| {
            let relation_kind: String = row.try_get("relation_kind")?;
            Ok(ProductConversationSource {
                deleted: false,
                source_product_conversation_id: row
                    .try_get::<String, _>("source_product_conversation_id")?
                    .parse::<ProductConversationId>()
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                source_conversation_id: row.try_get("source_conversation_id")?,
                relation_kind: ProductConversationSourceKind::from_db(&relation_kind).ok_or_else(
                    || {
                        DbError::Serialization(format!(
                            "unknown product conversation source kind: {relation_kind}"
                        ))
                    },
                )?,
                relation_key: row.try_get("relation_key")?,
            })
        })
        .transpose()
    }

    async fn get_product_conversation_messages_page_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
        before: Option<(i64, i64, String)>,
        segment_ceilings: Option<&[ProductConversationSegmentCeiling]>,
        limit: usize,
    ) -> DbResult<Vec<(i64, crate::Message)>> {
        let segment_ceilings = segment_ceilings
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let mut rows =
            Self::message_page_query(product_conversation_id, before, segment_ceilings, limit)
                .try_map(|row| {
                    let ordinal: i64 = row.try_get("ordinal")?;
                    super::parse_message_row(row).map(|message| (ordinal, message))
                })
                .fetch_all(&mut *connection)
                .await?;
        let mut messages = rows
            .iter()
            .map(|(_, message)| message.clone())
            .collect::<Vec<_>>();
        super::hydrate_attachments_conn(connection, &mut messages).await?;
        for ((_, message), hydrated) in rows.iter_mut().zip(messages) {
            *message = hydrated;
        }
        Ok(rows)
    }

    fn message_page_query(
        product_conversation_id: &ProductConversationId,
        before: Option<(i64, i64, String)>,
        segment_ceilings: Option<String>,
        limit: usize,
    ) -> sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
        let before_ordinal = before.as_ref().map(|(ordinal, _, _)| *ordinal);
        let before_sequence_id = before.as_ref().map(|(_, sequence_id, _)| *sequence_id);
        let before_message_id = before.map(|(_, _, message_id)| message_id);
        sqlx::query(PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL)
            .bind(product_conversation_id.as_str().to_string())
            .bind(segment_ceilings)
            .bind(before_ordinal)
            .bind(before_sequence_id)
            .bind(before_message_id)
            .bind(i64::try_from(limit).expect("page limit fits i64"))
    }

    /// Fetches one bounded, newest-first page across the ordered aggregate transcript.
    ///
    /// The cursor is a segment ordinal plus that segment's message sequence. The
    /// query deliberately limits before attachment hydration, so a long history
    /// in any member cannot make an aggregate page hydrate every member's rows.
    ///
    /// # Errors
    ///
    /// Returns a database error if the bounded message projection or attachment
    /// hydration fails.
    pub async fn get_product_conversation_messages_page(
        &self,
        product_conversation_id: &ProductConversationId,
        before: Option<(i64, i64, String)>,
        limit: usize,
    ) -> DbResult<Vec<(i64, crate::Message)>> {
        let before_ordinal = before.as_ref().map(|(ordinal, _, _)| *ordinal);
        let before_sequence_id = before.as_ref().map(|(_, sequence_id, _)| *sequence_id);
        let before_message_id = before.map(|(_, _, message_id)| message_id);
        let limit = i64::try_from(limit)
            .map_err(|error| DbError::Serialization(format!("invalid page limit: {error}")))?;
        let mut rows = sqlx::query(
            "WITH RECURSIVE transcript(id, ordinal) AS (
                 SELECT root.id, 0
                 FROM conversations root
                 WHERE root.product_conversation_id = ?1
                   AND root.runtime_role = 'user'
                   AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations predecessor
                       WHERE predecessor.product_conversation_id = root.product_conversation_id
                         AND predecessor.continued_in_conv_id = root.id
                   )
                 UNION ALL
                 SELECT successor.id, transcript.ordinal + 1
                 FROM transcript
                 JOIN conversations predecessor ON predecessor.id = transcript.id
                 JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
                 WHERE successor.product_conversation_id = ?1
                   AND successor.runtime_role = 'user'
                   AND successor.parent_conversation_id IS NULL
             )
             SELECT message_id, conversation_id, sequence_id, message_type, content,
                    display_data, usage_data, created_at, origin_kind, origin_product_conversation_id, origin_transcript_id, origin_subscription_event_id, origin_source_message_id, origin_source_tool_use_id, transcript.ordinal
             FROM transcript
             JOIN messages ON messages.conversation_id = transcript.id
             WHERE (?2 IS NULL
                    OR transcript.ordinal < ?2
                    OR (transcript.ordinal = ?2 AND (
                       messages.sequence_id < ?3
                       OR (messages.sequence_id = ?3 AND messages.message_id < ?4))))
             ORDER BY transcript.ordinal DESC, messages.sequence_id DESC, messages.message_id DESC
             LIMIT ?5",
        )
        .bind(product_conversation_id.as_str())
        .bind(before_ordinal)
        .bind(before_sequence_id)
        .bind(before_message_id)
        .bind(limit)
        .try_map(|row| {
            let ordinal: i64 = row.try_get("ordinal")?;
            super::parse_message_row(row).map(|message| (ordinal, message))
        })
        .fetch_all(&self.pool)
        .await?;
        let mut messages = rows
            .iter()
            .map(|(_, message)| message.clone())
            .collect::<Vec<_>>();
        super::hydrate_attachments(&self.pool, &mut messages).await?;
        for ((_, message), hydrated) in rows.iter_mut().zip(messages) {
            *message = hydrated;
        }
        Ok(rows)
    }

    async fn product_conversation_source(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<Option<ProductConversationSource>> {
        let row = sqlx::query(
            "SELECT source_product_conversation_id, source_conversation_id, relation_kind, relation_key
             FROM product_conversation_sources WHERE target_product_conversation_id = ?1",
        )
        .bind(product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let source_product_conversation_id = row
                .try_get::<String, _>("source_product_conversation_id")?
                .parse::<ProductConversationId>()
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let source_conversation_id: String = row.try_get("source_conversation_id")?;
            let relation_kind: String = row.try_get("relation_kind")?;
            let relation_kind =
                ProductConversationSourceKind::from_db(&relation_kind).ok_or_else(|| {
                    DbError::Serialization(format!(
                        "unknown product conversation source kind: {relation_kind}"
                    ))
                })?;
            Ok(ProductConversationSource {
                deleted: false,
                source_product_conversation_id,
                source_conversation_id,
                relation_kind,
                relation_key: row.try_get("relation_key")?,
            })
        })
        .transpose()
    }

    /// Determines whether a persisted source transcript still exists.
    ///
    /// # Errors
    ///
    /// Returns a database error when the source existence check cannot complete.
    pub async fn source_conversation_is_deleted(
        &self,
        source: &ProductConversationSource,
    ) -> DbResult<bool> {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM conversations
             WHERE id = ?1 AND product_conversation_id = ?2 AND runtime_role = 'user'
             LIMIT 1",
        )
        .bind(&source.source_conversation_id)
        .bind(source.source_product_conversation_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(exists.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContinuationContent, ContinueOutcome, ConvState, MessageContent,
        NewContinuationDispatchIntent,
    };
    use phoenix_core::domain::close::TranscriptConversationId;
    use phoenix_workflow::ClientTurnKey;
    use std::time::Instant;

    async fn full_message_page_rows(
        connection: &mut SqliteConnection,
        sql: &str,
        product_id: &str,
        before: Option<&(i64, i64, String)>,
        ceilings: Option<&str>,
        limit: i64,
    ) -> Vec<serde_json::Value> {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(product_id)
            .bind(ceilings)
            .bind(before.map(|cursor| cursor.0))
            .bind(before.map(|cursor| cursor.1))
            .bind(before.map(|cursor| cursor.2.as_str()))
            .bind(limit)
            .fetch_all(connection)
            .await
            .unwrap()
            .iter()
            .map(|row| {
                assert_eq!(row.len(), 15);
                let mut value = serde_json::Map::new();
                for column in [
                    "message_id",
                    "conversation_id",
                    "message_type",
                    "content",
                    "display_data",
                    "usage_data",
                    "created_at",
                    "origin_kind",
                    "origin_product_conversation_id",
                    "origin_transcript_id",
                    "origin_subscription_event_id",
                    "origin_source_message_id",
                    "origin_source_tool_use_id",
                ] {
                    value.insert(
                        column.into(),
                        serde_json::to_value(row.try_get::<Option<String>, _>(column).unwrap())
                            .unwrap(),
                    );
                }
                for column in ["sequence_id", "ordinal"] {
                    value.insert(column.into(), row.try_get::<i64, _>(column).unwrap().into());
                }
                serde_json::Value::Object(value)
            })
            .collect()
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn late_hydration_matches_full_row_oracle_matrix() {
        let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        // This isolated SQL fixture permits sequence ties without weakening the production schema.
        sqlx::raw_sql(
            "CREATE TABLE product_conversations (
                id TEXT PRIMARY KEY NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('ordinary', 'coordinator'))
             );
             INSERT INTO product_conversations VALUES
                ('product', 'ordinary'), ('empty-product', 'ordinary'), ('other-product', 'ordinary');
             CREATE TABLE conversations (
                id TEXT PRIMARY KEY, product_conversation_id TEXT NOT NULL,
                runtime_role TEXT NOT NULL, parent_conversation_id TEXT, continued_in_conv_id TEXT
             );
             CREATE TABLE messages (
                message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sequence_id INTEGER NOT NULL,
                message_type TEXT NOT NULL, content TEXT NOT NULL, display_data TEXT, usage_data TEXT,
                created_at TEXT NOT NULL, origin_kind TEXT NOT NULL, origin_product_conversation_id TEXT,
                origin_transcript_id TEXT, origin_subscription_event_id TEXT,
                origin_source_message_id TEXT, origin_source_tool_use_id TEXT
             );
             CREATE INDEX messages_conversation_sequence ON messages(conversation_id, sequence_id);
             INSERT INTO conversations VALUES
                ('s0', 'product', 'user', NULL, 's1'),
                ('s1', 'product', 'user', NULL, 's2'),
                ('s2', 'product', 'user', NULL, NULL),
                ('noise', 'other-product', 'user', NULL, NULL),
                ('child', 'product', 'user', 's0', NULL),
                ('agent', 'product', 'sub_agent', NULL, NULL),
                ('empty', 'empty-product', 'user', NULL, NULL);",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        let sequences = [0_i64, 7, 7, 9_007_199_254_740_993, i64::MAX];
        for segment in ["s0", "s1", "s2", "noise", "child", "agent"] {
            for (index, sequence) in sequences.iter().enumerate() {
                let id = format!("{segment}-{index}");
                let optional = |field| (index % 2 == 0).then(|| format!("{field}-{id}"));
                sqlx::query(
                    "INSERT INTO messages VALUES (?1, ?2, ?3, 'user', ?4, ?5, ?6,
                     '2026-01-01T00:00:00Z', 'internal_conversation', ?7, ?8, ?9, ?10, ?11)",
                )
                .bind(&id)
                .bind(segment)
                .bind(sequence)
                .bind(format!("{id}:{}", "payload".repeat(256)))
                .bind(optional("display"))
                .bind(optional("usage"))
                .bind(optional("product"))
                .bind(optional("transcript"))
                .bind(optional("subscription"))
                .bind(optional("source-message"))
                .bind(optional("source-tool"))
                .execute(&mut connection)
                .await
                .unwrap();
            }
        }
        let mut cursors = vec![
            None,
            Some((-1, 0, String::new())),
            Some((3, i64::MAX, "z".into())),
        ];
        for ordinal in 0..3_i64 {
            for (index, sequence) in sequences.iter().enumerate() {
                cursors.push(Some((ordinal, *sequence, format!("s{ordinal}-{index}"))));
            }
            cursors.push(Some((ordinal, 7, format!("s{ordinal}-1a"))));
        }
        let all_ceilings = (0..3)
            .map(|ordinal| ProductConversationSegmentCeiling {
                transcript_row_id: format!("s{ordinal}"),
                tail_sequence_id: i64::MAX,
                tail_message_id: Some(format!("s{ordinal}-4")),
            })
            .collect::<Vec<_>>();
        let partial_ceilings = vec![
            ProductConversationSegmentCeiling {
                transcript_row_id: "s0".into(),
                tail_sequence_id: 7,
                tail_message_id: Some("s0-1".into()),
            },
            ProductConversationSegmentCeiling {
                transcript_row_id: "s2".into(),
                tail_sequence_id: 9_007_199_254_740_993,
                tail_message_id: Some("s2-3".into()),
            },
        ];
        let null_tail_ceiling = vec![ProductConversationSegmentCeiling {
            transcript_row_id: "s1".into(),
            tail_sequence_id: 7,
            tail_message_id: None,
        }];
        let ceilings = [
            None,
            Some("[]".to_string()),
            Some(serde_json::to_string(&all_ceilings).unwrap()),
            Some(serde_json::to_string(&partial_ceilings).unwrap()),
            Some(serde_json::to_string(&null_tail_ceiling).unwrap()),
        ];
        for (kind, role) in [("ordinary", "user"), ("coordinator", "coordinator")] {
            sqlx::query("UPDATE product_conversations SET kind = ?1")
                .bind(kind)
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::query("UPDATE conversations SET runtime_role = ?1 WHERE runtime_role IN ('user', 'coordinator')")
                .bind(role).execute(&mut connection).await.unwrap();
            for product in ["product", "empty-product", "missing-product"] {
                for cursor in &cursors {
                    for ceiling in &ceilings {
                        for limit in [0, 1, 2, 5, 51] {
                            let expected = full_message_page_rows(
                                &mut connection,
                                ORIGINAL_PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
                                product,
                                cursor.as_ref(),
                                ceiling.as_deref(),
                                limit,
                            )
                            .await;
                            let actual = full_message_page_rows(
                                &mut connection,
                                PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
                                product,
                                cursor.as_ref(),
                                ceiling.as_deref(),
                                limit,
                            )
                            .await;
                            assert_eq!(
                                actual, expected,
                                "kind={kind} product={product} cursor={cursor:?} ceiling={ceiling:?} limit={limit}"
                            );
                        }
                    }
                }
            }
        }
        let complete = full_message_page_rows(
            &mut connection,
            PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
            "product",
            None,
            None,
            51,
        )
        .await;
        assert_eq!(complete.len(), 15);
        assert_eq!(complete[0]["message_id"], "s2-4");
        assert_eq!(complete[0]["sequence_id"].as_i64(), Some(i64::MAX));
        assert_eq!(
            complete[1]["sequence_id"].as_i64(),
            Some(9_007_199_254_740_993)
        );
        assert_eq!(complete[2]["message_id"], "s2-2");
        assert_eq!(complete[3]["message_id"], "s2-1");
        assert_eq!(complete[14]["message_id"], "s0-0");
    }

    #[tokio::test]
    async fn late_hydration_matches_oracle_after_attachment_hydration() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("hydrate", "hydrate", "/tmp", true, None, None)
            .await
            .unwrap();
        seed_reset_message(&db, &root).await;
        let mut connection = db.pool.acquire().await.unwrap();
        let mut expected = sqlx::query(ORIGINAL_PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL)
            .bind(root.product_conversation_id.as_str())
            .bind(None::<String>)
            .bind(None::<i64>)
            .bind(None::<i64>)
            .bind(None::<String>)
            .bind(2_i64)
            .try_map(|row| {
                let ordinal: i64 = row.try_get("ordinal")?;
                super::super::parse_message_row(row).map(|message| (ordinal, message))
            })
            .fetch_all(&mut *connection)
            .await
            .unwrap();
        assert_eq!(expected.len(), 1);
        let mut messages = expected
            .iter()
            .map(|(_, message)| message.clone())
            .collect::<Vec<_>>();
        super::super::hydrate_attachments_conn(&mut connection, &mut messages)
            .await
            .unwrap();
        for ((_, message), hydrated) in expected.iter_mut().zip(messages) {
            *message = hydrated;
        }
        let actual = Database::get_product_conversation_messages_page_on(
            &mut connection,
            &root.product_conversation_id,
            None,
            None,
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    async fn seed_reset_message(db: &Database, root: &crate::Conversation) {
        use phoenix_core::domain::db_schema::{
            FileAttachment, ImageData, InputOrigin, SourceToolCall,
        };
        let content = MessageContent::User(crate::UserContent {
            text: "old view".into(),
            images: vec![ImageData {
                media_type: "image/png".into(),
                data: "old-image".into(),
            }],
            files: vec![FileAttachment {
                original_name: "old.txt".into(),
                media_type: "text/plain".into(),
                size_bytes: 3,
                stored_path: "/tmp/old.txt".into(),
            }],
            llm_text: Some("expanded old view".into()),
            is_meta: true,
        });
        db.add_message_with_seq_and_origin(
            "old",
            &root.id,
            1,
            &content,
            Some(&serde_json::json!({"display": "retained"})),
            Some(&crate::UsageData {
                input_tokens: 7,
                output_tokens: 3,
                reasoning_tokens: Some(1),
                cache_creation_tokens: 2,
                cache_read_tokens: 1,
            }),
            &InputOrigin::InternalConversation {
                product_conversation_id: root.product_conversation_id.clone(),
                transcript_id: root.id.clone(),
                source_call: Some(Box::new(SourceToolCall {
                    message_id: "call-message".into(),
                    tool_use_id: "call".into(),
                })),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn late_hydration_snapshot_survives_committed_reset_and_preserves_hydrated_origin() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("race.db").to_str().unwrap())
            .await
            .unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        let root = db
            .create_conversation("race", "race", "/tmp", true, None, None)
            .await
            .unwrap();
        seed_reset_message(&db, &root).await;
        let expected = db.get_messages(&root.id).await.unwrap();
        let mut reader = db.pool.acquire().await.unwrap();
        let mut tx = reader.begin().await.unwrap();
        let before =
            Database::get_ordinary_product_conversation_on(&mut tx, &root.product_conversation_id)
                .await
                .unwrap();
        let before_value = serde_json::to_value(&before.root.conversation).unwrap();
        let mut writer = db.pool.begin().await.unwrap();
        sqlx::query("DELETE FROM messages WHERE conversation_id=?")
            .bind(&root.id)
            .execute(&mut *writer)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET transcript_generation=transcript_generation+1 WHERE id=?",
        )
        .bind(&root.id)
        .execute(&mut *writer)
        .await
        .unwrap();
        writer.commit().await.unwrap();
        let page = Database::get_product_conversation_messages_page_on(
            &mut tx,
            &root.product_conversation_id,
            None,
            None,
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(page.iter().map(|(_, m)| m).collect::<Vec<_>>()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let still =
            Database::get_ordinary_product_conversation_on(&mut tx, &root.product_conversation_id)
                .await
                .unwrap();
        assert_eq!(
            before.root.conversation.transcript_generation,
            still.root.conversation.transcript_generation
        );
        assert_eq!(before.root.tail_message_id, still.root.tail_message_id);
        assert_eq!(
            before_value,
            serde_json::to_value(&still.root.conversation).unwrap()
        );
        tx.rollback().await.unwrap();
        drop(reader);
        let fresh = db
            .read_ordinary_product_conversation_snapshot(
                root.product_conversation_id.as_str(),
                None,
                None,
                2,
            )
            .await
            .unwrap();
        assert!(fresh.messages.is_empty());
        assert_eq!(
            fresh.aggregate.root.conversation.transcript_generation,
            before.root.conversation.transcript_generation + 1
        );
        db.pool.close().await;
    }

    fn assert_page_ids(snapshot: &ProductConversationSnapshotRead, ids: &[&str]) {
        assert_eq!(
            snapshot
                .messages
                .iter()
                .map(|(_, m)| m.message_id.as_str())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[tokio::test]
    async fn late_hydration_snapshot_cursor_ceiling_and_metadata_share_one_generation() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("cursor", "cursor", "/tmp", true, None, None)
            .await
            .unwrap();
        for sequence in 1..=4 {
            db.add_message_with_seq(
                &format!("m{sequence}"),
                &root.id,
                sequence,
                &MessageContent::user(format!("message{sequence}")),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let first = db
            .read_ordinary_product_conversation_snapshot(&root.id, None, None, 3)
            .await
            .unwrap();
        assert_page_ids(&first, &["m4", "m3", "m2"]);
        let ceiling = ProductConversationSegmentCeiling {
            transcript_row_id: root.id.clone(),
            tail_sequence_id: 4,
            tail_message_id: Some("m4".into()),
        };
        db.add_message_with_seq(
            "m5",
            &root.id,
            5,
            &MessageContent::user("append"),
            None,
            None,
        )
        .await
        .unwrap();
        let bounded = db
            .read_ordinary_product_conversation_snapshot(
                &root.id,
                None,
                Some(std::slice::from_ref(&ceiling)),
                3,
            )
            .await
            .unwrap();
        assert_page_ids(&bounded, &["m4", "m3", "m2"]);
        let unbounded = db
            .read_ordinary_product_conversation_snapshot(&root.id, None, None, 3)
            .await
            .unwrap();
        assert_eq!(unbounded.messages[0].1.message_id, "m5");
        let second = db
            .read_ordinary_product_conversation_snapshot(
                &root.id,
                Some((0, 3, "m3".into())),
                Some(&[ceiling]),
                3,
            )
            .await
            .unwrap();
        assert_page_ids(&second, &["m2", "m1"]);
        assert_eq!(
            first.aggregate.product_conversation.id(),
            second.aggregate.product_conversation.id()
        );
        assert_eq!(
            first.aggregate.root.conversation.transcript_generation,
            second.aggregate.root.conversation.transcript_generation
        );
        assert_eq!(second.aggregate.root.tail_message_id.as_deref(), Some("m5"));
        assert_eq!(
            second.requested_transcript_row_id.as_str(),
            root.id.as_str()
        );
        let empty = db
            .read_ordinary_product_conversation_snapshot(&root.id, None, Some(&[]), 3)
            .await
            .unwrap();
        assert!(empty.messages.is_empty());
    }

    #[tokio::test]
    async fn source_call_survives_real_snapshot_and_message_page() {
        use phoenix_core::domain::db_schema::{InputOrigin, SourceToolCall};
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "source-projection",
                "source-projection",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let origin = InputOrigin::InternalConversation {
            product_conversation_id: conversation.product_conversation_id.clone(),
            transcript_id: conversation.id.clone(),
            source_call: Some(Box::new(SourceToolCall {
                message_id: "source-assistant".into(),
                tool_use_id: "source-call".into(),
            })),
        };
        db.add_message_with_seq_and_origin(
            "received",
            &conversation.id,
            1,
            &MessageContent::user("forwarded"),
            None,
            None,
            &origin,
        )
        .await
        .unwrap();
        let snapshot = db
            .read_ordinary_product_conversation_snapshot(&conversation.id, None, None, 10)
            .await
            .unwrap();
        assert_eq!(snapshot.messages[0].1.origin, origin);
        let page = db
            .get_product_conversation_messages_page(&conversation.product_conversation_id, None, 10)
            .await
            .unwrap();
        assert_eq!(page[0].1.origin, origin);
    }

    #[tokio::test]
    async fn cancelled_snapshot_returns_a_clean_connection() {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            assert_cancelled_snapshot_returns_a_clean_connection(),
        )
        .await
        .expect("cancelled snapshot cleanup did not complete");
    }

    async fn assert_cancelled_snapshot_returns_a_clean_connection() {
        for pause_after in [1, 20] {
            let db = Database::open_in_memory().await.unwrap();
            let root = db
                .create_conversation("cancel-root", "cancel-root", "/tmp", true, None, None)
                .await
                .unwrap();
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let mut connection = db.pool.acquire().await.unwrap();
            {
                let mut handle = connection.lock_handle().await.unwrap();
                let raw = handle.as_raw_handle().as_ptr() as usize;
                let mut entered_tx = Some(entered_tx);
                let mut active_steps = 0;
                handle.set_progress_handler(1, move || {
                    // SAFETY: SQLite invokes this callback on the owning connection's
                    // worker; the handle remains live and this call only reads its flag.
                    let active =
                        unsafe { libsqlite3_sys::sqlite3_get_autocommit(raw as *mut _) == 0 };
                    if active {
                        active_steps += 1;
                        if active_steps == pause_after {
                            entered_tx.take().unwrap().send(()).unwrap();
                            let _ = release_rx.recv();
                        }
                    }
                    true
                });
            }
            drop(connection);
            let mut read =
                Box::pin(db.read_ordinary_product_conversation_snapshot(&root.id, None, None, 10));
            tokio::select! {
                result = &mut read => panic!("snapshot completed before cancellation: {result:?}"),
                entered = entered_rx => entered.unwrap(),
            }
            drop(read);
            release_tx.send(()).unwrap();

            let mut connection = db.pool.acquire().await.unwrap();
            {
                let mut handle = connection.lock_handle().await.unwrap();
                handle.remove_progress_handler();
                // SAFETY: the locked handle excludes concurrent SQLite access.
                assert_ne!(
                    unsafe {
                        libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr())
                    },
                    0,
                    "cancelled snapshot left a transaction open"
                );
            }
            drop(connection);
            let tx = db
                .pool
                .begin()
                .await
                .expect("cancelled snapshot must not poison the pool");
            tx.rollback().await.unwrap();
            db.read_ordinary_product_conversation_snapshot(&root.id, None, None, 10)
                .await
                .unwrap();
        }
    }

    async fn performance_fixture(
        segment_count: usize,
        messages_per_segment: usize,
    ) -> (Database, crate::Conversation) {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("perf-root", "perf-root", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut segment = root.clone();
        let content = serde_json::to_string(&MessageContent::user("fixture payload")).unwrap();
        for segment_index in 0..segment_count {
            sqlx::query(
                "WITH RECURSIVE ordinal(value) AS (
                     VALUES(1) UNION ALL SELECT value + 1 FROM ordinal WHERE value < ?1
                 ) INSERT INTO messages (
                     message_id, conversation_id, sequence_id, message_type, content, created_at
                 ) SELECT printf('perf-%d-%d', ?2, value), ?3, value, 'user', ?4,
                          '2026-01-01T00:00:00Z'
                   FROM ordinal",
            )
            .bind(i64::try_from(messages_per_segment).unwrap())
            .bind(i64::try_from(segment_index).unwrap())
            .bind(&segment.id)
            .bind(&content)
            .execute(db.pool())
            .await
            .unwrap();
            if segment_index + 1 < segment_count {
                db.update_conversation_state(
                    &segment.id,
                    &ConvState::ContextExhausted {
                        summary: "fixture continuation".to_string(),
                    },
                )
                .await
                .unwrap();
                segment = match db.continue_conversation(&segment.id).await.unwrap() {
                    ContinueOutcome::Created(successor) => successor,
                    other @ (ContinueOutcome::AlreadyContinued(_)
                    | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                        panic!("expected fixture continuation, got {other:?}")
                    }
                };
            }
        }
        let distractor = db
            .create_conversation("perf-noise", "perf-noise", "/tmp", true, None, None)
            .await
            .unwrap();
        let noise_content = serde_json::to_string(&MessageContent::user("noise")).unwrap();
        for chunk in 0..200 {
            sqlx::query(
                "WITH RECURSIVE ordinal(value) AS (
                     VALUES(1) UNION ALL SELECT value + 1 FROM ordinal WHERE value < 1000
                 ) INSERT INTO messages (
                     message_id, conversation_id, sequence_id, message_type, content, created_at
                 ) SELECT printf('noise-%d-%d', ?1, value), ?2, (?1 * 1000) + value,
                          'user', ?3, '2026-01-01T00:00:00Z'
                   FROM ordinal",
            )
            .bind(chunk)
            .bind(&distractor.id)
            .bind(&noise_content)
            .execute(db.pool())
            .await
            .unwrap();
        }
        (db, root)
    }

    async fn measure_snapshot_stages(db: &Database, reference: &str) -> (u128, u128, u128, u128) {
        let mut connection = db.pool.begin().await.unwrap();
        let started = Instant::now();
        let resolved =
            Database::resolve_ordinary_product_conversation_on(&mut connection, reference)
                .await
                .unwrap();
        let resolve_micros = started.elapsed().as_micros();
        let started = Instant::now();
        let aggregate = Database::get_ordinary_product_conversation_on(
            &mut connection,
            &resolved.product_conversation_id,
        )
        .await
        .unwrap();
        let aggregate_micros = started.elapsed().as_micros();
        let started = Instant::now();
        let messages = Database::get_product_conversation_messages_page_on(
            &mut connection,
            aggregate.product_conversation.id(),
            None,
            None,
            51,
        )
        .await
        .unwrap();
        assert_eq!(messages.len(), 51);
        let page_micros = started.elapsed().as_micros();
        let started = Instant::now();
        connection.rollback().await.unwrap();
        let rollback_micros = started.elapsed().as_micros();
        (
            resolve_micros,
            aggregate_micros,
            page_micros,
            rollback_micros,
        )
    }

    #[tokio::test]
    async fn message_page_query_enforces_transcript_first_indexed_join() {
        let normalized = PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(normalized.contains("selected AS MATERIALIZED ( SELECT messages.message_id, messages.conversation_id, messages.sequence_id, transcript.ordinal"));
        assert!(normalized.contains("FROM selected JOIN messages m ON m.message_id = selected.message_id ORDER BY selected.ordinal DESC, selected.sequence_id DESC, selected.message_id DESC"));
        assert!(normalized.contains(
            "FROM transcript CROSS JOIN messages INDEXED BY messages_conversation_sequence ON messages.conversation_id = transcript.id"
        ));

        let db = Database::open_in_memory().await.unwrap();
        let page = Database::get_product_conversation_messages_page_on(
            &mut db.pool.acquire().await.unwrap(),
            &ProductConversationId::parse("fixture-product").unwrap(),
            None,
            None,
            51,
        )
        .await
        .unwrap();
        assert!(page.is_empty());
    }

    #[tokio::test]
    async fn message_page_materializes_bound_snapshot_ceiling_once_with_result_parity() {
        let (db, root) = bound_ceiling_fixture().await;
        let mut connection = db.pool.acquire().await.unwrap();
        let all = full_message_page_rows(
            &mut connection,
            PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
            root.product_conversation_id.as_str(),
            None,
            None,
            100,
        )
        .await;
        let ceilings: Vec<_> = all
            .iter()
            .filter(|row| row["message_id"].as_str().unwrap().ends_with("-2"))
            .map(|row| ProductConversationSegmentCeiling {
                transcript_row_id: row["conversation_id"].as_str().unwrap().to_owned(),
                tail_sequence_id: 2,
                tail_message_id: Some(row["message_id"].as_str().unwrap().to_owned()),
            })
            .collect();
        assert_eq!(ceilings.len(), 3);
        assert_eq!(all.len(), 12);
        let ceiling = serde_json::to_string(&ceilings).unwrap();
        let previous = PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL.replacen(
            "snapshot_ceiling AS MATERIALIZED (",
            "snapshot_ceiling AS (",
            1,
        );
        assert_ne!(previous, PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL);
        for boundary in [None, Some(ceiling.as_str()), Some("[]")] {
            let mut cursor = None;
            let mut pages = 0;
            let mut rows = 0;
            loop {
                let actual = full_message_page_rows(
                    &mut connection,
                    PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
                    root.product_conversation_id.as_str(),
                    cursor.as_ref(),
                    boundary,
                    2,
                )
                .await;
                let expected = full_message_page_rows(
                    &mut connection,
                    &previous,
                    root.product_conversation_id.as_str(),
                    cursor.as_ref(),
                    boundary,
                    2,
                )
                .await;
                assert_eq!(actual, expected);
                rows += actual.len();
                let Some(last) = actual.last() else {
                    break;
                };
                cursor = Some((
                    last["ordinal"].as_i64().unwrap(),
                    last["sequence_id"].as_i64().unwrap(),
                    last["message_id"].as_str().unwrap().to_owned(),
                ));
                pages += 1;
                assert!(pages <= all.len(), "cursor must advance to exhaustion");
            }
            assert_eq!(
                rows,
                match boundary {
                    None => all.len(),
                    Some("[]") => 0,
                    Some(_) => 6,
                }
            );
        }
        assert_materialized_ceiling_plan(
            &mut connection,
            root.product_conversation_id.as_str(),
            &ceiling,
        )
        .await;
    }

    async fn bound_ceiling_fixture() -> (Database, crate::Conversation) {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("ceiling-plan", "ceiling-plan", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut member = root.clone();
        let content = serde_json::to_string(&MessageContent::user("ceiling fixture")).unwrap();
        for ordinal in 0..3 {
            if ordinal > 0 {
                db.update_conversation_state(
                    &member.id,
                    &ConvState::ContextExhausted {
                        summary: "ceiling fixture continuation".into(),
                    },
                )
                .await
                .unwrap();
                member = match db.continue_conversation(&member.id).await.unwrap() {
                    ContinueOutcome::Created(successor) => successor,
                    other @ (ContinueOutcome::AlreadyContinued(_)
                    | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                        panic!("expected ceiling fixture continuation, got {other:?}")
                    }
                };
            }
            for sequence in 1..=4_i64 {
                sqlx::query("INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at) VALUES (?1, ?2, ?3, 'user', ?4, '2026-01-01T00:00:00Z')")
                    .bind(format!("perf-{ordinal}-{sequence}"))
                    .bind(&member.id).bind(sequence).bind(&content)
                    .execute(db.pool()).await.unwrap();
            }
        }
        (db, root)
    }

    async fn assert_materialized_ceiling_plan(
        connection: &mut SqliteConnection,
        product_id: &str,
        ceiling: &str,
    ) {
        let explain = format!("EXPLAIN QUERY PLAN {PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL}");
        let plan = sqlx::query(sqlx::AssertSqlSafe(explain.as_str()))
            .bind(product_id)
            .bind(ceiling)
            .bind(2_i64)
            .bind(2_i64)
            .bind("perf-2-3")
            .bind(2_i64)
            .fetch_all(&mut *connection)
            .await
            .unwrap();
        let details: Vec<String> = plan
            .iter()
            .map(|row| row.try_get("detail").unwrap())
            .collect();
        assert_eq!(
            details
                .iter()
                .filter(|detail| detail.contains("MATERIALIZE snapshot_ceiling"))
                .count(),
            1,
            "{details:?}"
        );
        assert_eq!(
            details
                .iter()
                .filter(|detail| detail.contains("json_each VIRTUAL TABLE"))
                .count(),
            1,
            "{details:?}"
        );
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("MATERIALIZE selected")),
            "{details:?}"
        );
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("messages_conversation_sequence")),
            "{details:?}"
        );
    }

    #[tokio::test]
    #[ignore = "diagnostic scale fixture; run explicitly and retain raw samples"]
    async fn product_conversation_snapshot_scale_diagnostic() {
        let (db, root) = performance_fixture(23, 1_020).await;
        let mut samples = Vec::new();
        for _ in 0..11 {
            samples.push(measure_snapshot_stages(&db, &root.id).await);
        }
        eprintln!("product_conversation_snapshot_stage_micros={samples:?}");
    }

    async fn opening_fixture(
        generated: bool,
    ) -> (Database, crate::Conversation, crate::Conversation) {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("opening-root", "opening-root", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "opening-boundary",
            &root.id,
            &MessageContent::Continuation(ContinuationContent {
                summary: "exact opening context".into(),
            }),
            None,
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "exact opening context".into(),
            },
        )
        .await
        .unwrap();
        let key = ClientTurnKey::try_from("reserved:key").unwrap();
        let intent = if generated {
            NewContinuationDispatchIntent::generated_predecessor_context(
                key,
                "exact opening context".into(),
            )
        } else {
            NewContinuationDispatchIntent::user_authorized(
                key,
                "exact opening context".into(),
                None,
            )
        };
        let (outcome, _) = db
            .continue_conversation_with_intent(&root.id, intent)
            .await
            .unwrap();
        let ContinueOutcome::Created(successor) = outcome else {
            panic!("expected created successor")
        };
        (db, root, successor)
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn opening_read_pending_to_accepted_authority_and_identity_matrix() {
        for generated in [false, true] {
            for canonical in [false, true] {
                let (db, root, successor) = opening_fixture(generated).await;
                assert_eq!(
                    db.read_continuation_opening(&root.id).await.unwrap(),
                    ContinuationOpeningRead::None
                );
                let pending = db.read_continuation_opening(&successor.id).await.unwrap();
                assert!(
                    matches!(&pending, ContinuationOpeningRead::Pending { edge, client_turn_key, .. }
                    if client_turn_key.as_str() == "reserved:key" && edge.continuation_message_id == "opening-boundary"
                    && edge.predecessor_transcript_row_id == root.id && edge.successor_transcript_row_id == successor.id)
                );
                let content = if generated {
                    MessageContent::Continuation(ContinuationContent {
                        summary: "exact opening context".into(),
                    })
                } else {
                    MessageContent::user("exact opening context")
                };
                db.add_message("wrong-id", &successor.id, &content, None, None)
                    .await
                    .unwrap();
                assert_eq!(
                    db.read_message_continuation_opening("wrong-id")
                        .await
                        .unwrap(),
                    None
                );
                assert_eq!(
                    db.read_continuation_opening(&successor.id).await.unwrap(),
                    pending
                );
                let id = if canonical {
                    format!("{}:reserved:key", successor.id)
                } else {
                    "reserved:key".into()
                };
                db.add_message(&id, &successor.id, &content, None, None)
                    .await
                    .unwrap();
                let accepted = db.read_continuation_opening(&successor.id).await.unwrap();
                assert!(
                    matches!(&accepted, ContinuationOpeningRead::Accepted { accepted_message_id, presentation, .. }
                    if accepted_message_id == &id && *presentation == if generated { ContinuationOpeningPresentation::Boundary } else { ContinuationOpeningPresentation::SeparateMessage })
                );
                assert_eq!(
                    db.read_message_continuation_opening(&id).await.unwrap(),
                    Some(accepted.clone())
                );
                let snapshot = db
                    .read_ordinary_product_conversation_snapshot(&root.id, None, None, 10)
                    .await
                    .unwrap();
                assert_eq!(snapshot.latest_opening, accepted);
                sqlx::query("UPDATE messages SET content = ?1 WHERE message_id = ?2")
                    .bind(
                        serde_json::to_string(
                            &if generated {
                                MessageContent::Continuation(ContinuationContent {
                                    summary: "edited".into(),
                                })
                            } else {
                                MessageContent::user("edited")
                            }
                            .to_stored_json(),
                        )
                        .unwrap(),
                    )
                    .bind(&id)
                    .execute(db.pool())
                    .await
                    .unwrap();
                assert!(matches!(
                    db.read_continuation_opening(&successor.id).await.unwrap(),
                    ContinuationOpeningRead::Accepted {
                        presentation: ContinuationOpeningPresentation::SeparateMessage,
                        ..
                    }
                ));
                // An older pending value is retained by value; it cannot change the durable receipt.
                assert!(pending.is_pending());
                assert!(!db
                    .read_continuation_opening(&successor.id)
                    .await
                    .unwrap()
                    .is_pending());
            }
        }
    }

    #[tokio::test]
    async fn opening_update_reconciles_superseded_edits_and_rejects_wrong_current_generation() {
        let (db, _, successor) = opening_fixture(true).await;
        let restored = MessageContent::Continuation(ContinuationContent {
            summary: "exact opening context".into(),
        });
        db.add_message("reserved:key", &successor.id, &restored, None, None)
            .await
            .unwrap();
        let edited = MessageContent::Continuation(ContinuationContent {
            summary: "edit A".into(),
        });
        for (generation, content) in [(1, &edited), (2, &restored)] {
            let mut tx = db.pool().begin().await.unwrap();
            sqlx::query("UPDATE messages SET content = ?1 WHERE message_id = 'reserved:key'")
                .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE conversations SET transcript_generation = ?1 WHERE id = ?2")
                .bind(generation)
                .bind(&successor.id)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        for (generation, content) in [
            (Some(2), &edited),
            (Some(3), &restored),
            (Some(-1), &restored),
            (None, &restored),
        ] {
            assert!(matches!(
                db.read_message_continuation_opening_at_update("reserved:key", generation, content)
                    .await,
                Err(DbError::TranscriptSnapshotChanged)
            ));
        }
        assert!(matches!(
            db.read_message_continuation_opening_at_update("reserved:key", Some(2), &restored)
                .await
                .unwrap(),
            Some(ContinuationOpeningUpdateRead::Current(
                ContinuationOpeningRead::Accepted {
                    presentation: ContinuationOpeningPresentation::Boundary,
                    ..
                }
            ))
        ));
        for content in [&edited, &restored] {
            assert!(matches!(
                db.read_message_continuation_opening_at_update("reserved:key", Some(1), content).await.unwrap(),
                Some(ContinuationOpeningUpdateRead::Superseded {
                    content,
                    transcript_generation: 2,
                    opening: ContinuationOpeningRead::Accepted {
                        presentation: ContinuationOpeningPresentation::Boundary, ..
                    }, ..
                }) if content == restored
            ));
        }
    }

    #[tokio::test]
    async fn opening_read_rollback_and_conflicting_stale_intent_are_not_acceptance() {
        let (db, root, successor) = opening_fixture(true).await;
        let pending = db.read_continuation_opening(&successor.id).await.unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        sqlx::query("INSERT INTO messages(message_id, conversation_id, sequence_id, message_type, content, created_at)
                     VALUES (?1, ?2, 1, 'continuation', ?3, '2026-01-01T00:00:00Z')")
            .bind("reserved:key").bind(&successor.id).bind(r#"{"summary":"exact opening context"}"#)
            .execute(&mut *tx).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM completed_continuation_handoffs")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 1);
        tx.rollback().await.unwrap();
        assert_eq!(
            db.read_continuation_opening(&successor.id).await.unwrap(),
            pending
        );
        assert_eq!(
            db.read_message_continuation_opening("reserved:key")
                .await
                .unwrap(),
            None
        );
        db.add_message(
            "reserved:key",
            &successor.id,
            &MessageContent::Continuation(ContinuationContent {
                summary: "exact opening context".into(),
            }),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO continuation_dispatch_intents(parent_conversation_id, successor_conversation_id, message_id, handoff, opening_authority, created_at)
                     VALUES (?1, ?2, 'conflicting-key', 'exact opening context', 'generated_predecessor_context', '2026-01-01T00:00:00Z')")
            .bind(&root.id).bind(&successor.id).execute(db.pool()).await.unwrap();
        assert!(db.read_continuation_opening(&successor.id).await.is_err());
        assert!(db
            .read_message_continuation_opening("reserved:key")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn opening_snapshot_rejects_old_transcript_tail_and_wrong_edge() {
        let (db, root, successor) = opening_fixture(true).await;
        let (generation, tail): (i64, Option<i64>) = sqlx::query_as(
            "SELECT transcript_generation,
                    (SELECT MAX(sequence_id) FROM messages WHERE conversation_id = conversations.id)
             FROM conversations WHERE id = ?1",
        )
        .bind(&successor.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(db
            .read_continuation_opening_at_snapshot(&successor.id, generation, tail)
            .await
            .unwrap()
            .is_pending());
        db.add_message(
            "other",
            &successor.id,
            &MessageContent::user("different message"),
            None,
            None,
        )
        .await
        .unwrap();
        let (current_generation, current_tail): (i64, Option<i64>) = sqlx::query_as(
            "SELECT transcript_generation,
                    (SELECT MAX(sequence_id) FROM messages WHERE conversation_id = conversations.id)
             FROM conversations WHERE id = ?1",
        )
        .bind(&successor.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(current_generation, generation);
        assert_ne!(current_tail, tail);
        assert!(matches!(
            db.read_continuation_opening_at_snapshot(&successor.id, generation, tail)
                .await,
            Err(DbError::TranscriptSnapshotChanged)
        ));
        assert!(db
            .read_continuation_opening_at_snapshot(&successor.id, current_generation, current_tail)
            .await
            .unwrap()
            .is_pending());
        let invalid_edge =
            sqlx::query("UPDATE conversations SET continued_in_conv_id = NULL WHERE id = ?1")
                .bind(&root.id)
                .execute(db.pool())
                .await
                .unwrap_err();
        let sqlx::Error::Database(constraint) = invalid_edge else {
            panic!("expected continuation edge constraint rejection");
        };
        assert_eq!(constraint.code().as_deref(), Some("1811"));
        assert_eq!(
            constraint.message(),
            "continuation must connect parent transcripts in one ProductConversation"
        );
        assert!(db
            .read_continuation_opening_at_snapshot(&successor.id, current_generation, current_tail)
            .await
            .unwrap()
            .is_pending());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn resolves_member_identity_and_preserves_equal_text_user_authorized_handoff() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("root", "root-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "exhausted".to_string(),
            },
        )
        .await
        .unwrap();
        let (outcome, _) = db
            .continue_conversation_with_intent(
                &root.id,
                NewContinuationDispatchIntent::user_authorized(
                    ClientTurnKey::try_from("opening").unwrap(),
                    "exact handoff".to_string(),
                    None,
                ),
            )
            .await
            .unwrap();
        let successor = match outcome {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        db.add_message(
            "boundary",
            &root.id,
            &MessageContent::Continuation(ContinuationContent {
                summary: "exact handoff".to_string(),
            }),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "opening",
            &successor.id,
            &MessageContent::user("exact handoff"),
            None,
            None,
        )
        .await
        .unwrap();
        let resolved = db
            .resolve_ordinary_product_conversation(&successor.id)
            .await
            .unwrap();
        assert_eq!(
            db.accepted_continuation_handoff_message_id(&successor.id)
                .await
                .unwrap()
                .as_deref(),
            Some("opening")
        );
        assert_eq!(
            db.accepted_continuation_handoff_message_id(&root.id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(resolved.requested_transcript_row_id, successor.id);
        let aggregate = db
            .get_ordinary_product_conversation(&resolved.product_conversation_id)
            .await
            .unwrap();
        assert_eq!(aggregate.root.conversation.id, root.id);
        assert_eq!(aggregate.latest_transcript_row_id, successor.id);
        assert_eq!(aggregate.segments[0].transcript_row.segment_ordinal, 0);
        let handoff = aggregate.segments[0].handoff.as_ref().unwrap();
        assert!(matches!(
            handoff,
            ProductConversationHandoff::Completed {
                continuation_message_id,
                accepted_successor_message_id,
                summary,
                opening_authority: ContinuationOpeningAuthority::UserAuthorizedInstruction,
                accepted_is_duplicate_summary: false,
                ..
            } if continuation_message_id == "boundary"
                && accepted_successor_message_id == "opening"
                && summary == "exact handoff"
        ));
        let completed: (String, String, String, String, String) = sqlx::query_as(
            "SELECT predecessor_conversation_id, successor_conversation_id,
                    continuation_message_id, accepted_successor_message_id, opening_authority
             FROM completed_continuation_handoffs",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            completed,
            (
                root.id,
                successor.id,
                "boundary".to_string(),
                "opening".to_string(),
                "user_authorized_instruction".to_string()
            )
        );
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM continuation_dispatch_intents")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(pending, 0);
    }

    async fn populate_coordinator_history(
        db: &Database,
        root: &crate::Conversation,
    ) -> (Vec<String>, crate::Conversation) {
        let mut member = root.clone();
        let mut expected = Vec::new();
        for ordinal in 0..3 {
            for index in 0..3 {
                let id = format!("global-{ordinal}-{index}");
                db.add_message(
                    &id,
                    &member.id,
                    &MessageContent::user(id.clone()),
                    None,
                    None,
                )
                .await
                .unwrap();
                expected.push(id);
            }
            if ordinal < 2 {
                let id = format!("global-boundary-{ordinal}");
                db.add_message(
                    &id,
                    &member.id,
                    &MessageContent::Continuation(ContinuationContent {
                        summary: format!("summary {ordinal}"),
                    }),
                    None,
                    None,
                )
                .await
                .unwrap();
                expected.push(id);
                db.update_conversation_state(
                    &member.id,
                    &ConvState::ContextExhausted {
                        summary: format!("summary {ordinal}"),
                    },
                )
                .await
                .unwrap();
                member = match db.continue_conversation(&member.id).await.unwrap() {
                    ContinueOutcome::Created(next) => next,
                    ContinueOutcome::AlreadyContinued(_) => {
                        panic!("unexpected existing continuation")
                    }
                    ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                        panic!("unexpected parent state {state_variant}")
                    }
                };
            }
        }
        (expected, member)
    }

    #[tokio::test]
    async fn coordinator_history_pages_across_three_members_without_ordinary_lifecycle() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let (expected, member) = populate_coordinator_history(&db, &root).await;
        let mut connection = db.pool.acquire().await.unwrap();
        let optimized = full_message_page_rows(
            &mut connection,
            PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
            root.product_conversation_id.as_str(),
            None,
            None,
            20,
        )
        .await;
        let oracle = full_message_page_rows(
            &mut connection,
            ORIGINAL_PRODUCT_CONVERSATION_MESSAGE_PAGE_SQL,
            root.product_conversation_id.as_str(),
            None,
            None,
            20,
        )
        .await;
        assert!(!optimized.is_empty());
        assert_eq!(optimized, oracle);
        drop(connection);
        let mut before = None;
        let mut actual = Vec::new();
        loop {
            let page = db
                .read_coordinator_product_conversation_snapshot(&root.id, before, None, 2)
                .await
                .unwrap();
            assert!(matches!(
                page.aggregate.product_conversation,
                ProductConversation::Coordinator { .. }
            ));
            assert_eq!(page.aggregate.latest_transcript_row_id, member.id);
            assert_eq!(page.aggregate.segments.len(), 3);
            if page.messages.is_empty() {
                break;
            }
            let (ordinal, last) = page.messages.last().unwrap();
            before = Some((*ordinal, last.sequence_id, last.message_id.clone()));
            actual.extend(
                page.messages
                    .into_iter()
                    .map(|(_, message)| message.message_id),
            );
        }
        actual.reverse();
        assert_eq!(actual, expected);
        assert!(db
            .read_ordinary_product_conversation_snapshot(&root.id, None, None, 2)
            .await
            .is_err());
        let ordinary = db
            .create_conversation("ordinary", "ordinary", "/tmp", true, None, None)
            .await
            .unwrap();
        assert!(db
            .read_coordinator_product_conversation_snapshot(&ordinary.id, None, None, 2)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn pre_feature_continuation_edge_projects_persisted_historical_boundary() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "exhausted".to_string(),
            },
        )
        .await
        .unwrap();
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation, got {other:?}")
            }
        };
        db.add_message(
            "legacy-boundary",
            &root.id,
            &MessageContent::Continuation(ContinuationContent {
                summary: "legacy handoff".to_string(),
            }),
            None,
            None,
        )
        .await
        .unwrap();
        let aggregate = db
            .get_ordinary_product_conversation(&root.product_conversation_id)
            .await
            .unwrap();
        assert_eq!(
            db.accepted_continuation_handoff_message_id(&successor.id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(aggregate.latest_transcript_row_id, successor.id);
        assert!(matches!(
            aggregate.segments[0].handoff.as_ref(),
            Some(ProductConversationHandoff::Historical {
                predecessor_transcript_row_id,
                successor_transcript_row_id,
                continuation_message_id,
                summary,
            }) if predecessor_transcript_row_id == &root.id
                && successor_transcript_row_id == &successor.id
                && continuation_message_id == "legacy-boundary"
                && summary == "legacy handoff"
        ));
    }

    #[tokio::test]
    async fn resolver_prioritizes_exact_conversation_id_over_slug_collision() {
        let db = Database::open_in_memory().await.unwrap();
        let slug_collision = db
            .create_conversation("collision", "exact", "/tmp", true, None, None)
            .await
            .unwrap();
        let exact_id = db
            .create_conversation("exact", "later", "/tmp", true, None, None)
            .await
            .unwrap();
        assert_ne!(slug_collision.id, exact_id.id);

        let resolved = db
            .resolve_ordinary_product_conversation(&exact_id.id)
            .await
            .unwrap();
        assert_eq!(
            resolved.product_conversation_id,
            exact_id.product_conversation_id
        );
        assert_eq!(resolved.requested_transcript_row_id, exact_id.id);

        let snapshot = db
            .read_ordinary_product_conversation_snapshot(&exact_id.id, None, None, 10)
            .await
            .unwrap();
        assert_eq!(snapshot.requested_transcript_row_id, exact_id.id);
        assert_eq!(
            snapshot.aggregate.product_conversation.id(),
            &exact_id.product_conversation_id
        );
    }

    #[tokio::test]
    async fn snapshot_read_includes_active_close_projection() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("snapshot-close", "snapshot-close", "/tmp", true, None, None)
            .await
            .unwrap();
        db.begin_close_foundation(
            &conversation.product_conversation_id,
            &TranscriptConversationId::parse(conversation.id.clone()).unwrap(),
            "snapshot-close-attempt",
        )
        .await
        .unwrap();

        let snapshot = db
            .read_ordinary_product_conversation_snapshot(&conversation.id, None, None, 1)
            .await
            .unwrap();

        let close = snapshot.close.expect("active Close projection");
        assert_eq!(
            close.obligation.attempt_id().as_str(),
            "snapshot-close-attempt"
        );
        assert_eq!(
            close.obligation.product_conversation_id(),
            &conversation.product_conversation_id
        );
    }

    #[tokio::test]
    async fn list_lifecycle_uses_aggregate_authority_despite_legacy_archived_bit() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("lifecycle", "lifecycle", "/tmp", true, None, None)
            .await
            .unwrap();

        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = ?1")
            .bind(&conversation.id)
            .execute(db.pool())
            .await
            .unwrap();
        let projections = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap();
        let projection = projections
            .iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::Open {
                close_availability: ProductConversationCloseAvailability::Available,
            }
        );

        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1",
        )
        .bind(conversation.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = ?1")
            .bind(&conversation.id)
            .execute(db.pool())
            .await
            .unwrap();

        let projections = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap();
        let projection = projections
            .iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::History
        );
    }

    #[tokio::test]
    async fn list_title_uses_effective_aggregate_rename() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "title-authority",
                "title-authority",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET title = 'Aggregate Title', chain_name = 'Legacy Override' WHERE id = ?1")
            .bind(&conversation.id)
            .execute(db.pool())
            .await
            .unwrap();

        let projections = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap();
        let projection = projections
            .iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();

        assert_eq!(projection.root_title.as_deref(), Some("Legacy Override"));
    }

    #[tokio::test]
    async fn list_projects_close_availability_from_close_preconditions() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation(
                "close-availability",
                "close-availability",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();

        let projection = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .into_iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::Open {
                close_availability: ProductConversationCloseAvailability::Available,
            }
        );

        db.update_conversation_state(&conversation.id, &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        let projection = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .into_iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::Open {
                close_availability: ProductConversationCloseAvailability::Available,
            }
        );

        db.update_conversation_state(
            &conversation.id,
            &ConvState::AwaitingContinuation {
                request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                    operation_id: "continue".to_string(),
                    rejected_tool_calls: Vec::new(),
                    attempt: 1,
                },
            },
        )
        .await
        .unwrap();
        let projection = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .into_iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::Open {
                close_availability: ProductConversationCloseAvailability::Unavailable(
                    ProductConversationCloseUnavailableReason::AwaitingContinuation,
                ),
            },
        );
    }

    #[tokio::test]
    async fn list_projects_task_approval_as_a_typed_close_blocker() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("close-approval", "close-approval", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &conversation.id,
            &ConvState::AwaitingTaskApproval {
                task_file: "tasks/00001-p1-ready--close.md".to_string(),
                title: "Close".to_string(),
                priority: phoenix_core::task_source::Priority::P1,
                plan: "plan".to_string(),
            },
        )
        .await
        .unwrap();

        let projection = db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .into_iter()
            .find(|projection| {
                projection.product_conversation_id == conversation.product_conversation_id
            })
            .unwrap();
        assert_eq!(
            projection.lifecycle,
            ProductConversationListLifecycle::Open {
                close_availability: ProductConversationCloseAvailability::Unavailable(
                    ProductConversationCloseUnavailableReason::AwaitingTaskApproval,
                ),
            },
        );
    }

    #[tokio::test]
    async fn legacy_title_clear_restores_original_title_fallback() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("legacy-title", "original-title", "/tmp", true, None, None)
            .await
            .unwrap();
        let original_title = conversation.title.clone();

        db.set_ordinary_product_conversation_legacy_title(&conversation.id, "Temporary Override")
            .await
            .unwrap();
        let renamed = db.get_conversation(&conversation.id).await.unwrap();
        assert_eq!(renamed.chain_name.as_deref(), Some("Temporary Override"));
        assert_eq!(renamed.title, original_title);

        db.clear_ordinary_product_conversation_legacy_title(&conversation.id)
            .await
            .unwrap();
        let cleared = db.get_conversation(&conversation.id).await.unwrap();
        assert_eq!(cleared.chain_name, None);
        assert_eq!(cleared.title, original_title);
    }

    #[tokio::test]
    async fn rename_rejects_history_without_mutating_title() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("rename-history", "rename-history", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET title = 'Original', chain_name = 'Legacy' WHERE id = ?1",
        )
        .bind(&conversation.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1",
        )
        .bind(conversation.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        assert!(matches!(
            db.set_ordinary_product_conversation_title(&conversation.id, "Changed").await,
            Err(DbError::ProductConversationUnavailable(id))
                if id == conversation.product_conversation_id
        ));
        assert!(matches!(
            db.clear_ordinary_product_conversation_legacy_title(&conversation.id).await,
            Err(DbError::ProductConversationUnavailable(id))
                if id == conversation.product_conversation_id
        ));
        let row = sqlx::query("SELECT title, chain_name FROM conversations WHERE id = ?1")
            .bind(&conversation.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(
            row.try_get::<Option<String>, _>("title")
                .unwrap()
                .as_deref(),
            Some("Original")
        );
        assert_eq!(
            row.try_get::<Option<String>, _>("chain_name")
                .unwrap()
                .as_deref(),
            Some("Legacy")
        );
    }

    #[tokio::test]
    async fn rejects_subagent_and_coordinator_references() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("root", "root", "/tmp", true, None, None)
            .await
            .unwrap();
        let subagent = db
            .create_subagent_conversation(
                "sub",
                "sub",
                "/tmp",
                &root.id,
                "model",
                &root.conv_mode,
                phoenix_core::llm_language::LlmLanguage::default(),
                root.attached_work_scope_id.as_ref(),
                crate::SubAgentExecution {
                    connection: "mock",
                    effort: None,
                    persona: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            db.resolve_ordinary_product_conversation(&subagent.id).await,
            Err(DbError::ConversationNotFound(_))
        ));
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        assert!(matches!(
            db.resolve_ordinary_product_conversation(&coordinator.id)
                .await,
            Err(DbError::ConversationNotFound(_))
        ));
    }
}
