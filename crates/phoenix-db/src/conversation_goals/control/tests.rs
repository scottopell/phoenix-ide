use super::*;

fn at() -> GoalTimestampUs {
    GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros() + 1_000_000).unwrap()
}

fn stop_request(owner: ProductConversationId) -> ControlRequest {
    ControlRequest {
        owner,
        principal_id: Uuid::new_v4(),
        request_id: Uuid::new_v4(),
        command: ControlCommand::Stop,
        expected_control_version: 1,
    }
}

async fn fixture() -> (Database, ProductConversationId) {
    let db = Database::open_in_memory()
        .await
        .unwrap()
        .install_goal_controls_for_tests()
        .await
        .unwrap();
    let conversation = db
        .create_conversation("control-test", "control-test", "/tmp", true, None, None)
        .await
        .unwrap();
    (db, conversation.product_conversation_id)
}

async fn installed_permit(db: &Database, owner: &ProductConversationId) -> GoalDispatchPermit {
    let gate = db.goal_control_runtime.owner(owner.as_str()).unwrap();
    let epoch = gate.lock().await;
    GoalDispatchPermit::admitted(gate.clone(), &epoch)
}

#[tokio::test]
async fn failed_commit_does_not_revoke_dispatch_authority() {
    let (db, owner) = fixture().await;
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        at(),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .expect("classified")
    .unwrap();

    let permit = installed_permit(&db, &owner).await;
    sqlx::query("CREATE TABLE commit_failure_parent (id INTEGER PRIMARY KEY)")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE commit_failure_child (
             parent_id INTEGER,
             FOREIGN KEY (parent_id) REFERENCES commit_failure_parent(id) DEFERRABLE INITIALLY DEFERRED
         )",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER inject_deferred_fk_failure
         AFTER INSERT ON conversation_goal_control_requests
         BEGIN INSERT INTO commit_failure_child(parent_id) VALUES (987654); END",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let request = stop_request(owner.clone());
    let error = db
        .apply_goal_control(&request, at(), || panic!("unexpected authority loss"))
        .await
        .established()
        .expect("classified")
        .unwrap_err();
    assert!(matches!(
        error,
        ControlError::Storage(sqlx::Error::Database(_))
    ));

    let receipt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_goal_control_requests WHERE owner_id=? AND principal_id=? AND request_id=?",
    )
    .bind(owner.as_str())
    .bind(request.principal_id.as_bytes().as_slice())
    .bind(request.request_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipt, 0);
    let state: (i64, i64) = sqlx::query_as(
        "SELECT control_version, stopped FROM conversation_goal_controls WHERE owner_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(state, (1, 0));
    assert_eq!(permit.run(async { 42 }).await.unwrap(), 42);
}

#[tokio::test]
async fn committed_stop_revokes_existing_dispatch_permit_without_polling_again() {
    let (db, owner) = fixture().await;
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        at(),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .expect("classified")
    .unwrap();
    let permit = installed_permit(&db, &owner).await;

    db.apply_goal_control(&stop_request(owner.clone()), at(), || {
        panic!("unexpected authority loss")
    })
    .await
    .established()
    .expect("classified")
    .unwrap();

    let polled = std::sync::atomic::AtomicBool::new(false);
    let result = permit
        .run(std::future::poll_fn(|_| {
            polled.store(true, std::sync::atomic::Ordering::SeqCst);
            std::task::Poll::Ready(42)
        }))
        .await;
    assert!(result.is_err());
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));

    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 2,
        },
        at(),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .expect("classified")
    .unwrap();
    assert_eq!(
        installed_permit(&db, &owner)
            .await
            .run(async { 42 })
            .await
            .unwrap(),
        42
    );
}

#[tokio::test]
async fn rejected_control_receipt_does_not_revoke_dispatch_authority() {
    let (db, owner) = fixture().await;
    db.apply_goal_control(
        &ControlRequest {
            owner: owner.clone(),
            principal_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            command: ControlCommand::Release,
            expected_control_version: 0,
        },
        at(),
        || panic!("unexpected authority loss"),
    )
    .await
    .established()
    .expect("classified")
    .unwrap();
    let permit = installed_permit(&db, &owner).await;
    let mut rejected = stop_request(owner);
    rejected.expected_control_version = 0;
    let receipt = db
        .apply_goal_control(&rejected, at(), || panic!("unexpected authority loss"))
        .await
        .established()
        .expect("classified")
        .unwrap();
    assert!(matches!(
        receipt.result,
        ControlOutcome::RejectedVersion { .. }
    ));
    assert_eq!(permit.run(async { 42 }).await.unwrap(), 42);
}

#[test]
fn authority_lost_callback_runs_once_when_unresolved_authority_drops() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let callback_calls = &calls;
    let authority = ControlCommitAuthority {
        unresolved: true,
        on_authority_lost: Some(move || {
            callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    };

    drop(authority);

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn authority_lost_callback_does_not_run_after_classification() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let callback_calls = &calls;
    let mut authority = ControlCommitAuthority {
        unresolved: true,
        on_authority_lost: Some(move || {
            callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    };

    authority.classified();
    drop(authority);

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn panicking_authority_lost_callback_is_consumed_before_unwinding() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let callback_calls = &calls;
    let authority = ControlCommitAuthority {
        unresolved: true,
        on_authority_lost: Some(move || {
            callback_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            panic!("authority callback panic");
        }),
    };

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(authority)));

    assert!(panic.is_err());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn committed_control_with_lost_acknowledgement_is_classified_before_revocation() {
    let (db, owner) = fixture().await;
    let permit = installed_permit(&db, &owner).await;
    let mut request = stop_request(owner.clone());
    request.expected_control_version = 0;
    let receipt = db
        .apply_goal_control_at_cut(
            &request,
            at(),
            || panic!("unexpected authority loss"),
            ControlCommitCut::LostAcknowledgement,
        )
        .await
        .established()
        .expect("classified commit")
        .unwrap();
    assert!(matches!(receipt.result, ControlOutcome::Accepted { .. }));
    assert!(permit.run(async { 42 }).await.is_err());
    let replay = db
        .apply_goal_control(&request, at(), || panic!("unexpected authority loss"))
        .await
        .established()
        .expect("classified replay")
        .unwrap();
    assert_eq!(receipt, replay);
}

#[tokio::test]
async fn cancelled_control_commit_boundary_closes_owner_without_publishing_revocation() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let (db, owner) = fixture().await;
    let permit = installed_permit(&db, &owner).await;
    let mut request = stop_request(owner.clone());
    request.expected_control_version = 0;
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = calls.clone();
    let (reached, committed) = tokio::sync::oneshot::channel();
    let (_resume, release) = tokio::sync::oneshot::channel();
    let task_db = db.clone();
    let task = tokio::spawn(async move {
        task_db
            .apply_goal_control_at_cut(
                &request,
                at(),
                move || {
                    callback_calls.fetch_add(1, Ordering::SeqCst);
                },
                ControlCommitCut::AfterCommit { reached, release },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), committed)
        .await
        .expect("commit barrier liveness")
        .expect("commit reached");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let stopped: i64 =
        sqlx::query_scalar("SELECT stopped FROM conversation_goal_controls WHERE owner_id=?")
            .bind(owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stopped, 1);
    // The callback owns process fencing; this raw epoch must not encode cancellation.
    assert_eq!(permit.run(async { 42 }).await.unwrap(), 42);
}

#[tokio::test]
async fn unclassified_control_commit_returns_closed_result_and_closes_owner_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (db, owner) = fixture().await;
    let permit = installed_permit(&db, &owner).await;
    let mut request = stop_request(owner);
    request.expected_control_version = 0;
    let calls = AtomicUsize::new(0);
    let result = db
        .apply_goal_control_at_cut(
            &request,
            at(),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
            },
            ControlCommitCut::LostClassification,
        )
        .await;
    assert!(matches!(
        result,
        crate::workflow::LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(permit.run(async { 42 }).await.unwrap(), 42);
}
