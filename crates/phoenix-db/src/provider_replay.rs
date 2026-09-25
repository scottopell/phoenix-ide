//! Persistence for `active_provider_replay_state`.
//!
//! One row per conversation. The entire [`AnthropicReplayPayload`] is written
//! and read as an opaque JSON blob; individual fields are never addressed by
//! SQL. The `provider`, `model`, and `response_id` columns are identity columns
//! for the FK constraint check and the checked provider discriminator; they are
//! derived from the last response set in the payload on write.

use crate::{Database, DbError, DbResult};
use phoenix_core::domain::provider_replay::AnthropicReplayPayload;
use sqlx::Row;

impl Database {
    /// Store (upsert) the replay payload for `conversation_id`.
    ///
    /// Extracts `provider`, `model`, and `response_id` from the last response
    /// set in `payload` for the checked SQL columns. An empty `response_sets`
    /// list is rejected because the SQL `CHECK` constraints on `model` and
    /// `response_id` forbid empty strings, and there is nothing meaningful to
    /// store.
    ///
    /// The payload is serialized whole; no field-wise SQL path is used.
    ///
    /// # Errors
    ///
    /// - [`DbError::Serialization`] if `payload.response_sets` is empty or
    ///   serialization fails.
    /// - [`DbError::Sqlx`] for database I/O errors.
    #[cfg(test)]
    pub async fn store_provider_replay_state(
        &self,
        conversation_id: &str,
        payload: &AnthropicReplayPayload,
    ) -> DbResult<()> {
        let last = payload.response_sets.last().ok_or_else(|| {
            DbError::Serialization(
                "cannot store provider replay state with no response sets".into(),
            )
        })?;
        let model = last.identity.model.clone();
        let response_id = last.identity.response_id.clone();
        let payload_json =
            serde_json::to_string(payload).map_err(|e| DbError::Serialization(e.to_string()))?;
        sqlx::query(
            "INSERT INTO active_provider_replay_state
                 (conversation_id, provider, model, response_id, payload)
             VALUES (?1, 'anthropic', ?2, ?3, ?4)
             ON CONFLICT(conversation_id) DO UPDATE SET
                 provider    = 'anthropic',
                 model       = excluded.model,
                 response_id = excluded.response_id,
                 payload     = excluded.payload",
        )
        .bind(conversation_id)
        .bind(model)
        .bind(response_id)
        .bind(payload_json)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Atomically persist conversation state and one provider replay mutation.
    ///
    /// # Errors
    /// Returns [`DbError`] when state serialization, replay decoding/encoding,
    /// conversation lookup, or the `SQLite` transaction fails.
    pub async fn update_state_and_provider_replay(
        &self,
        conversation_id: &str,
        state: &phoenix_core::domain::sm_state::ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
        update: &phoenix_core::domain::provider_replay::AnthropicReplayUpdate,
    ) -> DbResult<()> {
        use phoenix_core::domain::provider_replay::AnthropicReplayUpdate;
        let mut tx = self.pool().begin().await?;
        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let result = sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4 WHERE id = ?5",
        )
        .bind(state_json)
        .bind(crate::conv_state_kind(state))
        .bind(state_updated_at.to_rfc3339())
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(conversation_id.to_string()));
        }
        match update {
            AnthropicReplayUpdate::Append(response) => {
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT payload FROM active_provider_replay_state WHERE conversation_id = ?1",
                )
                .bind(conversation_id)
                .fetch_optional(&mut *tx)
                .await?;
                let mut sets = existing
                    .map(|json| serde_json::from_str::<AnthropicReplayPayload>(&json))
                    .transpose()
                    .map_err(|error| {
                        DbError::Serialization(format!("provider replay decode: {error}"))
                    })?
                    .map(|decoded| AnthropicReplayPayload::new(decoded.response_sets))
                    .transpose()
                    .map_err(|error| {
                        DbError::Serialization(format!("provider replay validation: {error}"))
                    })?
                    .map_or_else(Vec::new, |payload| payload.response_sets);
                if !sets
                    .iter()
                    .any(|set| set.identity.response_id == response.identity.response_id)
                {
                    sets.push(response.clone());
                }
                let payload = AnthropicReplayPayload::new(sets)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                let json = serde_json::to_string(&payload)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO active_provider_replay_state (conversation_id, provider, model, response_id, payload) VALUES (?1, 'anthropic', ?2, ?3, ?4) ON CONFLICT(conversation_id) DO UPDATE SET model=excluded.model, response_id=excluded.response_id, payload=excluded.payload",
                )
                .bind(conversation_id)
                .bind(&response.identity.model)
                .bind(&response.identity.response_id)
                .bind(json)
                .execute(&mut *tx)
                .await?;
            }
            AnthropicReplayUpdate::Clear => {
                sqlx::query("DELETE FROM active_provider_replay_state WHERE conversation_id = ?1")
                    .bind(conversation_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Atomically persist a tool checkpoint, selected state, and replay append.
    ///
    /// # Errors
    /// Returns [`DbError`] when message insertion, state/replay serialization,
    /// conversation lookup, or the `SQLite` transaction fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn persist_tool_round_state_and_provider_replay(
        &self,
        conversation_id: &str,
        assistant: &phoenix_core::domain::db_schema::Message,
        tool_results: &[phoenix_core::domain::db_schema::Message],
        state: &phoenix_core::domain::sm_state::ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
        update: &phoenix_core::domain::provider_replay::AnthropicReplayUpdate,
    ) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;
        crate::insert_message_tx(&mut tx, assistant).await?;
        for message in tool_results {
            crate::insert_message_tx(&mut tx, message).await?;
        }
        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query("UPDATE conversations SET state=?1, state_kind=?2, state_updated_at=?3, updated_at=?4 WHERE id=?5")
            .bind(state_json)
            .bind(crate::conv_state_kind(state))
            .bind(state_updated_at.to_rfc3339())
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        match update {
            phoenix_core::domain::provider_replay::AnthropicReplayUpdate::Append(response) => {
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT payload FROM active_provider_replay_state WHERE conversation_id=?1",
                )
                .bind(conversation_id)
                .fetch_optional(&mut *tx)
                .await?;
                let mut sets = existing
                    .map(|json| serde_json::from_str::<AnthropicReplayPayload>(&json))
                    .transpose()
                    .map_err(|error| {
                        DbError::Serialization(format!("provider replay decode: {error}"))
                    })?
                    .map(|decoded| AnthropicReplayPayload::new(decoded.response_sets))
                    .transpose()
                    .map_err(|error| {
                        DbError::Serialization(format!("provider replay validation: {error}"))
                    })?
                    .map_or_else(Vec::new, |payload| payload.response_sets);
                if !sets
                    .iter()
                    .any(|set| set.identity.response_id == response.identity.response_id)
                {
                    sets.push(response.clone());
                }
                let payload = AnthropicReplayPayload::new(sets)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                let json = serde_json::to_string(&payload)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                sqlx::query("INSERT INTO active_provider_replay_state (conversation_id, provider, model, response_id, payload) VALUES (?1,'anthropic',?2,?3,?4) ON CONFLICT(conversation_id) DO UPDATE SET model=excluded.model,response_id=excluded.response_id,payload=excluded.payload")
                    .bind(conversation_id).bind(&response.identity.model).bind(&response.identity.response_id).bind(json)
                    .execute(&mut *tx).await?;
            }
            phoenix_core::domain::provider_replay::AnthropicReplayUpdate::Clear => {
                sqlx::query("DELETE FROM active_provider_replay_state WHERE conversation_id=?1")
                    .bind(conversation_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Atomically persist one public message and clear private replay state.
    ///
    /// # Errors
    /// Returns [`DbError`] when message/state serialization, attachment writes,
    /// conversation lookup, or the `SQLite` transaction fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_message_and_clear_provider_replay(
        &self,
        message_id: &str,
        conversation_id: &str,
        sequence_id: i64,
        content: &phoenix_core::domain::db_schema::MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&phoenix_core::domain::db_schema::UsageData>,
        state: &phoenix_core::domain::sm_state::ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> DbResult<phoenix_core::domain::db_schema::Message> {
        let mut tx = self.pool().begin().await?;
        let now = chrono::Utc::now();
        let content_json = content.to_stored_json();
        let display_json = display_data
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let usage_json = usage_data
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(sequence_id)
        .bind(content.message_type().to_string())
        .bind(serde_json::to_string(&content_json).map_err(|error| DbError::Serialization(error.to_string()))?)
        .bind(display_json)
        .bind(usage_json)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        crate::message_attachments::insert(&mut tx, message_id, content).await?;
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query("UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3 WHERE id = ?4")
            .bind(state_json)
            .bind(crate::conv_state_kind(state))
            .bind(state_updated_at.to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM active_provider_replay_state WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let message = phoenix_core::domain::db_schema::Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: content.message_type(),
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        };
        if let Err(error) = crate::retrieval::fts_upsert(
            self.pool(),
            &message,
            self.sqlite_workload_collector.clone(),
        )
        .await
        {
            tracing::warn!(message_id = %message.message_id, %error, "failed to index message for retrieval; startup reconcile will repair");
        }
        Ok(message)
    }

    /// Load the replay payload for `conversation_id`, if one is stored.
    ///
    /// The payload blob is decoded from JSON. A malformed blob returns
    /// [`DbError::Serialization`] rather than `None` — deserialization failure
    /// is an explicit error, not a missing row.
    ///
    /// # Errors
    ///
    /// - [`DbError::Serialization`] if the stored JSON cannot be decoded into
    ///   [`AnthropicReplayPayload`].
    /// - [`DbError::Sqlx`] for database I/O errors.
    pub async fn load_provider_replay_state(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<AnthropicReplayPayload>> {
        let row = sqlx::query(
            "SELECT payload FROM active_provider_replay_state WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(self.pool())
        .await?;

        match row {
            None => Ok(None),
            Some(row) => {
                let blob: String = row.try_get("payload")?;
                let decoded: AnthropicReplayPayload = serde_json::from_str(&blob)
                    .map_err(|e| DbError::Serialization(format!("provider replay decode: {e}")))?;
                let payload =
                    AnthropicReplayPayload::new(decoded.response_sets).map_err(|error| {
                        DbError::Serialization(format!("provider replay validation: {error}"))
                    })?;
                Ok(Some(payload))
            }
        }
    }

    /// Delete the replay payload for `conversation_id`, if one exists.
    ///
    /// Idempotent: clearing a row that does not exist is not an error.
    ///
    /// # Errors
    ///
    /// - [`DbError::Sqlx`] for database I/O errors.
    #[cfg(test)]
    pub async fn clear_provider_replay_state(&self, conversation_id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM active_provider_replay_state WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::provider_replay::{
        AnthropicPrivateBlock, AnthropicResponseIdentity, AnthropicResponseSet, ContentIndex,
    };

    fn identity(id: &str, model: &str) -> AnthropicResponseIdentity {
        AnthropicResponseIdentity {
            response_id: id.into(),
            model: model.into(),
        }
    }

    fn thinking(index: usize, sig: &str) -> AnthropicPrivateBlock {
        AnthropicPrivateBlock::Thinking {
            index: ContentIndex(index),
            thinking: format!("thought-{index}"),
            signature: sig.into(),
        }
    }

    fn sample_payload() -> AnthropicReplayPayload {
        AnthropicReplayPayload::new(vec![
            AnthropicResponseSet::new(
                identity("msg-aaa", "claude-opus-4-5"),
                vec![thinking(0, "sig-aaa-0")],
            )
            .unwrap(),
            AnthropicResponseSet::new(
                identity("msg-bbb", "claude-opus-4-5"),
                vec![
                    thinking(0, "sig-bbb-0"),
                    AnthropicPrivateBlock::RedactedThinking {
                        index: ContentIndex(2),
                        data: "redacted-blob".into(),
                    },
                ],
            )
            .unwrap(),
        ])
        .unwrap()
    }

    // ─── schema: table exists in a fresh in-memory DB ───────────────────────────

    #[tokio::test]
    async fn table_exists_in_fresh_db() {
        let db = Database::open_in_memory().await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='active_provider_replay_state'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1, "table active_provider_replay_state must exist");
    }

    #[tokio::test]
    async fn table_schema_has_expected_columns() {
        let db = Database::open_in_memory().await.unwrap();
        let names: Vec<String> = sqlx::query("PRAGMA table_info(active_provider_replay_state)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get::<String, _>("name"))
            .collect();
        for col in [
            "conversation_id",
            "provider",
            "model",
            "response_id",
            "payload",
        ] {
            assert!(
                names.contains(&col.to_string()),
                "missing column {col} in active_provider_replay_state"
            );
        }
    }

    // ─── migration 105 creates the table on a db that starts without it ─────────

    #[tokio::test]
    async fn migration_105_creates_table() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use std::str::FromStr;

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str("sqlite::memory:")
                    .unwrap()
                    .journal_mode(SqliteJournalMode::Wal)
                    .foreign_keys(true),
            )
            .await
            .unwrap();

        sqlx::raw_sql(
            "CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 state TEXT NOT NULL,
                 cwd TEXT NOT NULL
             );",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(crate::migrations::MIGRATION_105_FOR_TEST)
            .execute(&pool)
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='active_provider_replay_state'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn state_and_replay_append_commit_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "conv-atomic-open",
            "conv-atomic-open",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let response = sample_payload().response_sets.into_iter().next().unwrap();
        let state = phoenix_core::domain::sm_state::ConvState::LlmRequesting { attempt: 2 };
        db.update_state_and_provider_replay(
            "conv-atomic-open",
            &state,
            chrono::Utc::now(),
            &phoenix_core::domain::provider_replay::AnthropicReplayUpdate::Append(response),
        )
        .await
        .unwrap();
        assert!(db
            .load_provider_replay_state("conv-atomic-open")
            .await
            .unwrap()
            .is_some());
        let persisted: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id='conv-atomic-open'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<phoenix_core::domain::sm_state::ConvState>(&persisted).unwrap(),
            state
        );
    }

    #[tokio::test]
    async fn terminal_message_state_and_replay_clear_commit_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "conv-atomic-close",
            "conv-atomic-close",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        db.store_provider_replay_state("conv-atomic-close", &sample_payload())
            .await
            .unwrap();
        let state = phoenix_core::domain::sm_state::ConvState::Idle;
        db.add_message_and_clear_provider_replay(
            "message-final",
            "conv-atomic-close",
            1,
            &phoenix_core::domain::db_schema::MessageContent::agent(vec![
                phoenix_core::domain::llm_types::ContentBlock::text("done"),
            ]),
            None,
            None,
            &state,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert!(db
            .load_provider_replay_state("conv-atomic-close")
            .await
            .unwrap()
            .is_none());
        let message_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id='message-final'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(message_count, 1);
        let persisted: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id='conv-atomic-close'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<phoenix_core::domain::sm_state::ConvState>(&persisted).unwrap(),
            state
        );
    }

    #[tokio::test]
    async fn model_switch_clears_active_replay_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-switch", "conv-switch", "/tmp", true, None, None)
            .await
            .unwrap();
        db.store_provider_replay_state("conv-switch", &sample_payload())
            .await
            .unwrap();
        db.update_conversation_model_and_effort(
            "conv-switch",
            "gpt-5.5",
            None,
            phoenix_core::domain::llm_types::ServiceTier::Standard,
            "openai_responses",
        )
        .await
        .unwrap();
        assert!(db
            .load_provider_replay_state("conv-switch")
            .await
            .unwrap()
            .is_none());
        let model: String =
            sqlx::query_scalar("SELECT model FROM conversations WHERE id='conv-switch'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(model, "gpt-5.5");
    }

    // ─── provider CHECK constraint ───────────────────────────────────────────────

    #[tokio::test]
    async fn checked_provider_rejects_unknown_value() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-chk", "conv-chk", "/tmp", true, None, None)
            .await
            .unwrap();
        let result = sqlx::query(
            "INSERT INTO active_provider_replay_state \
             (conversation_id, provider, model, response_id, payload) \
             VALUES ('conv-chk', 'openai', 'gpt-4o', 'resp-x', '{}')",
        )
        .execute(db.pool())
        .await;
        assert!(
            result.is_err(),
            "CHECK constraint must reject provider='openai'"
        );
    }

    // ─── FK cascade: row deleted when conversation is deleted ────────────────────

    #[tokio::test]
    async fn replay_row_cascades_with_conversation_delete() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-del", "conv-del", "/tmp", true, None, None)
            .await
            .unwrap();
        db.store_provider_replay_state("conv-del", &sample_payload())
            .await
            .unwrap();
        assert!(db
            .load_provider_replay_state("conv-del")
            .await
            .unwrap()
            .is_some());
        db.delete_conversation("conv-del").await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_provider_replay_state \
             WHERE conversation_id = 'conv-del'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    // ─── round-trip: store then load ────────────────────────────────────────────

    #[tokio::test]
    async fn store_then_load_round_trips() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-rt", "conv-rt", "/tmp", true, None, None)
            .await
            .unwrap();
        let original = sample_payload();
        db.store_provider_replay_state("conv-rt", &original)
            .await
            .unwrap();
        let loaded = db
            .load_provider_replay_state("conv-rt")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original, loaded);
    }

    #[tokio::test]
    async fn store_updates_existing_row() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-up", "conv-up", "/tmp", true, None, None)
            .await
            .unwrap();
        let first = AnthropicReplayPayload::new(vec![AnthropicResponseSet::new(
            identity("msg-first", "claude-opus-4-5"),
            vec![],
        )
        .unwrap()])
        .unwrap();
        db.store_provider_replay_state("conv-up", &first)
            .await
            .unwrap();
        let second = AnthropicReplayPayload::new(vec![
            AnthropicResponseSet::new(identity("msg-first", "claude-opus-4-5"), vec![]).unwrap(),
            AnthropicResponseSet::new(identity("msg-second", "claude-opus-4-5"), vec![]).unwrap(),
        ])
        .unwrap();
        db.store_provider_replay_state("conv-up", &second)
            .await
            .unwrap();
        let loaded = db
            .load_provider_replay_state("conv-up")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, loaded);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_provider_replay_state \
             WHERE conversation_id = 'conv-up'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn load_returns_none_when_no_row() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-no", "conv-no", "/tmp", true, None, None)
            .await
            .unwrap();
        assert!(db
            .load_provider_replay_state("conv-no")
            .await
            .unwrap()
            .is_none());
    }

    // ─── clear: idempotent delete ────────────────────────────────────────────────

    #[tokio::test]
    async fn clear_removes_existing_row() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-cl", "conv-cl", "/tmp", true, None, None)
            .await
            .unwrap();
        db.store_provider_replay_state("conv-cl", &sample_payload())
            .await
            .unwrap();
        db.clear_provider_replay_state("conv-cl").await.unwrap();
        assert!(db
            .load_provider_replay_state("conv-cl")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn clear_is_idempotent_when_no_row_exists() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-idem", "conv-idem", "/tmp", true, None, None)
            .await
            .unwrap();
        db.clear_provider_replay_state("conv-idem").await.unwrap();
        db.clear_provider_replay_state("conv-idem").await.unwrap();
    }

    // ─── store: empty response_sets rejected ────────────────────────────────────

    #[tokio::test]
    async fn store_rejects_empty_payload() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-empty", "conv-empty", "/tmp", true, None, None)
            .await
            .unwrap();
        let empty = AnthropicReplayPayload::new(vec![]).unwrap();
        let err = db
            .store_provider_replay_state("conv-empty", &empty)
            .await
            .unwrap_err();
        assert!(
            matches!(err, DbError::Serialization(_)),
            "expected Serialization error, got {err:?}"
        );
    }

    // ─── malformed JSON returns a decode error ───────────────────────────────────

    #[tokio::test]
    async fn load_returns_serialization_error_for_malformed_payload() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-bad", "conv-bad", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO active_provider_replay_state \
             (conversation_id, provider, model, response_id, payload) \
             VALUES ('conv-bad', 'anthropic', 'claude-opus', 'msg-x', 'not-valid-json')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let err = db.load_provider_replay_state("conv-bad").await.unwrap_err();
        assert!(
            matches!(err, DbError::Serialization(_)),
            "expected Serialization error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn load_rejects_semantically_invalid_ordinal() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "conv-invalid-ordinal",
            "conv-invalid-ordinal",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let payload = serde_json::json!({
            "response_sets": [{
                "identity": {"response_id": "resp", "model": "claude-opus-5-5"},
                "owner_message_id": "owner",
                "public_content": [{"type": "text", "text": "public"}],
                "private_blocks": [{"type": "thinking", "index": 2, "thinking": "", "signature": "sig"}]
            }]
        });
        sqlx::query("INSERT INTO active_provider_replay_state (conversation_id, provider, model, response_id, payload) VALUES (?1,'anthropic','claude-opus-5-5','resp',?2)")
            .bind("conv-invalid-ordinal")
            .bind(payload.to_string())
            .execute(db.pool()).await.unwrap();
        let error = db
            .load_provider_replay_state("conv-invalid-ordinal")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("validation"));
    }

    #[tokio::test]
    async fn load_returns_serialization_error_for_unknown_fields() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-unk", "conv-unk", "/tmp", true, None, None)
            .await
            .unwrap();
        let bad_json = r#"{"response_sets":[],"unknown_field":true}"#;
        sqlx::query(
            "INSERT INTO active_provider_replay_state \
             (conversation_id, provider, model, response_id, payload) \
             VALUES ('conv-unk', 'anthropic', 'claude-opus', 'msg-y', ?1)",
        )
        .bind(bad_json)
        .execute(db.pool())
        .await
        .unwrap();
        let err = db.load_provider_replay_state("conv-unk").await.unwrap_err();
        assert!(
            matches!(err, DbError::Serialization(_)),
            "expected Serialization error for unknown field, got {err:?}"
        );
    }
}
