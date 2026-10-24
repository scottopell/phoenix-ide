use super::*;
use crate::conversation_goals::control::GoalControlInstallation;
use crate::coordinator_watches::WatchOutcome;
use phoenix_core::domain::watch_goal::{WatchGoalCapture, WatchGoalState};

async fn installed() -> (Database, Message, GoalPendingReport) {
    let fixture = admitted_fixture_with_control(GoalControlInstallation::Installed).await;
    fixture
        .0
        .watch_product_conversation(&fixture.2.mutation().owner)
        .await
        .unwrap();
    fixture
}

#[tokio::test]
async fn nonboundary_report_has_no_standalone_packet_or_assumed_terminal() {
    let (db, assistant, report) = installed().await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 7,
    }];
    let state = ConvState::LlmRequesting { attempt: 2 };
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    persist(Owner::Normal, &db, &checkpoint).await.unwrap();
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .is_empty());
    assert!(persist(Owner::Normal, &db, &checkpoint).await.unwrap().1);
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn report_wait_capture_failure_rolls_back_the_actual_checkpoint_then_wait_owns_one_event() {
    let (db, assistant, report) = installed().await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 7,
    }];
    let state = ConvState::AwaitingTaskApproval {
        task_file: "tasks/12345-p1-ready--watch.md".into(),
        title: "Review".into(),
        priority: phoenix_core::task_source::Priority::P1,
        plan: "Review".into(),
    };
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    sqlx::query("CREATE TRIGGER fail_goal_watch_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'wait capture failure'); END").execute(db.pool()).await.unwrap();
    assert!(persist(Owner::Normal, &db, &checkpoint).await.is_err());
    assert_no_round(&db, &report).await;
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DROP TRIGGER fail_goal_watch_capture")
        .execute(db.pool())
        .await
        .unwrap();
    persist(Owner::Normal, &db, &checkpoint).await.unwrap();
    let events = db.pending_coordinator_watch_events(16).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, WatchOutcome::AwaitingTaskApproval);
    assert!(matches!(
        &events[0].goal_capture,
        Some(WatchGoalCapture::CapturedSavedGoal {
            owner_version: 2,
            state: WatchGoalState::Complete { .. },
            ..
        })
    ));
    assert!(persist(Owner::Normal, &db, &checkpoint).await.unwrap().1);
    assert_eq!(
        db.pending_coordinator_watch_events(16).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn committed_report_survives_later_terminal_capture_failure_with_no_early_notification() {
    let (db, assistant, report) = installed().await;
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 7,
    }];
    let state = ConvState::Idle;
    let checkpoint = GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &state,
        state_updated_at: Utc::now(),
    };
    persist(Owner::Terminal, &db, &checkpoint).await.unwrap();
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .is_empty());
    let message_count = db.get_messages("full-root").await.unwrap().len();
    sqlx::query("CREATE TRIGGER fail_goal_watch_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'terminal capture failure'); END").execute(db.pool()).await.unwrap();
    let input = TerminalizeAuthoritativeTurnInput {
        command: TurnCommand::Complete {
            turn_id: TurnAuthorityId(1),
            expected_generation: 0,
        },
        projection: Some(obligation(&checkpoint).projection),
        provider_replay_settlement: ProviderReplaySettlement::Preserve,
    };
    assert!(db
        .workflow_repository()
        .terminalize_authoritative_turn(&input)
        .await
        .is_err());
    assert_eq!(
        db.conversation_goal(&report.mutation().owner)
            .await
            .unwrap()
            .unwrap()
            .version
            .get(),
        2
    );
    assert_eq!(
        db.get_messages("full-root").await.unwrap().len(),
        message_count
    );
    assert_eq!(
        db.get_conversation("full-root").await.unwrap().state,
        ConvState::LlmRequesting { attempt: 1 }
    );
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DROP TRIGGER fail_goal_watch_capture")
        .execute(db.pool())
        .await
        .unwrap();
    db.workflow_repository()
        .terminalize_authoritative_turn(&input)
        .await
        .unwrap();
    let events = db.pending_coordinator_watch_events(16).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, WatchOutcome::Completed);
    assert!(matches!(
        &events[0].goal_capture,
        Some(WatchGoalCapture::CapturedSavedGoal {
            owner_version: 2,
            state: WatchGoalState::Complete { .. },
            ..
        })
    ));
}
