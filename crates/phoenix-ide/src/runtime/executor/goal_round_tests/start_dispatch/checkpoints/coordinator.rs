use super::*;

#[tokio::test]
async fn conversation_goal_coordinator_role_cannot_consume_an_ordinary_start_receipt() {
    let (_directory, db, _receipt, mut rt, client) = adopted().await;
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    rt.context.conversation_id = coordinator.id;
    rt.context.is_coordinator = true;
    let before = canonical_counts(&db).await;
    assert!(rt.execute_effect(Effect::RequestLlm).await.is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(canonical_counts(&db).await, before);
    assert!(rt.llm_task_handle.is_none());
}
