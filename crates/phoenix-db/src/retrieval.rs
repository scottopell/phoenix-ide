//! Scope-filtered message retrieval over an FTS5 index
//! (`specs/conversation-retrieval/`).
//!
//! One index (`message_fts`) over every conversation's messages; callers
//! differ only in the [`RetrievalScope`] they pass. The ranking backend sits
//! behind the [`MessageRetriever`] trait so a vector/hybrid backend can be
//! substituted without touching callers (REQ-RET-005). The index is a
//! rebuildable derived cache over `messages` (REQ-RET-003): kept current by
//! the persist/mutate/delete hooks the `Database` calls, and reconciled at
//! startup by [`Fts5Retriever::reconcile`].

// Type names intentionally share the module's "retrieval" stem — they are the
// vocabulary the spec defines (RetrievalScope, RetrievedChunk, …).
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{Message, MessageType};
use phoenix_core::domain::message_text::index_text;
use sqlx::{Row, SqlitePool};
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
    /// Return up to `top_k` message chunks within `scope`, ranked by relevance
    /// to `query`. A natural-language `query` is accepted directly; the
    /// implementation builds the backend query (REQ-RET-001).
    ///
    /// # Errors
    /// Returns [`RetrievalError`] if the backing query fails.
    async fn retrieve(
        &self,
        query: &str,
        scope: RetrievalScope,
        top_k: usize,
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

/// Lexical (FTS5/BM25) retrieval backend.
pub struct Fts5Retriever {
    pool: SqlitePool,
    reconciled: Arc<AtomicBool>,
}

impl Fts5Retriever {
    /// Build a retriever over the given pool. Call [`Self::reconcile`] once at
    /// startup to bring the index in line with `messages`.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            reconciled: Arc::new(AtomicBool::new(false)),
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

        // Physical index rows. We track a per-id row count alongside a sample
        // hash so a *duplicate* physical row for one message_id (which a plain
        // map would collapse) is detected and repaired — otherwise
        // `is_fresh_for` would reject the scope forever and chain Q&A would keep
        // disabling search after every restart.
        let existing_rows: Vec<(String, String)> =
            sqlx::query("SELECT message_id, content_hash FROM message_fts_rows")
                .try_map(|row: sqlx::sqlite::SqliteRow| {
                    Ok((
                        row.try_get::<String, _>("message_id")?,
                        row.try_get::<String, _>("content_hash")?,
                    ))
                })
                .fetch_all(&self.pool)
                .await?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut existing: HashMap<String, String> = HashMap::new();
        for (id, hash) in existing_rows {
            *counts.entry(id.clone()).or_default() += 1;
            existing.insert(id, hash);
        }

        let mut messages = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at FROM messages",
        )
        .try_map(crate::parse_message_row)
        .fetch_all(&self.pool)
        .await?;
        // Attachments live in child tables, not the content blob; hydrate so the
        // indexed text includes user/skill file-context tags (`index_text`).
        crate::hydrate_attachments(&self.pool, &mut messages).await?;

        let mut stats = ReconcileStats::default();
        for m in &messages {
            let fingerprint = content_fingerprint(&index_text(m));
            let count = counts.get(&m.message_id).copied().unwrap_or(0);
            if count == 0 {
                // Absent: insert (guarded so a concurrent add wins).
                if fts_reconcile_upsert(&self.pool, m, None).await? {
                    stats.indexed += 1;
                }
            } else if count == 1 {
                // Single row: re-index only if stale, compare-and-set so a
                // concurrent edit's fresh content is never clobbered.
                let prev = existing[&m.message_id].as_str();
                if prev != fingerprint.as_str()
                    && fts_reconcile_upsert(&self.pool, m, Some(prev)).await?
                {
                    stats.reindexed += 1;
                }
            } else {
                // Duplicate physical rows for one live id: force-collapse to a
                // single fresh row (unconditional delete-all + insert).
                fts_upsert(&self.pool, m).await?;
                stats.reindexed += 1;
            }
        }

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
        stats.pruned = orphan_rowids.len();

        self.reconciled.store(true, Ordering::Release);
        Ok(stats)
    }
}

#[async_trait]
impl MessageRetriever for Fts5Retriever {
    fn index_reconciled(&self) -> bool {
        self.index_reconciled()
    }

    async fn retrieve(
        &self,
        query: &str,
        scope: RetrievalScope,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError> {
        let Some(match_expr) = build_fts_query(query) else {
            return Ok(Vec::new());
        };
        let (scope_ids, excluding): (&[String], bool) = match &scope {
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
            "SELECT meta.message_id, meta.chunk_ordinal, meta.conversation_id, \
             meta.message_type, meta.created_at, \
             snippet(message_fts, 0, '', '', '…', 24) AS snippet, bm25(message_fts) AS score \
             FROM message_fts \
             JOIN message_fts_rows meta ON meta.fts_rowid = message_fts.rowid \
             JOIN messages source ON source.message_id = meta.message_id \
             WHERE message_fts MATCH ? \
               AND COALESCE(json_extract(source.display_data, '$.hidden'), 0) != 1",
        );
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
        sql.push_str(" ORDER BY score LIMIT ?");

        // `sql` interpolates only a placeholder count (one `?` per scoped id);
        // every value — the MATCH expression, the ids, the limit — is bound, so
        // there is no injection surface.
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(match_expr);
        for id in scope_ids {
            q = q.bind(id);
        }
        let limit = i64::try_from(top_k).unwrap_or(i64::MAX);
        q = q.bind(limit);

        let rows = q.try_map(parse_chunk_row).fetch_all(&self.pool).await?;
        Ok(rows)
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

/// (Re)index a single message: replace any existing index row(s) for its id
/// with a fresh extraction. Idempotent for a given message content.
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete or insert fails.
pub async fn fts_upsert(pool: &SqlitePool, message: &Message) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    fts_upsert_conn(&mut tx, message).await?;
    tx.commit().await?;
    Ok(())
}

/// Replace a message's index row(s) using a caller-provided connection or
/// transaction, so the index write commits **atomically with the source
/// write** (REQ-RET-003). The delete+insert pair is two statements; running
/// them on one connection keeps them in the caller's transaction, so a
/// concurrent upsert of the same `message_id` cannot interleave into duplicate
/// rows (`SQLite` serializes write transactions).
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete or insert fails.
pub async fn fts_upsert_conn(
    conn: &mut sqlx::SqliteConnection,
    message: &Message,
) -> Result<(), sqlx::Error> {
    let text = index_text(message);
    let fingerprint = content_fingerprint(&text);
    delete_message_rows(conn, &message.message_id).await?;
    let inserted = sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
        .bind(text)
        .execute(&mut *conn)
        .await?;
    record_fts_row(conn, inserted.last_insert_rowid(), message, &fingerprint).await?;
    Ok(())
}

async fn delete_message_rows(
    conn: &mut sqlx::SqliteConnection,
    message_id: &str,
) -> Result<(), sqlx::Error> {
    let rowids: Vec<i64> =
        sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE message_id = ?1")
            .bind(message_id)
            .fetch_all(&mut *conn)
            .await?;
    for rowid in rowids {
        sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
            .bind(rowid)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM message_fts_rows WHERE message_id = ?1")
        .bind(message_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn record_fts_row(
    conn: &mut sqlx::SqliteConnection,
    fts_rowid: i64,
    message: &Message,
    fingerprint: &str,
) -> Result<(), sqlx::Error> {
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
    .execute(&mut *conn)
    .await?;
    Ok(())
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
pub async fn fts_reconcile_upsert(
    pool: &SqlitePool,
    message: &Message,
    observed: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let text = index_text(message);
    let fingerprint = content_fingerprint(&text);
    let mut tx = pool.begin().await?;
    if let Some(prev) = observed {
        // Replace a stale row only while it still carries the observed hash.
        let candidates: Vec<(i64, bool)> = sqlx::query(
            "SELECT r.fts_rowid, f.rowid IS NOT NULL AS physical_match
             FROM message_fts_rows r
             LEFT JOIN message_fts f ON f.rowid = r.fts_rowid
             WHERE r.message_id = ?1 AND r.content_hash = ?2",
        )
        .bind(&message.message_id)
        .bind(prev)
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            Ok((
                row.try_get::<i64, _>("fts_rowid")?,
                row.try_get::<bool, _>("physical_match")?,
            ))
        })
        .fetch_all(&mut *tx)
        .await?;
        if candidates.is_empty() || candidates.iter().any(|(_, matches)| !matches) {
            tx.rollback().await?;
            return Ok(false);
        }
        for (rowid, _) in &candidates {
            let deleted = sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
                .bind(rowid)
                .execute(&mut *tx)
                .await?;
            if deleted.rows_affected() != 1 {
                tx.rollback().await?;
                return Ok(false);
            }
            sqlx::query("DELETE FROM message_fts_rows WHERE fts_rowid = ?1")
                .bind(rowid)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        // Insert an absent row only while it is still absent.
        let existing: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE message_id = ?1")
                .bind(&message.message_id)
                .fetch_one(&mut *tx)
                .await?;
        if existing > 0 {
            tx.rollback().await?;
            return Ok(false);
        }
    }
    let inserted = sqlx::query("INSERT INTO message_fts (text) VALUES (?1)")
        .bind(text)
        .execute(&mut *tx)
        .await?;
    record_fts_row(&mut tx, inserted.last_insert_rowid(), message, &fingerprint).await?;
    tx.commit().await?;
    Ok(true)
}

/// Remove all index rows for one message id.
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete fails.
pub async fn fts_delete_message(pool: &SqlitePool, message_id: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    delete_message_rows(&mut tx, message_id).await?;
    tx.commit().await?;
    Ok(())
}

/// Remove all index rows for a conversation (used on hard delete, since the
/// standalone FTS table has no FK cascade — REQ-RET-003).
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete fails.
pub async fn fts_delete_conversation(
    pool: &SqlitePool,
    conversation_id: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    fts_delete_conversation_conn(&mut tx, conversation_id).await?;
    tx.commit().await?;
    Ok(())
}

/// Remove all index rows for a conversation using a caller-provided
/// connection/transaction, so the index prune commits atomically with the
/// conversation/message delete — deleted content cannot resurface in recall
/// even if the process crashes between the source delete and the prune
/// (REQ-RET-003).
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete fails.
pub async fn fts_delete_conversation_conn(
    conn: &mut sqlx::SqliteConnection,
    conversation_id: &str,
) -> Result<(), sqlx::Error> {
    let rowids: Vec<i64> =
        sqlx::query_scalar("SELECT fts_rowid FROM message_fts_rows WHERE conversation_id = ?1")
            .bind(conversation_id)
            .fetch_all(&mut *conn)
            .await?;
    for rowid in rowids {
        sqlx::query("DELETE FROM message_fts WHERE rowid = ?1")
            .bind(rowid)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM message_fts_rows WHERE conversation_id = ?1")
        .bind(conversation_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

// ---- helpers ----

#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_chunk_row(row: sqlx::sqlite::SqliteRow) -> Result<RetrievedChunk, sqlx::Error> {
    let ordinal: i64 = row.try_get("chunk_ordinal")?;
    Ok(RetrievedChunk {
        conversation_id: row.try_get("conversation_id")?,
        message_id: row.try_get("message_id")?,
        chunk: ChunkRef {
            ordinal: u32::try_from(ordinal).unwrap_or(0),
            char_range: None,
        },
        message_type: crate::parse_message_type(&row.try_get::<String, _>("message_type")?),
        created_at: crate::parse_datetime(&row.try_get::<String, _>("created_at")?),
        snippet: row.try_get("snippet")?,
        score: row.try_get("score")?,
    })
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
/// with `OR` (each quoted as a literal). Returns `None` when nothing
/// content-bearing remains (the caller returns an empty result).
fn build_fts_query(natural: &str) -> Option<String> {
    let terms: Vec<String> = natural
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
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

    #[test]
    fn build_query_drops_filler_and_ors_terms() {
        let q = build_fts_query("what did we decide about the auth schema").unwrap();
        assert!(q.contains("\"auth\""));
        assert!(q.contains("\"schema\""));
        assert!(q.contains("\"decide\""));
        assert!(q.contains(" OR "));
        assert!(!q.contains("\"what\""));
        assert!(!q.contains("\"the\""));
    }

    #[test]
    fn build_query_empty_when_only_stopwords() {
        assert!(build_fts_query("what did we do").is_none());
        assert!(build_fts_query("   ").is_none());
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

        let r = Fts5Retriever::new(db.pool().clone());

        // Global finds the rate-limiter message.
        let hits = r
            .retrieve("how does the rate limiter work", RetrievalScope::Global, 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_id, "m1");
        assert_eq!(hits[0].conversation_id, "c-a");

        // Scoped to c-b only: the rate-limiter query returns nothing in-scope.
        let scoped = r
            .retrieve(
                "rate limiter",
                RetrievalScope::Conversations(vec!["c-b".into()]),
                10,
            )
            .await
            .unwrap();
        assert!(scoped.is_empty());

        // Scoped to c-b: its own content is found.
        let scoped = r
            .retrieve(
                "auth schema",
                RetrievalScope::Conversations(vec!["c-b".into()]),
                10,
            )
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

        let retriever = Fts5Retriever::new(db.pool().clone());
        assert_eq!(
            retriever
                .retrieve("confidential recovery", RetrievalScope::Global, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        db.update_message_display_data("hidden-1", &serde_json::json!({ "hidden": true }))
            .await
            .unwrap();

        assert!(retriever
            .retrieve("confidential recovery", RetrievalScope::Global, 10)
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
        let r = Fts5Retriever::new(db.pool().clone());
        assert_eq!(
            r.retrieve("alpha", RetrievalScope::Global, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        db.update_tool_message_content("t1", "compiling crate omega")
            .await
            .unwrap();
        // Old term gone, new term present.
        assert!(r
            .retrieve("alpha", RetrievalScope::Global, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            r.retrieve("omega", RetrievalScope::Global, 10)
                .await
                .unwrap()
                .len(),
            1
        );
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
        let r = Fts5Retriever::new(db.pool().clone());
        assert_eq!(
            r.retrieve("zebra", RetrievalScope::Global, 10)
                .await
                .unwrap()
                .len(),
            1
        );

        db.delete_conversation("c-a").await.unwrap();
        assert!(r
            .retrieve("zebra", RetrievalScope::Global, 10)
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
        let r = Fts5Retriever::new(db.pool().clone());

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
            r.retrieve("marmalade", RetrievalScope::Global, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(r
            .retrieve("ghost", RetrievalScope::Global, 10)
            .await
            .unwrap()
            .is_empty());
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

        let r = Fts5Retriever::new(db.pool().clone());
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

        let current_text = "current concurrent row";
        let current_hash = content_fingerprint(current_text);
        let current_rowid =
            insert_index_row(db.pool(), current_text, "m-cas", 0, "c-a", &current_hash).await;

        let snapshot = Message {
            content: MessageContent::user("reconciled snapshot"),
            ..stale
        };
        assert!(
            fts_reconcile_upsert(db.pool(), &snapshot, Some(&stale_hash))
                .await
                .unwrap()
        );

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
        fts_upsert(db.pool(), &msg).await.unwrap();
        fts_upsert(db.pool(), &msg).await.unwrap();

        let r = Fts5Retriever::new(db.pool().clone());
        let hits = r
            .retrieve("kangaroo", RetrievalScope::Global, 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "repeated upsert must not duplicate the row");
    }

    #[tokio::test]
    async fn is_fresh_for_detects_missing_and_stale_rows() {
        let db = seed().await;
        db.add_message("m1", "c-a", &MessageContent::user("indexed"), None, None)
            .await
            .unwrap();
        let r = Fts5Retriever::new(db.pool().clone());
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
        let r = Fts5Retriever::new(db.pool().clone());
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
        let r = Fts5Retriever::new(db.pool().clone());
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
        let r = Fts5Retriever::new(db.pool().clone());
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
