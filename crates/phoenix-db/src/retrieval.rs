//! Scope-filtered message retrieval over an FTS5 index
//! (`specs/conversation-retrieval/`).
//!
//! One index (`message_fts`) over every conversation's messages. Typed requests
//! select scope, visibility, grouping, and lexical matching policy. The ranking
//! backend sits behind the [`MessageRetriever`] trait so a vector/hybrid backend
//! can be substituted without touching callers (REQ-RET-005). The index is a
//! rebuildable derived cache over `messages` (REQ-RET-003): kept current by
//! the persist/mutate/delete hooks the `Database` calls, and reconciled at
//! startup by [`Fts5Retriever::reconcile`].

// Type names intentionally share the module's "retrieval" stem — they are the
// vocabulary the spec defines (RetrievalScope, RetrievedChunk, …).
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(test)]
#[derive(Debug, Default)]
struct SourceSnapshotTestBarrier {
    hydrated: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

use crate::sqlite_telemetry::{
    ParentSqliteObserver, SqliteOperation, SqlitePhase, SqliteTelemetry,
};
use crate::sqlite_workload::{SqliteAccessKind, SqliteWorkloadCategory, SqliteWorkloadCollector};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{Message, MessageType};
use phoenix_core::domain::message_text::index_text;
use sqlx::{Connection, Row, SqlitePool};
use thiserror::Error;

/// Which conversations a retrieval may draw from. The *only* axis separating
/// chain recall from application-wide recall (REQ-RET-001).
#[derive(Debug, Clone)]
pub enum RetrievalScope {
    /// Restrict to this set of conversation ids (the chain case).
    Conversations(Vec<String>),
    /// Span every conversation in the database (the application-wide case).
    Global,
    /// Span the application except the supplied conversation ids.
    GlobalExcluding(Vec<String>),
}

/// Which rows inside the selected scope are eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalVisibility {
    /// User-visible top-level conversations only, across active and archived.
    UserTopLevel,
    /// All conversations in scope, regardless of archival state.
    All,
}

/// How multiple hits from one conversation should be reduced before the final
/// limit is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalGrouping {
    /// Return every matching message chunk.
    None,
    /// Keep only the best hit per conversation before the outer limit.
    BestPerConversation,
}

/// How the caller wants natural-language query terms translated into the FTS
/// MATCH expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalMatchMode {
    /// Every content-bearing term must match exactly as a token.
    ExactTerms,
    /// The final content-bearing term may match by token prefix.
    FinalTokenPrefix,
}

/// Structured retrieval request so callers choose scope, visibility policy,
/// grouping, and limit as one coherent value.
#[derive(Debug, Clone)]
pub struct RetrievalRequest {
    query: String,
    scope: RetrievalScope,
    visibility: RetrievalVisibility,
    grouping: RetrievalGrouping,
    match_mode: RetrievalMatchMode,
    limit: usize,
}

impl RetrievalRequest {
    #[must_use]
    pub fn natural_language(query: impl Into<String>, scope: RetrievalScope, limit: usize) -> Self {
        Self {
            query: query.into(),
            scope,
            visibility: RetrievalVisibility::All,
            grouping: RetrievalGrouping::None,
            match_mode: RetrievalMatchMode::ExactTerms,
            limit,
        }
    }

    #[must_use]
    pub fn palette_conversation_search(query: impl Into<String>, limit: usize) -> Self {
        Self {
            query: query.into(),
            scope: RetrievalScope::Global,
            visibility: RetrievalVisibility::UserTopLevel,
            grouping: RetrievalGrouping::BestPerConversation,
            match_mode: RetrievalMatchMode::FinalTokenPrefix,
            limit,
        }
    }

    /// Natural-language query supplied by the caller.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Conversation scope searched by the backend.
    #[must_use]
    pub fn scope(&self) -> &RetrievalScope {
        &self.scope
    }

    /// Visibility policy applied before ranking and limiting.
    #[must_use]
    pub fn visibility(&self) -> RetrievalVisibility {
        self.visibility
    }

    /// Grouping policy applied before the final result limit.
    #[must_use]
    pub fn grouping(&self) -> RetrievalGrouping {
        self.grouping
    }

    /// Lexical matching policy requested by the caller.
    #[must_use]
    pub fn match_mode(&self) -> RetrievalMatchMode {
        self.match_mode
    }

    /// Maximum number of results returned after policy application.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// Identity of a chunk *within* its message (REQ-RET-006). One chunk per
/// message in the lexical backend (`ordinal` 0, `char_range` `None`); a
/// chunking backend assigns a distinct ordinal/range per chunk. Present
/// unconditionally so the result shape is stable across backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRef {
    /// 0 for a whole-message chunk.
    pub ordinal: u32,
    /// Character span within the message, when the backend splits messages.
    pub char_range: Option<(usize, usize)>,
}

/// One ranked retrieval result, carrying provenance (REQ-RET-006).
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    /// Source conversation.
    pub conversation_id: String,
    /// Source message.
    pub message_id: String,
    /// Chunk identity within the message.
    pub chunk: ChunkRef,
    /// Role of the source message.
    pub message_type: MessageType,
    /// When the source message was written.
    pub created_at: DateTime<Utc>,
    /// Display/assembly snippet around the match.
    pub snippet: String,
    /// Relevance score (lower BM25 = more relevant).
    pub score: f64,
    pub transcript_generation: i64,
    pub message_count: i64,
}

/// Error from a retrieval or index-maintenance operation.
#[derive(Debug, Error)]
pub enum RetrievalError {
    /// The backing SQLite/FTS5 query failed.
    #[error("retrieval query failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Ranked relevance recall over conversation message content. The seam that
/// lets a vector/hybrid backend replace the lexical one (REQ-RET-005).
#[async_trait]
pub trait MessageRetriever: Send + Sync {
    /// Return message chunks under the request's scope, policy, and limit,
    /// ranked by relevance to its natural-language query. The implementation
    /// builds the backend query (REQ-RET-001).
    ///
    /// # Errors
    /// Returns [`RetrievalError`] if the backing query fails.
    async fn retrieve(
        &self,
        request: RetrievalRequest,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError>;

    /// Whether startup reconciliation has completed. Consumers that query broad
    /// scopes use this to avoid treating a warming derived index as
    /// authoritative.
    fn index_reconciled(&self) -> bool;

    /// Whether the index **fully and freshly** covers the given conversations:
    /// every message present AND its indexed `content_hash` matching the
    /// current extraction (so a swallowed best-effort reindex that left a
    /// stale row reports as not-fresh, not just missing rows). Scoped — cheap
    /// for a chain's handful of members — so a consumer that must not answer
    /// from a partial index can block on it (chains REQ-CHN-009). Empty input
    /// is trivially fresh.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] if a backing query fails.
    async fn is_fresh_for(&self, conversation_ids: &[String]) -> Result<bool, RetrievalError>;
}

/// Counts from a reconciliation pass, for logging.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    /// Messages indexed that were absent from the index.
    pub indexed: usize,
    /// Messages re-indexed because their content changed.
    pub reindexed: usize,
    /// Index rows pruned because their source message is gone.
    pub pruned: usize,
}

#[derive(Debug, Clone)]
pub struct FtsReconcilePlan {
    locator_repair_required: bool,
    messages: Vec<Message>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsMessageReconcileOutcome {
    Unchanged,
    Indexed,
    Reindexed,
}

/// Lexical (FTS5/BM25) retrieval backend.
pub struct Fts5Retriever {
    pool: SqlitePool,
    reconciled: Arc<AtomicBool>,
    sqlite_workload_collector: SqliteWorkloadCollector,
    #[cfg(test)]
    source_snapshot_test_barrier: Option<Arc<SourceSnapshotTestBarrier>>,
}

impl Fts5Retriever {
    /// Build a retriever over the given pool. Call [`Self::reconcile`] once at
    /// startup to bring the index in line with `messages`.
    #[must_use]
    pub fn new(pool: SqlitePool, sqlite_workload_collector: SqliteWorkloadCollector) -> Self {
        Self {
            pool,
            reconciled: Arc::new(AtomicBool::new(false)),
            sqlite_workload_collector,
            #[cfg(test)]
            source_snapshot_test_barrier: None,
        }
    }

    #[cfg(test)]
    fn install_source_snapshot_test_barrier(&mut self, barrier: Arc<SourceSnapshotTestBarrier>) {
        self.source_snapshot_test_barrier = Some(barrier);
    }

    #[cfg(test)]
    async fn wait_at_source_snapshot_test_barrier(&self) {
        if let Some(barrier) = &self.source_snapshot_test_barrier {
            barrier.hydrated.notify_one();
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                barrier.release.notified(),
            )
            .await
            .expect("release reconciler source snapshot");
        }
    }

    /// Whether startup reconciliation has completed (REQ-RET-003 freshness:
    /// lets a consumer distinguish "no in-scope match" from "index warming").
    #[must_use]
    pub fn index_reconciled(&self) -> bool {
        self.reconciled.load(Ordering::Acquire)
    }

    /// Reconcile the index against `messages`: index absent messages,
    /// re-index changed ones (content fingerprint), and prune orphaned index
    /// rows whose source message is gone (REQ-RET-003). Idempotent.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] if any underlying query fails.
    pub async fn reconcile(&self) -> Result<ReconcileStats, RetrievalError> {
        let plan = self.discover_reconcile_plan().await?;
        if plan.locator_repair_required {
            self.repair_locator_rows().await?;
        }
        let mut stats = ReconcileStats::default();
        for message in plan.messages {
            match self.reconcile_message(message).await? {
                FtsMessageReconcileOutcome::Unchanged => {}
                FtsMessageReconcileOutcome::Indexed => stats.indexed += 1,
                FtsMessageReconcileOutcome::Reindexed => stats.reindexed += 1,
            }
        }
        stats.pruned = self.prune_orphans().await?;
        self.mark_reconciled();
        Ok(stats)
    }

    /// Discover source messages and locator drift without mutating the index.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] when `SQLite` discovery or attachment hydration fails.
    pub async fn discover_reconcile_plan(&self) -> Result<FtsReconcilePlan, RetrievalError> {
        let locator_repair_required: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM message_fts_rows
                WHERE fts_rowid NOT IN (SELECT rowid FROM message_fts)
                UNION ALL
                SELECT 1 FROM message_fts
                WHERE rowid NOT IN (SELECT fts_rowid FROM message_fts_rows)
            )",
        )
        .fetch_one(&self.pool)
        .await?;
        let mut messages = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at FROM messages",
        )
        .try_map(crate::parse_message_row)
        .fetch_all(&self.pool)
        .await?;
        crate::hydrate_attachments(&self.pool, &mut messages).await?;
        Ok(FtsReconcilePlan {
            locator_repair_required,
            messages,
        })
    }

    #[must_use]
    pub fn locator_repair_required(plan: &FtsReconcilePlan) -> bool {
        plan.locator_repair_required
    }

    /// Repair inconsistent FTS physical-row locators atomically.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] when the repair transaction fails.
    pub async fn repair_locator_rows(&self) -> Result<(), RetrievalError> {
        let mut locator_tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM message_fts_rows
             WHERE fts_rowid NOT IN (SELECT rowid FROM message_fts)",
        )
        .execute(&mut *locator_tx)
        .await?;
        sqlx::query(
            "DELETE FROM message_fts
             WHERE rowid NOT IN (SELECT fts_rowid FROM message_fts_rows)",
        )
        .execute(&mut *locator_tx)
        .await?;
        locator_tx.commit().await?;
        Ok(())
    }

    #[must_use]
    pub fn messages(plan: FtsReconcilePlan) -> Vec<Message> {
        plan.messages
    }

    /// Reconcile one discovered source message as one atomic write unit.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] when the index read or write fails.
    pub async fn reconcile_message(
        &self,
        planned_message: Message,
    ) -> Result<FtsMessageReconcileOutcome, RetrievalError> {
        let mut tx = self.pool.begin().await?;
        let mut messages = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE message_id = ?1",
        )
        .bind(&planned_message.message_id)
        .try_map(crate::parse_message_row)
        .fetch_all(&mut *tx)
        .await?;
        if messages.is_empty() {
            tx.rollback().await?;
            return Ok(FtsMessageReconcileOutcome::Unchanged);
        }
        crate::hydrate_attachments_conn(&mut tx, &mut messages).await?;
        let Some(message) = messages.pop() else {
            tx.rollback().await?;
            return Ok(FtsMessageReconcileOutcome::Unchanged);
        };
        #[cfg(test)]
        self.wait_at_source_snapshot_test_barrier().await;
        let existing_rows: Vec<FtsLocatorWitness> = sqlx::query(
            "SELECT r.fts_rowid, r.content_hash,
                    f.rowid IS NOT NULL AS physical_match
             FROM message_fts_rows r
             LEFT JOIN message_fts f ON f.rowid = r.fts_rowid
             WHERE r.message_id = ?1
             ORDER BY r.fts_rowid",
        )
        .bind(&message.message_id)
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            Ok(FtsLocatorWitness {
                fts_rowid: row.try_get("fts_rowid")?,
                content_hash: row.try_get("content_hash")?,
                physical_match: row.try_get("physical_match")?,
            })
        })
        .fetch_all(&mut *tx)
        .await?;
        tx.rollback().await?;
        let fingerprint = content_fingerprint(&index_text(&message));
        if existing_rows.len() == 1
            && existing_rows[0].physical_match
            && existing_rows[0].content_hash == fingerprint
        {
            return Ok(FtsMessageReconcileOutcome::Unchanged);
        }
        Ok(
            if fts_reconcile_upsert(
                &self.pool,
                &message,
                &existing_rows,
                self.sqlite_workload_collector.clone(),
            )
            .await?
            {
                if existing_rows.is_empty() {
                    FtsMessageReconcileOutcome::Indexed
                } else {
                    FtsMessageReconcileOutcome::Reindexed
                }
            } else {
                FtsMessageReconcileOutcome::Unchanged
            },
        )
    }

    /// Prune index rows whose source message no longer exists.
    ///
    /// # Errors
    /// Returns [`RetrievalError`] when the prune transaction fails.
    pub async fn prune_orphans(&self) -> Result<usize, RetrievalError> {
        // Prune index rows with no live source message, evaluated against the
        // CURRENT `messages` table — not the (now possibly stale) snapshot
        // loaded above. This closes a startup race: a hard delete can remove a
        // conversation (and atomically prune its FTS rows) *after* this sweep
        // snapshotted `messages`, and the upsert loop above would then
        // re-insert the deleted message from its stale snapshot. Re-deriving
        // orphans from the live table removes any such re-inserted row.
        let mut prune_tx = self.pool.begin().await?;
        let orphan_rowids: Vec<i64> = sqlx::query_scalar(
            "SELECT fts_rowid FROM message_fts_rows
             WHERE message_id NOT IN (SELECT message_id FROM messages)",
        )
        .fetch_all(&mut *prune_tx)
        .await?;
        for rowid in &orphan_rowids {
            sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                .bind(rowid)
                .execute(&mut *prune_tx)
                .await?;
        }
        sqlx::query(
            "DELETE FROM message_fts_rows
             WHERE message_id NOT IN (SELECT message_id FROM messages)",
        )
        .execute(&mut *prune_tx)
        .await?;
        prune_tx.commit().await?;
        Ok(orphan_rowids.len())
    }

    pub fn mark_reconciled(&self) {
        self.reconciled.store(true, Ordering::Release);
    }
}

impl Fts5Retriever {
    #[allow(clippy::too_many_lines)]
    async fn retrieve_match_expr(
        &self,
        request: &RetrievalRequest,
        match_expr: &str,
        raw_prefix_guard: Option<(&str, Option<&str>)>,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError> {
        let (scope_ids, excluding): (&[String], bool) = match &request.scope {
            RetrievalScope::Global => (&[], false),
            RetrievalScope::GlobalExcluding(ids) => (ids, true),
            RetrievalScope::Conversations(ids) => {
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                (ids, false)
            }
        };

        let mut sql = String::from(
            "WITH ranked_hits AS (\
                 SELECT meta.message_id, meta.chunk_ordinal, meta.conversation_id, \
                        meta.message_type, meta.created_at, c.transcript_generation, \
                        (SELECT COUNT(*) FROM messages count_source WHERE count_source.conversation_id = c.id) AS message_count, \
                        snippet(message_fts, 0, '', '', '…', 24) AS snippet, \
                        bm25(message_fts) AS score",
        );
        sql.push_str(
            " FROM message_fts \
               JOIN message_fts_rows meta ON meta.fts_rowid = message_fts.rowid \
               JOIN messages source ON source.message_id = meta.message_id \
               JOIN conversations c ON c.id = meta.conversation_id \
               WHERE message_fts MATCH ? \
                 AND COALESCE(json_extract(source.display_data, '$.hidden'), 0) != 1",
        );
        if request.visibility == RetrievalVisibility::UserTopLevel {
            sql.push_str(
                " AND c.user_initiated = 1 AND c.runtime_role = 'user' \
                  AND c.parent_conversation_id IS NULL \
                  AND NOT (c.archived = 1 AND EXISTS (\
                      SELECT 1 FROM conversation_creation_jobs j \
                      WHERE j.conversation_id = c.id AND j.status = 'deletion_pending'\
                  ))",
            );
        }
        if let Some((_, earlier_expr)) = raw_prefix_guard {
            if earlier_expr.is_some() {
                sql.push_str(
                    " AND (message_fts.rowid IN (\
                        SELECT rowid FROM message_fts WHERE message_fts MATCH ?\
                      ) OR instr(lower(message_fts.text), ?) > 0)",
                );
            } else {
                sql.push_str(" AND instr(lower(message_fts.text), ?) > 0");
            }
        }
        if !scope_ids.is_empty() {
            if excluding {
                sql.push_str(" AND meta.conversation_id NOT IN (");
            } else {
                sql.push_str(" AND meta.conversation_id IN (");
            }
            for i in 0..scope_ids.len() {
                if i > 0 {
                    sql.push(',');
                }
                sql.push('?');
            }
            sql.push(')');
        }
        sql.push(')');
        match request.grouping {
            RetrievalGrouping::None => {
                sql.push_str(
                    " SELECT message_id, chunk_ordinal, conversation_id, message_type, created_at, transcript_generation, message_count, snippet, score \
                      FROM ranked_hits \
                      ORDER BY score, created_at DESC \
                      LIMIT ?",
                );
            }
            RetrievalGrouping::BestPerConversation => {
                sql.push_str(
                    ", grouped_hits AS (\
                         SELECT message_id, chunk_ordinal, conversation_id, message_type, created_at, transcript_generation, message_count, snippet, score, \
                                ROW_NUMBER() OVER (\
                                    PARTITION BY conversation_id \
                                    ORDER BY score, created_at DESC, message_id\
                                ) AS conversation_rank \
                         FROM ranked_hits\
                     ) \
                     SELECT message_id, chunk_ordinal, conversation_id, message_type, created_at, transcript_generation, message_count, snippet, score \
                     FROM grouped_hits \
                     WHERE conversation_rank = 1 \
                     ORDER BY score, created_at DESC, conversation_id \
                     LIMIT ?",
                );
            }
        }

        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(match_expr);
        if let Some((guard, earlier_expr)) = raw_prefix_guard {
            if let Some(earlier_expr) = earlier_expr {
                q = q.bind(earlier_expr);
            }
            q = q.bind(guard);
        }
        for id in scope_ids {
            q = q.bind(id);
        }
        let limit = i64::try_from(request.limit).unwrap_or(i64::MAX);
        q = q.bind(limit);

        q.try_map(parse_chunk_row)
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl MessageRetriever for Fts5Retriever {
    fn index_reconciled(&self) -> bool {
        self.index_reconciled()
    }

    async fn retrieve(
        &self,
        request: RetrievalRequest,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError> {
        let Some(match_expr) = build_fts_query(&request.query, request.match_mode) else {
            return Ok(Vec::new());
        };
        let terms = content_terms(&request.query);
        let raw_prefix_guard = if request.match_mode == RetrievalMatchMode::FinalTokenPrefix {
            terms.last().and_then(|term| {
                raw_prefix_guard(term).map(|guard| {
                    let earlier = terms[..terms.len() - 1]
                        .iter()
                        .map(|term| format!("\"{term}\""))
                        .collect::<Vec<_>>()
                        .join(" OR ");
                    (guard, (!earlier.is_empty()).then_some(earlier))
                })
            })
        } else {
            None
        };
        self.retrieve_match_expr(
            &request,
            &match_expr,
            raw_prefix_guard
                .as_ref()
                .map(|(guard, earlier)| (guard.as_str(), earlier.as_deref())),
        )
        .await
    }

    async fn is_fresh_for(&self, conversation_ids: &[String]) -> Result<bool, RetrievalError> {
        if conversation_ids.is_empty() {
            return Ok(true);
        }
        let placeholders = {
            let mut s = String::new();
            for i in 0..conversation_ids.len() {
                if i > 0 {
                    s.push(',');
                }
                s.push('?');
            }
            s
        };

        // Current source messages for these conversations.
        let mut messages = {
            let sql = format!(
                "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at \
                 FROM messages WHERE conversation_id IN ({placeholders})"
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
            for id in conversation_ids {
                q = q.bind(id);
            }
            q.try_map(crate::parse_message_row)
                .fetch_all(&self.pool)
                .await?
        };
        // Hydrate attachments so the recomputed fingerprints match the
        // (hydrated) text that was indexed at write time — otherwise every
        // user/skill message carrying files would read as permanently stale.
        crate::hydrate_attachments(&self.pool, &mut messages).await?;

        // Indexed content fingerprints for these conversations. The ordinary
        // locator table makes this metadata-only scope check a B-tree lookup;
        // filtering the FTS5 virtual table by its UNINDEXED conversation_id
        // would scan the entire corpus. Keep the raw Vec — not just a
        // deduplicated map — so duplicate physical rows remain visible.
        let indexed_rows: Vec<(String, String, bool)> = {
            let sql = format!(
                "SELECT r.message_id, r.content_hash, f.rowid IS NOT NULL AS physical_match
                 FROM message_fts_rows r
                 LEFT JOIN message_fts f ON f.rowid = r.fts_rowid
                 WHERE r.conversation_id IN ({placeholders})"
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
            for id in conversation_ids {
                q = q.bind(id);
            }
            q.try_map(|row: sqlx::sqlite::SqliteRow| {
                Ok((
                    row.try_get::<String, _>("message_id")?,
                    row.try_get::<String, _>("content_hash")?,
                    row.try_get::<bool, _>("physical_match")?,
                ))
            })
            .fetch_all(&self.pool)
            .await?
        };
        if indexed_rows.iter().any(|(_, _, matches)| !matches) {
            return Ok(false);
        }
        let indexed: HashMap<&str, &str> = indexed_rows
            .iter()
            .map(|(id, hash, _)| (id.as_str(), hash.as_str()))
            .collect();

        // Fresh iff every current message has an index row whose fingerprint
        // matches the current extraction (missing OR stale → not fresh).
        for m in &messages {
            match indexed.get(m.message_id.as_str()) {
                Some(stored) if *stored == content_fingerprint(&index_text(m)) => {}
                _ => return Ok(false),
            }
        }
        // Reject orphaned/duplicate index rows: a *physical* row in these
        // conversations with no live source message (failed delete hook, or
        // before the startup reconcile prunes) — or a stale duplicate row for a
        // live message_id — would still surface deleted/edited-away content in
        // search. Every current message is present after the loop above, so a
        // physical-row count differing from the live message count means at
        // least one orphan or duplicate — not fresh until it is pruned. Counting
        // physical rows (not the deduplicated map) is deliberate: a `HashMap`
        // would collapse a stale+fresh pair for one id and hide the duplicate.
        if indexed_rows.len() != messages.len() {
            return Ok(false);
        }
        Ok(true)
    }
}

// ---- index maintenance (called by `Database` write paths) ----

fn sqlx_from_db_error(error: crate::DbError) -> sqlx::Error {
    match error {
        crate::DbError::Sqlx(error) => error,
        crate::DbError::ConversationNotFound(_)
        | crate::DbError::ConversationAlreadyExists(_)
        | crate::DbError::MessageNotFound(_)
        | crate::DbError::SlugExists(_)
        | crate::DbError::Serialization(_)
        | crate::DbError::ContinuationPrecondition(_)
        | crate::DbError::CloseFoundationConflict(_)
        | crate::DbError::CloseAdmissionFenced(_)
        | crate::DbError::CloseFoundationPrecondition(_)
        | crate::DbError::CloseFoundationRepairRequired(_)
        | crate::DbError::CloseFoundationNotFound(_)
        | crate::DbError::DirectTurnConflict(_)
        | crate::DbError::ForkProposalConflict(_)
        | crate::DbError::GitRepositoryWorkScopeProjectConflict { .. }
        | crate::DbError::DormantGitRepositoryCatchupPermitTargetMismatch
        | crate::DbError::DormantGitRepositoryCatchupStaleOperation
        | crate::DbError::DormantGitRepositoryCatchupBlockedByReadinessClaim
        | crate::DbError::DormantGitRepositoryReadinessCatchupInProgress
        | crate::DbError::DormantGitRepositoryReadinessReceiptTargetMismatch
        | crate::DbError::DormantGitRepositoryReadinessReceiptOperationMismatch => {
            unreachable!("retrieval telemetry closure only returns SQLx errors")
        }
    }
}

/// (Re)index a single message: replace any existing index row(s) for its id
/// with a fresh extraction. Idempotent for a given message content.
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete or insert fails.
pub async fn fts_upsert(
    pool: &SqlitePool,
    message: &Message,
    collector: SqliteWorkloadCollector,
) -> Result<(), sqlx::Error> {
    let telemetry = SqliteTelemetry::with_collector(
        SqliteOperation::FtsUpsert,
        SqliteWorkloadCategory::Fts,
        SqliteAccessKind::Write,
        collector,
    );
    let (mut connection, pool_timing) = telemetry
        .observe_pool_acquisition_sqlx(pool.acquire())
        .await?;
    let (mut tx, transaction_timing) = telemetry
        .observe_transaction_admission_db(pool_timing, async {
            Ok(connection.begin_with("BEGIN IMMEDIATE").await?)
        })
        .await
        .map_err(sqlx_from_db_error)?;
    fts_upsert_conn(&mut tx, message, FtsObservation::Standalone(&telemetry)).await?;
    telemetry
        .observe_commit_db(transaction_timing, async { Ok(tx.commit().await?) })
        .await
        .map_err(sqlx_from_db_error)?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum FtsObservation<'a> {
    Standalone(&'a SqliteTelemetry),
    ParentTransaction(ParentSqliteObserver<'a>),
}

impl FtsObservation<'_> {
    async fn observe<T>(
        self,
        phase: SqlitePhase,
        operation: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, sqlx::Error> {
        match self {
            Self::Standalone(telemetry) => telemetry.observe_sqlx(phase, operation).await,
            Self::ParentTransaction(observer) => observer.observe(phase, operation).await,
        }
    }
}

pub(crate) async fn fts_upsert_conn(
    conn: &mut sqlx::SqliteConnection,
    message: &Message,
    observation: FtsObservation<'_>,
) -> Result<(), sqlx::Error> {
    let text = index_text(message);
    let fingerprint = content_fingerprint(&text);
    delete_message_rows(conn, &message.message_id, observation).await?;
    let inserted = observation
        .observe(
            SqlitePhase::FtsInsert,
            sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
                .bind(text)
                .execute(&mut *conn),
        )
        .await?;
    record_fts_row(
        conn,
        inserted.last_insert_rowid(),
        message,
        &fingerprint,
        observation,
    )
    .await?;
    Ok(())
}

async fn delete_message_rows(
    conn: &mut sqlx::SqliteConnection,
    message_id: &str,
    observation: FtsObservation<'_>,
) -> Result<(), sqlx::Error> {
    let rowids: Vec<i64> = observation
        .observe(
            SqlitePhase::LocatorLookup,
            sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE message_id = ?1")
                .bind(message_id)
                .fetch_all(&mut *conn),
        )
        .await?;
    for rowid in rowids {
        observation
            .observe(
                SqlitePhase::FtsRowDelete,
                sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                    .bind(rowid)
                    .execute(&mut *conn),
            )
            .await?;
    }
    observation
        .observe(
            SqlitePhase::LocatorDelete,
            sqlx::query("DELETE FROM message_fts_rows WHERE message_id = ?1")
                .bind(message_id)
                .execute(&mut *conn),
        )
        .await?;
    Ok(())
}

async fn record_fts_row(
    conn: &mut sqlx::SqliteConnection,
    fts_rowid: i64,
    message: &Message,
    fingerprint: &str,
    observation: FtsObservation<'_>,
) -> Result<(), sqlx::Error> {
    observation
        .observe(
            SqlitePhase::LocatorInsert,
            sqlx::query(
                "INSERT INTO message_fts_rows
                 (fts_rowid, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash)
                 VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)",
            )
            .bind(fts_rowid)
            .bind(&message.message_id)
            .bind(&message.conversation_id)
            .bind(message.message_type.to_string())
            .bind(message.created_at.to_rfc3339())
            .bind(fingerprint)
            .execute(&mut *conn),
        )
        .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtsLocatorWitness {
    pub fts_rowid: i64,
    pub content_hash: String,
    pub physical_match: bool,
}

/// Reconcile-path upsert that will not clobber a fresher concurrent write.
///
/// The startup reconcile snapshots `messages`, then upserts changed/absent
/// rows one at a time. If a live edit re-indexes the same message between the
/// snapshot and this write, writing the snapshot would overwrite the fresher
/// content (and leave `is_fresh_for` false until the next update). Guard the
/// write on the index state the reconcile observed: for a stale row, replace
/// it only while it still carries the observed `content_hash` (compare-and-set
/// — a concurrent writer that already changed it wins); for an absent row,
/// insert only while it is still absent. `SQLite` serializes write
/// transactions, so the guard check and the write commit atomically against
/// the edit path. Returns whether a write was performed (for accurate stats).
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if a query fails.
#[allow(clippy::too_many_lines)] // one transaction validates and replaces one witnessed index generation
pub async fn fts_reconcile_upsert(
    pool: &SqlitePool,
    message: &Message,
    observed: &[FtsLocatorWitness],
    collector: SqliteWorkloadCollector,
) -> Result<bool, sqlx::Error> {
    let telemetry = SqliteTelemetry::with_collector(
        SqliteOperation::FtsReconcileUpsert,
        SqliteWorkloadCategory::Fts,
        SqliteAccessKind::Write,
        collector,
    );
    let text = index_text(message);
    let fingerprint = content_fingerprint(&text);
    let (mut connection, pool_timing) = telemetry
        .observe_pool_acquisition_sqlx(pool.acquire())
        .await?;
    let (mut tx, transaction_timing) = telemetry
        .observe_transaction_admission_db(pool_timing, async {
            Ok(connection.begin_with("BEGIN IMMEDIATE").await?)
        })
        .await
        .map_err(sqlx_from_db_error)?;
    if observed.is_empty() {
        let existing: i64 = telemetry
            .observe_sqlx(
                SqlitePhase::LocatorLookup,
                sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE message_id = ?1")
                    .bind(&message.message_id)
                    .fetch_one(&mut *tx),
            )
            .await?;
        if existing > 0 {
            telemetry
                .observe_rollback_db(transaction_timing, async { Ok(tx.rollback().await?) })
                .await
                .map_err(sqlx_from_db_error)?;
            return Ok(false);
        }
    } else {
        let current: Vec<FtsLocatorWitness> = telemetry
            .observe_sqlx(
                SqlitePhase::LocatorLookup,
                sqlx::query(
                    "SELECT r.fts_rowid, r.content_hash,
                            f.rowid IS NOT NULL AS physical_match
                     FROM message_fts_rows r
                     LEFT JOIN message_fts f ON f.rowid = r.fts_rowid
                     WHERE r.message_id = ?1
                     ORDER BY r.fts_rowid",
                )
                .bind(&message.message_id)
                .try_map(|row: sqlx::sqlite::SqliteRow| {
                    Ok(FtsLocatorWitness {
                        fts_rowid: row.try_get("fts_rowid")?,
                        content_hash: row.try_get("content_hash")?,
                        physical_match: row.try_get("physical_match")?,
                    })
                })
                .fetch_all(&mut *tx),
            )
            .await?;
        if current != observed || current.iter().any(|entry| !entry.physical_match) {
            telemetry
                .observe_rollback_db(transaction_timing, async { Ok(tx.rollback().await?) })
                .await
                .map_err(sqlx_from_db_error)?;
            return Ok(false);
        }
        for entry in &current {
            let deleted = telemetry
                .observe_sqlx(
                    SqlitePhase::FtsRowDelete,
                    sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                        .bind(entry.fts_rowid)
                        .execute(&mut *tx),
                )
                .await?;
            if deleted.rows_affected() != 1 {
                telemetry
                    .observe_rollback_db(transaction_timing, async { Ok(tx.rollback().await?) })
                    .await
                    .map_err(sqlx_from_db_error)?;
                return Ok(false);
            }
            telemetry
                .observe_sqlx(
                    SqlitePhase::LocatorDelete,
                    sqlx::query("DELETE FROM message_fts_rows WHERE fts_rowid = ?1")
                        .bind(entry.fts_rowid)
                        .execute(&mut *tx),
                )
                .await?;
        }
    }
    let inserted = telemetry
        .observe_sqlx(
            SqlitePhase::FtsInsert,
            sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
                .bind(text)
                .execute(&mut *tx),
        )
        .await?;
    record_fts_row(
        &mut tx,
        inserted.last_insert_rowid(),
        message,
        &fingerprint,
        FtsObservation::Standalone(&telemetry),
    )
    .await?;
    telemetry
        .observe_commit_db(transaction_timing, async { Ok(tx.commit().await?) })
        .await
        .map_err(sqlx_from_db_error)?;
    Ok(true)
}

pub(crate) async fn fts_index_message_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &Message,
    observer: ParentSqliteObserver<'_>,
) -> Result<(), sqlx::Error> {
    delete_message_rows(
        tx,
        &message.message_id,
        FtsObservation::ParentTransaction(observer),
    )
    .await?;
    let text = index_text(message);
    let inserted = observer
        .observe(
            SqlitePhase::FtsInsert,
            sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
                .bind(&text)
                .execute(&mut **tx),
        )
        .await?;
    record_fts_row(
        tx,
        inserted.last_insert_rowid(),
        message,
        &content_fingerprint(&text),
        FtsObservation::ParentTransaction(observer),
    )
    .await
}

pub(crate) async fn fts_hide_message_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &Message,
    observer: ParentSqliteObserver<'_>,
) -> Result<(), sqlx::Error> {
    delete_message_rows(
        tx,
        &message.message_id,
        FtsObservation::ParentTransaction(observer),
    )
    .await?;
    let inserted = observer
        .observe(
            SqlitePhase::FtsInsert,
            sqlx::query("INSERT INTO message_fts (text) VALUES ('')").execute(&mut **tx),
        )
        .await?;
    record_fts_row(
        tx,
        inserted.last_insert_rowid(),
        message,
        &content_fingerprint(""),
        FtsObservation::ParentTransaction(observer),
    )
    .await
}

pub(crate) async fn fts_delete_conversation_conn(
    conn: &mut sqlx::SqliteConnection,
    conversation_id: &str,
    observer: ParentSqliteObserver<'_>,
) -> Result<(), sqlx::Error> {
    let rowids: Vec<i64> =
        sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE conversation_id = ?1")
            .bind(conversation_id)
            .fetch_all(&mut *conn)
            .await?;
    for rowid in rowids {
        observer
            .observe(
                SqlitePhase::FtsRowDelete,
                sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                    .bind(rowid)
                    .execute(&mut *conn),
            )
            .await?;
    }
    observer
        .observe(
            SqlitePhase::LocatorDelete,
            sqlx::query("DELETE FROM message_fts_rows WHERE conversation_id = ?1")
                .bind(conversation_id)
                .execute(&mut *conn),
        )
        .await?;
    Ok(())
}

// ---- helpers ----

#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_chunk_row(row: sqlx::sqlite::SqliteRow) -> Result<RetrievedChunk, sqlx::Error> {
    const MAX_SNIPPET_CHARS: usize = 240;
    let ordinal: i64 = row.try_get("chunk_ordinal")?;
    let snippet: String = row.try_get("snippet")?;
    Ok(RetrievedChunk {
        conversation_id: row.try_get("conversation_id")?,
        message_id: row.try_get("message_id")?,
        chunk: ChunkRef {
            ordinal: u32::try_from(ordinal).unwrap_or(0),
            char_range: None,
        },
        message_type: crate::parse_message_type(&row.try_get::<String, _>("message_type")?),
        created_at: crate::parse_datetime(&row.try_get::<String, _>("created_at")?),
        snippet: truncate_chars(&snippet, MAX_SNIPPET_CHARS),
        score: row.try_get("score")?,
        transcript_generation: row.try_get("transcript_generation")?,
        message_count: row.try_get("message_count")?,
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let retained = max_chars.saturating_sub(1);
    let mut bounded: String = value.chars().take(retained).collect();
    bounded.push('…');
    bounded
}

/// Stop words dropped from a natural-language question so filler does not
/// constrain the lexical match (REQ-RET-001).
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "with", "what", "which", "who",
    "whom", "whose", "when", "where", "why", "how", "did", "do", "does", "is", "are", "was",
    "were", "be", "been", "we", "i", "you", "it", "this", "that", "these", "those", "about", "my",
    "our", "your", "me", "us",
];

/// Build an FTS5 MATCH expression from a natural-language question
/// (REQ-RET-001). Tokenizes on non-alphanumerics, lowercases, drops stop
/// words and FTS5-operator characters, and joins the remaining content terms
/// with `OR` (each quoted as a literal). The final content-bearing token is a
/// prefix term so incomplete palette input can match the start of the last
/// word typed. Returns `None` when nothing content-bearing remains (the caller
/// returns an empty result).
fn content_terms(natural: &str) -> Vec<String> {
    natural
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
}

fn build_fts_query(natural: &str, match_mode: RetrievalMatchMode) -> Option<String> {
    let mut terms = content_terms(natural);
    if terms.is_empty() {
        return None;
    }
    let last = terms.len() - 1;
    let terms: Vec<String> = terms
        .drain(..)
        .enumerate()
        .map(|(idx, t)| {
            if idx == last && match_mode == RetrievalMatchMode::FinalTokenPrefix {
                build_prefix_alternatives(&t)
            } else {
                format!("\"{t}\"")
            }
        })
        .collect();
    Some(terms.join(" OR "))
}

fn porter_fallback_stems(term: &str) -> Vec<String> {
    const MIN_STEM_PREFIX_CHARS: usize = 4;
    const SUFFIX_RULES: &[(&str, &str)] = &[
        ("izatio", ""),
        ("ational", "ate"),
        ("tional", "tion"),
        ("enci", "ence"),
        ("anci", "ance"),
        ("abli", "able"),
        ("izer", "ize"),
        ("alli", "al"),
        ("entli", "ent"),
        ("eli", "e"),
        ("ousli", "ous"),
        ("nni", "n"),
        ("ing", ""),
        ("ie", "i"),
        ("i", ""),
    ];
    if !term.is_ascii() || term.len() < MIN_STEM_PREFIX_CHARS {
        return Vec::new();
    }
    SUFFIX_RULES
        .iter()
        .filter_map(|(suffix, replacement)| {
            term.strip_suffix(suffix).and_then(|base| {
                let stem = format!("{base}{replacement}");
                (stem.len() >= 3 && stem != term).then_some(stem)
            })
        })
        .collect()
}

fn raw_prefix_guard(term: &str) -> Option<String> {
    (term.is_ascii() && term.len() >= 4).then(|| term.to_string())
}

fn build_prefix_alternatives(term: &str) -> String {
    let mut terms = vec![format!("\"{term}\"*")];
    terms.extend(
        porter_fallback_stems(term)
            .into_iter()
            .map(|stem| format!("\"{stem}\"*")),
    );
    if terms.len() == 1 {
        terms.pop().expect("exact prefix exists")
    } else {
        format!("({})", terms.join(" OR "))
    }
}

/// Stable, dependency-free content fingerprint (FNV-1a 64-bit, hex). Used to
/// detect in-place content changes during reconciliation. Stability across
/// runs is all that matters; if the algorithm ever changed, reconcile would
/// simply re-index once.
fn content_fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use phoenix_core::domain::db_schema::MessageContent;
    use std::collections::HashSet;

    fn global_request(query: &str) -> RetrievalRequest {
        RetrievalRequest::natural_language(query, RetrievalScope::Global, 10)
    }

    #[test]
    fn build_query_drops_filler_and_ors_terms() {
        let q = build_fts_query(
            "what did we decide about the auth schema",
            RetrievalMatchMode::ExactTerms,
        )
        .unwrap();
        assert!(q.contains("\"auth\""));
        assert!(q.contains("\"schema\""));
        assert!(q.contains("\"decide\""));
        assert!(q.contains(" OR "));
        assert!(!q.contains("\"what\""));
        assert!(!q.contains("\"the\""));
    }

    #[test]
    fn build_query_empty_when_only_stopwords() {
        assert!(build_fts_query("what did we do", RetrievalMatchMode::ExactTerms).is_none());
        assert!(build_fts_query("   ", RetrievalMatchMode::ExactTerms).is_none());
    }

    #[test]
    fn build_query_keeps_exact_terms_exact() {
        let q = build_fts_query("observed kang", RetrievalMatchMode::ExactTerms).unwrap();
        assert_eq!(q, "\"observed\" OR \"kang\"");
    }

    #[test]
    fn build_query_prefixes_only_for_palette_mode() {
        let q = build_fts_query("observed runni", RetrievalMatchMode::FinalTokenPrefix).unwrap();
        assert!(q.starts_with("\"observed\" OR (\"runni\"* OR "));
        assert!(q.contains("\"run\"*"));
    }

    #[test]
    fn retrieval_request_exposes_backend_policy_read_only() {
        let request = RetrievalRequest::palette_conversation_search("needle", 7);
        assert_eq!(request.query(), "needle");
        assert!(matches!(request.scope(), RetrievalScope::Global));
        assert_eq!(request.visibility(), RetrievalVisibility::UserTopLevel);
        assert_eq!(request.grouping(), RetrievalGrouping::BestPerConversation);
        assert_eq!(request.match_mode(), RetrievalMatchMode::FinalTokenPrefix);
        assert_eq!(request.limit(), 7);
    }

    #[test]
    fn snippets_are_bounded_by_unicode_characters() {
        let input = "🦀".repeat(300);
        let bounded = truncate_chars(&input, 240);
        assert_eq!(bounded.chars().count(), 240);
        assert!(bounded.ends_with('…'));
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn fingerprint_changes_with_content() {
        assert_ne!(content_fingerprint("alpha"), content_fingerprint("beta"));
        assert_eq!(content_fingerprint("alpha"), content_fingerprint("alpha"));
    }

    async fn seed() -> Database {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("c-a", "a", "/tmp/a", true, None, None)
            .await
            .unwrap();
        db.create_conversation("c-b", "b", "/tmp/b", true, None, None)
            .await
            .unwrap();
        db
    }

    async fn insert_index_row(
        pool: &SqlitePool,
        text: &str,
        message_id: &str,
        chunk_ordinal: i64,
        conversation_id: &str,
        content_hash: &str,
    ) -> i64 {
        let inserted = sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
            .bind(text)
            .execute(pool)
            .await
            .unwrap();
        let rowid = inserted.last_insert_rowid();
        sqlx::query(
            "INSERT INTO message_fts_rows
             (fts_rowid, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash)
             VALUES (?1, ?2, ?3, ?4, 'user', '2026-01-01T00:00:00Z', ?5)",
        )
        .bind(rowid)
        .bind(message_id)
        .bind(chunk_ordinal)
        .bind(conversation_id)
        .bind(content_hash)
        .execute(pool)
        .await
        .unwrap();
        rowid
    }

    #[tokio::test]
    async fn standalone_fts_upsert_records_exact_shared_collector_outcomes() {
        let db = seed().await;
        let message = Message {
            message_id: "fts-outcome".to_string(),
            conversation_id: "c-a".to_string(),
            sequence_id: 1,
            message_type: MessageType::User,
            content: MessageContent::user("shared collector outcome"),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let before = db.sqlite_workload_aggregate_report(
            crate::SqliteSnapshotWindow::OneHour,
            crate::sqlite_workload::unix_now_micros(),
        );

        fts_upsert(db.pool(), &message, db.sqlite_workload_collector.clone())
            .await
            .unwrap();
        let after_success = db.sqlite_workload_aggregate_report(
            crate::SqliteSnapshotWindow::OneHour,
            crate::sqlite_workload::unix_now_micros(),
        );
        let write = SqliteAccessKind::Write.index();
        let fts = SqliteWorkloadCategory::Fts.index();
        assert_eq!(
            after_success.outcomes[write][fts][crate::SqliteOutcome::Success.index()],
            before.outcomes[write][fts][crate::SqliteOutcome::Success.index()] + 1,
        );
        assert_eq!(
            crate::sqlite_workload::operation_count(&after_success.outcomes[write][fts]),
            crate::sqlite_workload::operation_count(&before.outcomes[write][fts]) + 1,
        );

        sqlx::query("DROP TABLE message_fts")
            .execute(db.pool())
            .await
            .unwrap();
        fts_upsert(db.pool(), &message, db.sqlite_workload_collector.clone())
            .await
            .unwrap_err();
        let after_failure = db.sqlite_workload_aggregate_report(
            crate::SqliteSnapshotWindow::OneHour,
            crate::sqlite_workload::unix_now_micros(),
        );
        assert_eq!(
            crate::sqlite_workload::operation_count(&after_failure.outcomes[write][fts]),
            crate::sqlite_workload::operation_count(&after_success.outcomes[write][fts]) + 1,
        );
        assert_eq!(
            after_failure.outcomes[write][fts][crate::SqliteOutcome::OtherFailure.index()],
            after_success.outcomes[write][fts][crate::SqliteOutcome::OtherFailure.index()] + 1,
        );
    }

    #[tokio::test]
    async fn indexes_on_insert_and_retrieves_in_scope() {
        let db = seed().await;
        db.add_message(
            "m1",
            "c-a",
            &MessageContent::user("the rate limiter uses a token bucket"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "m2",
            "c-b",
            &MessageContent::user("auth schema migration plan"),
            None,
            None,
        )
        .await
        .unwrap();

        let r = db.fts_retriever();

        // Global finds the rate-limiter message.
        let hits = r
            .retrieve(global_request("how does the rate limiter work"))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m1");
        assert_eq!(hits[0].conversation_id, "c-a");

        // Scoped to c-b only: the rate-limiter query returns nothing in-scope.
        let scoped = r
            .retrieve(RetrievalRequest {
                query: "rate limiter".to_string(),
                scope: RetrievalScope::Conversations(vec!["c-b".into()]),
                visibility: RetrievalVisibility::All,
                grouping: RetrievalGrouping::None,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 10,
            })
            .await
            .unwrap();
        assert!(scoped.is_empty());

        // Scoped to c-b: its own content is found.
        let scoped = r
            .retrieve(RetrievalRequest {
                query: "auth schema".to_string(),
                scope: RetrievalScope::Conversations(vec!["c-b".into()]),
                visibility: RetrievalVisibility::All,
                grouping: RetrievalGrouping::None,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].message_id, "m2");
    }

    #[tokio::test]
    async fn hidden_messages_are_excluded_even_when_already_indexed() {
        let db = seed().await;
        db.add_message(
            "hidden-1",
            "c-a",
            &MessageContent::user("confidential recovery artifact"),
            None,
            None,
        )
        .await
        .unwrap();

        let retriever = db.fts_retriever();
        assert_eq!(
            retriever
                .retrieve(global_request("confidential recovery"))
                .await
                .unwrap()
                .len(),
            1
        );

        db.update_message_display_data("hidden-1", &serde_json::json!({ "hidden": true }))
            .await
            .unwrap();

        assert!(retriever
            .retrieve(global_request("confidential recovery"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn reindexes_on_content_update() {
        let db = seed().await;
        db.add_message(
            "t1",
            "c-a",
            &MessageContent::tool("t-use", "compiling crate alpha", false),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();
        assert_eq!(r.retrieve(global_request("alpha")).await.unwrap().len(), 1);

        db.update_tool_message_content("t1", "compiling crate omega")
            .await
            .unwrap();
        // Old term gone, new term present.
        assert!(r
            .retrieve(global_request("alpha"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(r.retrieve(global_request("omega")).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prunes_index_on_conversation_delete() {
        let db = seed().await;
        db.add_message(
            "m1",
            "c-a",
            &MessageContent::user("zebra crossing"),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();
        assert_eq!(r.retrieve(global_request("zebra")).await.unwrap().len(), 1);

        db.delete_conversation("c-a").await.unwrap();
        assert!(r
            .retrieve(global_request("zebra"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn reconcile_backfills_and_prunes() {
        let db = seed().await;
        db.add_message(
            "m1",
            "c-a",
            &MessageContent::user("orange marmalade"),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();

        // Simulate a stale index: wipe it, plus inject an orphan row.
        sqlx::query("DELETE FROM message_fts")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM message_fts_rows")
            .execute(db.pool())
            .await
            .unwrap();
        insert_index_row(db.pool(), "ghost", "gone", 0, "c-a", "x").await;

        let stats = r.reconcile().await.unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.pruned, 1);
        assert!(r.index_reconciled());

        // Real message is searchable again; orphan is gone.
        assert_eq!(
            r.retrieve(global_request("marmalade")).await.unwrap().len(),
            1
        );
        assert!(r
            .retrieve(global_request("ghost"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn reconcile_plan_discovery_is_read_only() {
        let db = seed().await;
        db.add_message(
            "m1",
            "c-a",
            &MessageContent::user("planned retrieval"),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM message_fts")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM message_fts_rows")
            .execute(db.pool())
            .await
            .unwrap();
        let r = db.fts_retriever();

        let plan = r.discover_reconcile_plan().await.unwrap();

        assert_eq!(Fts5Retriever::messages(plan).len(), 1);
        let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(indexed, 0);
    }

    #[tokio::test]
    async fn reconcile_rolls_back_physical_prune_when_locator_prune_fails() {
        let db = seed().await;
        let orphan = insert_index_row(db.pool(), "ghost", "gone", 0, "c-a", "hash").await;
        sqlx::query(
            "CREATE TRIGGER fail_orphan_locator_delete
             BEFORE DELETE ON message_fts_rows
             WHEN OLD.message_id = 'gone'
             BEGIN
                 SELECT RAISE(ABORT, 'injected locator prune failure');
             END",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let r = db.fts_retriever();
        assert!(r.reconcile().await.is_err());
        let physical: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_fts WHERE rowid = ?1")
            .bind(orphan)
            .fetch_one(db.pool())
            .await
            .unwrap();
        let locator: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE fts_rowid = ?1")
                .bind(orphan)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((physical, locator), (1, 1));
    }

    #[tokio::test]
    async fn message_replacement_has_an_indexed_rowid_locator() {
        let db = seed().await;
        db.add_message(
            "m-plan",
            "c-a",
            &MessageContent::user("indexed locator"),
            None,
            None,
        )
        .await
        .unwrap();

        let locator_plan: Vec<String> = sqlx::query(
            "EXPLAIN QUERY PLAN
             SELECT fts_rowid FROM message_fts_rows WHERE message_id = 'm-plan'",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get("detail"))
        .collect();
        assert!(
            locator_plan
                .join("\n")
                .contains("idx_message_fts_rows_message_id"),
            "{}",
            locator_plan.join("\n")
        );
    }

    #[tokio::test]
    async fn freshness_scope_uses_the_conversation_locator_index() {
        let db = seed().await;
        let locator_plan: Vec<String> = sqlx::query(
            "EXPLAIN QUERY PLAN
             SELECT r.message_id, r.content_hash, f.rowid IS NOT NULL
             FROM message_fts_rows r
             LEFT JOIN message_fts f ON f.rowid = r.fts_rowid
             WHERE r.conversation_id IN ('c-a')",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get("detail"))
        .collect();
        assert!(
            locator_plan
                .join("\n")
                .contains("idx_message_fts_rows_conversation_id"),
            "{}",
            locator_plan.join("\n")
        );
    }

    #[tokio::test]
    async fn stale_reconcile_plan_cannot_overwrite_fresh_source_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retrieval-stale-plan.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        db.create_conversation("c-a", "a", "/tmp/a", true, None, None)
            .await
            .unwrap();
        let writer = Database::open(path.to_str().unwrap()).await.unwrap();
        db.add_message(
            "m-stale-plan",
            "c-a",
            &MessageContent::user("visible stale plan token"),
            None,
            None,
        )
        .await
        .unwrap();
        let mut retriever = db.fts_retriever();
        let stale_message =
            Fts5Retriever::messages(retriever.discover_reconcile_plan().await.unwrap())
                .into_iter()
                .find(|message| message.message_id == "m-stale-plan")
                .unwrap();
        let barrier = Arc::new(SourceSnapshotTestBarrier::default());
        retriever.install_source_snapshot_test_barrier(barrier.clone());
        let reconcile =
            tokio::spawn(async move { retriever.reconcile_message(stale_message).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            barrier.hydrated.notified(),
        )
        .await
        .expect("reconcile hydrates source snapshot");

        writer
            .update_message_display_data("m-stale-plan", &serde_json::json!({ "hidden": true }))
            .await
            .unwrap();
        barrier.release.notify_one();
        assert!(matches!(
            reconcile.await.unwrap().unwrap(),
            FtsMessageReconcileOutcome::Unchanged
        ));
        let verifier = db.fts_retriever();
        assert!(verifier
            .retrieve(global_request("visible stale plan token"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn reconcile_replaces_only_the_observed_locator_set() {
        let db = seed().await;
        let stale = db
            .add_message(
                "m-cas",
                "c-a",
                &MessageContent::user("stale source"),
                None,
                None,
            )
            .await
            .unwrap();
        let stale_hash = content_fingerprint(&index_text(&stale));
        let stale_rowid: i64 =
            sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE message_id = 'm-cas'")
                .fetch_one(db.pool())
                .await
                .unwrap();

        let current_text = "current concurrent row";
        let current_hash = content_fingerprint(current_text);
        let current_rowid =
            insert_index_row(db.pool(), current_text, "m-cas", 0, "c-a", &current_hash).await;

        let snapshot = Message {
            content: MessageContent::user("reconciled snapshot"),
            ..stale
        };
        assert!(!fts_reconcile_upsert(
            db.pool(),
            &snapshot,
            &[FtsLocatorWitness {
                fts_rowid: stale_rowid,
                content_hash: stale_hash,
                physical_match: true,
            }],
            db.sqlite_workload_collector.clone(),
        )
        .await
        .unwrap());

        let stale_locator_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE fts_rowid = ?1")
                .bind(stale_rowid)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(stale_locator_rows, 1);

        let current_fts_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts WHERE rowid = ?1")
                .bind(current_rowid)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let current_locator_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE fts_rowid = ?1")
                .bind(current_rowid)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(current_fts_rows, 1);
        assert_eq!(current_locator_rows, 1);
    }

    #[tokio::test]
    async fn fts_stores_only_searchable_text() {
        let db = seed().await;
        let columns: Vec<String> = sqlx::query("PRAGMA table_info(message_fts)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        assert_eq!(columns, vec!["text"]);
    }

    #[tokio::test]
    async fn repeated_upsert_keeps_a_single_index_row() {
        // The atomic delete+insert upsert must be idempotent: re-indexing the
        // same message (as the live path and reconcile can both do during
        // warmup) leaves exactly one row, not duplicate retrieval hits.
        let db = seed().await;
        let msg = db
            .add_message(
                "dup-1",
                "c-a",
                &MessageContent::user("kangaroo jurisprudence"),
                None,
                None,
            )
            .await
            .unwrap();

        // add_message already indexed once; upsert the same message twice more.
        fts_upsert(db.pool(), &msg, db.sqlite_workload_collector.clone())
            .await
            .unwrap();
        fts_upsert(db.pool(), &msg, db.sqlite_workload_collector.clone())
            .await
            .unwrap();

        let r = db.fts_retriever();
        let hits = r.retrieve(global_request("kangaroo")).await.unwrap();
        assert_eq!(hits.len(), 1, "repeated upsert must not duplicate the row");
    }

    #[tokio::test]
    async fn final_content_token_matches_by_prefix_when_requested() {
        let db = seed().await;
        db.add_message(
            "prefix-1",
            "c-a",
            &MessageContent::user("searchable kangaroo"),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest {
                query: "searchable kang".to_string(),
                scope: RetrievalScope::Global,
                visibility: RetrievalVisibility::All,
                grouping: RetrievalGrouping::None,
                match_mode: RetrievalMatchMode::FinalTokenPrefix,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "prefix-1");
    }

    #[tokio::test]
    async fn prefix_match_handles_partial_porter_stem() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("c1", "prefix-porter", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "m1",
            "c1",
            &MessageContent::user("Review the running optimization skies conversation"),
            None,
            None,
        )
        .await
        .unwrap();
        let retriever = db.fts_retriever();
        retriever.reconcile().await.unwrap();

        for query in ["runni", "optimizatio", "skie"] {
            let hits = retriever
                .retrieve(RetrievalRequest::palette_conversation_search(query, 10))
                .await
                .unwrap();
            assert_eq!(hits.len(), 1, "partial Porter input {query} should match");
            assert_eq!(hits[0].conversation_id, "c1");
        }
    }

    #[tokio::test]
    async fn literal_partial_term_does_not_disable_prefix_matches_elsewhere() {
        let db = Database::open_in_memory().await.unwrap();
        for (id, slug, content) in [
            ("c-literal", "literal", "runni diagnostics"),
            ("c-complete", "complete", "running diagnostics"),
            ("c-unrelated", "unrelated", "runbook diagnostics"),
        ] {
            db.create_conversation(id, slug, "/tmp", true, None, None)
                .await
                .unwrap();
            db.add_message(
                &format!("m-{id}"),
                id,
                &MessageContent::user(content),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let retriever = db.fts_retriever();
        retriever.reconcile().await.unwrap();

        let hits = retriever
            .retrieve(RetrievalRequest::palette_conversation_search("runni", 10))
            .await
            .unwrap();
        let ids = hits
            .into_iter()
            .map(|hit| hit.conversation_id)
            .collect::<HashSet<_>>();

        assert_eq!(
            ids,
            HashSet::from(["c-literal".to_string(), "c-complete".to_string()])
        );
    }

    #[tokio::test]
    async fn complete_porter_term_does_not_broaden_to_shorter_prefixes() {
        let db = Database::open_in_memory().await.unwrap();
        for (id, slug, content) in [
            ("c-optimization", "optimization", "optimization work"),
            ("c-option", "option", "option work"),
            ("c-optimistic", "optimistic", "optimistic work"),
        ] {
            db.create_conversation(id, slug, "/tmp", true, None, None)
                .await
                .unwrap();
            db.add_message(
                &format!("m-{id}"),
                id,
                &MessageContent::user(content),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let retriever = db.fts_retriever();
        retriever.reconcile().await.unwrap();

        let hits = retriever
            .retrieve(RetrievalRequest::palette_conversation_search(
                "optimization",
                10,
            ))
            .await
            .unwrap();

        assert_eq!(
            hits.into_iter()
                .map(|hit| hit.conversation_id)
                .collect::<Vec<_>>(),
            ["c-optimization"]
        );
    }

    #[tokio::test]
    async fn retrieval_bounds_snippets_with_unbroken_tokens() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("c1", "bounded-snippet", "/tmp", true, None, None)
            .await
            .unwrap();
        let long_token = format!("searchprefix{}", "x".repeat(500));
        db.add_message("m1", "c1", &MessageContent::user(&long_token), None, None)
            .await
            .unwrap();
        let retriever = db.fts_retriever();
        retriever.reconcile().await.unwrap();

        let hits = retriever
            .retrieve(RetrievalRequest::palette_conversation_search(
                "searchprefix",
                10,
            ))
            .await
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet.chars().count(), 240);
        assert!(hits[0].snippet.ends_with('…'));
    }

    #[tokio::test]
    async fn prefix_match_uses_unicode61_case_and_diacritic_rules() {
        let db = Database::open_in_memory().await.unwrap();
        for (id, slug, content) in [
            ("c-case", "case", "ÜBER diagnostics"),
            ("c-diacritic", "diacritic", "école diagnostics"),
        ] {
            db.create_conversation(id, slug, "/tmp", true, None, None)
                .await
                .unwrap();
            db.add_message(
                &format!("m-{id}"),
                id,
                &MessageContent::user(content),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let retriever = db.fts_retriever();
        retriever.reconcile().await.unwrap();

        for (query, expected) in [("übe", "c-case"), ("eco", "c-diacritic")] {
            let hits = retriever
                .retrieve(RetrievalRequest::palette_conversation_search(query, 10))
                .await
                .unwrap();
            assert_eq!(hits.len(), 1, "Unicode prefix {query} should match");
            assert_eq!(hits[0].conversation_id, expected);
        }
    }

    #[tokio::test]
    async fn exact_match_mode_does_not_prefix_match_final_token() {
        let db = seed().await;
        db.add_message(
            "exact-1",
            "c-a",
            &MessageContent::user("searchable kangaroo"),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest::natural_language(
                "kang",
                RetrievalScope::Global,
                10,
            ))
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn best_per_conversation_grouping_filters_before_limit() {
        let db = seed().await;
        db.add_message(
            "active-a",
            "c-a",
            &MessageContent::user("uniquealpha one"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "active-b",
            "c-a",
            &MessageContent::user("uniquealpha two"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "archived-a",
            "c-b",
            &MessageContent::user("uniquealpha archive"),
            None,
            None,
        )
        .await
        .unwrap();
        db.archive_conversation("c-b").await.unwrap();
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest {
                query: "uniquealpha".to_string(),
                scope: RetrievalScope::Global,
                visibility: RetrievalVisibility::UserTopLevel,
                grouping: RetrievalGrouping::BestPerConversation,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 2,
            })
            .await
            .unwrap();
        assert_eq!(
            hits.len(),
            2,
            "active and archived user conversations survive pre-limit grouping"
        );
        let conversations: HashSet<_> = hits.into_iter().map(|h| h.conversation_id).collect();
        assert!(conversations.contains("c-a"));
        assert!(conversations.contains("c-b"));
    }

    #[tokio::test]
    async fn best_per_conversation_grouping_uses_conversation_id_as_final_tiebreaker() {
        let db = seed().await;
        db.create_conversation("c-z", "z", "/tmp/z", true, None, None)
            .await
            .unwrap();
        db.create_conversation("c-y", "y", "/tmp/y", true, None, None)
            .await
            .unwrap();
        for (message_id, conversation_id) in [("m-z", "c-z"), ("m-y", "c-y")] {
            db.add_message(
                message_id,
                conversation_id,
                &MessageContent::user("deterministic tie"),
                None,
                None,
            )
            .await
            .unwrap();
        }
        sqlx::query(
            "UPDATE message_fts_rows SET created_at = '2026-01-01T00:00:00Z' \
             WHERE message_id IN ('m-z', 'm-y')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest {
                query: "deterministic".to_string(),
                scope: RetrievalScope::Global,
                visibility: RetrievalVisibility::UserTopLevel,
                grouping: RetrievalGrouping::BestPerConversation,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 2,
            })
            .await
            .unwrap();

        assert_eq!(
            hits.into_iter()
                .map(|hit| hit.conversation_id)
                .collect::<Vec<_>>(),
            ["c-y", "c-z"]
        );
    }

    #[tokio::test]
    async fn best_per_conversation_grouping_does_not_starve_other_conversations() {
        let db = seed().await;
        db.create_conversation("c-c", "c", "/tmp/c", true, None, None)
            .await
            .unwrap();
        for index in 0..12 {
            db.add_message(
                &format!("m-many-{index}"),
                "c-a",
                &MessageContent::user(format!("starveterm repeated {index}")),
                None,
                None,
            )
            .await
            .unwrap();
        }
        db.add_message(
            "m-other",
            "c-c",
            &MessageContent::user("starveterm survivor"),
            None,
            None,
        )
        .await
        .unwrap();
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest {
                query: "starveterm".to_string(),
                scope: RetrievalScope::Global,
                visibility: RetrievalVisibility::UserTopLevel,
                grouping: RetrievalGrouping::BestPerConversation,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 2,
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        let conversations: std::collections::HashSet<_> =
            hits.into_iter().map(|h| h.conversation_id).collect();
        assert!(conversations.contains("c-a"));
        assert!(conversations.contains("c-c"));
    }

    #[tokio::test]
    async fn user_top_level_excludes_archived_deletion_pending_before_limit() {
        let db = seed().await;
        db.create_conversation("c-c", "c", "/tmp/c", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "m-arch-visible",
            "c-b",
            &MessageContent::user("pendingdelete target"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "m-active-visible",
            "c-c",
            &MessageContent::user("pendingdelete survivor"),
            None,
            None,
        )
        .await
        .unwrap();
        db.archive_conversation("c-b").await.unwrap();
        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversation_creation_jobs)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        if columns.iter().any(|name| name == "status") {
            sqlx::query(
                "INSERT INTO conversation_creation_jobs (
                    id, conversation_id, message_id, status, stage, attempt, generation,
                    intent_json, error, accepted_at, provisioning_started_at, completed_at,
                    failed_at, cancelled_at, deletion_requested_at, created_at, updated_at
                 ) VALUES (
                    'job-delete', 'c-b', NULL, 'deletion_pending', 'finalize', 0, 0,
                    '{\"kind\":\"direct\",\"cwd\":\"/tmp/b\",\"prompt\":\"x\",\"model\":null,\"images\":[],\"files\":[]}',
                    NULL, '2026-01-01T00:00:00Z', NULL, NULL, NULL, NULL,
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
                 )",
            )
            .execute(db.pool())
            .await
            .unwrap();
        } else {
            sqlx::query(
                "INSERT INTO conversation_creation_jobs (
                    id, conversation_id, message_id, phase, intent_json,
                    error, accepted_at, provisioning_started_at, completed_at, failed_at,
                    created_at, updated_at
                 ) VALUES (
                    'job-delete', 'c-b', NULL, 'accepted',
                    '{\"kind\":\"direct\",\"cwd\":\"/tmp/b\",\"prompt\":\"x\",\"model\":null,\"images\":[],\"files\":[]}',
                    'deletion_pending sentinel',
                    '2026-01-01T00:00:00Z', NULL, NULL, NULL,
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
                 )",
            )
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "ALTER TABLE conversation_creation_jobs RENAME TO conversation_creation_jobs_base",
            )
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "CREATE VIEW conversation_creation_jobs AS
                 SELECT id, conversation_id,
                        CASE
                            WHEN error = 'deletion_pending sentinel' THEN 'deletion_pending'
                            ELSE phase
                        END AS status
                 FROM conversation_creation_jobs_base",
            )
            .execute(db.pool())
            .await
            .unwrap();
        }
        let r = db.fts_retriever();

        let hits = r
            .retrieve(RetrievalRequest {
                query: "pendingdelete".to_string(),
                scope: RetrievalScope::Global,
                visibility: RetrievalVisibility::UserTopLevel,
                grouping: RetrievalGrouping::BestPerConversation,
                match_mode: RetrievalMatchMode::ExactTerms,
                limit: 2,
            })
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation_id, "c-c");
    }

    #[tokio::test]
    async fn is_fresh_for_detects_missing_and_stale_rows() {
        let db = seed().await;
        db.add_message("m1", "c-a", &MessageContent::user("indexed"), None, None)
            .await
            .unwrap();
        let r = db.fts_retriever();
        assert!(
            r.is_fresh_for(&["c-a".into()]).await.unwrap(),
            "all messages indexed and fresh",
        );

        // Stale row: index text no longer matches the source (simulate a
        // swallowed best-effort reindex by corrupting the stored content_hash).
        sqlx::query("UPDATE message_fts_rows SET content_hash = 'stale' WHERE message_id = 'm1'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            !r.is_fresh_for(&["c-a".into()]).await.unwrap(),
            "a stale-content index row must report as not fresh",
        );

        // Missing row: a message with no index row at all.
        let rowids: Vec<i64> =
            sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE message_id = 'm1'")
                .fetch_all(db.pool())
                .await
                .unwrap();
        for rowid in rowids {
            sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                .bind(rowid)
                .execute(db.pool())
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM message_fts_rows WHERE message_id = 'm1'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(
            !r.is_fresh_for(&["c-a".into()]).await.unwrap(),
            "a missing index row must report as not fresh",
        );

        // A conversation set with no messages is trivially fresh.
        assert!(r.is_fresh_for(&[]).await.unwrap());
    }

    #[tokio::test]
    async fn is_fresh_for_rejects_missing_physical_rows() {
        let db = seed().await;
        db.add_message("m1", "c-a", &MessageContent::user("indexed"), None, None)
            .await
            .unwrap();
        let r = db.fts_retriever();
        let rowid: i64 =
            sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE message_id = 'm1'")
                .fetch_one(db.pool())
                .await
                .unwrap();

        sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
            .bind(rowid)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(!r.is_fresh_for(&["c-a".into()]).await.unwrap());
    }

    #[tokio::test]
    async fn is_fresh_for_rejects_orphaned_index_rows() {
        let db = seed().await;
        db.add_message("m1", "c-a", &MessageContent::user("indexed"), None, None)
            .await
            .unwrap();
        let r = db.fts_retriever();
        assert!(r.is_fresh_for(&["c-a".into()]).await.unwrap());

        // Orphan: an index row in this conversation with no live source message
        // (e.g. a failed delete hook). Every current message is still present
        // and fresh, but the orphan could surface deleted content in search, so
        // freshness must report false until the row is pruned.
        insert_index_row(db.pool(), "ghost", "orphan-1", 0, "c-a", "h").await;
        assert!(
            !r.is_fresh_for(&["c-a".into()]).await.unwrap(),
            "an orphaned index row must report as not fresh",
        );
    }

    #[tokio::test]
    async fn is_fresh_for_rejects_duplicate_rows_for_one_message() {
        let db = seed().await;
        db.add_message("m1", "c-a", &MessageContent::user("indexed"), None, None)
            .await
            .unwrap();
        let r = db.fts_retriever();
        assert!(r.is_fresh_for(&["c-a".into()]).await.unwrap());

        // A second physical row for the same live message_id carrying the same
        // (fresh) hash — e.g. a stale+fresh pair left by interleaved index
        // maintenance. The per-id loop is satisfied, but the duplicate row's
        // text is still searchable, so freshness must report false. A HashMap
        // would collapse the two rows and miss this; the physical-row count
        // catches it.
        let fresh_hash: String =
            sqlx::query_scalar("SELECT content_hash FROM message_fts_rows WHERE message_id = 'm1'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        insert_index_row(db.pool(), "dup", "m1", 1, "c-a", &fresh_hash).await;
        assert!(
            !r.is_fresh_for(&["c-a".into()]).await.unwrap(),
            "a duplicate physical row must report as not fresh",
        );
    }
}
