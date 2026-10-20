use super::*;
use phoenix_core::domain::conversation_goal::{GoalActor, GoalUserCommandKind};
use phoenix_db::{
    conversation_goals::{
        control::{ControlCommand, ControlRequest},
        user_create::{GoalCreateRefusal, GoalCreateResult},
    },
    Database,
};

async fn fixture() -> (GoalCreateService, ProductConversationId) {
    let db = Database::open_in_memory().await.unwrap();
    let conversation = db
        .create_conversation("user-create", "user Create", "/tmp", true, None, None)
        .await
        .unwrap();
    let db = db.install_goal_controls_for_tests().await.unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../phoenix-db/src/conversation_goals/user_control_candidate.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (
        GoalCreateService {
            runtime: Some(crate::goal_service::tests::test_runtime_manager(db)),
        },
        conversation.product_conversation_id,
    )
}

fn request(id: Uuid, version: i64, objective: &str) -> GoalCreateRequest {
    GoalCreateRequest {
        request_id: id,
        expected_version: GoalVersion::parse(version).unwrap(),
        objective: GoalText::parse(objective).unwrap(),
    }
}

fn entry(
    receipt: &GoalCreateReceipt,
) -> &phoenix_core::domain::conversation_goal::GoalHistoryEntry {
    let GoalCreateResult::Accepted { entry } = &receipt.result else {
        panic!("expected accepted Create")
    };
    entry
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_user_create_has_host_attribution_exact_retry_and_preserves_stop() {
    let (service, owner) = fixture().await;
    let runtime = service.runtime.as_ref().unwrap();
    let db = runtime.db();
    let principal = Uuid::parse_str(&db.instance_id().await.unwrap().to_string()).unwrap();
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: principal,
            request_id: Uuid::new_v4(),
            command: ControlCommand::Stop,
            expected_control_version: 0,
        },
        GoalTimestampUs::parse(10).unwrap(),
    )
    .await
    .unwrap();
    let messages_before: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let first = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(id, 0, " exact objective\n"),
        )
        .await
        .unwrap();
    assert!(matches!(&entry(&first).mutation.attribution.requested_by,
        GoalActor::UserControl { principal_id, request_id, command: GoalUserCommandKind::Create }
        if *principal_id == principal && *request_id == id));
    assert_eq!(
        entry(&first).mutation.attribution.requested_by,
        entry(&first).mutation.attribution.applied_by
    );
    let next = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 1, " exact objective\n"),
        )
        .await
        .unwrap();
    assert_ne!(
        entry(&first).snapshot.goal_id,
        entry(&next).snapshot.goal_id
    );
    assert_eq!(entry(&next).snapshot.version.get(), 2);
    let _product = runtime.lock_product_message_admission(owner.as_str()).await;
    let _acceptance = runtime.lock_message_acceptance("user-create").await;
    assert_eq!(
        service
            .create(
                OwnerAuthenticated,
                owner.clone(),
                request(id, 0, " exact objective\n")
            )
            .await
            .unwrap(),
        first
    );
    assert!(matches!(
        service
            .create(
                OwnerAuthenticated,
                owner.clone(),
                request(id, 0, "changed input")
            )
            .await,
        Err(GoalCreateServiceError::Goal(GoalDbError::RetryConflict))
    ));
    let control: (i64, i64) = sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(control, (1, 1));
    let messages_after: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(messages_after, messages_before);
    let copies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_user_requests WHERE rejected_objective IS NOT NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(copies, 0);
}

#[tokio::test]
async fn conversation_goal_user_create_busy_and_version_refusals_are_retained() {
    let (service, owner) = fixture().await;
    let runtime = service.runtime.as_ref().unwrap();
    let mut refused = Vec::new();
    for product_gate in [true, false] {
        let guard = if product_gate {
            runtime.lock_product_message_admission(owner.as_str()).await
        } else {
            runtime.lock_message_acceptance("user-create").await
        };
        let id = Uuid::new_v4();
        let receipt = service
            .create(
                OwnerAuthenticated,
                owner.clone(),
                request(id, 0, "busy objective"),
            )
            .await
            .unwrap();
        assert!(matches!(
            receipt.result,
            GoalCreateResult::Rejected {
                reason: GoalCreateRefusal::Busy,
                ..
            }
        ));
        drop(guard);
        refused.push((id, receipt));
    }
    let stale_id = Uuid::new_v4();
    let stale = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(stale_id, 9, "stale objective"),
        )
        .await
        .unwrap();
    assert!(matches!(
        stale.result,
        GoalCreateResult::Rejected {
            reason: GoalCreateRefusal::Version,
            ..
        }
    ));
    service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 0, "new objective"),
        )
        .await
        .unwrap();
    for (id, receipt) in refused {
        assert_eq!(
            service
                .create(
                    OwnerAuthenticated,
                    owner.clone(),
                    request(id, 0, "busy objective")
                )
                .await
                .unwrap(),
            receipt
        );
    }
    assert_eq!(
        service
            .create(
                OwnerAuthenticated,
                owner,
                request(stale_id, 9, "stale objective")
            )
            .await
            .unwrap(),
        stale
    );
}

#[tokio::test]
async fn conversation_goal_user_create_receipt_identity_actor_and_history_roll_back_together() {
    let (service, owner) = fixture().await;
    let db = service.runtime.as_ref().unwrap().db();
    let id = Uuid::new_v4();
    sqlx::query("CREATE TRIGGER fail_authenticated_create BEFORE INSERT ON conversation_goal_history BEGIN SELECT RAISE(ABORT,'Create cut'); END")
        .execute(db.pool()).await.unwrap();
    assert!(service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(id, 0, "atomic objective")
        )
        .await
        .is_err());
    let rows: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM conversation_goal_user_requests),(SELECT count(*) FROM conversation_goal_ids),(SELECT count(*) FROM conversation_goal_actors),(SELECT count(*) FROM conversation_goal_history)")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(rows, (0, 0, 0, 0));
    sqlx::query("DROP TRIGGER fail_authenticated_create")
        .execute(db.pool())
        .await
        .unwrap();
    let accepted = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(id, 0, "atomic objective"),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=?")
        .bind(owner.as_str())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(service
        .create(
            OwnerAuthenticated,
            owner,
            request(id, 0, "atomic objective")
        )
        .await
        .is_err());
    assert_eq!(entry(&accepted).snapshot.version.get(), 1);
}

#[tokio::test]
async fn conversation_goal_user_create_has_no_model_auth_fields_and_default_service_is_disabled() {
    use axum::{extract::FromRequestParts, http::Request};
    let id = Uuid::new_v4();
    for field in ["principal_id", "goal_id", "actor", "owner", "authenticated"] {
        let mut body =
            serde_json::json!({"request_id": id, "expected_version": 0, "objective": "objective"});
        body[field] = serde_json::json!("untrusted identity");
        assert!(serde_json::from_value::<GoalCreateRequest>(body).is_err());
    }
    let (mut parts, ()) = Request::new(()).into_parts();
    assert!(OwnerAuthenticated::from_request_parts(&mut parts, &())
        .await
        .is_err());
    parts
        .extensions
        .insert(crate::api::auth::PeerAuthenticated {
            enrollment_id: Uuid::new_v4(),
            caller_instance_id: phoenix_core::domain::instance_identity::InstanceId::new(),
            caller_display_name: "peer".into(),
        });
    assert!(OwnerAuthenticated::from_request_parts(&mut parts, &())
        .await
        .is_err());
    assert!(matches!(
        GoalCreateService::default()
            .create(
                OwnerAuthenticated,
                ProductConversationId::parse("owner").unwrap(),
                request(id, 0, "objective")
            )
            .await,
        Err(GoalCreateServiceError::Disabled)
    ));
}
