use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use phoenix_core::domain::{
    conversation_goal::{GoalHistoryEntry, GoalVersion},
    product_conversation::ProductConversationId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::api::handlers::{create_router, hard_delete_cascade_tests::make_test_state};

const PASSWORD: &str = "disposable-goal-owner";

struct Fixture {
    state: AppState,
    router: Router,
    owner: ProductConversationId,
    transcript: String,
}

impl Fixture {
    async fn new() -> Self {
        let mut state = make_test_state().await;
        state.password = Some(
            phoenix_core::domain::instance_identity::OwnerPassword::parse(PASSWORD.to_owned())
                .unwrap(),
        );
        let conversation = state
            .runtime
            .db()
            .create_conversation(
                "goal-api",
                std::env::temp_dir().to_str().unwrap(),
                "test-model",
                None,
                None,
                None,
            )
            .await
            .unwrap();
        let owner = state
            .runtime
            .db()
            .product_conversation_for_transcript(&conversation.id)
            .await
            .unwrap()
            .id;
        Self {
            router: create_router(state.clone()),
            state,
            owner,
            transcript: conversation.id,
        }
    }

    fn path(&self, suffix: &str) -> String {
        format!("/api/product-conversations/{}/goal{suffix}", self.owner)
    }

    async fn call(&self, method: &str, suffix: &str, body: Value) -> (StatusCode, Value) {
        request(self.router.clone(), method, &self.path(suffix), body, true).await
    }

    async fn current(&self) -> GoalHistoryEntry {
        let (status, body) = self.call("GET", "", Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }

    async fn snapshot(&self) -> Value {
        let db = self.state.runtime.db();
        json!({
            "goal": db.conversation_goal(&self.owner).await.unwrap(),
            "history": db.conversation_goal_history(&self.owner, GoalVersion::INITIAL).await.unwrap(),
            "watch": format!("{:?}", db.pending_coordinator_watch_events(64).await.unwrap()),
            "messages": db.get_messages(&self.transcript).await.unwrap(),
            "state": db.get_conversation(&self.transcript).await.unwrap().state,
        })
    }

    async fn create(&self) -> Value {
        let command = create_request(0, "Ship the approved objective");
        let (status, body) = self.call("POST", "", command.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        command
    }
}

async fn request(
    router: Router,
    method: &str,
    path: &str,
    body: Value,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if authenticated {
        builder = builder.header("authorization", format!("Bearer {PASSWORD}"));
    }
    let response = router
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    let body =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    (status, body)
}

fn create_request(version: i64, objective: &str) -> Value {
    json!({"request_id": uuid::Uuid::new_v4(), "expected_version": version, "objective": objective})
}

fn state_request(current: &GoalHistoryEntry, disposition: &str) -> Value {
    let reason = || GoalText::parse("user-requested metadata change").unwrap();
    let disposition = match disposition {
        "Underway" => GoalDisposition::Underway,
        "Blocked" => GoalDisposition::Blocked(reason()),
        "Suspended" => GoalDisposition::Suspended(reason()),
        _ => panic!("test disposition must be explicit"),
    };
    json!({"request_id": uuid::Uuid::new_v4(), "expected_version": current.snapshot().version(),
        "goal_id": current.snapshot().goal_id(), "disposition": disposition})
}

#[tokio::test]
async fn goal_routes_require_owner_authentication_and_do_not_mount_peer_routes() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot().await;
    for (method, suffix, body) in [
        ("GET", "", Value::Null),
        ("GET", "/history", Value::Null),
        ("POST", "", create_request(0, "unauthenticated")),
        ("POST", "/state", json!({})),
    ] {
        let (status, _) = request(
            fixture.router.clone(),
            method,
            &fixture.path(suffix),
            body,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let isolated = routes()
        .with_state(fixture.state.clone())
        .layer(axum::Extension(crate::api::auth::PeerAuthenticated {
            caller_instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
            caller_display_name: "authenticated peer, not owner".into(),
        }));
    let (status, _) = request(
        isolated.clone(),
        "POST",
        &fixture.path(""),
        create_request(0, "no owner marker"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = request(isolated, "GET", "/api/federation/goals", Value::Null, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.snapshot().await, before);
}

#[tokio::test]
async fn create_read_and_history_use_the_product_owner_and_stable_principal() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.call("GET", "", Value::Null).await,
        (StatusCode::OK, Value::Null)
    );
    fixture.create().await;
    let current = fixture.current().await;
    assert_eq!(current.snapshot().version().get(), 1);
    let (status, history) = fixture.call("GET", "/history?after=0", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<Vec<GoalHistoryEntry>>(history).unwrap(),
        vec![current.clone()]
    );
    assert_eq!(
        fixture.call("GET", "/history?after=1", Value::Null).await,
        (StatusCode::OK, json!([]))
    );
    let (status, _) = fixture
        .call("POST", "/state", state_request(&current, "Blocked"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let principals: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT principal_id) FROM conversation_goal_user_requests WHERE owner_id=?",
    )
    .bind(fixture.owner.as_str())
    .fetch_one(fixture.state.runtime.db().pool())
    .await
    .unwrap();
    assert_eq!(principals, 1);
}

#[tokio::test]
async fn product_and_transcript_contention_retain_refusals_without_goal_history_or_watch_changes() {
    for product_gate in [true, false] {
        let fixture = Fixture::new().await;
        let runtime = &fixture.state.runtime;
        let guard = if product_gate {
            runtime
                .try_lock_product_message_admission(fixture.owner.as_str())
                .await
                .unwrap()
        } else {
            runtime
                .try_lock_message_acceptance(&fixture.transcript)
                .await
                .unwrap()
        };
        let command = create_request(0, "busy command");
        let before = fixture.snapshot().await;
        let (status, refusal) = fixture.call("POST", "", command.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
        assert_eq!(refusal["result"]["reason"], "busy");
        assert_eq!(fixture.snapshot().await, before);
        drop(guard);
        assert_eq!(
            fixture.call("POST", "", command).await,
            (StatusCode::CONFLICT, refusal)
        );
        assert_eq!(fixture.snapshot().await, before);
        fixture.create().await;
    }
}

#[tokio::test]
async fn exact_accepted_retries_are_historical_reads_even_while_admission_is_busy() {
    let fixture = Fixture::new().await;
    let command = fixture.create().await;
    let first = fixture.call("POST", "", command.clone()).await;
    let current = fixture.current().await;
    assert_eq!(
        fixture
            .call("POST", "/state", state_request(&current, "Blocked"))
            .await
            .0,
        StatusCode::OK
    );
    let before = fixture.snapshot().await;
    let _guard = fixture
        .state
        .runtime
        .try_lock_product_message_admission(fixture.owner.as_str())
        .await
        .unwrap();
    assert_eq!(fixture.call("POST", "", command.clone()).await, first);
    let mut conflict = command;
    conflict["objective"] = json!("different request identity");
    assert_eq!(
        fixture.call("POST", "", conflict).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(fixture.snapshot().await, before);
}

#[tokio::test]
async fn stale_and_invalid_commands_never_change_goal_history_or_watch_evidence() {
    let fixture = Fixture::new().await;
    fixture.create().await;
    let current = fixture.current().await;
    let before = fixture.snapshot().await;
    let mut stale = state_request(&current, "Blocked");
    stale["expected_version"] = json!(0);
    assert_eq!(
        fixture.call("POST", "/state", stale).await.0,
        StatusCode::CONFLICT
    );
    let mut edit = state_request(&current, "Blocked");
    edit["objective"] = json!("objective edits are outside this API");
    assert_eq!(
        fixture.call("POST", "/state", edit).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for invalid in [
        create_request(-1, "negative version"),
        create_request(1, "   "),
    ] {
        assert_eq!(
            fixture.call("POST", "", invalid).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(fixture.snapshot().await, before);
}

#[tokio::test]
async fn blocked_underway_is_metadata_only_and_suspended_reactivation_requires_new_goal_identity() {
    let fixture = Fixture::new().await;
    fixture.create().await;
    let first_id = fixture.current().await.snapshot().goal_id().clone();
    let initial_messages = json!(fixture
        .state
        .runtime
        .db()
        .get_messages(&fixture.transcript)
        .await
        .unwrap());
    let initial_state = fixture
        .state
        .runtime
        .db()
        .get_conversation(&fixture.transcript)
        .await
        .unwrap()
        .state;
    for disposition in ["Blocked", "Underway", "Suspended"] {
        let current = fixture.current().await;
        assert_eq!(
            fixture
                .call("POST", "/state", state_request(&current, disposition))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(fixture.current().await.snapshot().goal_id(), &first_id);
        assert_eq!(
            json!(fixture
                .state
                .runtime
                .db()
                .get_messages(&fixture.transcript)
                .await
                .unwrap()),
            initial_messages
        );
        assert_eq!(
            fixture
                .state
                .runtime
                .db()
                .get_conversation(&fixture.transcript)
                .await
                .unwrap()
                .state,
            initial_state
        );
    }
    let suspended = fixture.current().await;
    let before = fixture.snapshot().await;
    assert_eq!(
        fixture
            .call("POST", "/state", state_request(&suspended, "Underway"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(fixture.snapshot().await, before);
    assert_eq!(
        fixture
            .call(
                "POST",
                "",
                create_request(
                    suspended.snapshot().version().get(),
                    "Replacement objective"
                )
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_ne!(fixture.current().await.snapshot().goal_id(), &first_id);
}

#[tokio::test]
async fn nonsettled_runtime_refuses_commands_and_unknown_product_targets_do_not_create_goals() {
    let fixture = Fixture::new().await;
    fixture
        .state
        .runtime
        .db()
        .update_conversation_state(
            &fixture.transcript,
            &phoenix_core::domain::sm_state::ConvState::LlmRequesting { attempt: 1 },
        )
        .await
        .unwrap();
    let before = fixture.snapshot().await;
    assert_eq!(
        fixture
            .call("POST", "", create_request(0, "not settled"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let unknown = ProductConversationId::new();
    let (status, _) = request(
        fixture.router.clone(),
        "POST",
        &format!("/api/product-conversations/{unknown}/goal"),
        create_request(0, "unknown"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.snapshot().await, before);
}
