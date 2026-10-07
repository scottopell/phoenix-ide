use crate::{Database, DbError, DbResult};
use phoenix_core::domain::responses_replay::ResponsesResponseSet;
use sqlx::{Row, Sqlite, Transaction};

pub(crate) async fn append_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    set: &ResponsesResponseSet,
) -> DbResult<()> {
    set.validate().map_err(DbError::Serialization)?;
    let existing: Option<(String, String, String)> = sqlx::query_as(
        "SELECT model,owner_message_id,public_content FROM active_responses_replay_sets WHERE conversation_id=?1 AND response_id=?2",
    ).bind(conversation_id).bind(&set.response_id).fetch_optional(&mut **tx).await?;
    if let Some((model, owner, content)) = existing {
        let public: Vec<phoenix_core::domain::llm_types::ContentBlock> =
            serde_json::from_str(&content)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let items: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM active_responses_replay_items WHERE conversation_id=?1 AND response_id=?2 ORDER BY ordinal",
        ).bind(conversation_id).bind(&set.response_id).fetch_all(&mut **tx).await?;
        let items: Vec<serde_json::Value> = items
            .iter()
            .map(|item| serde_json::from_str(item))
            .collect::<Result<_, _>>()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        if model != set.model
            || owner != set.owner_message_id
            || public != set.public_content
            || items != set.output_items
        {
            return Err(DbError::Serialization(
                "Responses replay identity was rewritten".into(),
            ));
        }
        return Ok(());
    }
    let ordinal: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal)+1,0) FROM active_responses_replay_sets WHERE conversation_id=?1",
    ).bind(conversation_id).fetch_one(&mut **tx).await?;
    let content = serde_json::to_string(&set.public_content)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    sqlx::query("INSERT INTO active_responses_replay_sets(conversation_id,response_id,ordinal,model,owner_message_id,public_content) VALUES(?1,?2,?3,?4,?5,?6)")
        .bind(conversation_id).bind(&set.response_id).bind(ordinal).bind(&set.model).bind(&set.owner_message_id).bind(content)
        .execute(&mut **tx).await?;
    for (ordinal, item) in set.output_items.iter().enumerate() {
        let payload = serde_json::to_string(item)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query("INSERT INTO active_responses_replay_items(conversation_id,response_id,ordinal,payload) VALUES(?1,?2,?3,?4)")
            .bind(conversation_id).bind(&set.response_id).bind(i64::try_from(ordinal).map_err(|error| DbError::Serialization(error.to_string()))?).bind(payload)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

impl Database {
    /// # Errors
    /// Returns an error if stored replay is malformed or the snapshot cannot be read.
    pub async fn load_responses_replay_state(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<ResponsesResponseSet>> {
        let mut tx = self.pool().begin().await?;
        let rows = sqlx::query("SELECT response_id,model,owner_message_id,public_content FROM active_responses_replay_sets WHERE conversation_id=?1 ORDER BY ordinal")
            .bind(conversation_id).fetch_all(&mut *tx).await?;
        let mut sets = Vec::with_capacity(rows.len());
        for row in rows {
            let response_id: String = row.try_get("response_id")?;
            let items = sqlx::query("SELECT ordinal,payload FROM active_responses_replay_items WHERE conversation_id=?1 AND response_id=?2 ORDER BY ordinal")
                .bind(conversation_id).bind(&response_id).fetch_all(&mut *tx).await?;
            let mut output_items = Vec::with_capacity(items.len());
            for (expected, item) in items.iter().enumerate() {
                if item.try_get::<i64, _>("ordinal")?
                    != i64::try_from(expected)
                        .map_err(|error| DbError::Serialization(error.to_string()))?
                {
                    return Err(DbError::Serialization(
                        "Responses replay item ordinals are not contiguous".into(),
                    ));
                }
                output_items.push(
                    serde_json::from_str(&item.try_get::<String, _>("payload")?)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                );
            }
            let set = ResponsesResponseSet {
                response_id,
                model: row.try_get("model")?,
                owner_message_id: row.try_get("owner_message_id")?,
                public_content: serde_json::from_str(&row.try_get::<String, _>("public_content")?)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                output_items,
            };
            set.validate().map_err(DbError::Serialization)?;
            sets.push(set);
        }
        tx.commit().await?;
        Ok(sets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::{
        llm_types::ContentBlock, provider_replay::ProviderReplayUpdate, sm_state::ConvState,
    };

    fn response(id: &str) -> ResponsesResponseSet {
        ResponsesResponseSet {
            response_id: id.into(),
            model: "model".into(),
            owner_message_id: format!("owner-{id}"),
            public_content: vec![ContentBlock::ToolUse {
                id: format!("call-{id}"),
                name: "bash".into(),
                input: serde_json::json!({}),
            }],
            output_items: vec![
                serde_json::json!({"type":"reasoning","id":format!("reason-{id}"),"encrypted_content":"opaque","summary":[],"provider_extension":null}),
                serde_json::json!({"type":"function_call","id":format!("item-{id}"),"call_id":format!("call-{id}"),"name":"bash","arguments":"{}"}),
            ],
        }
    }

    #[tokio::test]
    async fn responses_replay_is_ordered_idempotent_and_atomic_with_state() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("replay", "replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let state = ConvState::LlmRequesting { attempt: 1 };
        let first = response("first");
        let second = response("second");
        for set in [&first, &first, &second] {
            db.update_state_and_replay(
                "replay",
                &state,
                chrono::Utc::now(),
                &ProviderReplayUpdate::Responses(set.clone()),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            db.load_responses_replay_state("replay").await.unwrap(),
            vec![first.clone(), second]
        );
        let item_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(item_count, 4);
        let mut rewritten = first;
        rewritten.output_items[0]["encrypted_content"] = serde_json::json!("rewritten");
        assert!(db
            .update_state_and_replay(
                "replay",
                &ConvState::LlmRequesting { attempt: 2 },
                chrono::Utc::now(),
                &ProviderReplayUpdate::Responses(rewritten)
            )
            .await
            .is_err());
        assert_eq!(db.get_conversation("replay").await.unwrap().state, state);
        db.update_state_and_replay(
            "replay",
            &ConvState::Idle,
            chrono::Utc::now(),
            &ProviderReplayUpdate::Clear,
        )
        .await
        .unwrap();
        assert!(db
            .load_responses_replay_state("replay")
            .await
            .unwrap()
            .is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
