//! Durable conversation tool declarations and append-only provider policy context.
use std::collections::{BTreeMap, BTreeSet};

use phoenix_core::domain::llm_types::ToolDefinition;
use phoenix_core::domain::tool_availability::{
    PositionedToolChange, ToolAvailability, ToolChange, ToolPolicyMessage,
};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

fn definition(row: &sqlx::sqlite::SqliteRow) -> DbResult<ToolDefinition> {
    Ok(ToolDefinition {
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        input_schema: serde_json::from_str(&row.try_get::<String, _>("input_schema")?)
            .map_err(|e| DbError::Serialization(e.to_string()))?,
        defer_loading: row.try_get("defer_loading")?,
    })
}

fn identical(a: &ToolDefinition, b: &ToolDefinition) -> bool {
    a.name == b.name
        && a.description == b.description
        && a.input_schema == b.input_schema
        && a.defer_loading == b.defer_loading
}

fn legal_anchor(messages: &[ToolPolicyMessage], id: &str) -> bool {
    messages
        .iter()
        .position(|message| message.source_message_id.as_deref() == Some(id))
        .is_some_and(|index| {
            messages[index].role == phoenix_core::domain::llm_types::MessageRole::User
                && messages.get(index + 1).is_none_or(|next| {
                    next.role == phoenix_core::domain::llm_types::MessageRole::Assistant
                })
        })
}

type PolicyTransaction<'a> = sqlx::Transaction<'a, sqlx::Sqlite>;

struct NativePolicyState {
    initial: Vec<ToolDefinition>,
    changes: Vec<PositionedToolChange>,
    pending: Vec<ToolChange>,
}

async fn retain_policy(
    tx: &mut PolicyTransaction<'_>,
    conversation_id: &str,
    live_definitions: &[ToolDefinition],
    callable_names: &BTreeSet<String>,
) -> DbResult<ToolAvailability> {
    let mut live_names = BTreeSet::new();
    for tool in live_definitions {
        if !live_names.insert(&tool.name) {
            return Err(DbError::Serialization(
                "duplicate live tool declaration".into(),
            ));
        }
        sqlx::query("INSERT INTO conversation_tool_definitions (conversation_id,name,description,input_schema,defer_loading) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(conversation_id,name) DO UPDATE SET description=excluded.description,input_schema=excluded.input_schema,defer_loading=excluded.defer_loading")
                .bind(conversation_id).bind(&tool.name).bind(&tool.description)
                .bind(serde_json::to_string(&tool.input_schema).map_err(|e| DbError::Serialization(e.to_string()))?)
                .bind(tool.defer_loading).execute(&mut **tx).await?;
    }
    let retained = sqlx::query("SELECT name,description,input_schema,defer_loading FROM conversation_tool_definitions WHERE conversation_id=?1 ORDER BY name")
            .bind(conversation_id).fetch_all(&mut **tx).await?
            .iter().map(definition).collect::<DbResult<Vec<_>>>()?;
    let policy =
        ToolAvailability::new(retained, callable_names.clone()).map_err(DbError::Serialization)?;
    sqlx::query("DELETE FROM conversation_callable_tools WHERE conversation_id=?1")
        .bind(conversation_id)
        .execute(&mut **tx)
        .await?;
    for name in callable_names {
        sqlx::query(
            "INSERT INTO conversation_callable_tools (conversation_id,name) VALUES (?1,?2)",
        )
        .bind(conversation_id)
        .bind(name)
        .execute(&mut **tx)
        .await?;
    }
    Ok(policy)
}

async fn restored_reference_requires_prefix(
    tx: &mut PolicyTransaction<'_>,
    conversation_id: &str,
    retained: &[ToolDefinition],
    visible_messages: &[ToolPolicyMessage],
    historical_tool_references: &[(String, String)],
) -> DbResult<bool> {
    let initial_names: BTreeSet<String> = sqlx::query_scalar(
        "SELECT name FROM conversation_tool_context_initial WHERE conversation_id=?1",
    )
    .bind(conversation_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    let additions = sqlx::query(
            "SELECT name,after_message_id FROM conversation_tool_context_changes WHERE conversation_id=?1 AND kind='addition' ORDER BY ordinal",
        ).bind(conversation_id).fetch_all(&mut **tx).await?;
    let positions: BTreeMap<&str, usize> = visible_messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| message.source_message_id.as_deref().map(|id| (id, index)))
        .collect();
    let mut earliest_addition = BTreeMap::new();
    for addition in additions {
        let anchor: String = addition.try_get("after_message_id")?;
        if let Some(position) = positions.get(anchor.as_str()) {
            let name: String = addition.try_get("name")?;
            earliest_addition.entry(name).or_insert(*position);
        }
    }
    let restored_historical_schema = historical_tool_references.iter().any(|(message_id, name)| {
        !initial_names.contains(name)
            && retained.iter().any(|tool| tool.name == *name)
            && positions
                .get(message_id.as_str())
                .is_some_and(|reference_position| {
                    earliest_addition
                        .get(name)
                        .is_none_or(|addition_position| addition_position >= reference_position)
                })
    });
    Ok(restored_historical_schema)
}

async fn establish_context(
    tx: &mut PolicyTransaction<'_>,
    conversation_id: &str,
    route_key: &str,
    retained: &[ToolDefinition],
    visible_messages: &[ToolPolicyMessage],
    historical_tool_references: &[(String, String)],
) -> DbResult<()> {
    let previous_route: Option<String> = sqlx::query_scalar(
        "SELECT route_key FROM conversation_tool_contexts WHERE conversation_id=?1",
    )
    .bind(conversation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let stored_anchors: Vec<String> = sqlx::query_scalar(
        "SELECT after_message_id FROM conversation_tool_context_changes WHERE conversation_id=?1",
    )
    .bind(conversation_id)
    .fetch_all(&mut **tx)
    .await?;
    let history_window_changed = stored_anchors
        .iter()
        .any(|id| !legal_anchor(visible_messages, id));
    let restored_historical_schema = restored_reference_requires_prefix(
        tx,
        conversation_id,
        retained,
        visible_messages,
        historical_tool_references,
    )
    .await?;
    if previous_route
        .as_deref()
        .is_some_and(|old| old != route_key)
    {
        sqlx::query("DELETE FROM active_provider_replay_state WHERE conversation_id=?1")
            .bind(conversation_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM active_responses_replay_sets WHERE conversation_id=?1")
            .bind(conversation_id)
            .execute(&mut **tx)
            .await?;
    }
    if previous_route.as_deref() != Some(route_key)
        || history_window_changed
        || restored_historical_schema
    {
        sqlx::query("DELETE FROM conversation_tool_contexts WHERE conversation_id=?1")
            .bind(conversation_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO conversation_tool_contexts (conversation_id,continuation_id,route_key) VALUES (?1,?2,?3)")
                .bind(conversation_id).bind(uuid::Uuid::new_v4().to_string()).bind(route_key).execute(&mut **tx).await?;
        for (ordinal, tool) in retained.iter().enumerate() {
            sqlx::query("INSERT INTO conversation_tool_context_initial (conversation_id,ordinal,name,description,input_schema,defer_loading) VALUES (?1,?2,?3,?4,?5,?6)")
                    .bind(conversation_id).bind(i64::try_from(ordinal).map_err(|e| DbError::Serialization(e.to_string()))?).bind(&tool.name).bind(&tool.description)
                    .bind(serde_json::to_string(&tool.input_schema).map_err(|e| DbError::Serialization(e.to_string()))?)
                    .bind(tool.defer_loading).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

async fn reconcile_native_policy(
    tx: &mut PolicyTransaction<'_>,
    conversation_id: &str,
    retained: &[ToolDefinition],
    callable_names: &BTreeSet<String>,
) -> DbResult<NativePolicyState> {
    let initial = sqlx::query("SELECT name,description,input_schema,defer_loading FROM conversation_tool_context_initial WHERE conversation_id=?1 ORDER BY ordinal")
            .bind(conversation_id).fetch_all(&mut **tx).await?
            .iter().map(definition).collect::<DbResult<Vec<_>>>()?;
    let mut effective: BTreeMap<String, ToolDefinition> = initial
        .iter()
        .cloned()
        .map(|tool| (tool.name.clone(), tool))
        .collect();
    let mut offered: BTreeSet<String> = effective.keys().cloned().collect();
    let rows = sqlx::query("SELECT after_message_id,kind,name,description,input_schema,defer_loading FROM conversation_tool_context_changes WHERE conversation_id=?1 ORDER BY ordinal")
            .bind(conversation_id).fetch_all(&mut **tx).await?;
    let mut changes = Vec::new();
    for row in &rows {
        let name: String = row.try_get("name")?;
        let change = if row.try_get::<String, _>("kind")? == "addition" {
            let tool = definition(row)?;
            effective.insert(name.clone(), tool.clone());
            offered.insert(name);
            ToolChange::Addition(tool)
        } else {
            offered.remove(&name);
            ToolChange::Removal { name }
        };
        changes.push(PositionedToolChange {
            after_message_id: row.try_get("after_message_id")?,
            change,
        });
    }
    let mut pending = Vec::new();
    for tool in retained {
        let schema_changed = effective
            .get(&tool.name)
            .is_none_or(|old| !identical(old, tool));
        if schema_changed || (callable_names.contains(&tool.name) && !offered.contains(&tool.name))
        {
            pending.push(ToolChange::Addition(tool.clone()));
            offered.insert(tool.name.clone());
        }
        if !callable_names.contains(&tool.name) && offered.remove(&tool.name) {
            pending.push(ToolChange::Removal {
                name: tool.name.clone(),
            });
        }
    }
    Ok(NativePolicyState {
        initial,
        changes,
        pending,
    })
}

async fn append_native_changes(
    tx: &mut PolicyTransaction<'_>,
    conversation_id: &str,
    anchor_message_id: Option<&str>,
    visible_messages: &[ToolPolicyMessage],
    changes: &mut Vec<PositionedToolChange>,
    pending: Vec<ToolChange>,
) -> DbResult<()> {
    if let Some(anchor) = anchor_message_id.filter(|_| !pending.is_empty()) {
        if !legal_anchor(visible_messages, anchor) {
            return Err(DbError::Serialization(
                "tool policy anchor is absent from projected history".into(),
            ));
        }
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id=?1 AND conversation_id=?2)",
        )
        .bind(anchor)
        .bind(conversation_id)
        .fetch_one(&mut **tx)
        .await?;
        if !owned {
            return Err(DbError::Serialization(
                "tool policy anchor does not belong to conversation".into(),
            ));
        }
        for change in pending {
            let ordinal =
                i64::try_from(changes.len()).map_err(|e| DbError::Serialization(e.to_string()))?;
            let (kind, name, description, schema, defer_loading) = match &change {
                ToolChange::Addition(tool) => (
                    "addition",
                    tool.name.as_str(),
                    Some(tool.description.as_str()),
                    Some(
                        serde_json::to_string(&tool.input_schema)
                            .map_err(|e| DbError::Serialization(e.to_string()))?,
                    ),
                    Some(tool.defer_loading),
                ),
                ToolChange::Removal { name } => ("removal", name.as_str(), None, None, None),
            };
            sqlx::query("INSERT INTO conversation_tool_context_changes (conversation_id,ordinal,after_message_id,kind,name,description,input_schema,defer_loading) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
                    .bind(conversation_id).bind(ordinal).bind(anchor).bind(kind).bind(name).bind(description).bind(schema).bind(defer_loading).execute(&mut **tx).await?;
            changes.push(PositionedToolChange {
                after_message_id: anchor.into(),
                change,
            });
        }
    }
    Ok(())
}

impl Database {
    /// Load the policy frozen for the last provider request, without refreshing it.
    ///
    /// # Errors
    /// Returns database or persisted declaration validation failures.
    pub async fn load_tool_admission_policy(
        &self,
        conversation_id: &str,
    ) -> DbResult<ToolAvailability> {
        let mut tx = self.pool().begin().await?;
        let declarations = sqlx::query("SELECT name,description,input_schema,defer_loading FROM conversation_tool_definitions WHERE conversation_id=?1 ORDER BY name")
            .bind(conversation_id)
            .fetch_all(&mut *tx)
            .await?
            .iter()
            .map(definition)
            .collect::<DbResult<Vec<_>>>()?;
        let callable_names = sqlx::query_scalar(
            "SELECT name FROM conversation_callable_tools WHERE conversation_id=?1 ORDER BY name",
        )
        .bind(conversation_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let policy =
            ToolAvailability::new(declarations, callable_names).map_err(DbError::Serialization)?;
        tx.commit().await?;
        Ok(policy)
    }

    /// # Errors
    /// Rejects invalid policy, missing anchors for changes, or database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_tool_availability(
        &self,
        conversation_id: &str,
        route_key: &str,
        anchor_message_id: Option<&str>,
        live_definitions: &[ToolDefinition],
        callable_names: &BTreeSet<String>,
        visible_messages: &[ToolPolicyMessage],
        historical_tool_references: &[(String, String)],
    ) -> DbResult<ToolAvailability> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let policy =
            retain_policy(&mut tx, conversation_id, live_definitions, callable_names).await?;
        establish_context(
            &mut tx,
            conversation_id,
            route_key,
            policy.declarations(),
            visible_messages,
            historical_tool_references,
        )
        .await?;
        let NativePolicyState {
            initial,
            mut changes,
            pending,
        } = reconcile_native_policy(
            &mut tx,
            conversation_id,
            policy.declarations(),
            callable_names,
        )
        .await?;
        append_native_changes(
            &mut tx,
            conversation_id,
            anchor_message_id,
            visible_messages,
            &mut changes,
            pending,
        )
        .await?;
        let continuation_id: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id=?1",
        )
        .bind(conversation_id)
        .fetch_one(&mut *tx)
        .await?;
        let snapshot = policy
            .with_anthropic_context(initial, changes)
            .and_then(|snapshot| snapshot.with_continuation_id(continuation_id))
            .map_err(DbError::Serialization)?;
        tx.commit().await?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::db_schema::{MessageContent, UserContent};

    fn visible() -> Vec<ToolPolicyMessage> {
        use phoenix_core::domain::llm_types::MessageRole::{Assistant, User};
        [
            ("user-a", User),
            ("assistant-a", Assistant),
            ("user-b", User),
            ("assistant-b", Assistant),
            ("anchor", User),
            ("foreign", Assistant),
        ]
        .into_iter()
        .map(|(id, role)| ToolPolicyMessage {
            source_message_id: Some(id.into()),
            role,
        })
        .collect()
    }
    fn tool(name: &str, revision: i32) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("revision {revision}"),
            input_schema: serde_json::json!({"type":"object","revision":revision}),
            defer_loading: false,
        }
    }
    async fn database() -> Database {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("policy", "policy", "/tmp", true, None, None)
            .await
            .unwrap();
        for id in ["user-a", "user-b"] {
            db.add_message(
                id,
                "policy",
                &MessageContent::User(UserContent::new("continue")),
                None,
                None,
            )
            .await
            .unwrap();
        }
        db
    }
    #[tokio::test]
    async fn withdrawal_retry_schema_change_and_switch_preserve_definitions() {
        let db = database().await;
        let first = db
            .prepare_tool_availability(
                "policy",
                "anthropic",
                Some("user-a"),
                &[tool("bash", 1), tool("plan", 1)],
                &BTreeSet::from(["bash".into(), "plan".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert!(first.anthropic_changes().is_empty());
        let withdrawn = db
            .prepare_tool_availability(
                "policy",
                "anthropic",
                Some("user-b"),
                &[tool("bash", 1)],
                &BTreeSet::from(["bash".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(withdrawn.declarations().len(), 2);
        assert!(!withdrawn.is_callable("plan"));
        assert!(
            matches!(&withdrawn.anthropic_changes()[0].change,ToolChange::Removal { name } if name == "plan")
        );
        let retry = db
            .prepare_tool_availability(
                "policy",
                "anthropic",
                Some("user-b"),
                &[tool("bash", 1)],
                &BTreeSet::from(["bash".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(retry.anthropic_changes().len(), 1);
        let changed = db
            .prepare_tool_availability(
                "policy",
                "anthropic",
                Some("user-b"),
                &[tool("bash", 2)],
                &BTreeSet::from(["bash".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(changed.anthropic_changes().len(), 2);
        assert_eq!(
            changed.anthropic_initial_declarations()[0].description,
            "revision 1"
        );
        assert_eq!(changed.declarations()[0].description, "revision 2");
        sqlx::query("INSERT INTO active_responses_replay_sets (conversation_id,response_id,ordinal,model,owner_message_id,public_content) VALUES ('policy','response',0,'gpt','owner','[]')").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO active_responses_replay_items (conversation_id,response_id,ordinal,payload) VALUES ('policy','response',0,'{}')").execute(db.pool()).await.unwrap();
        db.update_conversation_model_and_effort(
            "policy",
            "gpt-5",
            None,
            phoenix_core::domain::llm_types::ServiceTier::Standard,
            "openai_responses",
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_responses_replay_items WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
        let switched = db
            .prepare_tool_availability(
                "policy",
                "responses",
                Some("user-b"),
                &[tool("bash", 2)],
                &BTreeSet::from(["bash".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(
            switched.anthropic_initial_declarations()[0].description,
            "revision 2"
        );
        assert_eq!(switched.anthropic_changes().len(), 1);
        assert_eq!(switched.declarations().len(), 2);
    }
    #[tokio::test]
    async fn paused_tail_defers_events_until_legal_anchor() {
        let db = database().await;
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        let paused = db
            .prepare_tool_availability(
                "policy",
                "a",
                None,
                &[tool("a", 2)],
                &BTreeSet::new(),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert!(paused.anthropic_changes().is_empty());
        assert!(!paused.is_callable("a"));
        assert_eq!(
            paused.anthropic_initial_declarations()[0].description,
            "revision 1"
        );
        let resumed = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[tool("a", 2)],
                &BTreeSet::new(),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(resumed.anthropic_changes().len(), 2);
        assert!(resumed
            .anthropic_changes()
            .iter()
            .all(|change| change.after_message_id == "user-b"));
    }
    #[tokio::test]
    async fn reopening_retains_prefix_and_change_positions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("policy.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation("policy", "policy", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "anchor",
            "policy",
            &MessageContent::User(UserContent::new("continue")),
            None,
            None,
        )
        .await
        .unwrap();
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("anchor"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("anchor"),
            &[],
            &BTreeSet::new(),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        let identity: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        let policy = reopened
            .prepare_tool_availability(
                "policy",
                "a",
                Some("anchor"),
                &[],
                &BTreeSet::new(),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(
            policy.anthropic_initial_declarations()[0].description,
            "revision 1"
        );
        assert_eq!(policy.anthropic_changes().len(), 1);
        assert_eq!(policy.anthropic_changes()[0].after_message_id, "anchor");
        let resumed_identity: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(reopened.pool())
        .await
        .unwrap();
        assert_eq!(identity, resumed_identity);
        reopened.pool().close().await;
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn history_window_change_retires_whole_native_context() {
        let db = database().await;
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[],
            &BTreeSet::new(),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO active_provider_replay_state (conversation_id,provider,model,response_id,payload) VALUES ('policy','anthropic','model','response','{}')").execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO active_responses_replay_sets (conversation_id,response_id,ordinal,model,owner_message_id,public_content) VALUES ('policy','response',0,'gpt','owner','[]')").execute(db.pool()).await.unwrap();
        let old: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let projected = vec![ToolPolicyMessage {
            source_message_id: Some("user-b".into()),
            role: phoenix_core::domain::llm_types::MessageRole::User,
        }];
        let compacted = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[],
                &BTreeSet::new(),
                &projected,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(compacted.declarations().len(), 1);
        assert_eq!(compacted.anthropic_changes().len(), 1);
        assert_eq!(compacted.anthropic_changes()[0].after_message_id, "user-b");
        let fresh: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_ne!(old, fresh);
        let private_count: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM active_provider_replay_state WHERE conversation_id='policy') + (SELECT COUNT(*) FROM active_responses_replay_sets WHERE conversation_id='policy')").fetch_one(db.pool()).await.unwrap();
        assert_eq!(private_count, 2);

        let retry = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[],
                &BTreeSet::new(),
                &projected,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(retry.anthropic_changes().len(), 1);
        let same: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(fresh, same);
        db.prepare_tool_availability(
            "policy",
            "b",
            Some("user-b"),
            &[],
            &BTreeSet::new(),
            &projected,
            &[],
        )
        .await
        .unwrap();
        let private_count: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM active_provider_replay_state WHERE conversation_id='policy') + (SELECT COUNT(*) FROM active_responses_replay_sets WHERE conversation_id='policy')").fetch_one(db.pool()).await.unwrap();
        assert_eq!(private_count, 0);

        assert!(db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-a"),
                &[tool("a", 2)],
                &BTreeSet::new(),
                &projected,
                &[]
            )
            .await
            .is_err());
    }
    #[tokio::test]
    async fn first_context_preserves_existing_private_replay() {
        let db = database().await;
        sqlx::query("INSERT INTO active_provider_replay_state (conversation_id,provider,model,response_id,payload) VALUES ('policy','anthropic','model','response','{}')").execute(db.pool()).await.unwrap();
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_provider_replay_state WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
    #[tokio::test]
    async fn authentic_reconnection_rebaselines_schema_before_old_reference() {
        let db = database().await;
        let references = vec![("user-a".into(), "slack".into())];
        let refused = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[tool("bash", 1)],
                &BTreeSet::from(["bash".into()]),
                &visible(),
                &references,
            )
            .await
            .unwrap();
        assert!(!refused
            .declarations()
            .iter()
            .any(|tool| tool.name == "slack"));
        let old: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let restored = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[tool("bash", 1), tool("slack", 1)],
                &BTreeSet::from(["bash".into(), "slack".into()]),
                &visible(),
                &references,
            )
            .await
            .unwrap();
        assert!(restored
            .anthropic_initial_declarations()
            .iter()
            .any(|tool| tool.name == "slack"));
        assert!(restored.anthropic_changes().is_empty());
        let fresh: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_ne!(old, fresh);
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-b"),
            &[tool("bash", 1), tool("slack", 1)],
            &BTreeSet::from(["bash".into(), "slack".into()]),
            &visible(),
            &references,
        )
        .await
        .unwrap();
        let retry: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(fresh, retry);
    }
    #[tokio::test]
    async fn earlier_native_addition_preserves_valid_reference_context() {
        let db = database().await;
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("bash", 1)],
            &BTreeSet::from(["bash".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("bash", 1), tool("slack", 1)],
            &BTreeSet::from(["bash".into(), "slack".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        let old: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let refs = vec![("user-b".into(), "slack".into())];
        let valid = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[tool("bash", 1), tool("slack", 1)],
                &BTreeSet::from(["bash".into(), "slack".into()]),
                &visible(),
                &refs,
            )
            .await
            .unwrap();
        assert!(!valid
            .anthropic_initial_declarations()
            .iter()
            .any(|tool| tool.name == "slack"));
        assert_eq!(valid.anthropic_changes().len(), 1);
        let same: String = sqlx::query_scalar(
            "SELECT continuation_id FROM conversation_tool_contexts WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(old, same);
    }
    #[test]
    fn source_less_user_neighbor_prevents_native_insertion() {
        use phoenix_core::domain::llm_types::MessageRole::{Assistant, User};
        let messages = vec![
            ToolPolicyMessage {
                source_message_id: Some("anchor".into()),
                role: User,
            },
            ToolPolicyMessage {
                source_message_id: None,
                role: User,
            },
            ToolPolicyMessage {
                source_message_id: None,
                role: Assistant,
            },
        ];
        assert!(!legal_anchor(&messages, "anchor"));
    }
    #[tokio::test]
    async fn revised_input_after_failed_attempt_rebaselines_illegal_anchor() {
        use phoenix_core::domain::llm_types::MessageRole::User;
        let db = database().await;
        let first = vec![ToolPolicyMessage {
            source_message_id: Some("user-a".into()),
            role: User,
        }];
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &first,
            &[],
        )
        .await
        .unwrap();
        let failed = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-a"),
                &[],
                &BTreeSet::new(),
                &first,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(failed.anthropic_changes()[0].after_message_id, "user-a");
        sqlx::query("INSERT INTO active_provider_replay_state (conversation_id,provider,model,response_id,payload) VALUES ('policy','anthropic','model','response','{}')").execute(db.pool()).await.unwrap();
        let revised = vec![
            ToolPolicyMessage {
                source_message_id: Some("user-a".into()),
                role: User,
            },
            ToolPolicyMessage {
                source_message_id: Some("user-b".into()),
                role: User,
            },
        ];
        let resumed = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[],
                &BTreeSet::new(),
                &revised,
                &[],
            )
            .await
            .unwrap();
        assert_ne!(failed.continuation_id(), resumed.continuation_id());
        assert_eq!(resumed.anthropic_changes().len(), 1);
        assert_eq!(resumed.anthropic_changes()[0].after_message_id, "user-b");
        assert_eq!(resumed.declarations().len(), 1);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_provider_replay_state WHERE conversation_id='policy'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
        let retry = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-b"),
                &[],
                &BTreeSet::new(),
                &revised,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(resumed.continuation_id(), retry.continuation_id());
        assert_eq!(retry.anthropic_changes().len(), 1);
    }
    #[tokio::test]
    async fn invalid_anchor_rolls_back_catalog_and_policy() {
        let db = database().await;
        db.prepare_tool_availability(
            "policy",
            "a",
            Some("user-a"),
            &[tool("a", 1)],
            &BTreeSet::from(["a".into()]),
            &visible(),
            &[],
        )
        .await
        .unwrap();
        assert!(db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("foreign"),
                &[tool("a", 2)],
                &BTreeSet::new(),
                &visible(),
                &[]
            )
            .await
            .is_err());
        let retry = db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-a"),
                &[tool("a", 1)],
                &BTreeSet::from(["a".into()]),
                &visible(),
                &[],
            )
            .await
            .unwrap();
        assert!(retry.anthropic_changes().is_empty());
        assert_eq!(retry.declarations()[0].description, "revision 1");
        assert!(db
            .prepare_tool_availability(
                "policy",
                "a",
                Some("user-a"),
                &[],
                &BTreeSet::from(["missing".into()]),
                &visible(),
                &[]
            )
            .await
            .is_err());
    }
}
