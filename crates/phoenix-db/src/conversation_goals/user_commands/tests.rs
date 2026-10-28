use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::conversation_goals::writer_tests::{at, counts, fixture, ordinary};

fn command(owner: ProductConversationId) -> GoalUserCommand {
    GoalUserCommand {
        owner,
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        expected_version: GoalVersion::INITIAL,
        change: GoalUserChange::Create {
            objective: GoalText::parse("classified user command").unwrap(),
        },
    }
}

async fn lost_acknowledgement_retains_receipt(busy: bool) {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "lost-user-command-ack").await;
    let input = command(owner.clone());
    let fatal = AtomicBool::new(false);
    let receipt = db
        .user_goal_command(
            &input,
            &transcript,
            at(),
            busy,
            || fatal.store(true, Ordering::SeqCst),
            UserCommitCut::LostAcknowledgement,
        )
        .await
        .established()
        .expect("classified lost acknowledgement")
        .unwrap();
    assert!(!fatal.load(Ordering::SeqCst));
    if busy {
        assert!(matches!(
            receipt.result,
            GoalUserResult::Rejected {
                reason: GoalUserRefusal::Busy,
                ..
            }
        ));
        assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    } else {
        assert!(matches!(receipt.result, GoalUserResult::Accepted { .. }));
        assert!(db.conversation_goal(&owner).await.unwrap().is_some());
    }
    let before = counts(&db).await;
    let retry = db
        .apply_user_goal_command(&input, &transcript, at(), || panic!("retry authority lost"))
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, before);
}

#[tokio::test]
async fn committed_user_command_with_lost_acknowledgement_returns_retained_receipt() {
    lost_acknowledgement_retains_receipt(false).await;
}

#[tokio::test]
async fn committed_busy_receipt_with_lost_acknowledgement_returns_retained_refusal() {
    lost_acknowledgement_retains_receipt(true).await;
}

#[tokio::test]
async fn classified_user_command_rollback_is_uncommitted_without_fatal_authority_loss() {
    for busy in [false, true] {
        let db = fixture().await;
        let (transcript, owner) = ordinary(&db, "rolled-back-user-command").await;
        let input = command(owner);
        let before = counts(&db).await;
        let fatal = AtomicBool::new(false);
        assert!(db
            .user_goal_command(
                &input,
                &transcript,
                at(),
                busy,
                || fatal.store(true, Ordering::SeqCst),
                UserCommitCut::RolledBack,
            )
            .await
            .established()
            .expect("classified rollback")
            .is_err());
        assert!(!fatal.load(Ordering::SeqCst));
        assert_eq!(counts(&db).await, before);
        assert!(db
            .retained_user_goal_command(&input, &transcript)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .apply_user_goal_command(&input, &transcript, at(), || panic!("retry authority lost"))
            .await
            .established()
            .unwrap()
            .is_ok());
    }
}

#[tokio::test]
async fn uncertain_user_and_busy_writes_with_unclassifiable_lookup_signal_authority_loss() {
    for busy in [false, true] {
        let db = fixture().await;
        let (transcript, owner) = ordinary(&db, "unclassifiable-user-command").await;
        let input = command(owner);
        let fatal = AtomicBool::new(false);
        let result = db
            .user_goal_command(
                &input,
                &transcript,
                at(),
                busy,
                || fatal.store(true, Ordering::SeqCst),
                UserCommitCut::LostClassification,
            )
            .await;
        assert!(matches!(
            result,
            LocalAuthorityResult::DurableFactUnclassified
        ));
        assert!(fatal.load(Ordering::SeqCst));
        assert!(db
            .retained_user_goal_command(&input, &transcript)
            .await
            .unwrap()
            .is_some());
    }
}
