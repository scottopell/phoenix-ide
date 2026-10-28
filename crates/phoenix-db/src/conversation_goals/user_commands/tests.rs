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

#[tokio::test]
async fn busy_receipt_uses_live_owner_after_sampled_transcript_continues_and_retries_exactly() {
    let db = fixture().await;
    let (sampled_transcript, owner) = ordinary(&db, "busy-after-continuation").await;
    let command = command(owner);
    assert!(db
        .retained_user_goal_command(&command, &sampled_transcript)
        .await
        .unwrap()
        .is_none());
    db.update_conversation_state(
        &sampled_transcript,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "continue before busy refusal".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        db.continue_conversation(&sampled_transcript).await.unwrap()
    else {
        panic!("continuation must create a new live transcript");
    };
    assert_ne!(successor.id, sampled_transcript);
    let mut expected_counts = counts(&db).await;
    expected_counts.6 += 1;
    let result = db
        .reject_busy_user_goal_command(&command, &sampled_transcript, at(), || {
            panic!("live busy refusal must be classified");
        })
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(receipt)) = result else {
        panic!("busy gate must retain a receipt across continuation");
    };
    assert!(matches!(
        receipt.result,
        GoalUserResult::Rejected {
            reason: GoalUserRefusal::Busy,
            ..
        }
    ));
    let retry = db
        .apply_user_goal_command(&command, &successor.id, at(), || {
            panic!("exact retry must remain classified");
        })
        .await;
    assert!(
        matches!(retry, LocalAuthorityResult::DurableFactEstablished(Ok(ref stored)) if stored == &receipt)
    );
    assert_eq!(
        db.retained_user_goal_command(&command, &successor.id)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(counts(&db).await, expected_counts);
    assert!(db
        .conversation_goal(&command.owner)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn retained_accepted_retry_uses_live_owner_after_sampled_transcript_continues() {
    let db = fixture().await;
    let (sampled_transcript, owner) = ordinary(&db, "accepted-retry-after-continuation").await;
    let mut input = command(owner.clone());
    let receipt = db
        .apply_user_goal_command(&input, &sampled_transcript, at(), || {
            panic!("initial command must remain classified");
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(&receipt.result, GoalUserResult::Accepted { .. }));
    db.update_conversation_state(
        &sampled_transcript,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "continue before retained accepted retry".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        db.continue_conversation(&sampled_transcript).await.unwrap()
    else {
        panic!("continuation must create a new live transcript");
    };
    assert_ne!(successor.id, sampled_transcript);
    assert_eq!(successor.product_conversation_id, owner);
    let before = counts(&db).await;
    assert_eq!(
        db.retained_user_goal_command(&input, &sampled_transcript)
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        db.retained_user_goal_command(&input, &successor.id)
            .await
            .unwrap(),
        Some(receipt)
    );
    input.change = GoalUserChange::Create {
        objective: GoalText::parse("conflicting accepted retry after continuation").unwrap(),
    };
    assert!(matches!(
        db.retained_user_goal_command(&input, &sampled_transcript)
            .await,
        Err(GoalDbError::RetryConflict)
    ));
    assert_eq!(counts(&db).await, before);
}

#[tokio::test]
async fn new_command_stale_sample_has_no_receipt_then_commits_on_live_successor() {
    let db = fixture().await;
    let (sampled_transcript, owner) = ordinary(&db, "new-command-after-continuation").await;
    let input = command(owner.clone());
    db.update_conversation_state(
        &sampled_transcript,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "continue before new command receipt lookup".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        db.continue_conversation(&sampled_transcript).await.unwrap()
    else {
        panic!("continuation must create a new live transcript");
    };
    assert_ne!(successor.id, sampled_transcript);
    assert_eq!(successor.product_conversation_id, owner);
    let before = counts(&db).await;
    assert!(db
        .retained_user_goal_command(&input, &sampled_transcript)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        db.apply_user_goal_command(&input, &sampled_transcript, at(), || {
            panic!("stale write target must remain classified");
        })
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(counts(&db).await, before);
    let receipt = db
        .apply_user_goal_command(&input, &successor.id, at(), || {
            panic!("live successor command must remain classified");
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(&receipt.result, GoalUserResult::Accepted { .. }));
    let after = counts(&db).await;
    assert_eq!(after.0, before.0 + 1);
    assert_eq!(after.1, before.1 + 1);
    assert_eq!(after.6, before.6 + 1);
    assert_eq!(
        db.retained_user_goal_command(&input, &sampled_transcript)
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    let retry = db
        .apply_user_goal_command(&input, &successor.id, at(), || {
            panic!("exact successor retry must remain classified");
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(retry, receipt);
    assert_eq!(counts(&db).await, after);
}

#[tokio::test]
async fn conflicting_retry_classification_after_lost_rollback_ack_is_nonfatal() {
    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "conflicting-retry-classification").await;
    let mut command = command(owner);
    let original = db
        .apply_user_goal_command(&command, &transcript, at(), || {
            panic!("initial receipt must commit");
        })
        .await;
    assert!(matches!(
        original,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    let original_change = std::mem::replace(
        &mut command.change,
        GoalUserChange::Create {
            objective: GoalText::parse("conflicting retry payload").unwrap(),
        },
    );
    let fatal = AtomicBool::new(false);
    let result = db
        .user_goal_command(
            &command,
            &transcript,
            at(),
            false,
            || fatal.store(true, Ordering::SeqCst),
            UserCommitCut::LostRollbackAcknowledgement,
        )
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(GoalDbError::RetryConflict))
    ));
    assert!(!fatal.load(Ordering::SeqCst));
    assert_eq!(
        db.conversation_goal_history(&command.owner, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
    command.change = original_change;
    let retry = db
        .apply_user_goal_command(&command, &transcript, at(), || {
            panic!("authority must remain available after typed conflict");
        })
        .await;
    assert!(
        matches!((original, retry), (LocalAuthorityResult::DurableFactEstablished(Ok(first)), LocalAuthorityResult::DurableFactEstablished(Ok(second))) if first == second)
    );
}
