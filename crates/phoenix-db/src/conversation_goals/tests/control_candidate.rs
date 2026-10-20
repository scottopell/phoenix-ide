use super::*;

async fn control_fixture() -> (tempfile::TempDir, Database) {
    let (dir, db) = fixture().await;
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
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO conversation_goal_control_requests (owner_id,principal_id,request_id,command,expected_control_version,outcome,result_control_version,result_stopped,recorded_at_us) VALUES ('owner',?1,?2,?3,?4,'accepted',?5,?6,10)")
        .bind(vec![1_u8; 16]).bind(vec![request; 16]).bind(command).bind(expected)
        .bind(expected + 1).bind(i64::from(command == "stop"))
        .execute(db.pool()).await.map(|_| ())
}

async fn bind_open(db: &Database, turn: i64, version: i64, request: u8) -> Result<(), sqlx::Error> {
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
