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
                "goal-api",
                std::env::temp_dir().to_str().unwrap(),
                true,
                None,
                Some("test-model"),
            )
            .await
            .unwrap();
        let owner = conversation.product_conversation_id.clone();
        state.runtime.db().set_auto_continue_on_context_exhaustion(
            &owner,
            phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
        ).await.unwrap();
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
async fn owner_goal_create_and_replace_require_explicit_auto_continue_without_partial_receipts() {
    use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion as Preference;

    let f = Fixture::new().await;
    let db = f.state.runtime.db();
    db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Disabled)
        .await
        .unwrap();
    let before = f.snapshot().await;
    let (status, refusal) = f
        .call("POST", "", create_request(0, "disabled create"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
    assert_eq!(refusal["error_type"], "goal_requires_auto_continue");
    assert!(refusal["error"]
        .as_str()
        .unwrap()
        .contains("enable automatic continuation"));
    assert_eq!(f.snapshot().await, before);
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_user_requests")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(receipts, 0);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&f.owner)
            .await
            .unwrap(),
        Preference::Disabled
    );

    db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Enabled)
        .await
        .unwrap();
    let command = create_request(0, "enabled create");
    let accepted = f.call("POST", "", command.clone()).await;
    assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
    db.set_auto_continue_on_context_exhaustion(&f.owner, Preference::Disabled)
        .await
        .unwrap();
    let before_replacement = f.snapshot().await;
    assert_eq!(f.call("POST", "", command).await, accepted);
    let (status, refusal) = f
        .call("POST", "", create_request(1, "disabled replacement"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
    assert_eq!(refusal["error_type"], "goal_requires_auto_continue");
    assert_eq!(f.snapshot().await, before_replacement);
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_user_requests")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(receipts, 1);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&f.owner)
            .await
            .unwrap(),
        Preference::Disabled
    );
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

#[tokio::test]
async fn unclassified_goal_write_latches_fatal_and_blocks_next_admission() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot().await;
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone());
    assert!(service
        .established_receipt(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified,)
        .is_err());
    assert!(fixture
        .state
        .runtime
        .acquire_local_authority_pass()
        .is_err());
    assert_eq!(
        fixture
            .call("POST", "", create_request(0, "must remain blocked"))
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    assert_eq!(fixture.snapshot().await, before);
}

#[tokio::test]
async fn goal_reads_reject_after_unclassified_write_closes_local_authority() {
    let fixture = Fixture::new().await;
    let (status, _) = fixture
        .call(
            "POST",
            "",
            create_request(0, "history before fatal closure"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for suffix in ["", "/history"] {
        let (status, _) = fixture.call("GET", suffix, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
    }
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone());
    assert!(matches!(
        service.established_receipt(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified
        ),
        Err(crate::api::handlers::AppError::Internal(_))
    ));
    assert!(fixture.state.runtime.fatal_local_authority_is_latched());
    for suffix in ["", "/history"] {
        let (status, body) = fixture.call("GET", suffix, Value::Null).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.to_string().contains("history before fatal closure"));
    }
    assert_eq!(
        fixture
            .state
            .runtime
            .db()
            .conversation_goal_history(&fixture.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn classified_goal_domain_error_preserves_next_admission() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot().await;
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone());
    assert!(service
        .established_receipt(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
                phoenix_db::conversation_goals::GoalDbError::RetryConflict,
            )),
        )
        .is_err());
    assert!(fixture.state.runtime.acquire_local_authority_pass().is_ok());
    assert_eq!(fixture.snapshot().await, before);
    assert_eq!(
        fixture
            .call(
                "POST",
                "",
                create_request(0, "allowed after domain rejection")
            )
            .await
            .0,
        StatusCode::OK,
    );
}

async fn assert_inflight_goal_read_rejects_after_snapshot(history: bool) {
    let fixture = Fixture::new().await;
    fixture.create().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone())
        .with_read_publication_barrier(barrier.clone());
    let read = async {
        if history {
            service
                .history(&fixture.owner, GoalVersion::INITIAL)
                .await
                .map(|entries| entries.len())
        } else {
            service
                .current(&fixture.owner)
                .await
                .map(|entry| usize::from(entry.is_some()))
        }
    };
    let close = async {
        barrier.wait().await;
        let writer = super::service::GoalCommandService::new(fixture.state.runtime.clone());
        assert!(matches!(
            writer.established_receipt(
                phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified
            ),
            Err(crate::api::handlers::AppError::Internal(_))
        ));
        assert!(fixture.state.runtime.fatal_local_authority_is_latched());
        barrier.wait().await;
    };
    let (result, ()) = tokio::join!(read, close);
    assert!(matches!(
        result,
        Err(crate::api::handlers::AppError::Internal(_))
    ));
    assert_eq!(
        fixture
            .state
            .runtime
            .db()
            .conversation_goal_history(&fixture.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn current_rechecks_authority_after_inflight_snapshot_before_publication() {
    assert_inflight_goal_read_rejects_after_snapshot(false).await;
}

#[tokio::test]
async fn history_rechecks_authority_after_inflight_snapshot_before_publication() {
    assert_inflight_goal_read_rejects_after_snapshot(true).await;
}

async fn command_across_real_rollover(
    fixture: &Fixture,
    request: &Value,
) -> Result<
    phoenix_db::conversation_goals::user_commands::GoalUserReceipt,
    crate::api::handlers::AppError,
> {
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone())
        .with_receipt_lookup_barrier(barrier.clone());
    let command = service.command(
        fixture.owner.clone(),
        uuid::Uuid::parse_str(request["request_id"].as_str().unwrap()).unwrap(),
        GoalVersion::INITIAL,
        phoenix_db::conversation_goals::user_commands::GoalUserChange::Create {
            objective: phoenix_core::domain::conversation_goal::GoalText::parse(
                request["objective"].as_str().unwrap(),
            )
            .unwrap(),
        },
        crate::api::auth::OwnerAuthenticated,
    );
    let rollover = async {
        barrier.wait().await;
        let db = fixture.state.runtime.db();
        db.update_conversation_state(
            &fixture.transcript,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "roll over between owner sample and receipt lookup".into(),
            },
        )
        .await
        .unwrap();
        let phoenix_db::ContinueOutcome::Created(successor) =
            db.continue_conversation(&fixture.transcript).await.unwrap()
        else {
            panic!("continuation must create a successor");
        };
        assert_ne!(successor.id, fixture.transcript);
        assert_eq!(successor.product_conversation_id, fixture.owner);
        barrier.wait().await;
    };
    let (result, ()) = tokio::join!(command, rollover);
    result
}

#[tokio::test]
async fn accepted_retry_rollover_before_receipt_lookup_preserves_receipt_over_busy_gate() {
    let fixture = Fixture::new().await;
    let request = fixture.create().await;
    let (status, expected) = fixture.call("POST", "", request.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let _product_guard = fixture
        .state
        .runtime
        .try_lock_product_message_admission(fixture.owner.as_str())
        .await
        .unwrap();
    let receipt = command_across_real_rollover(&fixture, &request)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(receipt).unwrap(), expected);
    assert_eq!(
        fixture
            .state
            .runtime
            .db()
            .conversation_goal_history(&fixture.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn new_command_rollover_before_receipt_lookup_commits_on_live_successor() {
    let fixture = Fixture::new().await;
    let request = create_request(0, "new command after rollover");
    let receipt = command_across_real_rollover(&fixture, &request)
        .await
        .unwrap();
    assert!(matches!(
        &receipt.result,
        phoenix_db::conversation_goals::user_commands::GoalUserResult::Accepted { .. }
    ));
    let (status, retry) = fixture.call("POST", "", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retry, serde_json::to_value(receipt).unwrap());
    assert_eq!(
        fixture
            .state
            .runtime
            .db()
            .conversation_goal_history(&fixture.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn new_command_rollover_before_receipt_lookup_retains_durable_busy_refusal() {
    let fixture = Fixture::new().await;
    let request = create_request(0, "busy command after rollover");
    let product_guard = fixture
        .state
        .runtime
        .try_lock_product_message_admission(fixture.owner.as_str())
        .await
        .unwrap();
    let receipt = command_across_real_rollover(&fixture, &request)
        .await
        .unwrap();
    assert!(matches!(
        &receipt.result,
        phoenix_db::conversation_goals::user_commands::GoalUserResult::Rejected {
            reason: phoenix_db::conversation_goals::user_commands::GoalUserRefusal::Busy,
            ..
        }
    ));
    drop(product_guard);
    let (status, retry) = fixture.call("POST", "", request).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(retry, serde_json::to_value(receipt).unwrap());
    assert!(fixture
        .state
        .runtime
        .db()
        .conversation_goal_history(&fixture.owner, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
}

#[derive(Clone, Copy)]
enum CommandPublicationPath {
    Retained,
    Accepted,
    ProductBusy,
    TranscriptBusy,
}

async fn assert_publication_receipt(
    fixture: &Fixture,
    command: &phoenix_db::conversation_goals::user_commands::GoalUserCommand,
    path: CommandPublicationPath,
    before: Option<&phoenix_db::conversation_goals::user_commands::GoalUserReceipt>,
) {
    use phoenix_db::conversation_goals::user_commands::{GoalUserRefusal, GoalUserResult};
    let retained = fixture
        .state
        .runtime
        .db()
        .retained_user_goal_command(command, &fixture.transcript)
        .await
        .unwrap()
        .expect("command must retain its result before publication");
    match path {
        CommandPublicationPath::Retained => assert_eq!(Some(&retained), before),
        CommandPublicationPath::Accepted => {
            assert!(matches!(retained.result, GoalUserResult::Accepted { .. }));
        }
        CommandPublicationPath::ProductBusy | CommandPublicationPath::TranscriptBusy => {
            assert!(matches!(
                retained.result,
                GoalUserResult::Rejected {
                    reason: GoalUserRefusal::Busy,
                    ..
                }
            ));
        }
    }
}

async fn assert_command_rechecks_authority(path: CommandPublicationPath) {
    use phoenix_db::conversation_goals::user_commands::{GoalUserChange, GoalUserCommand};
    let fixture = Fixture::new().await;
    let request = if matches!(path, CommandPublicationPath::Retained) {
        fixture.create().await
    } else {
        create_request(0, "receipt before authority closure")
    };
    let instance = fixture.state.runtime.db().instance_id().await.unwrap();
    let command = GoalUserCommand {
        owner: fixture.owner.clone(),
        principal_id: uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            instance.to_string().as_bytes(),
        ),
        request_id: uuid::Uuid::parse_str(request["request_id"].as_str().unwrap()).unwrap(),
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: phoenix_core::domain::conversation_goal::GoalText::parse(
                request["objective"].as_str().unwrap(),
            )
            .unwrap(),
        },
    };
    let _product = if matches!(path, CommandPublicationPath::ProductBusy) {
        Some(
            fixture
                .state
                .runtime
                .try_lock_product_message_admission(fixture.owner.as_str())
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let _transcript = if matches!(path, CommandPublicationPath::TranscriptBusy) {
        Some(
            fixture
                .state
                .runtime
                .try_lock_message_acceptance(&fixture.transcript)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let before = fixture
        .state
        .runtime
        .db()
        .retained_user_goal_command(&command, &fixture.transcript)
        .await
        .unwrap();
    assert_eq!(
        before.is_some(),
        matches!(path, CommandPublicationPath::Retained)
    );
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone())
        .with_command_publication_barrier(barrier.clone());
    let pending = service.command(
        fixture.owner.clone(),
        command.request_id,
        GoalVersion::INITIAL,
        GoalUserChange::Create {
            objective: phoenix_core::domain::conversation_goal::GoalText::parse(
                request["objective"].as_str().unwrap(),
            )
            .unwrap(),
        },
        crate::api::auth::OwnerAuthenticated,
    );
    let close = async {
        barrier.wait().await;
        assert_publication_receipt(&fixture, &command, path, before.as_ref()).await;
        let writer = super::service::GoalCommandService::new(fixture.state.runtime.clone());
        assert!(matches!(
            writer.established_receipt(
                phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified
            ),
            Err(crate::api::handlers::AppError::Internal(_))
        ));
        barrier.wait().await;
    };
    let (result, ()) = tokio::join!(pending, close);
    assert!(matches!(
        result,
        Err(crate::api::handlers::AppError::Internal(_))
    ));
    assert_publication_receipt(&fixture, &command, path, before.as_ref()).await;
}

#[tokio::test]
async fn retained_command_rechecks_authority_after_lookup_before_publication() {
    assert_command_rechecks_authority(CommandPublicationPath::Retained).await;
}

#[tokio::test]
async fn accepted_command_rechecks_authority_after_commit_before_publication() {
    assert_command_rechecks_authority(CommandPublicationPath::Accepted).await;
}

#[tokio::test]
async fn product_busy_command_rechecks_authority_after_receipt_before_publication() {
    assert_command_rechecks_authority(CommandPublicationPath::ProductBusy).await;
}

#[tokio::test]
async fn transcript_busy_command_rechecks_authority_after_receipt_before_publication() {
    assert_command_rechecks_authority(CommandPublicationPath::TranscriptBusy).await;
}
