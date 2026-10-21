use super::*;
use crate::conversation_goals::user_commands::{GoalUserChange, GoalUserCommand, GoalUserResult};
use crate::conversation_goals::{execution::GoalExecutionInstallation, start::GoalStartError};
use phoenix_core::domain::conversation_goal::{
    GoalDisposition, GoalFence, GoalId, GoalRequestLimit, GoalStartInput, GoalStartRequester,
    GoalState, GoalText, GoalTimestampUs, GoalVersion,
};
use uuid::Uuid;

pub(super) fn enable(db: &mut Database) {
    db.goal_execution_installation = GoalExecutionInstallation::Installed {
        ceiling: GoalRequestLimit::parse(3).unwrap(),
    };
}

pub(super) async fn input(db: &Database, conversation: &str) -> GoalStartInput {
    let owner = db
        .get_conversation(conversation)
        .await
        .unwrap()
        .product_conversation_id;
    let snapshot = db.conversation_goal(&owner).await.unwrap().unwrap();
    GoalStartInput::new(
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
        GoalStartRequester::Owner {
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
        },
        snapshot.fence(),
    )
    .unwrap()
}

async fn complete_goal(db: &Database, conversation: &str, input: &GoalStartInput) {
    let GoalStartRequester::Owner { principal_id } = &input.requester else {
        unreachable!()
    };
    let receipt = db
        .apply_user_goal_command(
            &GoalUserCommand {
                owner: input.goal().owner.clone(),
                principal_id: *principal_id,
                request_id: Uuid::new_v4(),
                expected_version: input.goal().version,
                change: GoalUserChange::State {
                    goal_id: input.goal().goal_id.clone(),
                    disposition: GoalDisposition::Complete(GoalText::parse("done").unwrap()),
                },
            },
            conversation,
            GoalTimestampUs::parse(2).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
}

#[tokio::test]
async fn conversation_goal_start_replay_disabled_and_authentication_do_not_touch_receipt_schema() {
    let (mut db, conversation) = fixture().await;
    let owner = db
        .get_conversation(&conversation)
        .await
        .unwrap()
        .product_conversation_id;
    seed_goal(&db, owner.clone(), &conversation).await;
    let owner_input = input(&db, &conversation).await;
    assert!(matches!(
        db.accept_owner_goal_start(
            &owner_input,
            &conversation,
            GoalTimestampUs::parse(1).unwrap()
        )
        .await,
        Err(GoalStartError::Disabled)
    ));
    assert!(matches!(
        db.replay_owner_goal_start(&owner_input).await,
        Err(GoalStartError::Disabled)
    ));
    enable(&mut db);
    let forged = GoalStartInput::new(
        owner_input.request_id,
        GoalStartRequester::Owner {
            principal_id: Uuid::new_v4(),
        },
        owner_input.goal().clone(),
    )
    .unwrap();
    assert!(matches!(
        db.replay_owner_goal_start(&forged).await,
        Err(GoalStartError::Unauthorized)
    ));
    assert!(matches!(
        db.replay_owner_goal_start(&owner_input).await,
        Err(GoalStartError::Sqlx(_))
    ));
}

#[tokio::test]
async fn conversation_goal_start_replay_uses_historical_observation_after_state_and_cleanup_advance(
) {
    let (mut db, conversation, _) = start_fixture().await;
    enable(&mut db);
    let input = input(&db, &conversation).await;
    let accepted = db.replay_owner_goal_start(&input).await.unwrap().unwrap();
    assert_eq!(accepted.live_turn_id(), Some(2));
    assert_eq!(
        db.replay_owner_goal_start(&input).await.unwrap().unwrap(),
        accepted
    );
    sqlx::query("DELETE FROM durable_turns WHERE turn_id=2")
        .execute(db.pool())
        .await
        .unwrap();
    complete_goal(&db, &conversation, &input).await;
    assert!(matches!(
        db.conversation_goal(&input.goal().owner)
            .await
            .unwrap()
            .unwrap()
            .state,
        GoalState::Complete(_)
    ));
    let detached = db.replay_owner_goal_start(&input).await.unwrap().unwrap();
    assert_eq!(detached.live_turn_id(), None);
    assert_eq!(detached.input(), accepted.input());
    assert_eq!(detached.execution(), accepted.execution());
    assert_eq!(detached.observation(), accepted.observation());
    assert_eq!(detached.historical_transcript().as_str(), conversation);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM durable_turns WHERE turn_id=2")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn conversation_goal_start_same_owner_key_rejects_changed_requester_goal_and_version() {
    let (mut db, conversation, _) = start_fixture().await;
    enable(&mut db);
    let original = input(&db, &conversation).await;
    let wrong_principal = GoalStartInput::new(
        original.request_id,
        GoalStartRequester::Owner {
            principal_id: Uuid::new_v4(),
        },
        original.goal().clone(),
    )
    .unwrap();
    assert!(matches!(
        db.replay_owner_goal_start(&wrong_principal).await,
        Err(GoalStartError::Unauthorized)
    ));
    for goal in [
        GoalFence {
            goal_id: GoalId::parse("another-goal").unwrap(),
            ..original.goal().clone()
        },
        GoalFence {
            version: GoalVersion::parse(2).unwrap(),
            ..original.goal().clone()
        },
    ] {
        let changed =
            GoalStartInput::new(original.request_id, original.requester.clone(), goal).unwrap();
        assert!(matches!(
            db.replay_owner_goal_start(&changed).await,
            Err(GoalStartError::Conflict)
        ));
    }
    assert_eq!(
        db.replay_owner_goal_start(&original)
            .await
            .unwrap()
            .unwrap()
            .input(),
        &original
    );
}
