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
        let mut existing: HashMap<String, String> = sqlx::query(
            "SELECT message_id, content_hash FROM message_fts",
        )
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            Ok((
                row.try_get::<String, _>("message_id")?,
                row.try_get::<String, _>("content_hash")?,
            ))
        })
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();

        let messages = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at FROM messages",
        )
        .try_map(crate::parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        let mut stats = ReconcileStats::default();
        for m in &messages {
            let fingerprint = content_fingerprint(&index_text(m));
            match existing.remove(&m.message_id) {
                Some(prev) if prev == fingerprint => {}
                Some(_) => {
                    fts_upsert(&self.pool, m).await?;
                    stats.reindexed += 1;
                }
                None => {
                    fts_upsert(&self.pool, m).await?;
                    stats.indexed += 1;
                }
            }
        }

        // Whatever remains in `existing` has no live source message — prune.
        for orphan_id in existing.keys() {
            fts_delete_message(&self.pool, orphan_id).await?;
        }
        stats.pruned = existing.len();

        self.reconciled.store(true, Ordering::Release);
        Ok(stats)
    }
}

#[async_trait]
impl MessageRetriever for Fts5Retriever {
    async fn retrieve(
        &self,
        query: &str,
        scope: RetrievalScope,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError> {
        let Some(match_expr) = build_fts_query(query) else {
            return Ok(Vec::new());
        };
        let scope_ids: &[String] = match &scope {
            RetrievalScope::Global => &[],
            RetrievalScope::Conversations(ids) => {
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                ids
            }
        };

        let mut sql = String::from(
            "SELECT message_id, chunk_ordinal, conversation_id, message_type, created_at, \
             snippet(message_fts, 0, '', '', '…', 24) AS snippet, bm25(message_fts) AS score \
             FROM message_fts WHERE message_fts MATCH ?",
        );
        if !scope_ids.is_empty() {
            sql.push_str(" AND conversation_id IN (");
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
}

// ---- index maintenance (called by `Database` write paths) ----

/// (Re)index a single message: replace any existing index row(s) for its id
/// with a fresh extraction. Idempotent for a given message content.
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete or insert fails.
pub async fn fts_upsert(pool: &SqlitePool, message: &Message) -> Result<(), sqlx::Error> {
    let text = index_text(message);
    let fingerprint = content_fingerprint(&text);
    fts_delete_message(pool, &message.message_id).await?;
    sqlx::query(
        "INSERT INTO message_fts (text, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash) \
         VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)",
    )
    .bind(text)
    .bind(&message.message_id)
    .bind(&message.conversation_id)
    .bind(message.message_type.to_string())
    .bind(message.created_at.to_rfc3339())
    .bind(fingerprint)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove all index rows for one message id.
///
/// # Errors
/// Returns the underlying [`sqlx::Error`] if the delete fails.
pub async fn fts_delete_message(pool: &SqlitePool, message_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM message_fts WHERE message_id = ?1")
        .bind(message_id)
        .execute(pool)
        .await?;
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
    sqlx::query("DELETE FROM message_fts WHERE conversation_id = ?1")
        .bind(conversation_id)
        .execute(pool)
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
        db.add_message("m1", "c-a", &MessageContent::user("zebra crossing"), None, None)
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
        db.add_message("m1", "c-a", &MessageContent::user("orange marmalade"), None, None)
            .await
            .unwrap();
        let r = Fts5Retriever::new(db.pool().clone());

        // Simulate a stale index: wipe it, plus inject an orphan row.
        sqlx::query("DELETE FROM message_fts")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO message_fts (text, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash) \
             VALUES ('ghost', 'gone', 0, 'c-a', 'user', '2026-01-01T00:00:00+00:00', 'x')",
        )
        .execute(db.pool())
        .await
        .unwrap();

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
}
