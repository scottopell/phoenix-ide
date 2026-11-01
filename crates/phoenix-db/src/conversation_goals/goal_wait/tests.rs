use std::time::Duration;

use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAttribution, GoalChange, GoalId, GoalMutation, GoalText, GoalVersion,
};
use phoenix_core::domain::instance_identity::{FederationCredentialVerifier, InstanceId};

use super::{
    CoordinatorGoalWaitRequest, CoordinatorGoalWaitRequestError, CoordinatorGoalWaitStatus,
    GoalDbError,
};
use crate::conversation_goals::writer_tests::{at, fixture, ordinary};

async fn coordinator(db: &crate::Database) -> String {
    db.get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap()
        .id
}

fn create(
    owner: &phoenix_core::domain::product_conversation::ProductConversationId,
    goal_id: &str,
) -> GoalMutation {
    let action_id = GoalText::parse("goal-wait-create").unwrap();
    let actor = GoalActor::GeneratedHost {
        action_id: action_id.clone(),
    };
    GoalMutation {
        owner: owner.clone(),
        expected_version: GoalVersion::INITIAL,
        action_id,
        attribution: GoalAttribution {
            requested_by: actor.clone(),
            applied_by: actor,
        },
        change: GoalChange::Create {
            goal_id: GoalId::parse(goal_id).unwrap(),
            objective: GoalText::parse("wait for a committed goal version").unwrap(),
        },
    }
}

#[test]
fn wait_request_rejects_unbounded_timeouts() {
    assert_eq!(
        CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, Duration::ZERO),
        Err(CoordinatorGoalWaitRequestError::ZeroTimeout)
    );
    assert_eq!(
        CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, Duration::from_secs(26)),
        Err(CoordinatorGoalWaitRequestError::TimeoutTooLarge)
    );
    assert!(CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, Duration::from_secs(25)).is_ok());
}

#[tokio::test]
async fn immediate_wait_returns_exact_transcript_identity() {
    let db = fixture().await;
    let coordinator = coordinator(&db).await;
    let (transcript, owner) = ordinary(&db, "owner-immediate").await;
    let result = db
        .wait_for_coordinator_goals(
            &coordinator,
            &owner,
            GoalVersion::INITIAL,
            CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, Duration::from_millis(100))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(result.status(), CoordinatorGoalWaitStatus::Satisfied);
    assert_eq!(result.destination_transcript_id().as_str(), transcript);
    assert!(result.current().is_none());
    assert!(result.history().is_empty());
}

#[tokio::test]
async fn committed_goal_wakes_waiter_for_the_matching_owner() {
    let db = fixture().await;
    let coordinator = coordinator(&db).await;
    let (transcript, owner) = ordinary(&db, "owner-wake").await;
    let waiting_db = db.clone();
    let waiting_coordinator = coordinator.clone();
    let waiting_owner = owner.clone();
    let waiter = tokio::spawn(async move {
        waiting_db
            .wait_for_coordinator_goals(
                &waiting_coordinator,
                &waiting_owner,
                GoalVersion::INITIAL,
                CoordinatorGoalWaitRequest::new(
                    GoalVersion::parse(1).unwrap(),
                    Duration::from_secs(1),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    db.apply_conversation_goal(
        &create(&owner, "goal-wait-matching-owner"),
        &transcript,
        at(),
    )
    .await
    .unwrap();
    let result = waiter.await.unwrap();

    assert_eq!(result.status(), CoordinatorGoalWaitStatus::Satisfied);
    assert_eq!(
        result.current().unwrap().snapshot().version(),
        GoalVersion::parse(1).unwrap()
    );
}

#[tokio::test]
async fn unrelated_owner_notification_does_not_satisfy_wait() {
    let db = fixture().await;
    let coordinator = coordinator(&db).await;
    let (transcript, owner) = ordinary(&db, "owner-filtered").await;
    let (other_transcript, other_owner) = ordinary(&db, "owner-other").await;
    let waiting_db = db.clone();
    let waiting_coordinator = coordinator.clone();
    let waiting_owner = owner.clone();
    let mut waiter = tokio::spawn(async move {
        waiting_db
            .wait_for_coordinator_goals(
                &waiting_coordinator,
                &waiting_owner,
                GoalVersion::INITIAL,
                CoordinatorGoalWaitRequest::new(
                    GoalVersion::parse(1).unwrap(),
                    Duration::from_secs(1),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;

    db.apply_conversation_goal(
        &create(&other_owner, "goal-wait-unrelated-owner"),
        &other_transcript,
        at(),
    )
    .await
    .unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiter)
        .await
        .is_err());

    db.apply_conversation_goal(
        &create(&owner, "goal-wait-filtered-owner"),
        &transcript,
        at(),
    )
    .await
    .unwrap();
    assert_eq!(
        waiter.await.unwrap().status(),
        CoordinatorGoalWaitStatus::Satisfied
    );
}

#[tokio::test]
async fn unsatisfied_wait_returns_authoritative_snapshot_at_timeout() {
    let db = fixture().await;
    let coordinator = coordinator(&db).await;
    let (transcript, owner) = ordinary(&db, "owner-timeout").await;
    let result = db
        .wait_for_coordinator_goals(
            &coordinator,
            &owner,
            GoalVersion::INITIAL,
            CoordinatorGoalWaitRequest::new(
                GoalVersion::parse(1).unwrap(),
                Duration::from_millis(10),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(result.status(), CoordinatorGoalWaitStatus::TimedOut);
    assert_eq!(result.destination_transcript_id().as_str(), transcript);
    assert!(result.current().is_none());
    assert!(result.history().is_empty());
}

#[tokio::test]
async fn federated_goal_read_rechecks_saved_authority() {
    let db = fixture().await;
    let (_, owner) = ordinary(&db, "owner-federated-authority").await;
    let caller = InstanceId::new();
    let verifier = FederationCredentialVerifier::from_bearer(b"goal-wait-federated-authority");
    db.replace_federation_enrollment(caller, "goal reader", &verifier)
        .await
        .unwrap();
    let authority = crate::FederationSendAuthority::new(caller, verifier);
    let request =
        CoordinatorGoalWaitRequest::new(GoalVersion::INITIAL, std::time::Duration::from_secs(1))
            .unwrap();

    let result = db
        .wait_for_federated_goals(&authority, &owner, GoalVersion::INITIAL, request)
        .await
        .unwrap();
    assert_eq!(result.status(), CoordinatorGoalWaitStatus::Satisfied);

    assert!(db.revoke_federation_enrollment(caller).await.unwrap());
    let error = db
        .wait_for_federated_goals(&authority, &owner, GoalVersion::INITIAL, request)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        GoalDbError::Admission(crate::DbError::FederationSendAuthorityInactive)
    ));
}
