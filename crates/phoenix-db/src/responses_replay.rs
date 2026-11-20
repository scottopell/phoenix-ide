use crate::{Database, DbError, DbResult};
use phoenix_core::domain::responses_replay::{
    ChatResponseSet, ChatToolSignature, ProviderResponseSet, ResponsesResponseSet,
};
use sqlx::{Row, Sqlite, Transaction};

fn replay_format(set: &ProviderResponseSet) -> &'static str {
    match set {
        ProviderResponseSet::Responses(_) => "responses",
        ProviderResponseSet::ChatCompletions(_) => "chat_completions",
    }
}

fn replay_items(set: &ProviderResponseSet) -> DbResult<Vec<String>> {
    match set {
        ProviderResponseSet::Responses(set) => set
            .output_items
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>(),
        ProviderResponseSet::ChatCompletions(set) => set
            .tool_signatures
            .iter()
            .map(serde_json::to_string)
            .collect(),
    }
    .map_err(|error| DbError::Serialization(error.to_string()))
}

fn decode_set(
    response_id: String,
    model: String,
    owner_message_id: String,
    public_content: &str,
    api_format: &str,
    items: &[String],
) -> DbResult<ProviderResponseSet> {
    let public_content = serde_json::from_str(public_content)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let set = match api_format {
        "responses" => ProviderResponseSet::Responses(ResponsesResponseSet {
            response_id,
            model,
            owner_message_id,
            public_content,
            output_items: items
                .iter()
                .map(|item| serde_json::from_str(item))
                .collect::<Result<_, _>>()
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        }),
        "chat_completions" => ProviderResponseSet::ChatCompletions(ChatResponseSet {
            response_id,
            model,
            owner_message_id,
            public_content,
            tool_signatures: items
                .iter()
                .map(|item| serde_json::from_str::<ChatToolSignature>(item))
                .collect::<Result<_, _>>()
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        }),
        _ => {
            return Err(DbError::Serialization(
                "Unknown output replay API format".into(),
            ))
        }
    };
    set.validate().map_err(DbError::Serialization)?;
    Ok(set)
}

pub(crate) async fn append_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    set: &ProviderResponseSet,
) -> DbResult<()> {
    set.validate().map_err(DbError::Serialization)?;
    let existing: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT model,owner_message_id,public_content,api_format FROM active_responses_replay_sets WHERE conversation_id=?1 AND response_id=?2",
    ).bind(conversation_id).bind(set.response_id()).fetch_optional(&mut **tx).await?;
    if let Some((model, owner, content, api_format)) = existing {
        let items: Vec<String> = sqlx::query_scalar(
            "SELECT payload FROM active_responses_replay_items WHERE conversation_id=?1 AND response_id=?2 ORDER BY ordinal",
        ).bind(conversation_id).bind(set.response_id()).fetch_all(&mut **tx).await?;
        let retained = decode_set(
            set.response_id().into(),
            model,
            owner,
            &content,
            &api_format,
            &items,
        )?;
        if &retained != set {
            return Err(DbError::Serialization(
                "Output replay identity was rewritten".into(),
            ));
        }
        return Ok(());
    }
    let ordinal: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal)+1,0) FROM active_responses_replay_sets WHERE conversation_id=?1",
    ).bind(conversation_id).fetch_one(&mut **tx).await?;
    let content = serde_json::to_string(set.public_content())
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    sqlx::query("INSERT INTO active_responses_replay_sets(conversation_id,response_id,ordinal,model,owner_message_id,public_content,api_format) VALUES(?1,?2,?3,?4,?5,?6,?7)")
        .bind(conversation_id).bind(set.response_id()).bind(ordinal).bind(set.model()).bind(set.owner_message_id()).bind(content).bind(replay_format(set))
        .execute(&mut **tx).await?;
    for (ordinal, payload) in replay_items(set)?.iter().enumerate() {
        sqlx::query("INSERT INTO active_responses_replay_items(conversation_id,response_id,ordinal,payload) VALUES(?1,?2,?3,?4)")
            .bind(conversation_id).bind(set.response_id()).bind(i64::try_from(ordinal).map_err(|error| DbError::Serialization(error.to_string()))?).bind(payload)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

impl Database {
    /// # Errors
    /// Returns an error if stored replay is malformed or the snapshot cannot be read.
    pub async fn load_output_replay_state(
        &self,
        conversation_id: &str,
    ) -> DbResult<Vec<ProviderResponseSet>> {
        let mut tx = self.pool().begin().await?;
        let rows = sqlx::query("SELECT response_id,model,owner_message_id,public_content,api_format FROM active_responses_replay_sets WHERE conversation_id=?1 ORDER BY ordinal")
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
                        "Output replay item ordinals are not contiguous".into(),
                    ));
                }
                output_items.push(item.try_get::<String, _>("payload")?);
            }
            sets.push(decode_set(
                response_id,
                row.try_get("model")?,
                row.try_get("owner_message_id")?,
                &row.try_get::<String, _>("public_content")?,
                &row.try_get::<String, _>("api_format")?,
                &output_items,
            )?);
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
    #[allow(clippy::too_many_lines)]
    async fn complete_tool_round_envelopes_survive_reopen_retry_and_settlement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("replay.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation("replay", "replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut first = response("first");
        first
            .public_content
            .insert(0, ContentBlock::text("Checking the workspace."));
        first.output_items.remove(0);
        first.output_items.insert(0,serde_json::json!({
            "type":"message", "id":"commentary-first", "role":"assistant", "phase":"commentary",
            "content":[{"type":"output_text", "text":"Checking the workspace.", "annotations":[], "extension":null}],
            "provider_extension":{"nullable":null,"ordered":[1,2]}
        }));
        let mut second = response("second");
        second
            .public_content
            .insert(0, ContentBlock::text("Checking the result."));
        second.output_items.insert(1,serde_json::json!({
            "type":"message", "id":"commentary-second", "role":"assistant", "phase":"commentary",
            "content":[{"type":"output_text", "text":"Checking the result.", "annotations":[]}],
            "provider_extension":null
        }));
        let expected = vec![first, second];
        for set in &expected {
            db.update_state_and_replay(
                "replay",
                &ConvState::LlmRequesting { attempt: 1 },
                chrono::Utc::now(),
                &ProviderReplayUpdate::Responses(set.clone()),
            )
            .await
            .unwrap();
        }
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        let loaded = reopened.load_output_replay_state("replay").await.unwrap();
        assert_eq!(
            loaded,
            expected
                .iter()
                .cloned()
                .map(ProviderResponseSet::Responses)
                .collect::<Vec<_>>()
        );
        assert!(expected[0]
            .output_items
            .iter()
            .all(|item| item["type"] != "reasoning"));
        for set in &expected {
            reopened
                .update_state_and_replay(
                    "replay",
                    &ConvState::LlmRequesting { attempt: 2 },
                    chrono::Utc::now(),
                    &ProviderReplayUpdate::Responses(set.clone()),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            reopened.load_output_replay_state("replay").await.unwrap(),
            expected
                .into_iter()
                .map(ProviderResponseSet::Responses)
                .collect::<Vec<_>>()
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(count, 5);
        reopened
            .update_state_and_replay(
                "replay",
                &ConvState::Idle,
                chrono::Utc::now(),
                &ProviderReplayUpdate::Clear,
            )
            .await
            .unwrap();
        reopened.pool().close().await;
        let settled = Database::open(path.to_str().unwrap()).await.unwrap();
        assert!(settled
            .load_output_replay_state("replay")
            .await
            .unwrap()
            .is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(settled.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            settled.get_conversation("replay").await.unwrap().state,
            ConvState::Idle
        );
        settled.pool().close().await;
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
            db.load_output_replay_state("replay").await.unwrap(),
            vec![
                ProviderResponseSet::Responses(first.clone()),
                ProviderResponseSet::Responses(second)
            ]
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
            .load_output_replay_state("replay")
            .await
            .unwrap()
            .is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    fn chat_response(id: &str) -> ChatResponseSet {
        ChatResponseSet {
            response_id: id.into(),
            model: "google/gemini-4-neon".into(),
            owner_message_id: format!("owner-{id}"),
            public_content: vec![ContentBlock::ToolUse {
                id: format!("call-{id}"),
                name: "report_number".into(),
                input: serde_json::json!({"number":7}),
            }],
            tool_signatures: vec![ChatToolSignature {
                tool_call_id: format!("call-{id}"),
                signature: format!("opaque-signature-{id}"),
            }],
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn chat_signatures_survive_reopen_retry_and_settle_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gemini-replay.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation("replay", "replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let first = chat_response("first");
        let second = chat_response("second");
        let state = ConvState::LlmRequesting { attempt: 1 };
        for set in [&first, &second] {
            db.update_state_and_replay(
                "replay",
                &state,
                chrono::Utc::now(),
                &ProviderReplayUpdate::ChatCompletions(set.clone()),
            )
            .await
            .unwrap();
        }
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        let expected = vec![
            ProviderResponseSet::ChatCompletions(first.clone()),
            ProviderResponseSet::ChatCompletions(second.clone()),
        ];
        assert_eq!(
            reopened.load_output_replay_state("replay").await.unwrap(),
            expected
        );
        reopened
            .update_state_and_replay(
                "replay",
                &state,
                chrono::Utc::now(),
                &ProviderReplayUpdate::ChatCompletions(first.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            reopened.load_output_replay_state("replay").await.unwrap(),
            expected
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(count, 2);
        let mut rewritten = first.clone();
        rewritten.tool_signatures[0].signature = "rewritten".into();
        assert!(reopened
            .update_state_and_replay(
                "replay",
                &ConvState::LlmRequesting { attempt: 2 },
                chrono::Utc::now(),
                &ProviderReplayUpdate::ChatCompletions(rewritten)
            )
            .await
            .is_err());
        let mut rewritten = first;
        rewritten.public_content[0] = ContentBlock::ToolUse {
            id: "call-first".into(),
            name: "report_number".into(),
            input: serde_json::json!({"number":8}),
        };
        assert!(reopened
            .update_state_and_replay(
                "replay",
                &ConvState::LlmRequesting { attempt: 2 },
                chrono::Utc::now(),
                &ProviderReplayUpdate::ChatCompletions(rewritten)
            )
            .await
            .is_err());
        assert_eq!(
            reopened.get_conversation("replay").await.unwrap().state,
            state
        );
        assert_eq!(
            reopened.load_output_replay_state("replay").await.unwrap(),
            expected
        );
        reopened
            .update_state_and_replay(
                "replay",
                &ConvState::Idle,
                chrono::Utc::now(),
                &ProviderReplayUpdate::Clear,
            )
            .await
            .unwrap();
        assert!(reopened
            .load_output_replay_state("replay")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened.get_conversation("replay").await.unwrap().state,
            ConvState::Idle
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_responses_replay_items")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        reopened.pool().close().await;
    }

    #[tokio::test]
    async fn protocol_migration_preserves_existing_responses_items_and_rejects_unknown_protocol() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("upgrade.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_through_for_test(db.pool(), 168)
            .await
            .unwrap();
        db.create_conversation("replay", "replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let assistant = db
            .add_message(
                "legacy-assistant",
                "replay",
                &phoenix_core::domain::db_schema::MessageContent::Agent(vec![ContentBlock::text(
                    "legacy",
                )]),
                None,
                None,
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO canonical_tool_round_receipts(conversation_id,assistant_message_id,selected_checkpoint_sha256,selected_state_updated_at_us,replay_kind,replay_payload_sha256) VALUES('replay',?1,?2,123,'responses',?3)")
    .bind(&assistant.message_id).bind(vec![1_u8;32]).bind(vec![2_u8;32]).execute(db.pool()).await.unwrap();
        let set = response("legacy");
        sqlx::query("INSERT INTO active_responses_replay_sets(conversation_id,response_id,ordinal,model,owner_message_id,public_content) VALUES('replay',?1,0,?2,?3,?4)")
        .bind(&set.response_id).bind(&set.model).bind(&set.owner_message_id)
        .bind(serde_json::to_string(&set.public_content).unwrap()).execute(db.pool()).await.unwrap();
        for (ordinal, item) in set.output_items.iter().enumerate() {
            sqlx::query("INSERT INTO active_responses_replay_items(conversation_id,response_id,ordinal,payload) VALUES('replay',?1,?2,?3)")
            .bind(&set.response_id).bind(i64::try_from(ordinal).unwrap()).bind(serde_json::to_string(item).unwrap())
            .execute(db.pool()).await.unwrap();
        }
        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 1);
        let receipt: (Vec<u8>, i64, String, Vec<u8>) = sqlx::query_as("SELECT selected_checkpoint_sha256,selected_state_updated_at_us,replay_kind,replay_payload_sha256 FROM canonical_tool_round_receipts WHERE assistant_message_id='legacy-assistant'")
    .fetch_one(db.pool()).await.unwrap();
        assert_eq!(
            receipt,
            (vec![1_u8; 32], 123, "responses".into(), vec![2_u8; 32])
        );
        sqlx::query("UPDATE canonical_tool_round_receipts SET replay_kind='chat_completions'")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(sqlx::query("INSERT INTO canonical_tool_round_receipts(conversation_id,assistant_message_id,selected_checkpoint_sha256,selected_state_updated_at_us,replay_kind,replay_payload_sha256) VALUES('replay','missing-assistant',?1,123,'chat_completions',?2)")
    .bind(vec![1_u8;32]).bind(vec![2_u8;32]).execute(db.pool()).await.is_err());

        assert_eq!(
            db.load_output_replay_state("replay").await.unwrap(),
            vec![ProviderResponseSet::Responses(set)]
        );
        assert!(
            sqlx::query("UPDATE active_responses_replay_sets SET api_format='unknown'")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO active_responses_replay_sets(conversation_id,response_id,ordinal,model,owner_message_id,public_content) VALUES('replay','missing-format',1,'model','owner','[]')").execute(db.pool()).await.is_err());
        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 0);
    }
    #[tokio::test]
    async fn chat_goal_round_replay_requires_exact_public_owner() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("replay", "replay", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut set = chat_response("owned");
        let assistant = db
            .add_message(
                &set.owner_message_id,
                "replay",
                &phoenix_core::domain::db_schema::MessageContent::Agent(set.public_content.clone()),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(crate::provider_replay::validate_goal_round_replay(
            &assistant,
            &ProviderReplayUpdate::ChatCompletions(set.clone())
        )
        .is_ok());
        set.public_content.push(ContentBlock::text("rewritten"));
        assert!(crate::provider_replay::validate_goal_round_replay(
            &assistant,
            &ProviderReplayUpdate::ChatCompletions(set.clone())
        )
        .is_err());
        set.public_content.pop();
        set.owner_message_id = "wrong-owner".into();
        assert!(crate::provider_replay::validate_goal_round_replay(
            &assistant,
            &ProviderReplayUpdate::ChatCompletions(set)
        )
        .is_err());
    }
}
