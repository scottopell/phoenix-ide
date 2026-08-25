use chrono::{DateTime, Utc};
use phoenix_core::domain::product_conversation::{
    OrdinaryProductConversationLifecycle, ProductConversation, ProductConversationId,
    ProductConversationKind,
};
use phoenix_core::work_scope::RuntimeRole;
use sqlx::Row;

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
pub struct ProductConversationTranscriptRow {
    pub conversation: crate::Conversation,
    pub segment_ordinal: i64,
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

    /// Lists ordinary aggregates in descending durable activity order.
    ///
    /// # Errors
    ///
    /// Returns a database or decode error when an aggregate cannot be projected.
    pub async fn list_ordinary_product_conversations(
        &self,
    ) -> DbResult<Vec<ProductConversationAggregate>> {
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM product_conversations WHERE kind = 'ordinary'",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut aggregates = Vec::with_capacity(ids.len());
        for id in ids {
            let id = id
                .parse::<ProductConversationId>()
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            aggregates.push(self.get_ordinary_product_conversation(&id).await?);
        }
        aggregates.sort_by(|left, right| {
            right.updated_at.cmp(&left.updated_at).then_with(|| {
                left.product_conversation
                    .id()
                    .as_str()
                    .cmp(right.product_conversation.id().as_str())
            })
        });
        Ok(aggregates)
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
             SELECT id, ordinal FROM transcript ORDER BY ordinal",
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

    async fn persisted_continuation_handoff(
        &self,
        predecessor_transcript_row_id: &str,
        successor_transcript_row_id: &str,
    ) -> DbResult<ProductConversationHandoff> {
        let row = sqlx::query(
            "SELECT message_id, content FROM messages
             WHERE conversation_id = ?1 AND message_type = 'continuation'
             ORDER BY sequence_id DESC LIMIT 1",
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
