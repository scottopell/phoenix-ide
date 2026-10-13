use phoenix_core::domain::instance_identity::InstanceId;

use super::TestInstance;

#[tokio::test]
async fn real_router_rejects_unpaired_https_query_without_touching_production() {
    let receiver = TestInstance::start().await;
    let destination = receiver.state.db.instance_id().await.unwrap();
    let request = serde_json::json!({
        "caller_instance_id": InstanceId::new(),
        "destination_instance_id": destination,
        "sql": "SELECT 1",
    });
    let response = receiver
        .client
        .post(receiver.endpoint("api/federation/peer/query-database"))
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(receiver.state.db.instance_id().await.unwrap(), destination);
    receiver.stop().await;
}
