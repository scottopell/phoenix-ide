use super::*;
use phoenix_core::domain::conversation_goal::{GoalActor, GoalUserCommandKind};
use phoenix_db::{
    conversation_goals::{
        control::{ControlCommand, ControlRequest},
        user_commands::{GoalUserRefusal, GoalUserResult},
    },
    Database,
};

async fn fixture() -> (GoalCommandService, ProductConversationId) {
    let db = Database::open_in_memory().await.unwrap();
    let conversation = db
        .create_conversation("user-create", "user Create", "/tmp", true, None, None)
        .await
        .unwrap();
    let db = db.install_goal_controls_for_tests().await.unwrap();
    (
        GoalCommandService {
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

fn state_request(
    id: Uuid,
    version: i64,
    goal_id: &GoalId,
    disposition: UserGoalDisposition,
) -> GoalStateRequest {
    GoalStateRequest {
        request_id: id,
        expected_version: GoalVersion::parse(version).unwrap(),
        goal_id: goal_id.clone(),
        disposition,
    }
}

fn entry(receipt: &GoalUserReceipt) -> &phoenix_core::domain::conversation_goal::GoalHistoryEntry {
    let GoalUserResult::Accepted { entry } = &receipt.result else {
        panic!("expected accepted goal command")
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
        Err(GoalCommandServiceError::Goal(GoalDbError::RetryConflict))
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
            GoalUserResult::Rejected {
                reason: GoalUserRefusal::Busy,
                ..
            }
        ));
        drop(guard);
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
        for changed in [
            request(id, 1, "busy objective"),
            request(id, 0, "changed rejected objective"),
        ] {
            assert!(matches!(
                service
                    .create(OwnerAuthenticated, owner.clone(), changed)
                    .await,
                Err(GoalCommandServiceError::Goal(GoalDbError::RetryConflict))
            ));
        }
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
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Version,
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
#[allow(clippy::too_many_lines)]
async fn conversation_goal_user_state_blocked_recovery_keeps_identity_stop_and_exact_receipt() {
    let (service, owner) = fixture().await;
    let runtime = service.runtime.as_ref().unwrap();
    let db = runtime.db();
    let created = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 0, "canonical objective"),
        )
        .await
        .unwrap();
    let goal_id = &entry(&created).snapshot.goal_id;
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
    assert!(matches!(
        service
            .state(
                OwnerAuthenticated,
                owner.clone(),
                state_request(
                    created.request_id,
                    0,
                    goal_id,
                    UserGoalDisposition::Blocked(GoalText::parse("different command").unwrap())
                )
            )
            .await,
        Err(GoalCommandServiceError::Goal(GoalDbError::RetryConflict))
    ));
    let blocked_id = Uuid::new_v4();
    let blocked = service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(
                blocked_id,
                1,
                goal_id,
                UserGoalDisposition::Blocked(GoalText::parse("need credentials").unwrap()),
            ),
        )
        .await
        .unwrap();
    assert_eq!(entry(&blocked).snapshot.goal_id, *goal_id);
    assert_eq!(
        entry(&blocked).snapshot.objective,
        entry(&created).snapshot.objective
    );
    assert!(
        matches!(&entry(&blocked).mutation.attribution.requested_by, GoalActor::UserControl { command: GoalUserCommandKind::State, request_id, principal_id } if *request_id == blocked_id && *principal_id == principal)
    );
    let resumed = service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(Uuid::new_v4(), 2, goal_id, UserGoalDisposition::Underway),
        )
        .await
        .unwrap();
    assert_eq!(entry(&resumed).snapshot.goal_id, *goal_id);
    assert!(matches!(
        entry(&resumed).snapshot.state,
        phoenix_core::domain::conversation_goal::GoalState::Underway
    ));
    let _product = runtime.lock_product_message_admission(owner.as_str()).await;
    let _acceptance = runtime.lock_message_acceptance("user-create").await;
    assert_eq!(
        service
            .state(
                OwnerAuthenticated,
                owner.clone(),
                state_request(
                    blocked_id,
                    1,
                    goal_id,
                    UserGoalDisposition::Blocked(GoalText::parse("need credentials").unwrap())
                )
            )
            .await
            .unwrap(),
        blocked
    );
    assert!(matches!(
        service
            .state(
                OwnerAuthenticated,
                owner.clone(),
                state_request(
                    blocked_id,
                    1,
                    goal_id,
                    UserGoalDisposition::Blocked(GoalText::parse("changed reason").unwrap())
                )
            )
            .await,
        Err(GoalCommandServiceError::Goal(GoalDbError::RetryConflict))
    ));
    let control: (i64, i64) = sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(control, (1, 1));
    let materializations: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_turns")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(materializations, 0);
}

#[tokio::test]
async fn conversation_goal_user_state_terminal_and_wrong_target_refusals_are_durable() {
    let (service, owner) = fixture().await;
    let created = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 0, "objective"),
        )
        .await
        .unwrap();
    let goal_id = &entry(&created).snapshot.goal_id;
    let wrong_id = GoalId::parse("not-the-current-goal").unwrap();
    let wrong = service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(
                Uuid::new_v4(),
                1,
                &wrong_id,
                UserGoalDisposition::Blocked(GoalText::parse("wrong target").unwrap()),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        wrong.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Transition,
            ..
        }
    ));
    service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(
                Uuid::new_v4(),
                1,
                goal_id,
                UserGoalDisposition::Complete(GoalText::parse("finished").unwrap()),
            ),
        )
        .await
        .unwrap();
    let request_id = Uuid::new_v4();
    let refused = service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(request_id, 2, goal_id, UserGoalDisposition::Underway),
        )
        .await
        .unwrap();
    assert!(matches!(
        refused.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Transition,
            ..
        }
    ));
    service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 2, "new objective"),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .state(
                OwnerAuthenticated,
                owner,
                state_request(request_id, 2, goal_id, UserGoalDisposition::Underway)
            )
            .await
            .unwrap(),
        refused
    );
}

#[tokio::test]
async fn conversation_goal_user_state_failure_rolls_back_accepted_actor_receipt_and_status() {
    let (service, owner) = fixture().await;
    let db = service.runtime.as_ref().unwrap().db();
    let created = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 0, "objective"),
        )
        .await
        .unwrap();
    let goal_id = &entry(&created).snapshot.goal_id;
    sqlx::query("CREATE TRIGGER fail_authenticated_state BEFORE INSERT ON conversation_goal_history BEGIN SELECT RAISE(ABORT,'State cut'); END").execute(db.pool()).await.unwrap();
    let id = Uuid::new_v4();
    assert!(service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(
                id,
                1,
                goal_id,
                UserGoalDisposition::Blocked(GoalText::parse("dependency").unwrap())
            )
        )
        .await
        .is_err());
    assert_eq!(
        db.conversation_goal(&owner).await.unwrap().as_ref(),
        Some(&entry(&created).snapshot)
    );
    let rows: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM conversation_goal_user_requests),(SELECT count(*) FROM conversation_goal_ids),(SELECT count(*) FROM conversation_goal_actors),(SELECT count(*) FROM conversation_goal_history)")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(rows, (1, 1, 1, 1));
    sqlx::query("DROP TRIGGER fail_authenticated_state")
        .execute(db.pool())
        .await
        .unwrap();
    let accepted = service
        .state(
            OwnerAuthenticated,
            owner,
            state_request(
                id,
                1,
                goal_id,
                UserGoalDisposition::Blocked(GoalText::parse("dependency").unwrap()),
            ),
        )
        .await
        .unwrap();
    assert_eq!(entry(&accepted).snapshot.version.get(), 2);
}

#[tokio::test]
async fn conversation_goal_user_state_question_wait_stays_underway_and_cannot_become_blocked() {
    use phoenix_core::domain::sm_state::{
        ConvState, QuestionOption, QuestionRequestAuthority, UserQuestion,
    };
    let (service, owner) = fixture().await;
    let db = service.runtime.as_ref().unwrap().db();
    let created = service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(Uuid::new_v4(), 0, "objective"),
        )
        .await
        .unwrap();
    let goal_id = &entry(&created).snapshot.goal_id;
    let awaiting = ConvState::AwaitingUserResponse {
        questions: vec![UserQuestion {
            question: "Which endpoint?".into(),
            header: "Endpoint".into(),
            options: vec![
                QuestionOption {
                    label: "A".into(),
                    description: None,
                    preview: None,
                },
                QuestionOption {
                    label: "B".into(),
                    description: None,
                    preview: None,
                },
            ],
            multi_select: false,
        }],
        tool_use_id: "question".into(),
        request_authority: QuestionRequestAuthority::new(),
    };
    db.update_conversation_state("user-create", &awaiting)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let refused = service
        .state(
            OwnerAuthenticated,
            owner.clone(),
            state_request(
                id,
                1,
                goal_id,
                UserGoalDisposition::Blocked(GoalText::parse("same question").unwrap()),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        refused.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Busy,
            ..
        }
    ));
    assert_eq!(
        db.conversation_goal(&owner).await.unwrap().as_ref(),
        Some(&entry(&created).snapshot)
    );
    assert_eq!(
        db.get_conversation("user-create").await.unwrap().state,
        awaiting
    );
    db.update_conversation_state("user-create", &ConvState::Idle)
        .await
        .unwrap();
    assert_eq!(
        service
            .state(
                OwnerAuthenticated,
                owner,
                state_request(
                    id,
                    1,
                    goal_id,
                    UserGoalDisposition::Blocked(GoalText::parse("same question").unwrap())
                )
            )
            .await
            .unwrap(),
        refused
    );
}

#[tokio::test]
async fn conversation_goal_user_commands_retained_replay_checks_latest_target_and_close() {
    use phoenix_core::domain::{
        close::TranscriptConversationId, db_schema::MessageContent, sm_state::ConvState,
    };
    let (service, owner) = fixture().await;
    let db = service.runtime.as_ref().unwrap().db();
    let id = Uuid::new_v4();
    service
        .create(
            OwnerAuthenticated,
            owner.clone(),
            request(id, 0, "objective"),
        )
        .await
        .unwrap();
    let input = GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::parse_str(&db.instance_id().await.unwrap().to_string()).unwrap(),
        request_id: id,
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: GoalText::parse("objective").unwrap(),
        },
    };
    let original = db
        .retained_user_goal_command(&input, "user-create")
        .await
        .unwrap()
        .unwrap();
    db.add_message(
        "goal-command-handoff",
        "user-create",
        &MessageContent::continuation("continued objective"),
        None,
        None,
    )
    .await
    .unwrap();
    db.update_conversation_state(
        "user-create",
        &ConvState::ContextExhausted {
            summary: "continued objective".into(),
        },
    )
    .await
    .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) =
        db.continue_conversation("user-create").await.unwrap()
    else {
        panic!("expected genuine continuation")
    };
    assert_eq!(successor.product_conversation_id, owner);
    assert!(matches!(
        db.retained_user_goal_command(&input, "user-create").await,
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(
        db.retained_user_goal_command(&input, &successor.id)
            .await
            .unwrap(),
        Some(original)
    );
    db.begin_direct_close_foundation(
        &owner,
        &TranscriptConversationId::parse(successor.id.clone()).unwrap(),
        "close-goal-create",
    )
    .await
    .unwrap();
    assert!(matches!(
        db.retained_user_goal_command(&input, &successor.id).await,
        Err(GoalDbError::Admission(
            phoenix_db::DbError::CloseAdmissionFenced(_)
        ))
    ));
    assert!(service
        .create(OwnerAuthenticated, owner, request(id, 0, "objective"))
        .await
        .is_err());
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
    let state_body = serde_json::json!({"request_id": id, "expected_version": 1, "goal_id": "goal", "disposition": {"state": "blocked", "reason": "dependency"}});
    for field in ["principal_id", "actor", "objective", "authenticated"] {
        let mut body = state_body.clone();
        body[field] = serde_json::json!("untrusted identity");
        assert!(serde_json::from_value::<GoalStateRequest>(body).is_err());
    }
    for disposition in [
        serde_json::json!({"state": "suspended", "reason": "legacy"}),
        serde_json::json!({"state": "blocked", "reason": " "}),
    ] {
        let mut body = state_body.clone();
        body["disposition"] = disposition;
        assert!(serde_json::from_value::<GoalStateRequest>(body).is_err());
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
        GoalCommandService::default()
            .create(
                OwnerAuthenticated,
                ProductConversationId::parse("owner").unwrap(),
                request(id, 0, "objective")
            )
            .await,
        Err(GoalCommandServiceError::Disabled)
    ));
}
