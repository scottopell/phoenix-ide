use super::*;
use crate::conversation_goals::execution::{
    append_request_tx, checkpoint_request_tx, retained_request_tx, GoalExecutionInstallation,
    GoalRequestError,
};
use phoenix_core::domain::conversation_goal::{
    GoalRequestCause, GoalRequestId, GoalRequestIntent, GoalRequestLimit,
};

fn installed(limit: i64) -> GoalExecutionInstallation {
    GoalExecutionInstallation::Installed {
        ceiling: GoalRequestLimit::parse(limit).unwrap(),
    }
}

async fn execution_fixture(limit: i64) -> (tempfile::TempDir, Database) {
    let (dir, mut db) = fixture().await;
    sqlx::raw_sql("ALTER TABLE durable_turns ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../execution_candidate.sql"))
        .execute(db.pool())
        .await
        .unwrap();
    db.goal_execution_installation = installed(limit);
    (dir, db)
}

async fn open_goal(db: &Database) -> GoalSnapshot {
    let snapshot = db
        .mutate_conversation_goal(&create(), at())
        .await
        .unwrap()
        .snapshot;
    bind(db, &binding(&snapshot)).await.unwrap();
    snapshot
}

fn intent(snapshot: &GoalSnapshot, id: &str, cause: GoalRequestCause) -> GoalRequestIntent {
    GoalRequestIntent::new(
        GoalRequestId::parse(id).unwrap(),
        snapshot.fence(),
        GoalExecution::parse(1, 1).unwrap(),
        0,
        cause,
    )
    .unwrap()
}

async fn append(
    db: &Database,
    intent: &GoalRequestIntent,
) -> std::result::Result<crate::conversation_goals::execution::GoalRequestAdmission, GoalRequestError>
{
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let admission = append_request_tx(&mut tx, db.goal_execution_installation(), intent).await?;
    tx.commit().await.unwrap();
    Ok(admission)
}

async fn checkpoint(db: &Database, intent: &GoalRequestIntent, key: &str) {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        intent,
        0,
        &t(key),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn request_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM conversation_goal_requests")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn conversation_goal_execution_disabled_bypasses_absent_policy_schema() {
    let (_dir, db) = fixture().await;
    assert_eq!(
        db.goal_execution_installation(),
        GoalExecutionInstallation::Disabled
    );
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "initial", GoalRequestCause::Initial);
    let mut tx = db.pool().begin().await.unwrap();
    assert!(matches!(
        append_request_tx(&mut tx, GoalExecutionInstallation::Disabled, &request).await,
        Err(GoalRequestError::Disabled)
    ));
    assert!(matches!(
        retained_request_tx(&mut tx, GoalExecutionInstallation::Disabled, &request).await,
        Err(GoalRequestError::Disabled)
    ));
    assert!(matches!(
        checkpoint_request_tx(
            &mut tx,
            GoalExecutionInstallation::Disabled,
            &request,
            0,
            &t("checkpoint")
        )
        .await,
        Err(GoalRequestError::Disabled)
    ));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn conversation_goal_execution_create_captures_policy_for_every_attributed_owner() {
    for actor in [user(), agent(false), agent(true)] {
        let (_dir, db) = execution_fixture(7).await;
        let mut mutation = create();
        mutation.attribution.requested_by = actor;
        let first = db.mutate_conversation_goal(&mutation, at()).await.unwrap();
        let limit: i64 = sqlx::query_scalar(
            "SELECT request_limit FROM conversation_goal_limits WHERE goal_id='goal-1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(limit, 7);
        let mut changed_policy = db.clone();
        changed_policy.goal_execution_installation = installed(99);
        assert_eq!(
            changed_policy
                .mutate_conversation_goal(&mutation, at())
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT request_limit FROM conversation_goal_limits WHERE goal_id='goal-1'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            7
        );
        let replacement = replace(&first.snapshot, user(), "new-goal");
        changed_policy
            .mutate_conversation_goal(&replacement, at())
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT request_limit FROM conversation_goal_limits WHERE goal_id='new-goal'"
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            99
        );
    }
}

#[tokio::test]
async fn conversation_goal_execution_prepolicy_replay_never_backfills_limit() {
    let (_dir, mut db) = execution_fixture(3).await;
    db.goal_execution_installation = GoalExecutionInstallation::Disabled;
    let first = db.mutate_conversation_goal(&create(), at()).await.unwrap();
    db.goal_execution_installation = installed(3);
    assert_eq!(
        db.mutate_conversation_goal(&create(), at()).await.unwrap(),
        first
    );
    bind(&db, &binding(&first.snapshot)).await.unwrap();
    assert!(matches!(
        append(
            &db,
            &intent(&first.snapshot, "first", GoalRequestCause::Initial)
        )
        .await,
        Err(GoalRequestError::Ineligible)
    ));
    assert!(
        sqlx::query("INSERT INTO conversation_goal_limits VALUES ('goal-1',3)")
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn conversation_goal_execution_exact_replay_precedes_capacity_and_causal_conflict() {
    let (_dir, db) = execution_fixture(1).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "first", GoalRequestCause::Initial);
    let accepted = append(&db, &request).await.unwrap();
    assert_eq!(accepted.ordinal, 1);
    assert_eq!(append(&db, &request).await.unwrap(), accepted);
    let different_uuid = intent(&snapshot, "second-uuid", GoalRequestCause::Initial);
    assert!(matches!(
        append(&db, &different_uuid).await,
        Err(GoalRequestError::Conflict)
    ));
    let summary = intent(
        &snapshot,
        "summary",
        GoalRequestCause::Summary {
            operation: t("continuation"),
        },
    );
    assert!(matches!(
        append(&db, &summary).await,
        Err(GoalRequestError::Exhausted)
    ));
    assert_eq!(request_count(&db).await, 1);
}

#[tokio::test]
async fn conversation_goal_execution_summary_and_next_checkpoint_are_new_charges_not_retries() {
    let (_dir, db) = execution_fixture(3).await;
    let snapshot = open_goal(&db).await;
    let initial = intent(&snapshot, "initial", GoalRequestCause::Initial);
    append(&db, &initial).await.unwrap();
    let next = intent(
        &snapshot,
        "next",
        GoalRequestCause::AfterCheckpoint {
            predecessor: initial.id.clone(),
        },
    );
    assert!(append(&db, &next).await.is_err());
    checkpoint(&db, &initial, "response-initial").await;
    assert_eq!(append(&db, &next).await.unwrap().ordinal, 2);
    let summary = intent(
        &snapshot,
        "summary",
        GoalRequestCause::Summary {
            operation: t("handoff"),
        },
    );
    assert_eq!(append(&db, &summary).await.unwrap().ordinal, 3);
    assert_eq!(append(&db, &summary).await.unwrap().ordinal, 3);
    let duplicate_cause = intent(&snapshot, "new-summary-uuid", summary.cause.clone());
    assert!(matches!(
        append(&db, &duplicate_cause).await,
        Err(GoalRequestError::Conflict)
    ));
    assert_eq!(request_count(&db).await, 3);
}

#[tokio::test]
async fn conversation_goal_execution_charges_survive_turn_workflow_and_transcript_cleanup() {
    let (_dir, db) = execution_fixture(2).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "first", GoalRequestCause::Initial);
    append(&db, &request).await.unwrap();
    checkpoint(&db, &request, "response").await;
    sqlx::query("DELETE FROM durable_turns WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::raw_sql("DELETE FROM durable_turns WHERE conversation_id='root'; DELETE FROM workflows WHERE workflow_id IN (1,4); DELETE FROM messages WHERE conversation_id='root'; DELETE FROM conversations WHERE id='root';")
        .execute(db.pool()).await.unwrap();
    let retained = append(&db, &request).await.unwrap();
    assert_eq!(retained.intent, request);
    assert_eq!(retained.live_turn_id, None);
    assert_eq!(retained.checkpoint, Some(t("response")));
    checkpoint(&db, &request, "response").await;
    for edit in [
        "UPDATE conversation_goal_requests SET live_turn_id=2",
        "UPDATE conversation_goal_requests SET goal_id='other'",
        "UPDATE conversation_goal_requests SET ordinal=2",
        "UPDATE conversation_goal_requests SET historical_turn_id=2",
        "UPDATE conversation_goal_requests SET cause='summary',summary_operation='changed'",
        "DELETE FROM conversation_goal_requests",
        "DELETE FROM conversation_goal_request_checkpoints",
        "UPDATE conversation_goal_request_checkpoints SET checkpoint_key='changed'",
        "UPDATE conversation_goal_limits SET request_limit=99",
        "DELETE FROM conversation_goal_limits",
    ] {
        assert!(
            sqlx::query(edit).execute(db.pool()).await.is_err(),
            "{edit}"
        );
    }
    sqlx::raw_sql("DELETE FROM conversations WHERE product_conversation_id='owner'; DELETE FROM product_conversations WHERE id='owner';").execute(db.pool()).await.unwrap();
    assert_eq!(request_count(&db).await, 0);
}

#[tokio::test]
async fn conversation_goal_execution_detachment_is_not_permission_for_new_checkpoint() {
    let (_dir, db) = execution_fixture(2).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "first", GoalRequestCause::Initial);
    append(&db, &request).await.unwrap();
    sqlx::query("DELETE FROM durable_turns WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        checkpoint_request_tx(
            &mut tx,
            db.goal_execution_installation(),
            &request,
            0,
            &t("late")
        )
        .await,
        Err(GoalRequestError::Detached)
    ));
    tx.rollback().await.unwrap();
    assert_eq!(request_count(&db).await, 1);
}

#[tokio::test]
async fn conversation_goal_execution_admission_and_checkpoint_rollback_with_owning_transaction() {
    let (_dir, db) = execution_fixture(2).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "first", GoalRequestCause::Initial);
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    append_request_tx(&mut tx, db.goal_execution_installation(), &request)
        .await
        .unwrap();
    checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &request,
        0,
        &t("response"),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(request_count(&db).await, 0);
    assert_eq!(append(&db, &request).await.unwrap().ordinal, 1);
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &request,
        0,
        &t("response"),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    assert_eq!(
        retained_request_tx(&mut tx, db.goal_execution_installation(), &request)
            .await
            .unwrap()
            .unwrap()
            .checkpoint,
        None
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn conversation_goal_execution_same_cause_competing_uuids_charge_once() {
    let (_dir, db) = execution_fixture(2).await;
    let snapshot = open_goal(&db).await;
    let first = intent(&snapshot, "one", GoalRequestCause::Initial);
    let second = intent(&snapshot, "two", GoalRequestCause::Initial);
    let (one, two) = tokio::join!(append(&db, &first), append(&db, &second));
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(GoalRequestError::Conflict)) | (Err(GoalRequestError::Conflict), Ok(_))
    ));
    assert_eq!(request_count(&db).await, 1);
}

#[tokio::test]
async fn conversation_goal_execution_schema_rejects_gaps_replacement_and_noninteger_capacity() {
    let (_dir, db) = execution_fixture(i64::MAX).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "first", GoalRequestCause::Initial);
    append(&db, &request).await.unwrap();
    for edit in [
        "INSERT OR REPLACE INTO conversation_goal_limits VALUES ('goal-1',9223372036854775807)",
        "INSERT OR REPLACE INTO conversation_goal_requests (request_id,goal_id,ordinal,owner_id,observed_version,historical_workflow_id,historical_turn_id,generation,cause,live_turn_id) VALUES ('first','goal-1',1,'owner',1,1,1,0,'initial',1)",
        "INSERT INTO conversation_goal_requests (request_id,goal_id,ordinal,owner_id,observed_version,historical_workflow_id,historical_turn_id,generation,cause,summary_operation,live_turn_id) VALUES ('gap','goal-1',3,'owner',1,1,1,0,'summary','gap',1)",
        "INSERT INTO conversation_goal_requests (request_id,goal_id,ordinal,owner_id,observed_version,historical_workflow_id,historical_turn_id,generation,cause,summary_operation,live_turn_id) VALUES ('real','goal-1',2.5,'owner',1,1,1,0,'summary','real',1)",
        "INSERT INTO conversation_goal_requests (request_id,goal_id,ordinal,owner_id,observed_version,historical_workflow_id,historical_turn_id,generation,cause,summary_operation,live_turn_id) VALUES ('overflow','goal-1',9223372036854775808,'owner',1,1,1,0,'summary','overflow',1)",
    ] {
        assert!(sqlx::query(edit).execute(db.pool()).await.is_err(), "{edit}");
    }
    assert_eq!(request_count(&db).await, 1);
    assert_eq!(append(&db, &request).await.unwrap().ordinal, 1);
}

#[tokio::test]
async fn conversation_goal_execution_create_and_limit_roll_back_as_one_aggregate() {
    let (_dir, db) = execution_fixture(4).await;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    apply_goal_tx(&mut tx, &create(), None, at(), db.goal_write_policies())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_goal_limits")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(db.conversation_goal(&p("owner")).await.unwrap().is_none());
    let created = db.mutate_conversation_goal(&create(), at()).await.unwrap();
    assert_eq!(created.snapshot.version.get(), 1);
}

#[tokio::test]
async fn conversation_goal_execution_restored_claim_keeps_initial_slot_and_historical_intent() {
    let (_dir, db) = execution_fixture(3).await;
    let snapshot = open_goal(&db).await;
    let request = intent(&snapshot, "initial", GoalRequestCause::Initial);
    append(&db, &request).await.unwrap();
    sqlx::query("UPDATE durable_turns SET generation=1 WHERE turn_id=1")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(append(&db, &request).await.unwrap().intent, request);
    let replacement = GoalRequestIntent::new(
        GoalRequestId::parse("replacement").unwrap(),
        snapshot.fence(),
        request.execution,
        1,
        GoalRequestCause::Initial,
    )
    .unwrap();
    assert!(matches!(
        append(&db, &replacement).await,
        Err(GoalRequestError::Conflict)
    ));
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        checkpoint_request_tx(
            &mut tx,
            db.goal_execution_installation(),
            &request,
            0,
            &t("response")
        )
        .await,
        Err(GoalRequestError::Detached)
    ));
    checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &request,
        1,
        &t("response"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(request_count(&db).await, 1);
}
