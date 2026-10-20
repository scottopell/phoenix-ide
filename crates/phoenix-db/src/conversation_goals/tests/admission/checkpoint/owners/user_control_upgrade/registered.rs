use super::*;
use crate::conversation_goals::tests::{full_substrate, full_substrate_before_user_control};
use crate::conversation_goals::user_commands::{GoalUserChange, GoalUserCommand, GoalUserResult};
use phoenix_core::domain::conversation_goal::{GoalText, GoalTimestampUs, GoalVersion};

const MIGRATION_NAME: &str = "durable_conversation_goal_user_control";

async fn assert_healthy(db: &Database) {
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(foreign_keys, 1);
    let stages: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_temp_master WHERE name LIKE 'goal_user_%'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stages, 0);
}

async fn assert_registered(db: &Database) {
    let name: String = sqlx::query_scalar("SELECT name FROM _migrations WHERE version=134")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(name, MIGRATION_NAME);
    crate::conversation_goals::tests::assert_current_135_schema_and_goal_ledger(db).await;
    assert_eq!(
        db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_healthy(db).await;
}

#[tokio::test]
async fn conversation_goal_registered_134_through_135_fresh_and_real_133_upgrade_have_same_schema_and_stay_disabled(
) {
    let fresh = Database::open_in_memory().await.unwrap();
    assert_registered(&fresh).await;
    let (prior, _) = full_substrate_before_user_control().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations WHERE version=134")
        .fetch_one(prior.pool())
        .await
        .unwrap();
    assert_eq!(before, 0);
    let control_tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('conversation_goal_controls','conversation_goal_user_requests')",
    ).fetch_one(prior.pool()).await.unwrap();
    assert_eq!(control_tables, 0);
    assert_eq!(
        crate::migrations::run_pending_migrations(prior.pool())
            .await
            .unwrap(),
        2
    );
    assert_eq!(schema(&prior).await, schema(&fresh).await);
    assert_registered(&prior).await;
    for (table, query) in [
        (
            "conversation_goal_controls",
            "SELECT count(*) FROM conversation_goal_controls",
        ),
        (
            "conversation_goal_control_requests",
            "SELECT count(*) FROM conversation_goal_control_requests",
        ),
        (
            "conversation_goal_user_requests",
            "SELECT count(*) FROM conversation_goal_user_requests",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(prior.pool())
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "schema upgrade cannot manufacture user authority: {table}"
        );
    }
}

async fn source_rows(db: &Database) -> serde_json::Value {
    let source = db.get_conversation("full-root").await.unwrap();
    let turns: Vec<(i64, i64, i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT turn_id,workflow_id,generation,terminal_kind,owns_conversation FROM durable_turns ORDER BY turn_id",
    ).fetch_all(db.pool()).await.unwrap();
    serde_json::json!({
        "messages":db.get_messages("full-root").await.unwrap(),
        "state":source.state, "state_updated_at":source.state_updated_at, "turns":turns,
        "anthropic":db.load_provider_replay_state("full-root").await.unwrap(),
        "responses":db.load_responses_replay_state("full-root").await.unwrap(),
    })
}

#[tokio::test]
async fn conversation_goal_registered_134_through_135_preserves_real_report_cycle_and_exact_checkpoint_replay(
) {
    for owner in [Owner::Normal, Owner::Anthropic, Owner::Responses] {
        populated_upgrade(owner).await;
    }
}

async fn populated_upgrade(owner: Owner) {
    let (db, report) = populated_for(GoalControlInstallation::Disabled, owner).await;
    assert_eq!(
        db.load_provider_replay_state("full-root")
            .await
            .unwrap()
            .is_some(),
        matches!(owner, Owner::Anthropic)
    );
    assert_eq!(
        !db.load_responses_replay_state("full-root")
            .await
            .unwrap()
            .is_empty(),
        matches!(owner, Owner::Responses)
    );
    let before = retained(&db).await;
    let source = source_rows(&db).await;
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        2
    );
    assert_eq!(retained(&db).await, before);
    assert_eq!(source_rows(&db).await, source);
    let fresh = Database::open_in_memory().await.unwrap();
    assert_eq!(schema(&db).await, schema(&fresh).await);
    assert_registered(&db).await;
    let cycle: (i64, i64) = sqlx::query_as(
        "SELECT h.report_turn_id,b.observed_version FROM conversation_goal_history h JOIN conversation_goal_bindings b ON b.turn_id=h.report_turn_id WHERE h.change_kind='report'",
    ).fetch_one(db.pool()).await.unwrap();
    assert_eq!(cycle, (report.execution().turn_id(), 1));
    replay_report(owner, &db, &report).await;
    assert_eq!(retained(&db).await, before);
    assert_eq!(source_rows(&db).await, source);
    assert_healthy(&db).await;
    db.delete_conversation("full-root").await.unwrap();
    assert_eq!(
        retained(&db).await,
        RetainedRows {
            actors: vec![],
            history: vec![],
            bindings: vec![],
            identities: vec![]
        }
    );
    assert_healthy(&db).await;
}

async fn replay_report(owner: Owner, db: &Database, report: &GoalPendingReport) {
    let messages = db.get_messages("full-root").await.unwrap();
    let assistant = messages
        .iter()
        .find(|message| matches!(message.content, MessageContent::Agent(_)))
        .unwrap();
    let conversation = db.get_conversation("full-root").await.unwrap();
    let results = [ToolRoundResult::PendingGoalReport {
        report: report.clone(),
        duration_ms: 1,
    }];
    let checkpoint = GoalRoundCheckpoint {
        assistant,
        execution: report.execution(),
        results: &results,
        state: &conversation.state,
        state_updated_at: conversation.state_updated_at,
    };
    let (replayed_results, replayed) = persist(owner, db, &checkpoint).await.unwrap();
    assert!(replayed);
    for message in replayed_results {
        let retained = messages
            .iter()
            .find(|retained| retained.message_id == message.message_id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::to_value(retained).unwrap()
        );
    }
}

#[tokio::test]
async fn conversation_goal_registered_134_through_135_real_runner_failure_rolls_back_cycle_schema_and_ledger(
) {
    let (db, _) = populated_at(GoalControlInstallation::Disabled).await;
    sqlx::query("CREATE TRIGGER fail_registered_goal_rebuild BEFORE DELETE ON conversation_goal_bindings BEGIN SELECT RAISE(ABORT,'registered goal rebuild cut'); END")
        .execute(db.pool()).await.unwrap();
    let before = retained(&db).await;
    let before_schema = schema(&db).await;
    let source = source_rows(&db).await;
    let error = crate::migrations::run_pending_migrations(db.pool())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("registered goal rebuild cut"),
        "{error}"
    );
    assert_eq!(retained(&db).await, before);
    assert_eq!(schema(&db).await, before_schema);
    assert_eq!(source_rows(&db).await, source);
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations WHERE version=134")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(ledger, 0);
    assert_healthy(&db).await;
    sqlx::query("DROP TRIGGER fail_registered_goal_rebuild")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        2
    );
    assert_eq!(retained(&db).await, before);
    assert_registered(&db).await;
}

async fn assert_unreceipted_actor_rejected(db: &Database, command: &GoalUserCommand) {
    let unreceipted_request = uuid::Uuid::new_v4();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let error = sqlx::query(
        "INSERT INTO conversation_goal_actors(owner_id,actor_id,kind,control_principal_id,control_request_id,control_command,control_result_version) VALUES(?,9000,'user_control',?,?,'state',2)",
    ).bind(command.owner.as_str()).bind(command.principal_id.as_bytes().as_slice())
        .bind(unreceipted_request.as_bytes().as_slice()).execute(&mut *tx).await.unwrap_err();
    assert!(
        error
            .as_database_error()
            .is_some_and(|error| error.is_foreign_key_violation()),
        "immediate actor->receipt FK must reject valid actor shape: {error}"
    );
    tx.rollback().await.unwrap();
    assert_healthy(db).await;
}

async fn assert_receipt_without_next_history_rejected(db: &Database, command: &GoalUserCommand) {
    let request_id = uuid::Uuid::new_v4();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let inserted = sqlx::query(
        "INSERT INTO conversation_goal_user_requests(owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,recorded_at_us) SELECT owner_id,?,?,'state',1,goal_id,'accepted',2,101 FROM conversation_goal_history WHERE owner_id=? AND version=1",
    ).bind(command.principal_id.as_bytes().as_slice()).bind(request_id.as_bytes().as_slice())
        .bind(command.owner.as_str()).execute(&mut *tx).await.unwrap();
    assert_eq!(inserted.rows_affected(), 1);
    let error = tx.commit().await.unwrap_err();
    assert!(
        error
            .as_database_error()
            .is_some_and(|error| error.is_foreign_key_violation()),
        "deferred receipt->history FK must reject missing next version at commit: {error}"
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_user_requests WHERE owner_id=? AND principal_id=? AND request_id=?",
    ).bind(command.owner.as_str()).bind(command.principal_id.as_bytes().as_slice())
        .bind(request_id.as_bytes().as_slice()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(receipts, 0);
    assert_healthy(db).await;
}

#[tokio::test]
async fn conversation_goal_registered_134_through_135_receipts_replay_exactly_and_reject_changed_intent(
) {
    let (db, owner) = full_substrate().await;
    let command = GoalUserCommand {
        owner,
        principal_id: uuid::Uuid::new_v4(),
        request_id: uuid::Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: GoalText::parse("Exact registered objective\n").unwrap(),
        },
    };
    let first = db
        .apply_user_goal_command(&command, "full-root", GoalTimestampUs::parse(10).unwrap())
        .await
        .unwrap();
    assert!(matches!(first.result, GoalUserResult::Accepted { .. }));
    let before = retained(&db).await;
    let replay = db
        .apply_user_goal_command(&command, "full-root", GoalTimestampUs::parse(99).unwrap())
        .await
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(retained(&db).await, before);
    assert_unreceipted_actor_rejected(&db, &command).await;
    assert_eq!(retained(&db).await, before);
    assert_receipt_without_next_history_rejected(&db, &command).await;
    assert_eq!(retained(&db).await, before);
    let changed = GoalUserCommand {
        change: GoalUserChange::Create {
            objective: GoalText::parse("Different objective").unwrap(),
        },
        ..command
    };
    assert!(matches!(
        db.apply_user_goal_command(&changed, "full-root", GoalTimestampUs::parse(100).unwrap())
            .await,
        Err(crate::conversation_goals::GoalDbError::RetryConflict)
    ));
    assert_eq!(retained(&db).await, before);
    assert_registered(&db).await;
}
