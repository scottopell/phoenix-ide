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
            || Ok(at()),
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
        .apply_user_goal_command(
            &input,
            &transcript,
            || Ok(at()),
            || panic!("retry authority lost"),
        )
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
                || Ok(at()),
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
            .apply_user_goal_command(
                &input,
                &transcript,
                || Ok(at()),
                || panic!("retry authority lost")
            )
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
                || Ok(at()),
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
        .reject_busy_user_goal_command(
            &command,
            &sampled_transcript,
            || Ok(at()),
            || {
                panic!("live busy refusal must be classified");
            },
        )
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
        .apply_user_goal_command(
            &command,
            &successor.id,
            || Ok(at()),
            || {
                panic!("exact retry must remain classified");
            },
        )
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
        .apply_user_goal_command(
            &input,
            &sampled_transcript,
            || Ok(at()),
            || {
                panic!("initial command must remain classified");
            },
        )
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
        db.apply_user_goal_command(
            &input,
            &sampled_transcript,
            || Ok(at()),
            || {
                panic!("stale write target must remain classified");
            }
        )
        .await
        .established()
        .unwrap(),
        Err(GoalDbError::TargetChanged)
    ));
    assert_eq!(counts(&db).await, before);
    let receipt = db
        .apply_user_goal_command(
            &input,
            &successor.id,
            || Ok(at()),
            || {
                panic!("live successor command must remain classified");
            },
        )
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
        .apply_user_goal_command(
            &input,
            &successor.id,
            || Ok(at()),
            || {
                panic!("exact successor retry must remain classified");
            },
        )
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
        .apply_user_goal_command(
            &command,
            &transcript,
            || Ok(at()),
            || {
                panic!("initial receipt must commit");
            },
        )
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
            || Ok(at()),
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
        .apply_user_goal_command(
            &command,
            &transcript,
            || Ok(at()),
            || {
                panic!("authority must remain available after typed conflict");
            },
        )
        .await;
    assert!(
        matches!((original, retry), (LocalAuthorityResult::DurableFactEstablished(Ok(first)), LocalAuthorityResult::DurableFactEstablished(Ok(second))) if first == second)
    );
}

async fn assert_closed_receipt_retry(
    db: &Database,
    input: &GoalUserCommand,
    transcript: &str,
    receipt: &GoalUserReceipt,
    history: bool,
) {
    let before = counts(db).await;
    assert_eq!(
        db.retained_user_goal_command(input, transcript)
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    for busy in [false, true] {
        let retry = db
            .user_goal_command(
                input,
                transcript,
                || panic!("retained retry must not sample a new timestamp"),
                busy,
                || panic!("retained retry must remain classified"),
                UserCommitCut::None,
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        assert_eq!(&retry, receipt);
    }
    let mut fresh = command(input.owner.clone());
    fresh.expected_version = input.expected_version;
    let read = db.retained_user_goal_command(&fresh, transcript).await;
    let write = db
        .apply_user_goal_command(
            &fresh,
            transcript,
            || panic!("closed owner must fail admission before sampling the clock"),
            || panic!("refused new write must remain classified"),
        )
        .await
        .established()
        .unwrap();
    for result in [read.map(|_| ()), write.map(|_| ())] {
        if history {
            assert!(matches!(
                result,
                Err(GoalDbError::Admission(
                    crate::DbError::ProductConversationUnavailable(_)
                ))
            ));
        } else {
            assert!(matches!(
                result,
                Err(GoalDbError::Admission(
                    crate::DbError::CloseAdmissionFenced(_)
                ))
            ));
        }
    }
    assert_eq!(counts(db).await, before);
}

async fn retire_captured_close_scope(
    db: &crate::Database,
    attempt: &phoenix_core::domain::close::CloseAttemptId,
) {
    use crate::close_foundation::{
        CaptureCloseRetirementInventoryRequest, CaptureCloseRetirementInventoryScopeRequest,
        RecordCloseRetirementEvidenceRequest, ReplaceCloseInspectionRequest,
    };
    use phoenix_core::domain::close::{
        CloseCompletionOutcome, CloseOwnedResourceInventory, ClosePhase, RetirementOutcome,
    };

    let scopes = db
        .list_close_attempt_scopes(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(scopes.len(), 1);
    let scope = &scopes[0];
    assert!(scope.captured_worktree.is_none());
    let inspection_targets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_scopes captured
         JOIN work_scopes scope ON scope.id = captured.scope
         WHERE captured.attempt_id = ?1 AND scope.environment_kind = 'allocated_worktree'",
    )
    .bind(attempt.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(inspection_targets, 0);
    db.replace_close_inspection(ReplaceCloseInspectionRequest {
        attempt_id: attempt.clone(),
        scopes: Vec::new(),
    })
    .await
    .unwrap();
    let snapshot = db
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap()
        .snapshot()
        .unwrap()
        .clone();
    assert_eq!(snapshot.generation(), "no-worktree");
    assert_eq!(snapshot.fingerprint(), "no-worktree");
    let expected = db
        .capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
            attempt_id: attempt.clone(),
            snapshot,
            scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
                scope: scope.scope.clone(),
                inventory: CloseOwnedResourceInventory {
                    worktree: None,
                    work_scopes: Default::default(),
                    bash_process_groups: Default::default(),
                    tmux_servers: Default::default(),
                    pty_sessions: Default::default(),
                    browser_sessions: Default::default(),
                    equivalent_live_resources: Default::default(),
                },
            }],
        })
        .await
        .unwrap();
    assert_eq!(expected.len(), 1);
    assert_eq!(
        expected[0].resource.kind(),
        phoenix_core::domain::close::RetiredResourceKind::WorkScope
    );
    assert_eq!(expected[0].scope, scope.scope);
    for resource in expected {
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: resource.attempt_id,
            scope: resource.scope,
            snapshot: resource.snapshot,
            resource: resource.resource,
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
    }
    let completed = db.complete_close_retirement(attempt).await.unwrap();
    assert_eq!(completed.phase(), ClosePhase::Completed);
    assert_eq!(
        completed.close_outcome(),
        Some(CloseCompletionOutcome::Archived)
    );
}

#[tokio::test]
async fn accepted_create_and_state_lost_responses_retry_through_close_fence_and_history() {
    use phoenix_core::domain::close::{CloseAttemptId, TranscriptConversationId};

    for state in [false, true] {
        let db = fixture().await;
        let (transcript, owner) = ordinary(&db, "accepted-before-close").await;
        let mut input = command(owner.clone());
        if state {
            let created = db
                .apply_user_goal_command(
                    &input,
                    &transcript,
                    || Ok(at()),
                    || panic!("authority lost"),
                )
                .await
                .established()
                .unwrap()
                .unwrap();
            let GoalUserResult::Accepted { entry } = created.result else {
                panic!("create must be accepted");
            };
            input.request_id = Uuid::new_v4();
            input.expected_version = entry.snapshot().version();
            input.change = GoalUserChange::State {
                goal_id: entry.snapshot().goal_id().clone(),
                disposition: GoalDisposition::Suspended(
                    GoalText::parse("pause before close").unwrap(),
                ),
            };
        }
        let receipt = db
            .user_goal_command(
                &input,
                &transcript,
                || Ok(at()),
                false,
                || panic!("lost response must be classified"),
                UserCommitCut::LostAcknowledgement,
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        assert!(matches!(&receipt.result, GoalUserResult::Accepted { .. }));
        let attempt = CloseAttemptId::parse("close-after-lost-response").unwrap();
        db.begin_close_foundation(
            &owner,
            &TranscriptConversationId::parse(transcript.clone()).unwrap(),
            attempt.as_str(),
        )
        .await
        .unwrap();
        assert_closed_receipt_retry(&db, &input, &transcript, &receipt, false).await;
        db.begin_close_idle_settlement(attempt.as_str())
            .await
            .unwrap();
        db.advance_close_settlement_when_quiescent(attempt.as_str())
            .await
            .unwrap();
        retire_captured_close_scope(&db, &attempt).await;
        assert!(db.get_conversation(&transcript).await.unwrap().archived);
        assert_closed_receipt_retry(&db, &input, &transcript, &receipt, true).await;
    }
}

#[tokio::test]
async fn queued_user_write_samples_clock_after_watch_enrollment_and_uses_one_timestamp() {
    use std::{
        cell::Cell,
        future::Future,
        task::{Context, Waker},
    };

    let db = fixture().await;
    let (transcript, owner) = ordinary(&db, "watch-enrollment-before-user-writer").await;
    let input = command(owner.clone());
    let admitted_at = Cell::new(None);
    let sampled = Cell::new(false);
    let blocker = db.pool().acquire().await.unwrap();
    let mut enrollment = Box::pin(db.watch_product_conversation(&owner));
    let mut writer = Box::pin(db.apply_user_goal_command(
        &input,
        &transcript,
        || {
            assert!(!sampled.replace(true));
            let enrollment = admitted_at
                .get()
                .expect("watch must commit before writer admission");
            Ok(GoalTimestampUs::parse(enrollment + 1).unwrap())
        },
        || panic!("writer must remain classified"),
    ));
    let mut context = Context::from_waker(Waker::noop());
    assert!(enrollment.as_mut().poll(&mut context).is_pending());
    assert!(writer.as_mut().poll(&mut context).is_pending());
    assert!(!sampled.get());
    drop(blocker);
    let watch = enrollment.await.unwrap();
    admitted_at.set(Some(watch.enrolled_at_us));
    let receipt = writer.await.established().unwrap().unwrap();
    assert!(sampled.get());
    let GoalUserResult::Accepted { entry } = &receipt.result else {
        panic!("write after watch enrollment must be accepted");
    };
    let at = entry.recorded_at_us().get();
    assert!(at >= watch.enrolled_at_us);
    assert_eq!(at, watch.enrolled_at_us + 1);
    let events = db.pending_coordinator_watch_events(10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_occurrence_kind, "goal_change");
    assert_eq!(events[0].occurred_at_us, at);
    assert_eq!(
        events[0].source_generation,
        entry.snapshot().version().get()
    );
    let retained_at: i64 = sqlx::query_scalar(
        "SELECT recorded_at_us FROM conversation_goal_user_requests WHERE request_id=?",
    )
    .bind(input.request_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(retained_at, at);
    assert_eq!(
        db.retained_user_goal_command(&input, &transcript)
            .await
            .unwrap(),
        Some(receipt)
    );
}
