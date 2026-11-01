use super::control::{ControlCommand, ControlRequest, GoalControlInstallation};
use super::execution::{append_request_tx, GoalExecutionInstallation, GoalRequestError};
use super::user_commands::{GoalUserChange, GoalUserCommand, GoalUserRefusal, GoalUserResult};
use super::*;
use phoenix_core::domain::conversation_goal::{
    GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalRequestLimit,
};
use uuid::Uuid;

mod checkpoint;

pub(super) async fn fixture() -> Database {
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

pub(super) async fn ordinary(db: &Database, name: &str) -> (String, ProductConversationId) {
    let conversation = db
        .create_conversation(name, name, "/tmp", true, None, None)
        .await
        .unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &conversation.product_conversation_id,
        crate::AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .unwrap();
    (conversation.id, conversation.product_conversation_id)
}

pub(super) fn at() -> GoalTimestampUs {
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

pub(super) async fn counts(db: &Database) -> (i64, i64, i64, i64, i64, i64, i64) {
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
        || panic!("unexpected control authority loss"),
    )
    .await
    .established()
    .expect("classified control")
    .unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn goal_admission_requires_enabled_preference_but_legacy_safety_exits_remain_available() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "goal-prerequisite").await;
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    let first = create(
        &owner,
        GoalVersion::INITIAL,
        "goal-prerequisite",
        "create-off",
    );
    let before = counts(&db).await;
    assert!(matches!(
        db.apply_conversation_goal(&first, &transcript, at()).await,
        Err(GoalDbError::AutoContinueDisabled)
    ));
    assert_eq!(counts(&db).await, before);

    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .unwrap();
    let accepted = db
        .apply_conversation_goal(&first, &transcript, at())
        .await
        .unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&owner)
            .await
            .unwrap(),
        crate::AutoContinueOnContextExhaustion::Disabled
    );

    let suspension_actor = GoalActor::GeneratedHost {
        action_id: GoalText::parse("suspend-prerequisite").unwrap(),
    };
    let suspended = GoalMutation {
        owner: owner.clone(),
        expected_version: accepted.snapshot().version(),
        action_id: GoalText::parse("suspend-prerequisite").unwrap(),
        attribution: GoalAttribution {
            requested_by: suspension_actor.clone(),
            applied_by: suspension_actor,
        },
        change: GoalChange::SetDisposition {
            goal_id: accepted.snapshot().goal_id().clone(),
            disposition: GoalDisposition::Suspended(GoalText::parse("paused").unwrap()),
        },
    };
    let suspended_entry = db
        .apply_conversation_goal(&suspended, &transcript, at())
        .await
        .unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    assert_eq!(
        db.apply_conversation_goal(&first, &transcript, at())
            .await
            .unwrap(),
        accepted
    );
    let replacement = create(
        &owner,
        suspended_entry.snapshot().version(),
        "goal-replacement-off",
        "replace-off",
    );
    let before_replacement = counts(&db).await;
    assert!(matches!(
        db.apply_conversation_goal(&replacement, &transcript, at())
            .await,
        Err(GoalDbError::AutoContinueDisabled)
    ));
    assert_eq!(counts(&db).await, before_replacement);

    let activation_actor = GoalActor::GeneratedHost {
        action_id: GoalText::parse("resume-prerequisite").unwrap(),
    };
    let activation = GoalMutation {
        owner: owner.clone(),
        expected_version: suspended_entry.snapshot().version(),
        action_id: GoalText::parse("resume-prerequisite").unwrap(),
        attribution: GoalAttribution {
            requested_by: activation_actor.clone(),
            applied_by: activation_actor,
        },
        change: GoalChange::SetDisposition {
            goal_id: accepted.snapshot().goal_id().clone(),
            disposition: GoalDisposition::Blocked(GoalText::parse("reactivate").unwrap()),
        },
    };
    assert!(matches!(
        db.apply_conversation_goal(&activation, &transcript, at())
            .await,
        Err(GoalDbError::Domain(GoalError::Inapplicable))
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn disabled_legacy_goals_preserve_reads_metadata_and_all_safety_exits() {
    let db = fixture().await;
    for (name, disposition) in [
        (
            "complete",
            GoalDisposition::Complete(GoalText::parse("done").unwrap()),
        ),
        (
            "suspend",
            GoalDisposition::Suspended(GoalText::parse("pause").unwrap()),
        ),
        (
            "exhaust",
            GoalDisposition::Exhausted(GoalText::parse("limit").unwrap()),
        ),
    ] {
        let (transcript, owner) = ordinary(&db, name).await;
        let first = create(&owner, GoalVersion::INITIAL, name, "create");
        let admitted = db
            .apply_conversation_goal(&first, &transcript, at())
            .await
            .unwrap();
        db.set_auto_continue_on_context_exhaustion(
            &owner,
            crate::AutoContinueOnContextExhaustion::Disabled,
        )
        .await
        .unwrap();
        assert_eq!(
            db.conversation_goal(&owner).await.unwrap().as_ref(),
            Some(admitted.snapshot())
        );
        assert_eq!(
            db.apply_conversation_goal(&first, &transcript, at())
                .await
                .unwrap(),
            admitted
        );
        let mut current = admitted;
        for (action, state) in [
            (
                "block",
                GoalDisposition::Blocked(GoalText::parse("waiting").unwrap()),
            ),
            ("underway", GoalDisposition::Underway),
            ("exit", disposition),
        ] {
            let actor = GoalActor::GeneratedHost {
                action_id: GoalText::parse(action).unwrap(),
            };
            let mutation = GoalMutation {
                owner: owner.clone(),
                expected_version: current.snapshot().version(),
                action_id: GoalText::parse(action).unwrap(),
                attribution: GoalAttribution {
                    requested_by: actor.clone(),
                    applied_by: actor,
                },
                change: GoalChange::SetDisposition {
                    goal_id: current.snapshot().goal_id().clone(),
                    disposition: state,
                },
            };
            current = db
                .apply_conversation_goal(&mutation, &transcript, at())
                .await
                .unwrap();
        }
        db.apply_goal_control(
            &ControlRequest {
                owner: owner.clone(),
                principal_id: Uuid::new_v4(),
                request_id: Uuid::new_v4(),
                command: ControlCommand::Stop,
                expected_control_version: 0,
            },
            at(),
            || panic!("unexpected authority loss"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
        assert_eq!(
            db.conversation_goal(&owner).await.unwrap().as_ref(),
            Some(current.snapshot())
        );
        assert_eq!(
            db.auto_continue_on_context_exhaustion(&owner)
                .await
                .unwrap(),
            crate::AutoContinueOnContextExhaustion::Disabled
        );
    }
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
    let fact: (String, Option<String>, String, i64, String, String) = sqlx::query_as(
        "SELECT e.terminal_kind,e.terminal_reason,c.kind,c.owner_version,c.goal_id,c.objective
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
            None,
            "saved_goal".into(),
            first.snapshot().version().get(),
            "captured-a".into(),
            "objective-captured-a".into()
        )
    );
    let pending = db.pending_coordinator_watch_events(16).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].product_conversation_id, owner);
    assert_eq!(pending[0].source_occurrence_kind, "goal_change");
    assert_eq!(
        pending[0].outcome,
        crate::coordinator_watches::WatchOutcome::GoalChanged
    );
    assert!(matches!(
        &pending[0].route,
        crate::coordinator_watches::WatchEventRoute::Subscription
    ));
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
        .apply_user_goal_command(
            &command,
            &transcript,
            || Ok(at()),
            || panic!("authority lost"),
        )
        .await
        .established()
        .unwrap()
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
        db.apply_user_goal_command(
            &command,
            &transcript,
            || Ok(at()),
            || panic!("authority lost")
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        receipt
    );
    assert_eq!(counts(&db).await, before);
    let stale = GoalUserCommand {
        request_id: Uuid::new_v4(),
        ..command
    };
    assert!(matches!(
        db.apply_user_goal_command(
            &stale,
            &transcript,
            || Ok(at()),
            || panic!("authority lost")
        )
        .await
        .established()
        .unwrap()
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

use crate::workflow::ClaimAuthoritativeTurnInput;
use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, LeaseExpiry, PreparedTurn,
    ProcessIncarnation, Timestamp, TurnAuthorityId, TurnCommand, WorkflowId,
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

async fn retained_generation_zero_requests_fixture() -> (
    Database,
    GoalExecution,
    GoalRequestIntent,
    GoalRequestIntent,
) {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "request-rearm-owner").await;
    let goal = db
        .apply_conversation_goal(
            &create(&owner, GoalVersion::INITIAL, "rearm-goal", "rearm-create"),
            &transcript,
            at(),
        )
        .await
        .unwrap();
    release(&db, &owner).await;
    let repo = db.workflow_repository();
    let execution =
        accept_execution(&repo, &runtime_input(&transcript, "request-rearm-turn")).await;
    let original = GoalRequestIntent::new(
        GoalRequestId::parse("request-before-rearm").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Initial,
    )
    .unwrap();
    let after_rearm = GoalRequestIntent::new(
        GoalRequestId::parse("request-after-rearm").unwrap(),
        goal.snapshot().fence(),
        execution,
        0,
        GoalRequestCause::Summary {
            operation: GoalText::parse("summary-after-rearm").unwrap(),
        },
    )
    .unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    append_request_tx(&mut tx, db.goal_execution_installation(), &original)
        .await
        .unwrap();
    append_request_tx(&mut tx, db.goal_execution_installation(), &after_rearm)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (db, execution, original, after_rearm)
}

#[tokio::test]
async fn retained_generation_zero_request_cannot_checkpoint_after_fail_and_rearm() {
    let (db, execution, original, after_rearm) = retained_generation_zero_requests_fixture().await;
    let repo = db.workflow_repository();
    let turn_id = TurnAuthorityId(u64::try_from(execution.turn_id()).unwrap());
    let workflow_id = WorkflowId(u64::try_from(execution.workflow_id()).unwrap());
    let claim = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(20),
            now: Timestamp(20),
            lease_until: LeaseExpiry(30),
        })
        .await
        .unwrap();
    assert!(claim.authority.is_some());
    let checkpoint = GoalText::parse("existing-checkpoint").unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    execution::checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &original,
        0,
        &checkpoint,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    repo.terminate_authoritative_turn(TurnCommand::Fail {
        turn_id,
        expected_generation: 0,
        reason: "failed".into(),
    })
    .await
    .unwrap();
    let rearmed = repo
        .rearm_terminal_runtime_direct_turn(&crate::workflow::RearmAuthoritativeTurnInput {
            turn_id,
            expected_generation: 1,
            rearmed_at: Timestamp(40),
        })
        .await
        .unwrap();
    let crate::workflow::RearmAuthoritativeTurnOutcome::Rearmed { turn } = rearmed else {
        panic!("expected turn to rearm");
    };
    assert_eq!(turn.generation, 2);

    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        execution::checkpoint_request_tx(
            &mut tx,
            db.goal_execution_installation(),
            &after_rearm,
            2,
            &GoalText::parse("must-not-be-written").unwrap(),
        )
        .await,
        Err(GoalRequestError::Detached)
    ));
    assert_eq!(
        execution::retained_request_tx(&mut tx, db.goal_execution_installation(), &original)
            .await
            .unwrap()
            .unwrap()
            .checkpoint,
        Some(checkpoint.clone())
    );
    execution::checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &original,
        0,
        &checkpoint,
    )
    .await
    .unwrap();
    let checkpoint_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_request_checkpoints WHERE request_id=?",
    )
    .bind(after_rearm.id().as_str())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(checkpoint_count, 0);
    tx.commit().await.unwrap();
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
        || panic!("unexpected control authority loss"),
    )
    .await
    .established()
    .expect("classified control")
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
