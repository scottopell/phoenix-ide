use super::*;
use crate::conversation_goals::control::{
    ControlCommand, ControlError, ControlRequest, GoalControlInstallation,
};

async fn command(db: &Database, goal: &GoalMutation, command: ControlCommand, version: u64) {
    db.apply_goal_control(
        &ControlRequest {
            owner: goal.owner.clone(),
            principal_id: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            command,
            expected_control_version: version,
        },
        at(),
    )
    .await
    .unwrap();
}

async fn counts(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM workflows),(SELECT count(*) FROM durable_turns),(SELECT count(*) FROM conversation_goal_bindings),(SELECT count(*) FROM conversation_goal_control_bindings)")
        .fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn conversation_goal_installed_control_refusal_rolls_back_real_acceptance_with_or_without_goal(
) {
    for with_goal in [false, true] {
        let (db, _, goal) = fixture().await;
        if with_goal {
            db.apply_conversation_goal(&goal, "capture-root", at())
                .await
                .unwrap();
        }
        let db = db.install_goal_controls_for_tests().await.unwrap();
        let clone = db.clone();
        assert_eq!(
            clone.goal_control_installation(),
            GoalControlInstallation::Installed
        );
        let repo = clone.workflow_repository();
        assert!(repo
            .accept_authoritative_turn(&input("capture-root", "missing-control", 1))
            .await
            .is_err());
        assert_eq!(counts(&db).await, (0, 0, 0, 0));
        command(&db, &goal, ControlCommand::Stop, 0).await;
        assert!(repo
            .accept_authoritative_turn(&input("capture-root", "stopped-control", 1))
            .await
            .is_err());
        assert_eq!(counts(&db).await, (0, 0, 0, 0));
        command(&db, &goal, ControlCommand::Release, 1).await;
        let request = input("capture-root", "released-control", 1);
        let accepted = repo.accept_authoritative_turn(&request).await.unwrap();
        let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("created")
        };
        let execution = execution(&repo, turn_id).await;
        assert_eq!(counts(&db).await, (1, 1, 1, 1));
        let binding = db
            .conversation_goal_execution_binding(execution)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            matches!(binding.observation, GoalAdmissionObservation::Goal(_)),
            with_goal
        );
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        crate::conversation_goals::control::require_current_control_tx(
            &mut tx,
            &goal.owner,
            execution,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        command(&db, &goal, ControlCommand::Stop, 2).await;
        command(&db, &goal, ControlCommand::Release, 3).await;
        assert!(matches!(
            repo.accept_authoritative_turn(&request)
                .await
                .unwrap()
                .outcome,
            TurnOutcome::ExactReplay { .. }
        ));
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        assert!(matches!(
            crate::conversation_goals::control::require_current_control_tx(
                &mut tx,
                &goal.owner,
                execution
            )
            .await,
            Err(ControlError::AdmissionStopped)
        ));
        tx.rollback().await.unwrap();
        let version: i64 = sqlx::query_scalar(
            "SELECT control_version FROM conversation_goal_control_bindings WHERE turn_id=?",
        )
        .bind(execution.turn_id())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            version, 2,
            "exact acceptance retry must never recapture current control"
        );
        assert_eq!(counts(&db).await, (1, 1, 1, 1));
    }
}

#[tokio::test]
async fn conversation_goal_installed_capture_shares_real_acceptance_crash_cuts() {
    for cut in [TransactionCut::BeforeCommit, TransactionCut::AfterCommit] {
        let (db, _, goal) = fixture().await;
        let db = db.install_goal_controls_for_tests().await.unwrap();
        command(&db, &goal, ControlCommand::Release, 0).await;
        let repo = db.workflow_repository();
        let request = input("capture-root", "control-cut", 1);
        assert!(repo
            .accept_authoritative_turn_at_cut(&request, cut)
            .await
            .is_err());
        let expected = i64::from(cut == TransactionCut::AfterCommit);
        assert_eq!(counts(&db).await, (expected, expected, expected, expected));
        let retry = repo.accept_authoritative_turn(&request).await.unwrap();
        assert!(match cut {
            TransactionCut::BeforeCommit => matches!(retry.outcome, TurnOutcome::Created { .. }),
            TransactionCut::AfterCommit => matches!(retry.outcome, TurnOutcome::ExactReplay { .. }),
            TransactionCut::None => false,
        });
        assert_eq!(counts(&db).await, (1, 1, 1, 1));
    }
}

#[tokio::test]
async fn conversation_goal_schema_presence_does_not_enable_default_admission() {
    let (db, repo, _) = fixture().await;
    sqlx::raw_sql(include_str!(
        "../../../../conversation_goals/control_candidate.sql"
    ))
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(
        db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert!(matches!(
        repo.accept_authoritative_turn(&input("capture-root", "disabled", 1))
            .await
            .unwrap()
            .outcome,
        TurnOutcome::Created { .. }
    ));
    assert_eq!(counts(&db).await, (1, 1, 1, 0));
}

#[tokio::test]
async fn conversation_goal_installed_missing_schema_is_storage_failure_and_no_acceptance() {
    let (db, _, _) = fixture().await;
    let db = db.install_goal_controls_for_tests().await.unwrap();
    sqlx::query("DROP TABLE conversation_goal_control_bindings")
        .execute(db.pool())
        .await
        .unwrap();
    let error = db
        .workflow_repository()
        .accept_authoritative_turn(&input("capture-root", "missing-schema", 1))
        .await
        .unwrap_err();
    assert!(matches!(error, crate::DbError::Sqlx(_)));
    let rows: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM workflows),(SELECT count(*) FROM durable_turns),(SELECT count(*) FROM conversation_goal_bindings)")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(rows, (0, 0, 0));
}
