use chrono::{DateTime, Utc};
use phoenix_core::domain::product_conversation::{
    OrdinaryProductConversationLifecycle, ProductConversation, ProductConversationId,
    ProductConversationKind,
};
use phoenix_core::work_scope::RuntimeRole;
use sqlx::{Executor, Row, SqliteConnection};

use crate::{Database, DbError, DbResult, MessageContent, MessageType};

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

#[derive(Debug, Clone)]
pub struct ProductConversationListProjection {
    pub product_conversation_id: ProductConversationId,
    pub lifecycle: OrdinaryProductConversationLifecycle,
    pub root_transcript_row_id: String,
    pub root_slug: Option<String>,
    pub root_title: Option<String>,
    pub latest_transcript_row_id: String,
    pub latest_state: crate::ConvState,
    pub latest_continued_in_conv_id: Option<String>,
    pub latest_archived: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProductConversationSnapshotRead {
    pub aggregate: ProductConversationAggregate,
    pub messages: Vec<(i64, crate::Message)>,
    pub requested_transcript_row_id: String,
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
    pub branch_name: String,
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
pub struct ProductConversationHandoff {
    pub predecessor_transcript_row_id: String,
    pub successor_transcript_row_id: String,
    pub continuation_message_id: String,
    pub summary: String,
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
            Some(branch_name),
            Some(base_branch),
        ) => Ok(Some(ProductConversationWorkIdentity {
            worktree_path,
            branch_name,
            base_branch,
            task_id: row.try_get("cm_task_id")?,
            task_title: row.try_get("cm_task_title")?,
        })),
        _ => Ok(None),
    }
}

impl Database {
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
               AND (p.id = ?1 OR c.id = ?1 OR c.slug = ?1)
             ORDER BY CASE WHEN p.id = ?1 THEN 0 ELSE 1 END, c.created_at ASC, c.id ASC
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
                 SELECT transcript.product_conversation_id, MAX(conversation.updated_at) AS updated_at
                 FROM transcript JOIN conversations conversation ON conversation.id = transcript.id
                 GROUP BY transcript.product_conversation_id
             )
             SELECT product.id AS product_conversation_id, product.ordinary_lifecycle,
                    root.id AS root_transcript_row_id, root.slug AS root_slug, root.title AS root_title,
                    latest.id AS latest_transcript_row_id, latest.state AS latest_state,
                    latest.continued_in_conv_id AS latest_continued_in_conv_id,
                    latest.archived AS latest_archived, activity.updated_at
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
        rows.into_iter()
            .map(|row| {
                let product_conversation_id = row
                    .try_get::<String, _>("product_conversation_id")?
                    .parse::<ProductConversationId>()
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                let lifecycle = if row.try_get("latest_archived")? {
                    OrdinaryProductConversationLifecycle::History
                } else {
                    OrdinaryProductConversationLifecycle::Open
                };
                let latest_state = serde_json::from_str(&row.try_get::<String, _>("latest_state")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                Ok(ProductConversationListProjection {
                    product_conversation_id,
                    lifecycle,
                    root_transcript_row_id: row.try_get("root_transcript_row_id")?,
                    root_slug: row.try_get("root_slug")?,
                    root_title: row.try_get("root_title")?,
                    latest_transcript_row_id: row.try_get("latest_transcript_row_id")?,
                    latest_state,
                    latest_continued_in_conv_id: row.try_get("latest_continued_in_conv_id")?,
                    latest_archived: row.try_get("latest_archived")?,
                    updated_at: row
                        .try_get::<String, _>("updated_at")?
                        .parse::<DateTime<Utc>>()
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                })
            })
            .collect()
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
            sqlx::query("SELECT kind, ordinary_lifecycle FROM product_conversations WHERE id = ?1")
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
                    conversation.cm_task_id, conversation.cm_task_title
             FROM transcript
             JOIN conversations conversation ON conversation.id = transcript.id
             LEFT JOIN work_scope_environments environment
               ON environment.work_scope_id = conversation.work_scope_id
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
                Some(successor_transcript_row_id) => Some(
                    self.persisted_continuation_handoff(
                        &transcript_row.conversation.id,
                        successor_transcript_row_id,
                    )
                    .await?,
                ),
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
        limit: usize,
    ) -> DbResult<ProductConversationSnapshotRead> {
        let mut connection = self.pool.acquire().await?;
        connection.execute("BEGIN").await?;
        let result = async {
            let resolved =
                Self::resolve_ordinary_product_conversation_on(&mut connection, reference).await?;
            let aggregate = Self::get_ordinary_product_conversation_on(
                &mut connection,
                &resolved.product_conversation_id,
            )
            .await?;
            let messages = Self::get_product_conversation_messages_page_on(
                &mut connection,
                aggregate.product_conversation.id(),
                before,
                limit,
            )
            .await?;
            Ok(ProductConversationSnapshotRead {
                aggregate,
                messages,
                requested_transcript_row_id: resolved.requested_transcript_row_id,
            })
        }
        .await;
        connection.execute("ROLLBACK").await?;
        result
    }

    async fn resolve_ordinary_product_conversation_on(
        connection: &mut SqliteConnection,
        reference: &str,
    ) -> DbResult<ResolvedProductConversation> {
        let row = sqlx::query(
            "SELECT c.product_conversation_id, c.id
             FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id
             WHERE p.kind = 'ordinary' AND c.runtime_role = 'user' AND c.parent_conversation_id IS NULL
               AND (p.id = ?1 OR c.id = ?1 OR c.slug = ?1)
             ORDER BY CASE WHEN p.id = ?1 THEN 0 ELSE 1 END, c.created_at ASC, c.id ASC LIMIT 1",
        )
        .bind(reference)
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

    #[allow(clippy::too_many_lines)]
    async fn get_ordinary_product_conversation_on(
        connection: &mut SqliteConnection,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<ProductConversationAggregate> {
        let product =
            sqlx::query("SELECT kind, ordinary_lifecycle FROM product_conversations WHERE id = ?1")
                .bind(product_conversation_id.as_str())
                .fetch_optional(&mut *connection)
                .await?;
        let Some(product) = product else {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        };
        if ProductConversationKind::from_db_str(&product.try_get::<String, _>("kind")?)
            != Some(ProductConversationKind::Ordinary)
        {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.to_string(),
            ));
        }
        let lifecycle = product.try_get::<String, _>("ordinary_lifecycle")?;
        let lifecycle =
            OrdinaryProductConversationLifecycle::from_db_str(&lifecycle).ok_or_else(|| {
                DbError::Serialization(format!("unknown ordinary lifecycle: {lifecycle}"))
            })?;
        let rows = sqlx::query(
            "WITH RECURSIVE transcript(id, ordinal) AS (
                 SELECT root.id, 0 FROM conversations root WHERE root.product_conversation_id = ?1
                   AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id)
                 UNION ALL
                 SELECT successor.id, transcript.ordinal + 1 FROM transcript
                 JOIN conversations predecessor ON predecessor.id = transcript.id
                 JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
                 WHERE successor.product_conversation_id = ?1 AND successor.runtime_role = 'user' AND successor.parent_conversation_id IS NULL
             ) SELECT transcript.id, transcript.ordinal,
                    COALESCE((SELECT MAX(message.sequence_id) FROM messages message WHERE message.conversation_id = transcript.id), 0) AS tail_sequence_id,
                    (SELECT message.message_id FROM messages message
                     WHERE message.conversation_id = transcript.id
                     ORDER BY message.sequence_id DESC, message.message_id DESC LIMIT 1) AS tail_message_id,
                    conversation.work_scope_id, environment.environment_kind,
                    environment.worktree_path, environment.branch_name, environment.base_branch,
                    conversation.cm_task_id, conversation.cm_task_title
             FROM transcript
             JOIN conversations conversation ON conversation.id = transcript.id
             LEFT JOIN work_scope_environments environment
               ON environment.work_scope_id = conversation.work_scope_id
             ORDER BY ordinal",
        ).bind(product_conversation_id.as_str()).fetch_all(&mut *connection).await?;
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
            return Err(DbError::Serialization(format!("ordinary ProductConversation {product_conversation_id} has no parent transcript root")));
        };
        let mut segments = Vec::with_capacity(transcript_rows.len());
        for transcript_row in transcript_rows {
            let handoff = match transcript_row.conversation.continued_in_conv_id.as_deref() {
                Some(successor) => Some(
                    Self::persisted_continuation_handoff_on(
                        connection,
                        &transcript_row.conversation.id,
                        successor,
                    )
                    .await?,
                ),
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

    async fn persisted_continuation_handoff_on(
        connection: &mut SqliteConnection,
        predecessor: &str,
        successor: &str,
    ) -> DbResult<ProductConversationHandoff> {
        let row = sqlx::query("SELECT message_id, content FROM messages WHERE conversation_id = ?1 AND message_type = 'continuation' ORDER BY sequence_id DESC, message_id DESC LIMIT 1")
            .bind(predecessor).fetch_optional(&mut *connection).await?;
        let Some(row) = row else {
            return Err(DbError::Serialization(format!(
                "continuation edge from {predecessor} has no persisted continuation message"
            )));
        };
        let content: serde_json::Value =
            serde_json::from_str(&row.try_get::<String, _>("content")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let MessageContent::Continuation(content) =
            MessageContent::from_stored_json(MessageType::Continuation, content)
                .map_err(DbError::Serialization)?
        else {
            return Err(DbError::Serialization(
                "continuation message parsed as another content type".to_string(),
            ));
        };
        Ok(ProductConversationHandoff {
            predecessor_transcript_row_id: predecessor.to_string(),
            successor_transcript_row_id: successor.to_string(),
            continuation_message_id: row.try_get("message_id")?,
            summary: content.summary,
        })
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
        limit: usize,
    ) -> DbResult<Vec<(i64, crate::Message)>> {
        let mut rows = Self::message_page_query(product_conversation_id, before, limit)
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
        limit: usize,
    ) -> sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
        let before_ordinal = before.as_ref().map(|(ordinal, _, _)| *ordinal);
        let before_sequence_id = before.as_ref().map(|(_, sequence_id, _)| *sequence_id);
        let before_message_id = before.map(|(_, _, message_id)| message_id);
        sqlx::query("WITH RECURSIVE transcript(id, ordinal) AS (
             SELECT root.id, 0 FROM conversations root WHERE root.product_conversation_id = ?1 AND root.runtime_role = 'user' AND root.parent_conversation_id IS NULL
             AND NOT EXISTS (SELECT 1 FROM conversations predecessor WHERE predecessor.product_conversation_id = root.product_conversation_id AND predecessor.continued_in_conv_id = root.id)
             UNION ALL SELECT successor.id, transcript.ordinal + 1 FROM transcript JOIN conversations predecessor ON predecessor.id = transcript.id JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
             WHERE successor.product_conversation_id = ?1 AND successor.runtime_role = 'user' AND successor.parent_conversation_id IS NULL
         ) SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at, transcript.ordinal FROM transcript JOIN messages ON messages.conversation_id = transcript.id
         WHERE (?2 IS NULL OR transcript.ordinal < ?2 OR (transcript.ordinal = ?2 AND (messages.sequence_id < ?3 OR (messages.sequence_id = ?3 AND messages.message_id < ?4)))) ORDER BY transcript.ordinal DESC, messages.sequence_id DESC, messages.message_id DESC LIMIT ?5")
        .bind(product_conversation_id.as_str().to_string()).bind(before_ordinal).bind(before_sequence_id).bind(before_message_id).bind(i64::try_from(limit).expect("page limit fits i64"))
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
                    display_data, usage_data, created_at, transcript.ordinal
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

    async fn persisted_continuation_handoff(
        &self,
        predecessor_transcript_row_id: &str,
        successor_transcript_row_id: &str,
    ) -> DbResult<ProductConversationHandoff> {
        let row = sqlx::query(
            "SELECT message_id, content FROM messages
             WHERE conversation_id = ?1 AND message_type = 'continuation'
             ORDER BY sequence_id DESC, message_id DESC LIMIT 1",
        )
        .bind(predecessor_transcript_row_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(DbError::Serialization(format!(
                "continuation edge from {predecessor_transcript_row_id} has no persisted continuation message"
            )));
        };
        let content: serde_json::Value =
            serde_json::from_str(&row.try_get::<String, _>("content")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let MessageContent::Continuation(content) =
            MessageContent::from_stored_json(MessageType::Continuation, content)
                .map_err(DbError::Serialization)?
        else {
            return Err(DbError::Serialization(
                "continuation message parsed as another content type".to_string(),
            ));
        };
        Ok(ProductConversationHandoff {
            predecessor_transcript_row_id: predecessor_transcript_row_id.to_string(),
            successor_transcript_row_id: successor_transcript_row_id.to_string(),
            continuation_message_id: row.try_get("message_id")?,
            summary: content.summary,
        })
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
    use crate::{ContinuationContent, ContinueOutcome, ConvState, MessageContent};

    #[tokio::test]
    async fn resolves_member_identity_and_reads_typed_handoff() {
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
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
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
        let resolved = db
            .resolve_ordinary_product_conversation(&successor.id)
            .await
            .unwrap();
        assert_eq!(resolved.requested_transcript_row_id, successor.id);
        let aggregate = db
            .get_ordinary_product_conversation(&resolved.product_conversation_id)
            .await
            .unwrap();
        assert_eq!(aggregate.root.conversation.id, root.id);
        assert_eq!(aggregate.latest_transcript_row_id, successor.id);
        assert_eq!(aggregate.segments[0].transcript_row.segment_ordinal, 0);
        let handoff = aggregate.segments[0].handoff.as_ref().unwrap();
        assert_eq!(handoff.continuation_message_id, "boundary");
        assert_eq!(handoff.summary, "exact handoff");
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
