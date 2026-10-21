use super::*;
use crate::conversation_goals::{
    control::{ControlCommand, ControlRequest, GoalControlInstallation},
    start::{GoalStartAcceptance, GoalStartError},
};
use phoenix_core::domain::conversation_goal::{GoalStartInput, GoalTimestampUs};
use uuid::Uuid;

async fn ready() -> (Database, String, GoalStartInput) {
    let (mut db, transcript) = fixture().await;
    migrate(&db).await.unwrap();
    // fixture() inserts workflow/turn 1 directly after sequence initialization.
    sqlx::query("INSERT INTO workflow_global_sequences(sequence_name,next_value) VALUES ('workflow',2),('direct_turn',2) ON CONFLICT(sequence_name) DO UPDATE SET next_value=max(next_value,2)")
        .execute(db.pool()).await.unwrap();
    start_replay::enable(&mut db);
    db.goal_control_installation = GoalControlInstallation::Installed;
    let owner = db
        .get_conversation(&transcript)
        .await
        .unwrap()
        .product_conversation_id;
    seed_goal(&db, owner.clone(), &transcript).await;
    db.apply_goal_control(
        &ControlRequest {
            owner,
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        GoalTimestampUs::parse(2).unwrap(),
    )
    .await
    .unwrap();
    let input = start_replay::input(&db, &transcript).await;
    (db, transcript, input)
}

async fn counts(db: &Database) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM workflows),(SELECT count(*) FROM durable_turns),(SELECT count(*) FROM conversation_goal_starts),(SELECT count(*) FROM conversation_goal_requests),(SELECT count(*) FROM messages)")
        .fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn conversation_goal_start_acceptance_is_atomic_typed_and_exact_retry_is_not_a_new_request() {
    let (db, transcript, input) = ready().await;
    let before = counts(&db).await;
    let accepted = db
        .accept_owner_goal_start(&input, &transcript, GoalTimestampUs::parse(3).unwrap())
        .await
        .unwrap();
    let GoalStartAcceptance::Accepted(receipt) = accepted else {
        panic!("fresh Start")
    };
    let after = counts(&db).await;
    assert_eq!(
        after,
        (before.0 + 1, before.1 + 1, before.2 + 1, before.3, before.4)
    );
    let row: (String, i64, Option<String>, Option<Vec<u8>>, Option<String>) = sqlx::query_as("SELECT input_kind,materialized,canonical_message_id,prepared_payload,origin_kind FROM durable_turns WHERE turn_id=?")
        .bind(receipt.execution().turn_id()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(row, ("goal_start".into(), 0, None, None, None));
    let turns = db
        .workflow_repository()
        .list_discoverable_accepted_runtime_direct_turns(None, 20)
        .await
        .unwrap();
    assert!(turns.candidates.iter().any(|turn| turn.turn_id.0
        == u64::try_from(receipt.execution().turn_id()).unwrap()
        && matches!(
            turn.prepared.input(),
            phoenix_workflow::PreparedTurnInput::GoalStart(_)
        )));
    db.apply_goal_control(
        &ControlRequest {
            owner: input.goal().owner.clone(),
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Stop,
            expected_control_version: 1,
        },
        GoalTimestampUs::parse(4).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        db.accept_owner_goal_start(&input, &transcript, GoalTimestampUs::parse(5).unwrap())
            .await
            .unwrap(),
        GoalStartAcceptance::Replay(receipt)
    );
    assert_eq!(counts(&db).await, after);
}

#[tokio::test]
async fn conversation_goal_start_failed_control_binding_rolls_back_workflow_receipt_input_and_bindings(
) {
    let (db, transcript, input) = ready().await;
    sqlx::raw_sql("CREATE TRIGGER reject_start_binding BEFORE INSERT ON conversation_goal_control_bindings BEGIN SELECT RAISE(ABORT,'injected binding failure'); END;")
        .execute(db.pool()).await.unwrap();
    let before = counts(&db).await;
    assert!(matches!(
        db.accept_owner_goal_start(&input, &transcript, GoalTimestampUs::parse(3).unwrap())
            .await,
        Err(GoalStartError::Database(_))
    ));
    assert_eq!(counts(&db).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_goal_start_canonical_inputs"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER reject_start_binding")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        db.accept_owner_goal_start(&input, &transcript, GoalTimestampUs::parse(4).unwrap())
            .await
            .unwrap(),
        GoalStartAcceptance::Accepted(_)
    ));
}

#[tokio::test]
async fn conversation_goal_start_target_authorization_precedes_exact_historical_replay() {
    let (db, transcript, input) = ready().await;
    db.accept_owner_goal_start(&input, &transcript, GoalTimestampUs::parse(3).unwrap())
        .await
        .unwrap();
    let before = counts(&db).await;
    assert!(matches!(
        db.accept_owner_goal_start(
            &input,
            "not-the-owner-target",
            GoalTimestampUs::parse(4).unwrap()
        )
        .await,
        Err(GoalStartError::Goal(_))
    ));
    assert_eq!(counts(&db).await, before);
}
