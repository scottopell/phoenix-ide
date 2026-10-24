use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    middleware,
};
use phoenix_core::domain::{
    close::TranscriptConversationId,
    conversation_goal::{GoalActor, GoalState, GoalUserCommandKind},
    instance_identity::OwnerPassword,
};
use phoenix_db::{conversation_goals::control::GoalControlInstallation, Database};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use crate::api::{handlers::hard_delete_cascade_tests::make_test_state, AppState};

async fn fixture(installed: bool) -> (AppState, ProductConversationId) {
    let mut state = make_test_state().await;
    let db = if installed {
        state
            .db
            .clone()
            .install_goal_controls_for_tests()
            .await
            .unwrap()
    } else {
        state.db.clone()
    };
    let conversation = db
        .create_conversation("goal-http-root", "goal HTTP", "/tmp", true, None, None)
        .await
        .unwrap();
    state.runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        state.llm_registry.clone(),
        state.platform.clone(),
        state.mcp_manager.clone(),
        None,
    ));
    state.db = db;
    state.password = Some(OwnerPassword::parse("goal-owner-secret".to_owned()).unwrap());
    (state, conversation.product_conversation_id)
}

fn authenticated_router(state: &AppState, installed: bool) -> Router {
    let router = if installed {
        private_install::<AppState>(state.runtime.clone()).unwrap()
    } else {
        routes::<AppState>()
    };
    router
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::api::auth::auth_middleware,
        ))
        .with_state(state.clone())
}

fn request(method: Method, path: &str, body: Option<&Value>, owner: bool) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if owner {
        builder = builder.header("authorization", "Bearer goal-owner-secret");
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn create_body(id: Uuid, version: i64) -> Value {
    json!({"request_id":id,"expected_version":version,"objective":"exact objective"})
}

fn accepted(body: &Value) -> GoalHistoryEntry {
    assert_eq!(body["result"]["outcome"], "accepted");
    serde_json::from_value(body["result"]["entry"].clone()).unwrap()
}

async fn counts(db: &Database) -> Vec<(String, i64)> {
    let mut counts = Vec::new();
    for (table, query) in [
        ("messages", "SELECT count(*) FROM messages"),
        (
            "conversation_goal_ids",
            "SELECT count(*) FROM conversation_goal_ids",
        ),
        (
            "conversation_goal_controls",
            "SELECT count(*) FROM conversation_goal_controls",
        ),
        (
            "conversation_goal_history",
            "SELECT count(*) FROM conversation_goal_history",
        ),
        (
            "conversation_goal_actors",
            "SELECT count(*) FROM conversation_goal_actors",
        ),
        (
            "conversation_goal_user_requests",
            "SELECT count(*) FROM conversation_goal_user_requests",
        ),
        (
            "conversation_goal_control_requests",
            "SELECT count(*) FROM conversation_goal_control_requests",
        ),
        (
            "conversation_goal_bindings",
            "SELECT count(*) FROM conversation_goal_bindings",
        ),
        (
            "conversation_goal_control_bindings",
            "SELECT count(*) FROM conversation_goal_control_bindings",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(db.pool())
            .await
            .unwrap();
        counts.push((table.to_owned(), count));
    }
    counts
}

#[tokio::test]
async fn conversation_goal_private_install_default_off_auth_first_and_all_or_nothing() {
    let (state, owner) = fixture(false).await;
    assert_eq!(
        state.db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert!(matches!(
        private_install::<AppState>(state.runtime.clone()),
        Err(GoalCommandServiceError::Disabled)
    ));
    assert!(GoalControlService::from_installed_database(state.db.clone()).is_err());
    assert!(GoalCommandService::from_installed_runtime(state.runtime.clone()).is_err());
    let router = authenticated_router(&state, false);
    let goal = format!("/api/product-conversations/{owner}/goal");
    let paths = [
        (Method::GET, goal.clone(), None),
        (Method::GET, format!("{goal}/history?after=0"), None),
        (
            Method::GET,
            format!("/api/product-conversations/{owner}/goal-control"),
            None,
        ),
        (
            Method::POST,
            goal.clone(),
            Some(create_body(Uuid::new_v4(), 0)),
        ),
        (
            Method::POST,
            format!("{goal}/state"),
            Some(
                json!({"request_id":Uuid::new_v4(),"expected_version":1,"goal_id":"goal","disposition":{"state":"underway"}}),
            ),
        ),
        (
            Method::POST,
            format!("/api/product-conversations/{owner}/goal-control"),
            Some(
                json!({"request_id":Uuid::new_v4(),"command":"stop","expected_control_version":0}),
            ),
        ),
    ];
    let before = counts(&state.db).await;
    for (method, path, body) in paths {
        assert_eq!(
            response(
                &router,
                request(method.clone(), &path, body.as_ref(), false)
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            response(&router, request(method, &path, body.as_ref(), true))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(counts(&state.db).await, before);
    assert_eq!(
        state.db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert!(GoalCommandService::default()
        .current(OwnerAuthenticated, owner.clone())
        .await
        .is_err());
    assert!(GoalCommandService::default()
        .history(OwnerAuthenticated, owner, GoalVersion::INITIAL)
        .await
        .is_err());
}

#[tokio::test]
async fn conversation_goal_private_install_rejects_guest_peer_spoof_and_noncanonical_owner() {
    let (state, owner) = fixture(true).await;
    let subordinate = state
        .db
        .create_conversation(
            "goal-http-agent",
            "goal-worker",
            "/tmp",
            false,
            Some("goal-http-root"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(subordinate.product_conversation_id, owner);
    let coordinator = state
        .db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal");
    let before = counts(&state.db).await;
    let mut spoof = request(Method::GET, &path, None, false);
    spoof
        .headers_mut()
        .insert("x-owner-authenticated", "true".parse().unwrap());
    assert_eq!(response(&router, spoof).await.0, StatusCode::UNAUTHORIZED);
    let mut peer = request(Method::GET, &path, None, false);
    peer.headers_mut().insert(
        "authorization",
        "Bearer phx_peer_not_owner".parse().unwrap(),
    );
    assert_eq!(response(&router, peer).await.0, StatusCode::UNAUTHORIZED);
    let peer_only = private_install::<()>(state.runtime.clone())
        .unwrap()
        .layer(Extension(crate::api::auth::PeerAuthenticated {
            enrollment_id: Uuid::new_v4(),
            caller_instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
            caller_display_name: "peer".into(),
        }));
    assert_eq!(
        response(&peer_only, request(Method::GET, &path, None, false))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for key in ["principal_id", "actor", "owner", "authenticated", "goal_id"] {
        let mut body = create_body(Uuid::new_v4(), 0);
        body[key] = json!("forged identity");
        assert_eq!(
            response(&router, request(Method::POST, &path, Some(&body), true))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for reference in [
        "goal-http-root".to_owned(),
        "goal-http-agent".to_owned(),
        "foreign-owner".to_owned(),
        coordinator.product_conversation_id.to_string(),
    ] {
        let path = format!("/api/product-conversations/{reference}/goal");
        assert_eq!(
            response(&router, request(Method::GET, &path, None, true))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            response(
                &router,
                request(
                    Method::POST,
                    &path,
                    Some(&create_body(Uuid::new_v4(), 0)),
                    true
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
async fn conversation_goal_private_install_host_attribution_exact_replay_stop_and_factual_reason() {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal");
    let control_path = format!("/api/product-conversations/{owner}/goal-control");
    let stop = json!({"request_id":Uuid::new_v4(),"command":"stop","expected_control_version":0});
    let stopped = response(
        &router,
        request(Method::POST, &control_path, Some(&stop), true),
    )
    .await;
    assert_eq!(stopped.0, StatusCode::OK);
    assert_eq!(
        response(
            &router,
            request(Method::POST, &control_path, Some(&stop), true)
        )
        .await,
        stopped
    );
    let id = Uuid::new_v4();
    let body = create_body(id, 0);
    let created = response(&router, request(Method::POST, &path, Some(&body), true)).await;
    assert_eq!(created.0, StatusCode::OK);
    assert_eq!(
        response(&router, request(Method::POST, &path, Some(&body), true)).await,
        created
    );
    let entry = accepted(&created.1);
    let principal = Uuid::parse_str(&state.db.instance_id().await.unwrap().to_string()).unwrap();
    assert!(
        matches!(&entry.mutation.attribution.requested_by, GoalActor::UserControl { principal_id, request_id, command:GoalUserCommandKind::Create } if *principal_id==principal && *request_id==id)
    );
    assert_eq!(
        entry.mutation.attribution.requested_by,
        entry.mutation.attribution.applied_by
    );
    let control_before: (i64, i64) = sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(control_before, (1, 1));
    let blocked = json!({"request_id":Uuid::new_v4(),"expected_version":1,"goal_id":entry.snapshot.goal_id,"disposition":{"state":"blocked","reason":"need credentials"}});
    let result = response(
        &router,
        request(Method::POST, &format!("{path}/state"), Some(&blocked), true),
    )
    .await;
    assert_eq!(result.0, StatusCode::OK);
    let blocked_entry = accepted(&result.1);
    assert!(
        matches!(&blocked_entry.snapshot.state, GoalState::Blocked(reason) if reason.as_str()=="need credentials")
    );
    let control_after: (i64, i64) = sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(state.db.pool())
    .await
    .unwrap();
    assert_eq!(control_after, control_before);
    let before = counts(&state.db).await;
    let (status, fact) = response(&router, request(Method::GET, &path, None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(fact).unwrap(),
        blocked_entry
    );
    let (status, history) = response(
        &router,
        request(Method::GET, &format!("{path}/history?after=0"), None, true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let history: Vec<GoalHistoryEntry> = serde_json::from_value(history).unwrap();
    assert_eq!(history, [entry, blocked_entry]);
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_private_install_continuation_live_close_and_history_read_write_fences() {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal");
    let body = create_body(Uuid::new_v4(), 0);
    let created = response(&router, request(Method::POST, &path, Some(&body), true)).await;
    assert_eq!(created.0, StatusCode::OK);
    state
        .db
        .add_message(
            "goal-http-handoff",
            "goal-http-root",
            &phoenix_core::domain::db_schema::MessageContent::continuation("continued objective"),
            None,
            None,
        )
        .await
        .unwrap();
    state
        .db
        .update_conversation_state(
            "goal-http-root",
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "continued objective".into(),
            },
        )
        .await
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = state
        .db
        .continue_conversation("goal-http-root")
        .await
        .unwrap()
    else {
        panic!("expected genuine continuation")
    };
    assert_eq!(successor.product_conversation_id, owner);
    let before = counts(&state.db).await;
    let entry = accepted(&created.1);
    let (status, fact) = response(&router, request(Method::GET, &path, None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(fact).unwrap(),
        entry
    );
    assert_eq!(
        response(&router, request(Method::POST, &path, Some(&body), true)).await,
        created
    );
    assert_eq!(counts(&state.db).await, before);
    let blocked = json!({"request_id":Uuid::new_v4(),"expected_version":1,"goal_id":entry.snapshot.goal_id,"disposition":{"state":"blocked","reason":"latest owner admission"}});
    let blocked_result = response(
        &router,
        request(Method::POST, &format!("{path}/state"), Some(&blocked), true),
    )
    .await;
    assert_eq!(blocked_result.0, StatusCode::OK);
    let blocked_entry = accepted(&blocked_result.1);
    state
        .db
        .begin_direct_close_foundation(
            &owner,
            &TranscriptConversationId::parse(successor.id.clone()).unwrap(),
            "goal-http-close",
        )
        .await
        .unwrap();
    let before = counts(&state.db).await;
    for (method, suffix, body) in [
        (Method::POST, String::new(), create_body(Uuid::new_v4(), 2)),
        (
            Method::POST,
            "/state".to_owned(),
            json!({"request_id":Uuid::new_v4(),"expected_version":2,"goal_id":blocked_entry.snapshot.goal_id,"disposition":{"state":"complete","reason":"must not bypass Close"}}),
        ),
    ] {
        assert_eq!(
            response(
                &router,
                request(method, &format!("{path}{suffix}"), Some(&body), true)
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let stop = json!({"request_id":Uuid::new_v4(),"command":"stop","expected_control_version":0});
    assert_eq!(
        response(
            &router,
            request(
                Method::POST,
                &format!("/api/product-conversations/{owner}/goal-control"),
                Some(&stop),
                true
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, fact) = response(&router, request(Method::GET, &path, None, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(fact).unwrap(),
        blocked_entry
    );
    let (status, history) = response(
        &router,
        request(Method::GET, &format!("{path}/history?after=1"), None, true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_value::<Vec<GoalHistoryEntry>>(history).unwrap(),
        [blocked_entry]
    );
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
async fn conversation_goal_private_install_settled_gates_and_empty_read_do_not_materialize_goals() {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal");
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, Value::Null)
    );
    assert_eq!(counts(&state.db).await, before);
    let busy_body = create_body(Uuid::new_v4(), 0);
    let refusal;
    {
        let _product = state
            .runtime
            .try_lock_product_message_admission(owner.as_str())
            .await
            .unwrap();
        refusal = response(
            &router,
            request(Method::POST, &path, Some(&busy_body), true),
        )
        .await;
        assert_eq!(refusal.0, StatusCode::CONFLICT);
        assert_eq!(refusal.1["result"]["reason"], "busy");
    }
    assert_eq!(
        response(
            &router,
            request(Method::POST, &path, Some(&busy_body), true)
        )
        .await,
        refusal
    );
    assert!(state.db.conversation_goal(&owner).await.unwrap().is_none());
    let created = response(
        &router,
        request(
            Method::POST,
            &path,
            Some(&create_body(Uuid::new_v4(), 0)),
            true,
        ),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    let entry = accepted(&created.1);
    let blocked = json!({"request_id":Uuid::new_v4(),"expected_version":1,"goal_id":entry.snapshot.goal_id,"disposition":{"state":"blocked","reason":"not admitted while busy"}});
    let state_refusal;
    {
        let _acceptance = state
            .runtime
            .try_lock_message_acceptance("goal-http-root")
            .await
            .unwrap();
        state_refusal = response(
            &router,
            request(Method::POST, &format!("{path}/state"), Some(&blocked), true),
        )
        .await;
        assert_eq!(state_refusal.0, StatusCode::CONFLICT);
        assert_eq!(state_refusal.1["result"]["reason"], "busy");
    }
    assert_eq!(
        response(
            &router,
            request(Method::POST, &format!("{path}/state"), Some(&blocked), true)
        )
        .await,
        state_refusal
    );
    let (status, fact) = response(&router, request(Method::GET, &path, None, true)).await;
    assert_eq!(status, StatusCode::OK);
    let fact: GoalHistoryEntry = serde_json::from_value(fact).unwrap();
    assert_eq!(fact, entry);
    assert_eq!(fact.snapshot.state, GoalState::Underway);
}

#[tokio::test]
async fn conversation_goal_private_install_history_is_bounded_typed_ordered_and_read_only() {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal");
    for version in 0..102 {
        assert_eq!(
            response(
                &router,
                request(
                    Method::POST,
                    &path,
                    Some(&create_body(Uuid::new_v4(), version)),
                    true
                )
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let before = counts(&state.db).await;
    let (status, page) = response(
        &router,
        request(Method::GET, &format!("{path}/history?after=0"), None, true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page: Vec<GoalHistoryEntry> = serde_json::from_value(page).unwrap();
    assert_eq!(page.len(), 100);
    assert_eq!(
        page.iter()
            .map(|entry| entry.snapshot.version.get())
            .collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
    let (status, page) = response(
        &router,
        request(
            Method::GET,
            &format!("{path}/history?after=100"),
            None,
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page: Vec<GoalHistoryEntry> = serde_json::from_value(page).unwrap();
    assert_eq!(
        page.iter()
            .map(|entry| entry.snapshot.version.get())
            .collect::<Vec<_>>(),
        [101, 102]
    );
    for query in [
        "after=-1",
        "after=9223372036854775808",
        "after=word",
        "after=0&actor=forged",
    ] {
        assert_eq!(
            response(
                &router,
                request(Method::GET, &format!("{path}/history?{query}"), None, true)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
async fn conversation_goal_control_factual_read_requires_real_owner_and_canonical_ordinary_identity(
) {
    let (state, owner) = fixture(true).await;
    let subordinate = state
        .db
        .create_conversation(
            "goal-http-agent",
            "goal-worker",
            "/tmp",
            false,
            Some("goal-http-root"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(subordinate.product_conversation_id, owner);
    let coordinator = state
        .db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let router = authenticated_router(&state, true);
    let peer_only = private_install::<()>(state.runtime.clone())
        .unwrap()
        .layer(Extension(crate::api::auth::PeerAuthenticated {
            enrollment_id: Uuid::new_v4(),
            caller_instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
            caller_display_name: "peer".into(),
        }));
    let before = counts(&state.db).await;
    let control_path = format!("/api/product-conversations/{owner}/goal-control");
    assert_eq!(
        response(&router, request(Method::GET, &control_path, None, false))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        response(&peer_only, request(Method::GET, &control_path, None, false))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut spoof = request(Method::GET, &control_path, None, false);
    spoof
        .headers_mut()
        .insert("x-owner-authenticated", "true".parse().unwrap());
    assert_eq!(response(&router, spoof).await.0, StatusCode::UNAUTHORIZED);
    for reference in [
        "goal-http-root".to_owned(),
        "goal-http-agent".to_owned(),
        "foreign-owner".to_owned(),
        coordinator.product_conversation_id.to_string(),
    ] {
        let path = format!("/api/product-conversations/{reference}/goal-control");
        assert_eq!(
            response(&router, request(Method::GET, &path, None, true))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
async fn conversation_goal_control_factual_read_absent_zero_cas_stop_release_and_replay() {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal-control");
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, Value::Null)
    );
    assert_eq!(counts(&state.db).await, before);
    let stop = json!({"request_id":Uuid::new_v4(), "command":"stop", "expected_control_version":0});
    let stopped = response(&router, request(Method::POST, &path, Some(&stop), true)).await;
    assert_eq!(stopped.0, StatusCode::OK);
    assert_eq!(stopped.1["result"]["outcome"], "accepted");
    assert_eq!(
        stopped.1["result"]["snapshot"],
        json!({"version":1,"stopped":true})
    );
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, stopped.1["result"]["snapshot"].clone())
    );
    assert_eq!(counts(&state.db).await, before);
    assert_eq!(
        response(&router, request(Method::POST, &path, Some(&stop), true)).await,
        stopped
    );
    let release =
        json!({"request_id":Uuid::new_v4(), "command":"release", "expected_control_version":1});
    let released = response(&router, request(Method::POST, &path, Some(&release), true)).await;
    assert_eq!(released.0, StatusCode::OK);
    assert_eq!(
        released.1["result"]["snapshot"],
        json!({"version":2,"stopped":false})
    );
    assert_eq!(
        response(&router, request(Method::POST, &path, Some(&release), true)).await,
        released
    );
    let outdated_stop =
        json!({"request_id":Uuid::new_v4(), "command":"stop", "expected_control_version":1});
    let conflict = response(
        &router,
        request(Method::POST, &path, Some(&outdated_stop), true),
    )
    .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(conflict.1["result"]["outcome"], "rejected_version");
    assert_eq!(
        conflict.1["result"]["observed"],
        json!({"version":2,"stopped":false})
    );
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, released.1["result"]["snapshot"].clone())
    );
    assert_eq!(counts(&state.db).await, before);
}

#[tokio::test]
async fn conversation_goal_control_factual_read_survives_continuation_and_live_close_without_materialization(
) {
    let (state, owner) = fixture(true).await;
    let router = authenticated_router(&state, true);
    let path = format!("/api/product-conversations/{owner}/goal-control");
    let stop = json!({"request_id":Uuid::new_v4(), "command":"stop", "expected_control_version":0});
    let stopped = response(&router, request(Method::POST, &path, Some(&stop), true)).await;
    assert_eq!(stopped.0, StatusCode::OK);
    state
        .db
        .add_message(
            "goal-control-read-handoff",
            "goal-http-root",
            &phoenix_core::domain::db_schema::MessageContent::continuation("control facts persist"),
            None,
            None,
        )
        .await
        .unwrap();
    state
        .db
        .update_conversation_state(
            "goal-http-root",
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "control facts persist".into(),
            },
        )
        .await
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = state
        .db
        .continue_conversation("goal-http-root")
        .await
        .unwrap()
    else {
        panic!("expected real continuation")
    };
    assert_eq!(successor.product_conversation_id, owner);
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, stopped.1["result"]["snapshot"].clone())
    );
    assert_eq!(counts(&state.db).await, before);
    state
        .db
        .begin_direct_close_foundation(
            &owner,
            &TranscriptConversationId::parse(successor.id).unwrap(),
            "goal-control-read-close",
        )
        .await
        .unwrap();
    let before = counts(&state.db).await;
    assert_eq!(
        response(&router, request(Method::GET, &path, None, true)).await,
        (StatusCode::OK, stopped.1["result"]["snapshot"].clone())
    );
    let release =
        json!({"request_id":Uuid::new_v4(), "command":"release", "expected_control_version":1});
    assert_eq!(
        response(&router, request(Method::POST, &path, Some(&release), true))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(counts(&state.db).await, before);
}
