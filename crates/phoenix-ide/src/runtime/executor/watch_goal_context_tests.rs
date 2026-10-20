use super::*;
use crate::coordinator_watch_delivery::captured_notification_fixture::{
    assert_private_absent, captured_notification, GOAL_ID, OBJECTIVE, REASON,
};
use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
use crate::runtime::traits::DatabaseStorage;
use phoenix_core::domain::db_schema::InputOrigin;

fn runtime(
    db: crate::db::Database,
    conversation: &str,
    cwd: &Path,
    client: Arc<MockLlmClient>,
) -> ConversationRuntime<DatabaseStorage, Arc<MockLlmClient>, Arc<MockToolExecutor>> {
    let mut context = ConvContext::new(conversation, cwd.to_path_buf(), "test-model", 200_000);
    context.is_coordinator = true;
    let (tx, rx) = mpsc::channel(8);
    ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 1 },
        DatabaseStorage::new(db),
        client,
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        SseBroadcaster::new(16, 0),
    )
}

async fn assemble(
    storage: &DatabaseStorage,
    conversation: &str,
    messages: &[crate::db::Message],
    excluded_message_id: Option<&str>,
    cache: &std::sync::Mutex<Option<i64>>,
) -> Result<Vec<LlmMessage>, String> {
    assemble_cleared_messages(
        storage,
        conversation,
        messages,
        excluded_message_id,
        &std::collections::HashSet::new(),
        200_000,
        cache,
    )
    .await
}

fn rendered_text(messages: &[LlmMessage]) -> String {
    messages
        .iter()
        .flat_map(|message| message.content.iter().map(ContentBlock::render_text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn saved_goal_capture_is_request_local_immutable_and_budgeted_before_freeze() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let (message, source, diagnostic) = captured_notification(&db).await;
    let storage = DatabaseStorage::new(db.clone());
    let messages = db.get_messages(&message.conversation_id).await.unwrap();
    let cache = std::sync::Mutex::new(None);
    let first = assemble(&storage, &message.conversation_id, &messages, None, &cache)
        .await
        .unwrap();
    let text = rendered_text(&first);
    assert!(text.contains(&diagnostic));
    for secret in [OBJECTIVE, REASON, GOAL_ID] {
        assert!(text.contains(secret));
    }
    let no_capture = render_messages(
        &messages,
        &std::collections::HashSet::new(),
        &std::collections::HashMap::new(),
    )
    .unwrap();
    assert!(estimate_messages_tokens(&first) > estimate_messages_tokens(&no_capture));
    let budget = plan_continuation_history(first.clone(), 200_000, 0, None);
    assert_eq!(
        budget.estimated_history_tokens,
        estimate_messages_tokens(&first)
    );
    db.delete_conversation(&source).await.unwrap();
    let replay = assemble(&storage, &message.conversation_id, &messages, None, &cache)
        .await
        .unwrap();
    assert_eq!(rendered_text(&replay), text);
    assert_private_absent(
        &serde_json::to_value(db.get_messages(&message.conversation_id).await.unwrap()).unwrap(),
    );
}

#[tokio::test]
async fn included_capture_failure_is_not_a_clear_watermark_fallback_and_omitted_origins_do_not_lookup(
) {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let (message, _, _) = captured_notification(&db).await;
    sqlx::query("ALTER TABLE coordinator_watch_goal_captures RENAME TO unavailable_captures")
        .execute(db.pool())
        .await
        .unwrap();
    let storage = DatabaseStorage::new(db);
    for cached in [None, Some(0)] {
        let cache = std::sync::Mutex::new(cached);
        let error = assemble(
            &storage,
            "missing-watermark-owner",
            std::slice::from_ref(&message),
            None,
            &cache,
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("failed to load subscription goal facts:"));
        let excluded = assemble(
            &storage,
            "missing-watermark-owner",
            std::slice::from_ref(&message),
            Some(&message.message_id),
            &cache,
        )
        .await
        .unwrap();
        assert!(excluded.is_empty());
        let mut wake = message.clone();
        let MessageContent::User(content) = &mut wake.content else {
            panic!("notification");
        };
        content.is_meta = true;
        wake.display_data = Some(serde_json::json!({"type":"wake_result","adopted":false}));
        assert!(
            assemble(&storage, "missing-watermark-owner", &[wake], None, &cache)
                .await
                .unwrap()
                .is_empty()
        );
        for origin in [
            InputOrigin::UserApi,
            InputOrigin::RemoteApi {
                instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
            },
        ] {
            let mut ordinary = message.clone();
            ordinary.origin = origin;
            let rendered = assemble(
                &storage,
                "missing-watermark-owner",
                &[ordinary],
                None,
                &cache,
            )
            .await
            .unwrap();
            assert!(!rendered_text(&rendered).contains("Saved goal facts"));
        }
    }
}

#[tokio::test]
async fn expected_captured_fact_missing_or_corrupt_fails_context_instead_of_no_goal() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let (message, _, _) = captured_notification(&db).await;
    sqlx::query("ALTER TABLE coordinator_watch_goal_captures RENAME TO retained_captures")
        .execute(db.pool())
        .await
        .unwrap();
    let storage = DatabaseStorage::new(db.clone());
    for definition in [
        "CREATE VIEW coordinator_watch_goal_captures AS SELECT * FROM retained_captures WHERE 0",
        "CREATE VIEW coordinator_watch_goal_captures AS SELECT event_id,kind,owner_id,owner_version,goal_id,objective,'invalid' AS state,reason,usage_kind FROM retained_captures",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(definition)).execute(db.pool()).await.unwrap();
        let error = assemble(&storage, &message.conversation_id, std::slice::from_ref(&message), None, &std::sync::Mutex::new(Some(0))).await.unwrap_err();
        assert!(error.starts_with("failed to load subscription goal facts:"));
        sqlx::query("DROP VIEW coordinator_watch_goal_captures").execute(db.pool()).await.unwrap();
    }
}

#[tokio::test]
async fn normal_and_continuation_capture_load_errors_refuse_provider_dispatch() {
    for continuation in [false, true] {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let (message, _, _) = captured_notification(&db).await;
        sqlx::query("ALTER TABLE coordinator_watch_goal_captures RENAME TO unavailable_captures")
            .execute(db.pool())
            .await
            .unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let client = Arc::new(MockLlmClient::new("test-model"));
        let mut rt = runtime(db, &message.conversation_id, cwd.path(), client.clone());
        if continuation {
            let request = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "failed-captured-goal-continuation".into(),
                rejected_tool_calls: vec![],
                attempt: 1,
            };
            rt.state = ConvState::AwaitingContinuation {
                request: request.clone(),
            };
            assert!(
                matches!(rt.execute_effect(Effect::RequestContinuation { request }).await.unwrap(), Some(Event::ContinuationFailed { error, .. }) if error.starts_with("failed to load subscription goal facts:"))
            );
        } else {
            assert!(rt
                .execute_effect(Effect::RequestLlm)
                .await
                .unwrap_err()
                .starts_with("failed to load subscription goal facts:"));
        }
        assert!(rt.llm_task_handle.is_none());
        assert!(client.recorded_requests().is_empty());
    }
}

#[tokio::test]
async fn normal_and_continuation_requests_freeze_private_facts_without_persisting_them() {
    for continuation in [false, true] {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let (message, source, _) = captured_notification(&db).await;
        let cwd = tempfile::tempdir().unwrap();
        let client = Arc::new(MockLlmClient::new("test-model"));
        let mut rt = runtime(
            db.clone(),
            &message.conversation_id,
            cwd.path(),
            client.clone(),
        );
        let effect = if continuation {
            let request = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "captured-goal-continuation".into(),
                rejected_tool_calls: vec![],
                attempt: 1,
            };
            rt.state = ConvState::AwaitingContinuation {
                request: request.clone(),
            };
            Effect::RequestContinuation { request }
        } else {
            Effect::RequestLlm
        };
        rt.execute_effect(effect).await.unwrap();
        let task = rt
            .llm_task_handle
            .take()
            .expect("provider task registered after prompt freeze");
        db.delete_conversation(&source).await.unwrap();
        sqlx::query("ALTER TABLE coordinator_watch_goal_captures RENAME TO captures_after_freeze")
            .execute(db.pool())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        let requests = client.recorded_requests();
        assert_eq!(requests.len(), 1);
        assert_private_absent(&serde_json::json!(requests[0]
            .system
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()));
        assert!(requests[0]
            .messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && rendered_text(std::slice::from_ref(message)).contains(OBJECTIVE)));
        let text = rendered_text(&requests[0].messages);
        for secret in [OBJECTIVE, REASON, GOAL_ID] {
            assert!(text.contains(secret), "missing saved fact: {secret}");
        }
        assert_private_absent(
            &serde_json::to_value(db.get_messages(&message.conversation_id).await.unwrap())
                .unwrap(),
        );
        let stored = db.get_messages(&message.conversation_id).await.unwrap();
        assert_eq!(
            serde_json::to_value(&stored[0].content).unwrap(),
            serde_json::to_value(&message.content).unwrap()
        );
    }
}
