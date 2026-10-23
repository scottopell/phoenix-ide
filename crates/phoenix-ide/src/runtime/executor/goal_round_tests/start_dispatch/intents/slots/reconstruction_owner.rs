use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct HeldStartProvider(AtomicUsize);

#[async_trait]
impl phoenix_llm::LlmService for HeldStartProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::invalid_request("held Start reached provider"))
    }

    fn model_id(&self) -> &'static str {
        "claude-sonnet-5"
    }
}

fn manager(db: Database, provider: Arc<HeldStartProvider>) -> Arc<crate::runtime::RuntimeManager> {
    Arc::new(crate::runtime::RuntimeManager::new(
        db,
        Arc::new(ModelRegistry::for_test_with_sonnet(provider)),
        phoenix_core::platform::PlatformCapability::None {
            details: "Start hold fixture".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ))
}

#[tokio::test]
async fn conversation_goal_real_constructor_keeps_unmaterialized_start_held_without_provider() {
    let (_directory, db, receipt, _authority) = claimed_start().await;
    let provider = Arc::new(HeldStartProvider::default());
    let manager = manager(db.clone(), provider.clone());
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT generation,materialized,owns_conversation FROM durable_turns WHERE turn_id=?",
    )
    .bind(receipt.execution().turn_id())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let handle = manager.get_or_create(CONVERSATION).await.unwrap();
    let startup = handle.startup_llm_recovery_ack.lock().await.take().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), startup)
        .await
        .expect("startup acknowledgment liveness")
        .unwrap()
        .unwrap();
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT generation,materialized,owns_conversation FROM durable_turns WHERE turn_id=?",
    )
    .bind(receipt.execution().turn_id())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(after, before);
    let admitted: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_requests")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(admitted, 0);
}

#[tokio::test]
async fn conversation_goal_actual_allowance_terminal_failure_reopens_through_existing_manager_before_idle(
) {
    use super::super::super::checkpoints::{
        dispatch_response, guard_terminal_projection_order, receive_automatic_response,
    };
    use phoenix_core::domain::conversation_goal::GoalState;
    let (directory, db, receipt, mut rt, client) = adopted().await;
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    guard_terminal_projection_order(&db).await;
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(rt.state.clone());
    rt.state_watcher = Some(watch_tx);
    for answer in ["ordinary yield before crash", "last ordinary answer"] {
        client
            .responses
            .lock()
            .unwrap()
            .push_back(LlmResponse::non_streaming(
                vec![ContentBlock::text(answer)],
                true,
                phoenix_core::domain::llm_types::Usage::default(),
            ));
    }
    let first = dispatch_response(&mut rt).await;
    rt.process_llm_completion(first).await.unwrap();
    let final_response = receive_automatic_response(&mut rt).await;
    sqlx::query(
        "CREATE TRIGGER fail_goal_terminal_settlement BEFORE UPDATE OF terminal_kind ON durable_turns
         WHEN NEW.input_kind='goal_start' AND NEW.terminal_kind='Completed'
         BEGIN SELECT RAISE(ABORT, 'terminal owner failure after canonical commit'); END",
    ).execute(db.pool()).await.unwrap();
    assert!(rt.process_llm_completion(final_response).await.is_err());
    assert!(rt.active_direct_turn.is_some());
    assert!(rt.pending_direct_turn_terminal.is_some());
    assert!(
        rt.pending_goal_request_checkpoint.is_some(),
        "failed settlement must not hide its accounting receipt"
    );
    assert_eq!(
        rt.direct_turn_terminal_fact,
        TerminalFactDurability::Durable
    );
    assert_eq!(
        rt.recovery_disposition,
        RuntimeRecoveryDisposition::RecreateFromDatabase
    );
    assert!(
        matches!(&*watch_rx.borrow(), ConvState::LlmRequesting { .. }),
        "Idle cannot escape failed terminal settlement"
    );
    assert!(matches!(
        db.get_conversation(CONVERSATION).await.unwrap().state,
        ConvState::LlmRequesting { .. }
    ));
    let obligation = db
        .workflow_repository()
        .load_active_terminal_obligation(&phoenix_workflow::ConversationAuthority(
            CONVERSATION.into(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(obligation.turn_id, active.turn_id);
    assert_eq!(obligation.expected_generation, active.generation);
    assert_eq!(obligation.projection.state, ConvState::Idle);
    assert!(matches!(
        db.conversation_goal(&receipt.input().goal().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Exhausted(_)
    ));
    let before = ledger(&db).await;
    assert_eq!(before, (2, 2, 1, 2));
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    drop(rt);
    drop(client);
    db.pool().close().await;
    drop(db);
    let db = install(
        Database::open(directory.path().join("start.db").to_str().unwrap())
            .await
            .unwrap(),
    )
    .await;
    sqlx::query("DROP TRIGGER fail_goal_terminal_settlement")
        .execute(db.pool())
        .await
        .unwrap();
    assert_terminal_reopen_has_no_request(&db, active.turn_id.0, active.generation, before).await;
}

async fn assert_terminal_reopen_has_no_request(
    db: &Database,
    turn_id: u64,
    generation: u64,
    before: (i64, i64, i64, i64),
) {
    let provider = Arc::new(HeldStartProvider::default());
    let manager = manager(db.clone(), provider.clone());
    let handle = manager.get_or_create(CONVERSATION).await.unwrap();
    assert_eq!(*handle.state_rx.borrow(), ConvState::Idle);
    super::super::super::checkpoints::assert_completed_start_owner(db, turn_id, generation).await;
    assert_eq!(
        ledger(db).await,
        before,
        "terminal recovery never repeats or replenishes a completed request"
    );
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn conversation_goal_real_constructor_default_off_denies_adopted_recovery_without_provider() {
    let (directory, db, receipt, _old_runtime, _client) = adopted().await;
    let counts = ledger(&db).await;
    let disabled = Database::open(directory.path().join("start.db").to_str().unwrap())
        .await
        .unwrap();
    let provider = Arc::new(HeldStartProvider::default());
    let manager = manager(disabled, provider.clone());
    let Err(error) = manager.get_or_create(CONVERSATION).await else {
        panic!("default-off constructor cannot adopt Start ownership");
    };
    assert!(error.contains("reconstruction refused"), "{error}");
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    assert_eq!(ledger(&db).await, counts);
    let generation: i64 =
        sqlx::query_scalar("SELECT generation FROM durable_turns WHERE turn_id=?")
            .bind(receipt.execution().turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(generation, 0);
}
