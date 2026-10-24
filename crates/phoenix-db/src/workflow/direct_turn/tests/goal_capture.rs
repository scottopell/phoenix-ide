use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAdmissionObservation, GoalAttribution, GoalChange, GoalDisposition,
    GoalExecution, GoalId, GoalMutation, GoalText, GoalTimestampUs, GoalVersion,
};

use super::*;

async fn fixture() -> (Database, WorkflowRepository, GoalMutation) {
    let db = Database::open_in_memory().await.unwrap();
    let conversation = db
        .create_conversation("capture-root", "capture", "/tmp", true, None, None)
        .await
        .unwrap();
    let actor = GoalActor::GeneratedHost {
        action_id: text("capture-fixture"),
    };
    let command = GoalMutation {
        owner: conversation.product_conversation_id,
        expected_version: GoalVersion::INITIAL,
        action_id: text("goal-created"),
        attribution: GoalAttribution {
            requested_by: actor.clone(),
            applied_by: actor,
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse("captured-goal").unwrap(),
            objective: text("prepare integration"),
        },
    };
    let repo = db.workflow_repository();
    (db, repo, command)
}

fn text(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}
fn at() -> GoalTimestampUs {
    GoalTimestampUs::parse(10).unwrap()
}

async fn execution(repo: &WorkflowRepository, turn_id: TurnAuthorityId) -> GoalExecution {
    let workflow = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
    GoalExecution::parse(
        i64::try_from(workflow.0).unwrap(),
        i64::try_from(turn_id.0).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn no_goal_is_captured_once_with_the_real_accepted_turn() {
    let (db, repo, _) = fixture().await;
    let request = input("capture-root", "no-goal", 1);
    let first = repo.accept_authoritative_turn(&request).await.unwrap();
    let TurnOutcome::Created { turn_id, .. } = first.outcome else {
        panic!("expected create")
    };
    let execution = execution(&repo, turn_id).await;
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        binding.observation,
        GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL
        }
    ));
    let retry = repo.accept_authoritative_turn(&request).await.unwrap();
    assert!(matches!(retry.outcome, TurnOutcome::ExactReplay { .. }));
    assert_eq!(
        db.conversation_goal_execution_binding(execution)
            .await
            .unwrap(),
        Some(binding)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_bindings")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn underway_snapshot_is_bound_by_admission_not_later_tool_dispatch() {
    let (db, repo, command) = fixture().await;
    let goal = db
        .apply_conversation_goal(&command, "capture-root", at())
        .await
        .unwrap()
        .snapshot;
    let turn = repo
        .accept_authoritative_turn(&input("capture-root", "with-goal", 1))
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = turn.outcome else {
        panic!("expected create")
    };
    let execution = execution(&repo, turn_id).await;
    let binding = db
        .conversation_goal_execution_binding(execution)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding.owner, goal.owner);
    assert_eq!(
        binding.observation,
        GoalAdmissionObservation::Goal(goal.fence())
    );
    let wrong_workflow =
        GoalExecution::parse(execution.workflow_id() + 1, execution.turn_id()).unwrap();
    assert!(matches!(
        db.conversation_goal_execution_binding(wrong_workflow).await,
        Err(crate::conversation_goals::GoalDbError::BindingConflict)
    ));
}

#[tokio::test]
async fn blocked_goal_does_not_gain_report_permission_from_an_unrelated_turn() {
    let (db, repo, command) = fixture().await;
    let created = db
        .apply_conversation_goal(&command, "capture-root", at())
        .await
        .unwrap()
        .snapshot;
    let blocked = GoalMutation {
        expected_version: created.version,
        action_id: text("blocked"),
        change: GoalChange::SetDisposition {
            goal_id: created.goal_id,
            disposition: GoalDisposition::Blocked(text("environment requires access")),
        },
        ..command
    };
    let stopped = db
        .apply_conversation_goal(&blocked, "capture-root", at())
        .await
        .unwrap()
        .snapshot;
    let turn = repo
        .accept_authoritative_turn(&input("capture-root", "ordinary-message", 1))
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = turn.outcome else {
        panic!("expected create")
    };
    let binding = db
        .conversation_goal_execution_binding(execution(&repo, turn_id).await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.observation,
        GoalAdmissionObservation::NoGoal {
            observed_version: stopped.version
        }
    );
}

#[tokio::test]
async fn turn_and_goal_observation_share_both_admission_crash_cuts() {
    for cut in [TransactionCut::BeforeCommit, TransactionCut::AfterCommit] {
        let (db, repo, command) = fixture().await;
        db.apply_conversation_goal(&command, "capture-root", at())
            .await
            .unwrap();
        let request = input("capture-root", "crash-cut", 1);
        assert!(repo
            .accept_authoritative_turn_at_cut(&request, cut)
            .await
            .is_err());
        let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_turns")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_bindings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let expected = i64::from(cut == TransactionCut::AfterCommit);
        assert_eq!((turns, bindings), (expected, expected));
        let retry = repo.accept_authoritative_turn(&request).await.unwrap();
        assert!(match cut {
            TransactionCut::BeforeCommit => matches!(retry.outcome, TurnOutcome::Created { .. }),
            TransactionCut::AfterCommit => matches!(retry.outcome, TurnOutcome::ExactReplay { .. }),
            TransactionCut::None => false,
        });
    }
}

#[tokio::test]
async fn accepted_steering_does_not_mint_an_execution_goal_binding() {
    let (db, repo, _) = fixture().await;
    let runtime = repo
        .accept_authoritative_turn(&input("capture-root", "runtime-owner", 1))
        .await
        .unwrap();
    let TurnOutcome::Created {
        turn_id: runtime_turn,
        ..
    } = runtime.outcome
    else {
        panic!("expected the runtime owner before steering");
    };
    let runtime_execution = execution(&repo, runtime_turn).await;
    let runtime_binding = db
        .conversation_goal_execution_binding(runtime_execution)
        .await
        .unwrap()
        .expect("runtime admission must capture its observation");
    let turn = repo
        .accept_authoritative_turn(&input_with_disposition(
            "capture-root",
            "steering",
            2,
            AcceptedDisposition::Steering,
        ))
        .await
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = turn.outcome else {
        panic!("expected create")
    };
    assert!(db
        .conversation_goal_execution_binding(execution(&repo, turn_id).await)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.conversation_goal_execution_binding(runtime_execution)
            .await
            .unwrap(),
        Some(runtime_binding),
    );
}
