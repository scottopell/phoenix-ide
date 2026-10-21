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
