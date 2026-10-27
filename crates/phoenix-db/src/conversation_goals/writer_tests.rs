use super::control::{ControlCommand, ControlRequest, GoalControlInstallation};
use super::execution::{append_request_tx, GoalExecutionInstallation, GoalRequestError};
use super::user_commands::{GoalUserChange, GoalUserCommand, GoalUserRefusal, GoalUserResult};
use super::*;
use phoenix_core::domain::conversation_goal::{
    GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalRequestLimit,
};
use uuid::Uuid;

async fn fixture() -> Database {
    Database::open_in_memory()
        .await
        .unwrap()
        .install_goal_controls_for_tests()
        .await
        .unwrap()
        .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
        .await
        .unwrap()
}

async fn ordinary(db: &Database, name: &str) -> (String, ProductConversationId) {
    let conversation = db
        .create_conversation(name, name, "/tmp", true, None, None)
        .await
        .unwrap();
    (conversation.id, conversation.product_conversation_id)
}

fn at() -> GoalTimestampUs {
    GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros() + 1_000_000).unwrap()
}

fn create(
    owner: &ProductConversationId,
    version: GoalVersion,
    id: &str,
    action: &str,
) -> GoalMutation {
    let actor = GoalActor::GeneratedHost {
        action_id: GoalText::parse(action).unwrap(),
    };
    GoalMutation {
        owner: owner.clone(),
        expected_version: version,
        action_id: GoalText::parse(action).unwrap(),
        attribution: GoalAttribution {
            requested_by: actor.clone(),
            applied_by: actor,
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse(id).unwrap(),
            objective: GoalText::parse(format!("objective-{id}")).unwrap(),
        },
    }
}

async fn counts(db: &Database) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
        (SELECT COUNT(*) FROM conversation_goal_ids),
        (SELECT COUNT(*) FROM conversation_goal_history),
        (SELECT COUNT(*) FROM conversation_goal_actors),
        (SELECT COUNT(*) FROM conversation_goal_limits),
        (SELECT COUNT(*) FROM coordinator_watch_events),
        (SELECT COUNT(*) FROM coordinator_watch_goal_captures),
        (SELECT COUNT(*) FROM conversation_goal_user_requests)",
    )
    .fetch_one(db.pool())
    .await
    .unwrap()
}

async fn release(db: &Database, owner: &ProductConversationId) {
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        at(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn retained_goal_ids_reject_same_and_cross_owner_after_replacement_without_side_effects() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "retained-owner").await;
    let (other_transcript, other) = ordinary(&db, "other-owner").await;
    db.watch_product_conversation(&owner).await.unwrap();
    db.watch_product_conversation(&other).await.unwrap();
    let a = create(&owner, GoalVersion::INITIAL, "goal-a", "create-a");
    let first = db
        .apply_conversation_goal(&a, &transcript, at())
        .await
        .unwrap();
    let b = create(&owner, first.snapshot().version(), "goal-b", "replace-b");
    let replaced = db
        .apply_conversation_goal(&b, &transcript, at())
        .await
        .unwrap();
    let before = counts(&db).await;
    // Accepted action identity wins over ID retention, stale target, version and clock.
    assert_eq!(
        db.apply_conversation_goal(&a, "no-longer-current", at())
            .await
            .unwrap(),
        first
    );
    assert_eq!(counts(&db).await, before);
    for (target_owner, target_transcript, version, action) in [
        (
            &owner,
            transcript.as_str(),
            replaced.snapshot().version(),
            "reuse-a",
        ),
        (
            &other,
            other_transcript.as_str(),
            GoalVersion::INITIAL,
            "steal-a",
        ),
    ] {
        let attempt = create(target_owner, version, "goal-a", action);
        assert!(matches!(
            db.apply_conversation_goal(&attempt, target_transcript, at())
                .await,
            Err(GoalDbError::DuplicateGoalId)
        ));
        assert_eq!(counts(&db).await, before);
    }
    assert_eq!(
        db.conversation_goal(&owner).await.unwrap().unwrap(),
        *replaced.snapshot()
    );
    assert!(db.conversation_goal(&other).await.unwrap().is_none());
    let mut changed_retry = a.clone();
    changed_retry.change = GoalChange::Create {
        goal_id: GoalId::parse("new-id").unwrap(),
        objective: GoalText::parse("different").unwrap(),
    };
    assert!(matches!(
        db.apply_conversation_goal(&changed_retry, &transcript, at())
            .await,
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(counts(&db).await, before);
}

#[tokio::test]
async fn stale_target_and_version_reject_without_allocating_any_facts() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "stale-owner").await;
    let (other_transcript, _) = ordinary(&db, "wrong-transcript").await;
    let before = counts(&db).await;
    assert!(db
        .apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "stale-goal", "stale"),
            &other_transcript,
            at()
        )
        .await
        .is_err());
    assert_eq!(counts(&db).await, before);
    let accepted = db
        .apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "current-goal", "current"),
            &transcript,
            at(),
        )
        .await
        .unwrap();
    let before = counts(&db).await;
    assert!(matches!(
        db.apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "next-goal", "stale-version"),
            &transcript,
            at()
        )
        .await,
        Err(GoalDbError::Domain(GoalError::Conflict))
    ));
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.conversation_goal(&owner).await.unwrap().unwrap(),
        *accepted.snapshot()
    );
}

#[tokio::test]
async fn settled_goal_watch_capture_commits_or_rolls_back_with_the_mutation() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "capture-owner").await;
    db.watch_product_conversation(&owner).await.unwrap();
    let first = db
        .apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "captured-a", "capture-a"),
            &transcript,
            at(),
        )
        .await
        .unwrap();
    let fact: (String, String, i64, String, String) = sqlx::query_as(
        "SELECT e.terminal_kind,c.kind,c.owner_version,c.goal_id,c.objective
         FROM coordinator_watch_events e JOIN coordinator_watch_goal_captures c USING(event_id)
         WHERE e.source_occurrence_kind='goal_change' AND c.owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        fact,
        (
            "goal_changed".into(),
            "saved_goal".into(),
            first.snapshot().version().get(),
            "captured-a".into(),
            "objective-captured-a".into()
        )
    );
    let before = counts(&db).await;
    sqlx::query("CREATE TRIGGER reject_goal_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'injected capture failure'); END")
        .execute(db.pool()).await.unwrap();
    let replacement = create(
        &owner,
        first.snapshot().version(),
        "captured-b",
        "capture-b",
    );
    assert!(db
        .apply_conversation_goal(&replacement, &transcript, at())
        .await
        .is_err());
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        db.conversation_goal(&owner).await.unwrap().unwrap(),
        *first.snapshot()
    );
    sqlx::query("DROP TRIGGER reject_goal_capture")
        .execute(db.pool())
        .await
        .unwrap();
    db.apply_conversation_goal(&replacement, &transcript, at())
        .await
        .unwrap();
    assert_eq!(counts(&db).await.4, before.4 + 1);
    assert_eq!(counts(&db).await.5, before.5 + 1);
}

#[tokio::test]
async fn user_goal_commands_retain_typed_receipts_and_control_attribution() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "user-goal-owner").await;
    let command = GoalUserCommand {
        owner: owner.clone(),
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: GoalText::parse("user objective").unwrap(),
        },
    };
    let receipt = db
        .apply_user_goal_command(&command, &transcript, at())
        .await
        .unwrap();
    let GoalUserResult::Accepted { ref entry } = receipt.result else {
        panic!("expected accepted goal");
    };
    assert!(matches!(
        entry.mutation().attribution.requested_by,
        GoalActor::UserControl {
            command: GoalUserCommandKind::Create,
            ..
        }
    ));
    let before = counts(&db).await;
    assert_eq!(
        db.apply_user_goal_command(&command, &transcript, at())
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(counts(&db).await, before);
    let stale = GoalUserCommand {
        request_id: Uuid::new_v4(),
        ..command
    };
    assert!(matches!(
        db.apply_user_goal_command(&stale, &transcript, at())
            .await
            .unwrap()
            .result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Version,
            ..
        }
    ));
    assert_eq!(counts(&db).await.1, before.1);
    assert_eq!(counts(&db).await.2, before.2);
}

use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
};

async fn accept_execution(
    repo: &crate::workflow::WorkflowRepository,
    input: &crate::workflow::AcceptAuthoritativeTurn,
) -> GoalExecution {
    let accepted = repo.accept_authoritative_turn(input).await.unwrap();
    let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("expected fresh Runtime admission");
    };
    let workflow = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
    GoalExecution::parse(
        i64::try_from(workflow.0).unwrap(),
        i64::try_from(turn_id.0).unwrap(),
    )
    .unwrap()
}

fn runtime_input(transcript: &str, key: &str) -> crate::workflow::AcceptAuthoritativeTurn {
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: InputOrigin::UserApi,
            text: "source input".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "source input".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    crate::workflow::AcceptAuthoritativeTurn {
        client_key: ClientTurnKey::new(key).unwrap(),
        prepared: PreparedTurn::from_exact_payload(
            &ConversationAuthority(transcript.into()),
            payload.to_exact_bytes().unwrap(),
        ),
        disposition: AcceptedDisposition::Runtime,
        accepted_at: Timestamp(1),
    }
}

#[tokio::test]
async fn request_projection_replay_causal_conflict_and_creation_time_limit() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "request-owner").await;
    let mutation = create(&owner, GoalVersion::INITIAL, "request-goal", "create");
    let goal = db
        .apply_conversation_goal(&mutation, &transcript, at())
        .await
        .unwrap();
    release(&db, &owner).await;
    let repo = db.workflow_repository();
    let execution = accept_execution(&repo, &runtime_input(&transcript, "request-turn")).await;
    let intent = GoalRequestIntent::new(
        GoalRequestId::parse("request-1").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Initial,
    )
    .unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let first = append_request_tx(&mut tx, db.goal_execution_installation(), &intent)
        .await
        .unwrap();
    assert_eq!(first.ordinal, 1);
    assert_eq!(
        append_request_tx(&mut tx, db.goal_execution_installation(), &intent)
            .await
            .unwrap(),
        first
    );
    let same_slot = GoalRequestIntent::new(
        GoalRequestId::parse("request-other").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Initial,
    )
    .unwrap();
    assert!(matches!(
        append_request_tx(&mut tx, db.goal_execution_installation(), &same_slot).await,
        Err(GoalRequestError::Conflict)
    ));
    let summary = GoalRequestIntent::new(
        GoalRequestId::parse("summary-1").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Summary {
            operation: GoalText::parse("summary-operation-1").unwrap(),
        },
    )
    .unwrap();
    // A changed installation ceiling does not replace the immutable creation-time limit.
    let changed_installation = GoalExecutionInstallation::Installed {
        ceiling: GoalRequestLimit::parse(99).unwrap(),
    };
    assert_eq!(
        append_request_tx(&mut tx, changed_installation, &summary)
            .await
            .unwrap()
            .ordinal,
        2
    );
    let overflow = GoalRequestIntent::new(
        GoalRequestId::parse("summary-2").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Summary {
            operation: GoalText::parse("summary-operation-2").unwrap(),
        },
    )
    .unwrap();
    assert!(matches!(
        append_request_tx(&mut tx, changed_installation, &overflow).await,
        Err(GoalRequestError::Exhausted)
    ));
    assert_eq!(
        execution::retained_request_tx(&mut tx, changed_installation, &intent)
            .await
            .unwrap()
            .unwrap(),
        first
    );
    let projected: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT request_id,purpose,ordinal FROM conversation_goal_requests ORDER BY ordinal",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        projected,
        vec![
            ("request-1".into(), "normal".into(), 1),
            ("summary-1".into(), "summary".into(), 2)
        ]
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn fresh_runtime_turn_captures_goal_and_control_once_before_replay() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "binding-owner").await;
    let first = db
        .apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "binding-a", "binding-create"),
            &transcript,
            at(),
        )
        .await
        .unwrap();
    release(&db, &owner).await;
    let input = runtime_input(&transcript, "binding-turn");
    let repo = db.workflow_repository();
    let execution = accept_execution(&repo, &input).await;
    let original = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        original.observation(),
        &GoalAdmissionObservation::Goal(first.snapshot().fence())
    );
    let bound_control: i64 = sqlx::query_scalar(
        "SELECT control_version FROM conversation_goal_control_bindings WHERE turn_id=?",
    )
    .bind(execution.turn_id())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(bound_control, 1);
    // Simulate a subsequently accepted owner fact inside the writer transaction.
    let replacement = create(
        &owner,
        first.snapshot().version(),
        "binding-b",
        "binding-replace",
    );
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    apply_goal_tx(&mut tx, &replacement, None, at(), db.goal_write_policies())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Stop,
            expected_control_version: 1,
        },
        at(),
    )
    .await
    .unwrap();
    let replay = repo.accept_authoritative_turn(&input).await.unwrap();
    assert!(matches!(
        replay.outcome,
        phoenix_workflow::TurnOutcome::ExactReplay { .. }
    ));
    assert_eq!(
        db.conversation_goal_execution_binding(execution)
            .await
            .unwrap()
            .unwrap(),
        original
    );
    let count: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM conversation_goal_bindings WHERE turn_id=?1), (SELECT control_version FROM conversation_goal_control_bindings WHERE turn_id=?1)")
        .bind(execution.turn_id()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(count, (1, 1));
}

#[tokio::test]
async fn default_policies_remain_disabled_and_no_goal_turn_has_explicit_absence() {
    let db = Database::open_in_memory().await.unwrap();
    assert_eq!(
        db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert_eq!(
        db.goal_execution_installation(),
        GoalExecutionInstallation::Disabled
    );
    let (transcript, _) = ordinary(&db, "no-goal-owner").await;
    let repo = crate::workflow::WorkflowRepository::new(db.pool().clone());
    let execution = accept_execution(&repo, &runtime_input(&transcript, "no-goal-turn")).await;
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.observation(),
        &GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL
        }
    );
}
