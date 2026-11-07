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
    let caller_instance_id = phoenix_core::domain::instance_identity::InstanceId::new();
    let isolated = routes()
        .with_state(fixture.state.clone())
        .layer(axum::Extension(crate::api::auth::PeerAuthenticated {
            caller_instance_id,
            send_authority: phoenix_db::FederationSendAuthority::new(
                caller_instance_id,
                phoenix_core::domain::instance_identity::FederationCredentialVerifier::from_bearer(
                    b"authenticated-peer-not-owner",
                ),
            ),
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
async fn preparation_refusal_rechecks_authority_at_publication() {
    let fixture = Fixture::new().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone())
        .with_command_publication_barrier(barrier.clone());
    let caller = fixture.transcript.clone();
    let owner = fixture.owner.clone();
    let pending = tokio::spawn(async move {
        service
            .prepare_coordinator(
                &caller,
                &phoenix_core::domain::db_schema::SourceToolCall {
                    message_id: "unused-ordinary-caller-message".into(),
                    tool_use_id: "unused-ordinary-caller-tool".into(),
                },
                &owner,
                phoenix_db::conversation_goals::prepared::GoalPreparationIntent::Create {
                    objective: phoenix_core::domain::conversation_goal::GoalText::parse(
                        "refused ordinary caller",
                    )
                    .unwrap(),
                },
                phoenix_core::domain::sm_event::DirectTurnTurnId(0),
            )
            .await
    });
    barrier.wait().await;
    assert!(!fixture.state.runtime.fatal_local_authority_is_latched());
    fixture
        .state
        .runtime
        .signal_fatal_local_authority("preparation_publication_race_test");
    barrier.wait().await;
    assert!(
        matches!(pending.await.unwrap(), Err(AppError::Internal(message)) if message == "local authority is closed")
    );
}

#[tokio::test]
async fn synthetic_unclassified_preparation_closes_authority_before_releasing_admission() {
    let fixture = Fixture::new().await;
    let authority = super::service::CoordinatorGoalPreparationAuthorityPass::new(
        fixture
            .state
            .runtime
            .acquire_local_authority_pass()
            .unwrap(),
    );

    assert!(authority
        .complete::<()>(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified)
        .is_err());
    assert!(fixture.state.runtime.fatal_local_authority_is_latched());
    assert_eq!(
        fixture
            .state
            .runtime
            .fatal_authority_owners_at_first_close(),
        Some(1)
    );
}

#[tokio::test]
async fn synthetic_abandoned_preparation_closes_authority_before_releasing_admission() {
    let fixture = Fixture::new().await;
    let authority = super::service::CoordinatorGoalPreparationAuthorityPass::new(
        fixture
            .state
            .runtime
            .acquire_local_authority_pass()
            .unwrap(),
    );

    drop(authority);
    assert!(fixture.state.runtime.fatal_local_authority_is_latched());
    assert_eq!(
        fixture
            .state
            .runtime
            .fatal_authority_owners_at_first_close(),
        Some(1)
    );
}

#[tokio::test]
async fn synthetic_panicked_preparation_closes_authority_before_releasing_admission() {
    let fixture = Fixture::new().await;
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _authority = super::service::CoordinatorGoalPreparationAuthorityPass::new(
            fixture
                .state
                .runtime
                .acquire_local_authority_pass()
                .unwrap(),
        );
        panic!("synthetic preparation panic");
    }));

    assert!(panicked.is_err());
    assert!(fixture.state.runtime.fatal_local_authority_is_latched());
    assert_eq!(
        fixture
            .state
            .runtime
            .fatal_authority_owners_at_first_close(),
        Some(1)
    );
}

#[tokio::test]
async fn established_preparation_rejection_disarms_without_closing_authority() {
    let fixture = Fixture::new().await;
    let authority = super::service::CoordinatorGoalPreparationAuthorityPass::new(
        fixture
            .state
            .runtime
            .acquire_local_authority_pass()
            .unwrap(),
    );

    assert!(authority
        .complete::<()>(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
                phoenix_db::conversation_goals::GoalDbError::RetryConflict,
            )),
        )
        .is_err());
    assert!(!fixture.state.runtime.fatal_local_authority_is_latched());
    assert!(fixture.state.runtime.acquire_local_authority_pass().is_ok());
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

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn concurrent_remote_apply_waits_for_classification_and_replays_receipt() {
    use phoenix_core::domain::{
        conversation_goal::{
            GoalAgentSource, GoalExecution, GoalPreparationSource, GoalText, RemoteGoalAgentSource,
        },
        instance_identity::{FederationCredentialVerifier, InstanceId},
    };
    use phoenix_db::conversation_goals::prepared::{
        install_remote_goal_claim_pause, GoalPreparationIntent, RemoteGoalClaimPause,
    };

    let fixture = Fixture::new().await;
    let db = fixture.state.runtime.db();
    let caller = InstanceId::new();
    let verifier = FederationCredentialVerifier::from_bearer(b"concurrent-goal-apply");
    db.replace_federation_enrollment(caller, &"concurrent Goal peer".parse().unwrap(), &verifier)
        .await
        .unwrap();
    let authority = phoenix_db::FederationSendAuthority::new(caller, verifier);
    let source = RemoteGoalAgentSource {
        instance_id: caller,
        source: GoalAgentSource {
            product_conversation_id: ProductConversationId::new(),
            transcript_id: GoalText::parse(uuid::Uuid::new_v4().to_string()).unwrap(),
            execution: GoalExecution::parse(1, 1).unwrap(),
            message_id: GoalText::parse("concurrent-goal-message").unwrap(),
            tool_use_id: GoalText::parse("concurrent-goal-tool").unwrap(),
        },
    };
    let action = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &fixture.owner,
            GoalPreparationIntent::Create {
                objective: GoalText::parse("exactly once remote objective").unwrap(),
            },
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(action.source, GoalPreparationSource::Remote(source.clone()));

    let pause = RemoteGoalClaimPause::new();
    install_remote_goal_claim_pause(action.reference, pause.clone());
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone());
    let first_service = service.clone();
    let first_authority = authority.clone();
    let reference = action.reference;
    let first = tokio::spawn(async move {
        first_service
            .apply_remote(&first_authority, reference)
            .await
    });
    pause.wait_until_claimed().await;

    let (state, receipt_count): (String, i64) = sqlx::query_as(
        "SELECT apply_state,
                (SELECT COUNT(*) FROM conversation_goal_coordinator_receipts
                 WHERE preparation_reference=?1)
         FROM conversation_goal_preparations WHERE reference=?1",
    )
    .bind(reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state, "claimed");
    assert_eq!(receipt_count, 0);

    let gate_arrival = fixture
        .state
        .runtime
        .observe_next_remote_goal_apply_gate_arrival(reference)
        .await;
    let second_service = service.clone();
    let second_authority = authority.clone();
    let second = tokio::spawn(async move {
        second_service
            .apply_remote(&second_authority, reference)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate_arrival.notified())
        .await
        .expect("duplicate apply should reach the held reference gate");
    assert!(
        !second.is_finished(),
        "duplicate apply completed while the first request held the reference gate"
    );

    pause.resume();
    let first_result = tokio::time::timeout(std::time::Duration::from_secs(30), first)
        .await
        .expect("first remote apply timed out")
        .unwrap()
        .unwrap();
    let second_result = tokio::time::timeout(std::time::Duration::from_secs(30), second)
        .await
        .expect("duplicate remote apply timed out")
        .unwrap()
        .unwrap();
    assert_eq!(first_result, second_result);

    let expired_reference = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversation_goal_preparations(
           reference,owner_id,source_kind,enrollment_id,expires_at_us,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
           operation,expected_version,observed_goal_id,objective,disposition,reason,apply_state)
         SELECT ?,owner_id,source_kind,enrollment_id,0,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'expired-replay-call',
           operation,expected_version,observed_goal_id,objective,disposition,reason,'claimed'
         FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(expired_reference.as_bytes().as_slice())
    .bind(reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversation_goal_coordinator_receipts(
           preparation_reference,owner_id,outcome,result_version,recorded_at_us)
         VALUES (?,?,'rejected_busy',0,?)",
    )
    .bind(expired_reference.as_bytes().as_slice())
    .bind(fixture.owner.as_str())
    .bind(chrono::Utc::now().timestamp_micros())
    .execute(db.pool())
    .await
    .unwrap();
    let replay_after_expiry = service
        .apply_remote(&authority, expired_reference)
        .await
        .unwrap();
    let super::service::RemoteGoalApplyServiceResult::Classified(
        _,
        phoenix_db::conversation_goals::user_commands::RemoteGoalApplyOutcome::Receipt(
            replay_receipt,
        ),
    ) = &replay_after_expiry
    else {
        panic!("expired preparation with a receipt must replay that receipt");
    };
    assert_eq!(replay_receipt.request_id, expired_reference);
    assert!(matches!(
        &replay_receipt.result,
        phoenix_db::conversation_goals::user_commands::GoalUserResult::Rejected {
            reason: phoenix_db::conversation_goals::user_commands::GoalUserRefusal::Busy,
            observed_version,
            ..
        } if *observed_version == GoalVersion::INITIAL
    ));

    let history_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?1")
            .bind(fixture.owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_coordinator_receipts
         WHERE preparation_reference=?1",
    )
    .bind(reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(history_count, 1);
    assert_eq!(receipt_count, 1);
    assert!(!fixture.state.runtime.fatal_local_authority_is_latched());
    assert!(fixture.state.runtime.acquire_local_authority_pass().is_ok());
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn expired_remote_apply_returns_typed_outcome_without_closing_authority() {
    use phoenix_core::domain::{
        conversation_goal::{
            GoalAgentSource, GoalExecution, GoalPreparationSource, GoalText, RemoteGoalAgentSource,
        },
        instance_identity::{FederationCredentialVerifier, InstanceId},
    };
    use phoenix_db::conversation_goals::{
        prepared::GoalPreparationIntent, user_commands::RemoteGoalApplyOutcome,
    };

    let fixture = Fixture::new().await;
    let db = fixture.state.runtime.db();
    let caller = InstanceId::new();
    let verifier = FederationCredentialVerifier::from_bearer(b"expired-goal-apply");
    db.replace_federation_enrollment(caller, &"expired Goal peer".parse().unwrap(), &verifier)
        .await
        .unwrap();
    let authority = phoenix_db::FederationSendAuthority::new(caller, verifier);
    let source = RemoteGoalAgentSource {
        instance_id: caller,
        source: GoalAgentSource {
            product_conversation_id: ProductConversationId::new(),
            transcript_id: GoalText::parse(uuid::Uuid::new_v4().to_string()).unwrap(),
            execution: GoalExecution::parse(1, 1).unwrap(),
            message_id: GoalText::parse("expired-goal-message").unwrap(),
            tool_use_id: GoalText::parse("expired-goal-tool").unwrap(),
        },
    };
    let action = db
        .prepare_remote_coordinator_goal_action(
            &authority,
            &source,
            &fixture.owner,
            GoalPreparationIntent::Create {
                objective: GoalText::parse("must never become the Goal").unwrap(),
            },
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(action.source, GoalPreparationSource::Remote(source));
    let expired_reference = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO conversation_goal_preparations(
           reference,owner_id,source_kind,enrollment_id,expires_at_us,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,
           operation,expected_version,observed_goal_id,objective,disposition,reason)
         SELECT ?,owner_id,source_kind,enrollment_id,0,
           caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,'expired-service-call',
           operation,expected_version,observed_goal_id,objective,disposition,reason
         FROM conversation_goal_preparations WHERE reference=?",
    )
    .bind(expired_reference.as_bytes().as_slice())
    .bind(action.reference.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let expired_action = db
        .read_remote_coordinator_goal_preparation(&authority, expired_reference)
        .await
        .unwrap();

    let history_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?")
            .bind(fixture.owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    let service = super::service::GoalCommandService::new(fixture.state.runtime.clone());
    let super::service::RemoteGoalApplyServiceResult::Classified(returned_action, outcome) =
        service
            .apply_remote(&authority, expired_reference)
            .await
            .unwrap()
    else {
        panic!("known expired preparation must remain classified");
    };

    assert_eq!(*returned_action, expired_action);
    assert_eq!(
        outcome,
        RemoteGoalApplyOutcome::Expired {
            action_reference: expired_reference
        }
    );
    let (apply_state, receipts): (String, i64) = sqlx::query_as(
        "SELECT apply_state,
                (SELECT COUNT(*) FROM conversation_goal_coordinator_receipts
                 WHERE preparation_reference=?1)
         FROM conversation_goal_preparations WHERE reference=?1",
    )
    .bind(expired_reference.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let history_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversation_goal_history WHERE owner_id=?")
            .bind(fixture.owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(apply_state, "prepared");
    assert_eq!(receipts, 0);
    assert_eq!(history_after, history_before);
    assert!(!fixture.state.runtime.fatal_local_authority_is_latched());
    assert!(fixture.state.runtime.acquire_local_authority_pass().is_ok());
}
