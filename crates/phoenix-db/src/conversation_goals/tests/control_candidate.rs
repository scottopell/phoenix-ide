use super::*;

#[test]
fn conversation_goal_control_db_error_preserves_typed_admission() {
    use crate::conversation_goals::control::{db_error, ControlError};
    use phoenix_core::domain::product_conversation::ProductConversationId;

    let owner = ProductConversationId::parse("unavailable-owner").unwrap();
    let error = db_error(ControlError::Admission(
        crate::DbError::ProductConversationUnavailable(owner.clone()),
    ));
    assert!(
        matches!(error, crate::DbError::ProductConversationUnavailable(actual) if actual == owner)
    );

    let error = db_error(ControlError::Admission(
        crate::DbError::ConversationNotFound("missing-owner".to_owned()),
    ));
    assert!(
        matches!(error, crate::DbError::ConversationNotFound(actual) if actual == "missing-owner")
    );
}

async fn control_fixture() -> (tempfile::TempDir, Database) {
    let (dir, db) = fixture().await;
    sqlx::query("ALTER TABLE durable_turns ADD COLUMN materialized INTEGER GENERATED ALWAYS AS (canonical_message_id IS NOT NULL) VIRTUAL")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql("ALTER TABLE product_conversations ADD COLUMN ordinary_lifecycle TEXT NOT NULL DEFAULT 'open' CHECK (ordinary_lifecycle IN ('open','history')); CREATE TABLE close_obligations (product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE, phase TEXT NOT NULL);")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql(include_str!("../control_candidate.sql"))
        .execute(db.pool())
        .await
        .unwrap();
    (dir, db)
}

async fn accepted(
    db: &Database,
    command: &str,
    expected: i64,
    request: u8,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,result_control_version,result_stopped,recorded_at_us) VALUES ('owner',?1,?2,?3,?4,'accepted',?5,?6,10)")
        .bind(vec![1_u8; 16]).bind(vec![request; 16]).bind(command).bind(expected)
        .bind(expected + 1).bind(i64::from(command == "stop"))
        .execute(db.pool()).await.map(|_| ())
}

async fn bind_open(
    db: &Database,
    turn: i64,
    version: i64,
    request: u8,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO conversation_goal_control_bindings (owner_id,turn_id,workflow_id,control_version,principal_id,request_id) VALUES ('owner',?1,?1,?2,?3,?4)")
        .bind(turn).bind(version).bind(vec![1_u8; 16]).bind(vec![request; 16])
        .execute(db.pool()).await.map(|_| ())
}

async fn control_version(db: &Database) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT control_version,stopped FROM conversation_goal_controls WHERE owner_id='owner'",
    )
    .fetch_optional(db.pool())
    .await
    .unwrap()
}

async fn binding_is_current(db: &Database, turn: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversation_goal_control_bindings b JOIN conversation_goal_controls c ON c.owner_id=b.owner_id AND c.control_version=b.control_version AND c.stopped=0 WHERE b.turn_id=?)")
        .bind(turn).fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn conversation_goal_control_candidate_replace_cannot_rewrite_receipt_rewind_or_recapture() {
    let (_dir, db) = control_fixture().await;
    accepted(&db, "release", 0, 1).await.unwrap();
    bind_open(&db, 1, 1, 1).await.unwrap();
    accepted(&db, "stop", 1, 2).await.unwrap();
    accepted(&db, "release", 2, 3).await.unwrap();
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_control_bindings (owner_id,turn_id,workflow_id,control_version,principal_id,request_id) VALUES ('owner',1,1,3,?1,?2)")
        .bind(vec![1_u8;16]).bind(vec![3_u8;16]).execute(db.pool()).await.is_err());
    let version: i64 = sqlx::query_scalar(
        "SELECT control_version FROM conversation_goal_control_bindings WHERE turn_id=1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(version, 1);
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_controls (owner_id,control_version,stopped,principal_id,request_id) VALUES ('owner',1,0,?1,?2)")
        .bind(vec![1_u8;16]).bind(vec![1_u8;16]).execute(db.pool()).await.is_err());
    assert_eq!(control_version(&db).await, Some((3, 0)));
    let rejected = "INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us) VALUES ('owner',?1,?2,'stop',10,'rejected','control_version_conflict',3,0,10)";
    sqlx::query(rejected)
        .bind(vec![1_u8; 16])
        .bind(vec![4_u8; 16])
        .execute(db.pool())
        .await
        .unwrap();
    assert!(sqlx::query("INSERT OR REPLACE INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us) VALUES ('owner',?1,?2,'stop',10,'rejected','control_version_conflict',3,0,99)")
        .bind(vec![1_u8; 16])
        .bind(vec![4_u8; 16])
        .execute(db.pool())
        .await
        .is_err());
    let recorded: i64 = sqlx::query_scalar(
        "SELECT recorded_at_us FROM conversation_goal_control_requests WHERE outcome='rejected'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(recorded, 10);
}

#[tokio::test]
async fn conversation_goal_control_candidate_null_owner_cannot_bypass_receipt_foreign_key() {
    let (_dir, db) = control_fixture().await;
    let error = sqlx::query("INSERT INTO conversation_goal_controls (owner_id,control_version,stopped,principal_id,request_id) VALUES (NULL,1,0,?1,?2)")
        .bind(vec![1_u8; 16])
        .bind(vec![1_u8; 16])
        .execute(db.pool())
        .await
        .unwrap_err();
    assert!(
        matches!(error, sqlx::Error::Database(ref db_error) if matches!(db_error.kind(), sqlx::error::ErrorKind::NotNullViolation))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_goal_controls")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn conversation_goal_control_candidate_missing_and_stopped_are_non_admitting() {
    let (_dir, db) = control_fixture().await;
    assert_eq!(control_version(&db).await, None);
    assert!(bind_open(&db, 1, 1, 1).await.is_err());
    accepted(&db, "stop", 0, 1).await.unwrap();
    assert_eq!(control_version(&db).await, Some((1, 1)));
    assert!(bind_open(&db, 1, 1, 1).await.is_err());
    accepted(&db, "release", 1, 2).await.unwrap();
    bind_open(&db, 1, 2, 2).await.unwrap();
    assert!(binding_is_current(&db, 1).await);
    accepted(&db, "stop", 2, 3).await.unwrap();
    assert!(!binding_is_current(&db, 1).await);
    accepted(&db, "release", 3, 4).await.unwrap();
    assert!(!binding_is_current(&db, 1).await);
    assert!(bind_open(&db, 4, 2, 2).await.is_err());
    bind_open(&db, 4, 4, 4).await.unwrap();
    assert!(binding_is_current(&db, 4).await);
}

#[tokio::test]
async fn conversation_goal_control_candidate_rejected_receipt_is_retained_not_authority() {
    let (_dir, db) = control_fixture().await;
    sqlx::query("INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,rejection,result_control_version,result_stopped,recorded_at_us) VALUES ('owner',?1,?2,'release',8,'rejected','control_version_conflict',0,NULL,10)")
        .bind(vec![1_u8; 16]).bind(vec![7_u8; 16]).execute(db.pool()).await.unwrap();
    assert_eq!(control_version(&db).await, None);
    assert!(accepted(&db, "release", 0, 7).await.is_err());
    assert!(sqlx::query(
        "UPDATE conversation_goal_control_requests SET outcome='accepted' WHERE owner_id='owner'"
    )
    .execute(db.pool())
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM conversation_goal_control_requests WHERE owner_id='owner'")
            .execute(db.pool())
            .await
            .is_err()
    );
    accepted(&db, "release", 0, 8).await.unwrap();
    assert!(bind_open(&db, 1, 1, 7).await.is_err());
    let outcome: String = sqlx::query_scalar(
        "SELECT outcome FROM conversation_goal_control_requests WHERE request_id=?",
    )
    .bind(vec![7_u8; 16])
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(outcome, "rejected");
}

#[tokio::test]
async fn conversation_goal_control_candidate_receipt_and_latch_roll_back_together() {
    let (_dir, db) = control_fixture().await;
    accepted(&db, "release", 0, 1).await.unwrap();
    sqlx::raw_sql("CREATE TRIGGER fail_control_update BEFORE UPDATE ON conversation_goal_controls BEGIN SELECT RAISE(ABORT,'control fault'); END;")
        .execute(db.pool()).await.unwrap();
    assert!(accepted(&db, "stop", 1, 2).await.is_err());
    assert_eq!(control_version(&db).await, Some((1, 0)));
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_control_requests WHERE owner_id='owner'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 1);
    sqlx::query("DROP TRIGGER fail_control_update")
        .execute(db.pool())
        .await
        .unwrap();
    accepted(&db, "stop", 1, 2).await.unwrap();
    assert_eq!(control_version(&db).await, Some((2, 1)));
}

#[tokio::test]
async fn conversation_goal_control_candidate_ordinary_owner_and_source_deletion_constraints() {
    let (_dir, db) = control_fixture().await;
    accepted(&db, "release", 0, 1).await.unwrap();
    bind_open(&db, 1, 1, 1).await.unwrap();
    assert!(sqlx::query("INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,result_control_version,result_stopped,recorded_at_us) VALUES ('global',?1,?2,'release',0,'accepted',1,0,10)")
        .bind(vec![1_u8; 16]).bind(vec![1_u8; 16]).execute(db.pool()).await.is_err());
    sqlx::raw_sql("DELETE FROM durable_turns WHERE conversation_id IN ('root','successor'); DELETE FROM messages WHERE conversation_id IN ('root','successor'); DELETE FROM conversations WHERE product_conversation_id='owner';")
        .execute(db.pool()).await.unwrap();
    assert_eq!(control_version(&db).await, Some((1, 0)));
    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_control_bindings WHERE owner_id='owner'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(bindings, 1);
    sqlx::query("DELETE FROM product_conversations WHERE id='owner'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(control_version(&db).await, None);
    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_control_bindings WHERE owner_id='owner'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(bindings, 0);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_goal_control_requests WHERE owner_id='owner'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 0);
}

async fn check_current_control(
    db: &Database,
    owner: &str,
    workflow: i64,
    turn: i64,
) -> std::result::Result<(), crate::conversation_goals::control::ControlError> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let result = crate::conversation_goals::control::require_current_control_tx(
        &mut tx,
        &p(owner),
        GoalExecution::parse(workflow, turn).unwrap(),
    )
    .await;
    tx.rollback().await?;
    result
}

#[tokio::test]
async fn conversation_goal_control_candidate_execution_recheck_preserves_storage_failure() {
    let (_dir, db) = fixture().await;
    assert!(matches!(
        check_current_control(&db, "owner", 1, 1).await,
        Err(crate::conversation_goals::control::ControlError::Storage(_))
    ));
}

#[tokio::test]
async fn conversation_goal_control_candidate_execution_recheck_never_recaptures_after_stop_release()
{
    use crate::conversation_goals::control::{ControlCommand, ControlError, ControlRequest};
    let (_dir, db) = control_fixture().await;
    let principal_id = uuid::Uuid::new_v4();
    assert!(matches!(
        check_current_control(&db, "owner", 1, 1).await,
        Err(ControlError::AdmissionStopped)
    ));
    for (expected, command) in [
        (0, ControlCommand::Release),
        (1, ControlCommand::Stop),
        (2, ControlCommand::Release),
    ] {
        db.apply_goal_control(
            &ControlRequest {
                owner: p("owner"),
                principal_id,
                request_id: uuid::Uuid::new_v4(),
                command,
                expected_control_version: expected,
            },
            at(),
        )
        .await
        .unwrap();
        if expected == 0 {
            let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
            crate::conversation_goals::control::bind_control_at_admission_tx(
                &mut tx,
                &p("owner"),
                GoalExecution::parse(1, 1).unwrap(),
            )
            .await
            .unwrap();
            crate::conversation_goals::control::require_current_control_tx(
                &mut tx,
                &p("owner"),
                GoalExecution::parse(1, 1).unwrap(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            assert!(check_current_control(&db, "owner", 1, 1).await.is_ok());
            for (owner, workflow, turn) in [("other", 1, 1), ("owner", 4, 1), ("owner", 4, 4)] {
                assert!(matches!(
                    check_current_control(&db, owner, workflow, turn).await,
                    Err(ControlError::AdmissionStopped)
                ));
            }
        } else {
            assert!(matches!(
                check_current_control(&db, "owner", 1, 1).await,
                Err(ControlError::AdmissionStopped)
            ));
        }
    }
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    crate::conversation_goals::control::bind_control_at_admission_tx(
        &mut tx,
        &p("owner"),
        GoalExecution::parse(4, 4).unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(check_current_control(&db, "owner", 4, 4).await.is_ok());
    sqlx::query(
        "INSERT INTO close_obligations (product_conversation_id,phase) VALUES ('owner','pending')",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(matches!(
        check_current_control(&db, "owner", 4, 4).await,
        Err(ControlError::AdmissionStopped)
    ));
    sqlx::query(
        "UPDATE close_obligations SET phase='completed' WHERE product_conversation_id='owner'",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(check_current_control(&db, "owner", 4, 4).await.is_ok());
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='owner'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        check_current_control(&db, "owner", 4, 4).await,
        Err(ControlError::AdmissionStopped)
    ));
}

#[tokio::test]
async fn conversation_goal_control_candidate_goal_or_no_goal_capture_shares_atomic_transaction() {
    use crate::conversation_goals::control::{bind_control_at_admission_tx, ControlError};
    for with_goal in [false, true] {
        let (_dir, db) = control_fixture().await;
        accepted(&db, "stop", 0, 1).await.unwrap();
        let observation = if with_goal {
            let goal = db
                .mutate_conversation_goal(&create(), at())
                .await
                .unwrap()
                .snapshot;
            GoalAdmissionObservation::Goal(goal.fence())
        } else {
            GoalAdmissionObservation::NoGoal {
                observed_version: GoalVersion::INITIAL,
            }
        };
        assert_eq!(control_version(&db).await, Some((1, 1)));
        let binding = GoalExecutionBinding {
            owner: ProductConversationId::parse("owner").unwrap(),
            execution: GoalExecution::parse(1, 1).unwrap(),
            observation,
        };
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        crate::conversation_goals::capture_goal_at_new_turn_tx(&mut tx, binding.execution)
            .await
            .unwrap();
        assert!(matches!(
            bind_control_at_admission_tx(&mut tx, &binding.owner, binding.execution).await,
            Err(ControlError::AdmissionStopped)
        ));
        tx.rollback().await.unwrap();
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM conversation_goal_bindings WHERE turn_id=1")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rows, 0);
        accepted(&db, "release", 1, 2).await.unwrap();
        let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        crate::conversation_goals::capture_goal_at_new_turn_tx(&mut tx, binding.execution)
            .await
            .unwrap();
        bind_control_at_admission_tx(&mut tx, &binding.owner, binding.execution)
            .await
            .unwrap();
        crate::conversation_goals::control::require_current_control_tx(
            &mut tx,
            &binding.owner,
            binding.execution,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(binding_is_current(&db, 1).await);
        let captured = db
            .conversation_goal_execution_binding(binding.execution)
            .await
            .unwrap();
        assert_eq!(captured, Some(binding));
    }
}
