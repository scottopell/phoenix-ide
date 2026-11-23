use super::*;
use chrono::Utc;
use phoenix_core::domain::{
    db_schema::{FinalizedToolResult, InputOrigin, MessageType, ToolResult},
    llm_types::MessageRole,
    provider_replay::ProviderReplayUpdate,
    responses_replay::{ProviderResponseSet, ResponsesResponseSet},
    sm_state::ConvState,
};

const CONVERSATION: &str = "tool-discovery";
const ISSUES: &str = "mcp__github__issues";

fn tool(name: &str, revision: i64, deferred: bool) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: format!("Search {name}"),
        input_schema: serde_json::json!({"type":"object","properties":{"revision":{"const":revision}}}),
        defer_loading: deferred,
    }
}

fn catalog(revision: i64) -> Vec<ToolDefinition> {
    vec![
        tool_search_definition(),
        tool("read_file", 1, false),
        tool(ISSUES, revision, true),
        tool("mcp__slack__messages", 1, true),
    ]
}

async fn create(db: &Database) {
    db.create_conversation(CONVERSATION, CONVERSATION, "/tmp", true, None, None)
        .await
        .unwrap();
}

async fn prepare(
    db: &Database,
    revision: i64,
    allowed: Option<BTreeSet<String>>,
    visible: &[ToolPolicyMessage],
    mode: ToolDiscoveryMode,
) -> ToolAvailability {
    let tools = catalog(revision);
    let callable = allowed.unwrap_or_else(|| tools.iter().map(|tool| tool.name.clone()).collect());
    db.prepare_tool_availability(
        CONVERSATION,
        "route",
        None,
        &tools,
        &callable,
        visible,
        &[],
        mode,
    )
    .await
    .unwrap()
}

fn round(
    query: serde_json::Value,
    output: String,
    error: bool,
) -> (Message, Vec<FinalizedToolResult>) {
    named_round("search-owner", "search-call", 10, query, output, error)
}

fn named_round(
    owner: &str,
    call: &str,
    sequence: i64,
    query: serde_json::Value,
    output: String,
    error: bool,
) -> (Message, Vec<FinalizedToolResult>) {
    let assistant = Message {
        message_id: owner.into(),
        conversation_id: CONVERSATION.into(),
        sequence_id: sequence,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.into(),
            name: TOOL_SEARCH_NAME.into(),
            input: query,
        }]),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    };
    let result = if error {
        ToolResult::error(call.into(), output)
    } else {
        ToolResult::success(call.into(), output)
    };
    let finalized = FinalizedToolResult::new(
        &assistant.message_id,
        CONVERSATION,
        sequence + 1,
        assistant.created_at,
        &result,
    );
    (assistant, vec![finalized])
}

#[tokio::test]
async fn new_discoveries_append_after_existing_eager_and_loaded_schema_prefix() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    let (first, first_results) = successful_round(&db, "messages").await;
    let mut history = visible(&first, &first_results);
    let before = prepare(&db, 1, None, &history, ToolDiscoveryMode::Local).await;
    let before_names = before
        .declarations()
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        before_names,
        vec!["read_file", "tool_search", "mcp__slack__messages"]
    );
    let query = serde_json::json!({"query":"issues"});
    let selected = search_tools(
        &db.load_tool_search_catalog(CONVERSATION).await.unwrap(),
        &ToolSearchQuery::parse_input(&query).unwrap(),
    );
    let (second, second_results) = named_round(
        "second-owner",
        "second-call",
        20,
        query,
        tool_search_result_text(&selected),
        false,
    );
    db.persist_tool_round(CONVERSATION, &second, &second_results)
        .await
        .unwrap();
    history.extend(visible(&second, &second_results));
    let after = prepare(&db, 1, None, &history, ToolDiscoveryMode::Local).await;
    let after_names = after
        .declarations()
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(&after_names[..before_names.len()], before_names.as_slice());
    assert_eq!(after_names.last().unwrap(), ISSUES);
    assert_eq!(
        after.discovery_results()[0].owner_message_id,
        "search-owner"
    );
    assert_eq!(
        after.discovery_results()[1].owner_message_id,
        "second-owner"
    );
}

#[tokio::test]
async fn historical_only_schema_keeps_its_position_before_later_discovery() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Eager).await;
    let (mut previous, _) = round(serde_json::json!({}), String::new(), false);
    previous.content = MessageContent::Agent(vec![ContentBlock::ToolUse {
        id: "old-call".into(),
        name: "mcp__slack__messages".into(),
        input: serde_json::json!({"revision":1}),
    }]);
    let result = FinalizedToolResult::new(
        &previous.message_id,
        CONVERSATION,
        11,
        previous.created_at,
        &ToolResult::success("old-call".into(), "old messages".into()),
    );
    db.persist_tool_round(CONVERSATION, &previous, std::slice::from_ref(&result))
        .await
        .unwrap();
    let mut history = visible(&previous, std::slice::from_ref(&result));
    let before = prepare(&db, 1, None, &history, ToolDiscoveryMode::Local).await;
    let query = serde_json::json!({"query":"issues"});
    let (next, results) = named_round(
        "new-owner",
        "new-call",
        20,
        query,
        tool_search_result_text(&[tool(ISSUES, 1, true)]),
        false,
    );
    db.persist_tool_round(CONVERSATION, &next, &results)
        .await
        .unwrap();
    history.extend(visible(&next, &results));
    let after = prepare(&db, 1, None, &history, ToolDiscoveryMode::Local).await;
    let names = |snapshot: &ToolAvailability| {
        snapshot
            .declarations()
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        &names(&after)[..before.declarations().len()],
        names(&before).as_slice()
    );
    assert_eq!(after.declarations().last().unwrap().name, ISSUES);
}

fn visible(assistant: &Message, results: &[FinalizedToolResult]) -> Vec<ToolPolicyMessage> {
    std::iter::once(ToolPolicyMessage {
        source_message_id: Some(assistant.message_id.clone()),
        role: MessageRole::Assistant,
    })
    .chain(results.iter().map(|result| ToolPolicyMessage {
        source_message_id: Some(result.row().message_id.clone()),
        role: MessageRole::User,
    }))
    .collect()
}

async fn successful_round(db: &Database, query: &str) -> (Message, Vec<FinalizedToolResult>) {
    let input = serde_json::json!({"query":query});
    let selected = search_tools(
        &db.load_tool_search_catalog(CONVERSATION).await.unwrap(),
        &ToolSearchQuery::parse_input(&input).unwrap(),
    );
    let (assistant, results) = round(input, tool_search_result_text(&selected), false);
    db.persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .unwrap();
    (assistant, results)
}

#[tokio::test]
async fn deferred_schemas_require_successful_search_before_request_admission() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    let initial = prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    assert_eq!(initial.declarations().len(), 2);
    assert!(!initial.is_callable(ISSUES));
    assert!(!db
        .load_tool_admission_policy(CONVERSATION)
        .await
        .unwrap()
        .is_callable(ISSUES));
    let catalog = db.load_tool_search_catalog(CONVERSATION).await.unwrap();
    assert_eq!(catalog.len(), 3);
    assert!(catalog.iter().any(|tool| tool.name == ISSUES));
    let (assistant, results) = successful_round(&db, "issues").await;
    let snapshot = prepare(
        &db,
        1,
        None,
        &visible(&assistant, &results),
        ToolDiscoveryMode::Local,
    )
    .await;
    assert!(snapshot.is_callable(ISSUES));
    assert_eq!(snapshot.declarations().len(), 3);
    assert!(db
        .load_tool_admission_policy(CONVERSATION)
        .await
        .unwrap()
        .is_callable(ISSUES));
    let discovery = snapshot
        .discovery_result(&results[0].row().message_id, "search-call")
        .unwrap();
    assert_eq!(discovery.protocol, ToolDiscoveryProtocol::Function);
    assert_eq!(discovery.tools.len(), 1);
    assert_eq!(discovery.tools[0].name, ISSUES);
    assert!(
        sqlx::query("UPDATE tool_discovery_result_tools SET input_schema='{}'")
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE tool_discovery_results SET protocol='openai_client'")
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn forged_public_search_output_rolls_back_the_owning_round() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    let (assistant, results) = round(
        serde_json::json!({"query":"issues"}),
        "{\"tools\":[\"mcp__slack__messages\"]}".into(),
        false,
    );
    assert!(db
        .persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE conversation_id=?")
            .bind(CONVERSATION)
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tool_discovery_results")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn unavailable_search_errors_persist_without_admitting_any_schemas() {
    for mode in [
        ToolDiscoveryMode::Eager,
        ToolDiscoveryMode::OpenAiHosted,
        ToolDiscoveryMode::Local,
    ] {
        let db = Database::open_in_memory().await.unwrap();
        create(&db).await;
        let allowed = BTreeSet::from(["read_file".into()]);
        prepare(&db, 1, Some(allowed.clone()), &[], mode).await;
        let (assistant, results) = round(
            serde_json::json!({"malformed":"ignored on unavailable calls"}),
            "EUNAVAIL: tool_search is unavailable".into(),
            true,
        );
        db.persist_tool_round(CONVERSATION, &assistant, &results)
            .await
            .unwrap();
        let snapshot = prepare(&db, 1, Some(allowed), &visible(&assistant, &results), mode).await;
        assert_eq!(snapshot.discovery_results().len(), 1);
        assert!(snapshot.discovery_results()[0].tools.is_empty());
        assert_eq!(
            snapshot.discovery_results()[0].protocol,
            ToolDiscoveryProtocol::Function
        );
        assert!(!snapshot.is_callable(TOOL_SEARCH_NAME));
        assert!(!snapshot.is_callable(ISSUES));
        let admission = db.load_tool_admission_policy(CONVERSATION).await.unwrap();
        assert!(!admission.is_callable(TOOL_SEARCH_NAME));
        assert!(!admission.is_callable(ISSUES));
    }
}

#[tokio::test]
async fn unadvertised_search_cannot_forge_a_successful_empty_selection() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Eager).await;
    let (assistant, results) = round(
        serde_json::json!({"query":"nothing-matches"}),
        tool_search_result_text(&[]),
        false,
    );
    assert!(db
        .persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tool_discovery_results")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn failed_and_empty_searches_have_distinct_canonical_results_and_empty_selections() {
    for (query, error) in [("nothing-matches", false), ("", true)] {
        let db = Database::open_in_memory().await.unwrap();
        create(&db).await;
        prepare(&db, 1, None, &[], ToolDiscoveryMode::OpenAiClient).await;
        let (assistant, results) = round(
            serde_json::json!({"query":query}),
            if error {
                "invalid query".into()
            } else {
                tool_search_result_text(&[])
            },
            error,
        );
        db.persist_tool_round(CONVERSATION, &assistant, &results)
            .await
            .unwrap();
        let snapshot = prepare(
            &db,
            1,
            None,
            &visible(&assistant, &results),
            ToolDiscoveryMode::OpenAiClient,
        )
        .await;
        assert!(snapshot
            .discovery_result(&results[0].row().message_id, "search-call")
            .unwrap()
            .tools
            .is_empty());
        assert!(!snapshot.is_callable(ISSUES));
        let MessageContent::Tool(content) = &results[0].row().content else {
            panic!("tool result")
        };
        assert_eq!(content.is_error, error);
    }
}

#[tokio::test]
async fn discovery_selection_survives_reopen_and_immutable_replay_but_not_schema_or_policy_change()
{
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("discovery.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    let (assistant, results) = successful_round(&db, "issues").await;
    let history = visible(&assistant, &results);
    let expected = prepare(&db, 1, None, &history, ToolDiscoveryMode::Local).await;
    db.pool().close().await;
    let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
    let restored = prepare(&reopened, 1, None, &history, ToolDiscoveryMode::Local).await;
    assert!(restored.is_callable(ISSUES));
    assert!(same_definition(
        &expected.discovery_results()[0].tools[0],
        &restored.discovery_results()[0].tools[0]
    ));
    let changed = prepare(&reopened, 2, None, &history, ToolDiscoveryMode::Local).await;
    assert!(!changed.is_callable(ISSUES));
    reopened
        .persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .unwrap();
    let allowed = BTreeSet::from([
        TOOL_SEARCH_NAME.into(),
        "read_file".into(),
        "mcp__slack__messages".into(),
    ]);
    let disabled = prepare(
        &reopened,
        1,
        Some(allowed),
        &history,
        ToolDiscoveryMode::Local,
    )
    .await;
    assert!(!disabled.is_callable(ISSUES));
    assert!(same_definition(
        &expected.discovery_results()[0].tools[0],
        &disabled.discovery_results()[0].tools[0]
    ));
    let hidden = prepare(&reopened, 1, None, &[], ToolDiscoveryMode::Local).await;
    assert!(hidden.discovery_results().is_empty());
    assert!(!hidden.is_callable(ISSUES));
    reopened.pool().close().await;
}

#[tokio::test]
async fn child_write_failure_does_not_publish_messages_or_partial_selection() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    sqlx::query("CREATE TRIGGER fail_discovery_child BEFORE INSERT ON tool_discovery_result_tools BEGIN SELECT RAISE(ABORT,'injected child failure'); END").execute(db.pool()).await.unwrap();
    let query = serde_json::json!({"query":"issues"});
    let selected = search_tools(&catalog(1), &ToolSearchQuery::parse_input(&query).unwrap());
    let (assistant, results) = round(query, tool_search_result_text(&selected), false);
    assert!(db
        .persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .is_err());
    for table in [
        "messages",
        "tool_discovery_results",
        "tool_discovery_result_tools",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table}"
            )))
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
    }
    sqlx::query("DROP TRIGGER fail_discovery_child")
        .execute(db.pool())
        .await
        .unwrap();
    db.persist_tool_round(CONVERSATION, &assistant, &results)
        .await
        .unwrap();
}

#[tokio::test]
async fn native_client_protocol_survives_private_replay_retirement() {
    for (error, namespaced) in [(false, false), (false, true), (true, false), (true, true)] {
        let db = Database::open_in_memory().await.unwrap();
        create(&db).await;
        prepare(
            &db,
            1,
            None,
            &[],
            if error {
                ToolDiscoveryMode::Eager
            } else {
                ToolDiscoveryMode::OpenAiClient
            },
        )
        .await;
        let (assistant, results) = round(
            serde_json::json!({"query":"issues"}),
            if error {
                "search failed".into()
            } else {
                tool_search_result_text(&[tool(ISSUES, 1, true)])
            },
            error,
        );
        let MessageContent::Agent(public) = &assistant.content else {
            panic!("assistant")
        };
        let set = ProviderResponseSet::Responses(ResponsesResponseSet {
            response_id: "native-response".into(),
            model: "gpt-6.2-sol".into(),
            owner_message_id: assistant.message_id.clone(),
            public_content: public.clone(),
            output_items: vec![if namespaced {
                serde_json::json!({"type":"function_call","namespace":"tool_search","name":"tool_search_tool","call_id":"search-call","arguments":"{\"query\":\"issues\"}","status":"completed"})
            } else {
                serde_json::json!({"type":"tool_search_call","execution":"client","call_id":"search-call","arguments":{"query":"issues"},"status":"completed"})
            }],
        });
        let mut tx = db.pool().begin().await.unwrap();
        crate::responses_replay::append_tx(&mut tx, CONVERSATION, &set)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        db.persist_tool_round(CONVERSATION, &assistant, &results)
            .await
            .unwrap();
        db.update_state_and_replay(
            CONVERSATION,
            &ConvState::Idle,
            Utc::now(),
            &ProviderReplayUpdate::Clear,
        )
        .await
        .unwrap();
        let snapshot = prepare(
            &db,
            1,
            None,
            &visible(&assistant, &results),
            ToolDiscoveryMode::Local,
        )
        .await;
        assert_eq!(
            snapshot.discovery_results()[0].protocol,
            ToolDiscoveryProtocol::OpenAiClient
        );
        assert_eq!(snapshot.discovery_results()[0].tools.is_empty(), error);
        assert!(db
            .load_output_replay_state(CONVERSATION)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn switching_function_search_history_to_native_client_requires_native_discovery() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    prepare(&db, 1, None, &[], ToolDiscoveryMode::Local).await;
    let (assistant, results) = successful_round(&db, "issues").await;
    let mut history = visible(&assistant, &results);
    assert!(prepare(&db, 1, None, &history, ToolDiscoveryMode::Local)
        .await
        .is_callable(ISSUES));
    let mut actual_call = assistant.clone();
    actual_call.message_id = "executed-owner".into();
    actual_call.sequence_id = 20;
    actual_call.content = MessageContent::Agent(vec![ContentBlock::ToolUse {
        id: "executed-call".into(),
        name: ISSUES.into(),
        input: serde_json::json!({"revision":1}),
    }]);
    let actual_result = FinalizedToolResult::new(
        &actual_call.message_id,
        CONVERSATION,
        21,
        actual_call.created_at,
        &ToolResult::success("executed-call".into(), "found issue".into()),
    );
    db.persist_tool_round(
        CONVERSATION,
        &actual_call,
        std::slice::from_ref(&actual_result),
    )
    .await
    .unwrap();
    history.extend(visible(&actual_call, std::slice::from_ref(&actual_result)));
    let native = prepare(&db, 1, None, &history, ToolDiscoveryMode::OpenAiClient).await;
    assert_eq!(native.discovery_results().len(), 1);
    assert_eq!(
        native.discovery_results()[0].protocol,
        ToolDiscoveryProtocol::Function
    );
    assert!(!native.is_callable(ISSUES));
    assert!(!native
        .declarations()
        .iter()
        .any(|definition| definition.name == ISSUES));
    assert!(!db
        .load_tool_admission_policy(CONVERSATION)
        .await
        .unwrap()
        .is_callable(ISSUES));
    assert!(db
        .load_tool_search_catalog(CONVERSATION)
        .await
        .unwrap()
        .iter()
        .any(|definition| definition.name == ISSUES));
    assert!(prepare(&db, 1, None, &history, ToolDiscoveryMode::Local)
        .await
        .is_callable(ISSUES));
}

#[tokio::test]
async fn native_and_eager_routes_keep_full_catalog_without_exposing_local_search() {
    let db = Database::open_in_memory().await.unwrap();
    create(&db).await;
    for mode in [
        ToolDiscoveryMode::Eager,
        ToolDiscoveryMode::Anthropic,
        ToolDiscoveryMode::OpenAiHosted,
    ] {
        let snapshot = prepare(&db, 1, None, &[], mode).await;
        assert!(snapshot.is_callable(ISSUES));
        assert!(!snapshot.is_callable(TOOL_SEARCH_NAME));
        assert!(snapshot
            .declarations()
            .iter()
            .any(|tool| tool.name == ISSUES));
    }
}

#[tokio::test]
async fn migration_170_preserves_existing_request_admission_snapshot() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool.clone(), String::new());
    db.run_migrations().await.unwrap();
    crate::migrations::run_through_for_test(&pool, 169)
        .await
        .unwrap();
    create(&db).await;
    sqlx::query(
        "INSERT INTO conversation_tool_definitions VALUES (?, 'legacy', 'legacy', '{}', 0)",
    )
    .bind(CONVERSATION)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO conversation_callable_tools VALUES (?, 'legacy')")
        .bind(CONVERSATION)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        crate::migrations::run_through_for_test(&pool, 170)
            .await
            .unwrap(),
        1
    );
    assert!(db
        .load_tool_admission_policy(CONVERSATION)
        .await
        .unwrap()
        .is_callable("legacy"));
    assert_eq!(
        crate::migrations::run_through_for_test(&pool, 170)
            .await
            .unwrap(),
        0
    );
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap()
        .is_empty());
}
