use super::*;
use crate::coordinator_watches::{
    record_creation_event_tx, record_steering_event_tx, record_wait_entry_tx,
    MessageExecutionSource, WatchOutcome,
};
use crate::Database;
use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalAttribution, GoalChange, GoalDisposition, GoalId, GoalMutation, GoalText,
    GoalTimestampUs, GoalVersion,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;

async fn fixture() -> (Database, String, ProductConversationId) {
    let db = Database::open_in_memory().await.unwrap();
    let source = db
        .create_conversation("goal-watch", "goal-watch", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE coordinator_watches SET enrolled_at_us=1 WHERE source_product_conversation_id=?",
    )
    .bind(source.product_conversation_id.as_str())
    .execute(db.pool())
    .await
    .unwrap();
    (db, source.id, source.product_conversation_id)
}

fn mutation(
    owner: &ProductConversationId,
    expected: i64,
    key: &str,
    change: GoalChange,
) -> GoalMutation {
    let actor = GoalActor::GeneratedHost {
        action_id: GoalText::parse(key).unwrap(),
    };
    GoalMutation {
        owner: owner.clone(),
        expected_version: GoalVersion::parse(expected).unwrap(),
        action_id: GoalText::parse(key).unwrap(),
        attribution: GoalAttribution {
            requested_by: actor.clone(),
            applied_by: actor,
        },
        change,
    }
}

async fn create_goal(
    db: &Database,
    transcript: &str,
    owner: &ProductConversationId,
) -> phoenix_core::domain::conversation_goal::GoalHistoryEntry {
    db.apply_conversation_goal(
        &mutation(
            owner,
            0,
            "create-goal",
            GoalChange::Create {
                goal_id: GoalId::parse("saved-goal").unwrap(),
                objective: GoalText::parse("  saved objective  ").unwrap(),
            },
        ),
        transcript,
        GoalTimestampUs::parse(10).unwrap(),
    )
    .await
    .unwrap()
}

async fn creation(
    db: &Database,
    transcript: &str,
    key: &str,
    installation: GoalControlInstallation,
) -> DbResult<()> {
    let mut tx = db.pool().begin().await?;
    record_creation_event_tx(&mut tx, key, 1, transcript, "Completed", None, installation).await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn disabled_real_boundaries_do_not_touch_unavailable_goal_storage() {
    let (db, transcript, _) = fixture().await;
    assert_eq!(
        db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    assert_eq!(
        db.workflow_repository()
            .goal_control_installation_for_tests(),
        GoalControlInstallation::Disabled
    );
    assert_eq!(
        crate::workflow::WorkflowRepository::new(db.pool().clone())
            .goal_control_installation_for_tests(),
        GoalControlInstallation::Disabled
    );
    assert_eq!(
        db.clone()
            .workflow_repository()
            .goal_control_installation_for_tests(),
        GoalControlInstallation::Disabled
    );
    sqlx::query("ALTER TABLE conversation_goal_history RENAME TO unavailable_goal_history")
        .execute(db.pool())
        .await
        .unwrap();
    creation(
        &db,
        &transcript,
        "disabled-create",
        db.goal_control_installation(),
    )
    .await
    .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    record_steering_event_tx(
        &mut tx,
        MessageExecutionSource::Steering,
        "disabled-steer",
        &transcript,
        "Failed",
        Some("fixture failure"),
        db.goal_control_installation(),
    )
    .await
    .unwrap();
    let wait = ConvState::AwaitingTaskApproval {
        task_file: "tasks/12345-p1-ready--goal-fixture.md".into(),
        title: "Review".into(),
        priority: phoenix_core::task_source::Priority::P1,
        plan: "Review".into(),
    };
    record_wait_entry_tx(&mut tx, &transcript, &wait, db.goal_control_installation())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let pending = db.pending_coordinator_watch_events(64).await.unwrap();
    assert_eq!(pending.len(), 3);
    assert!(pending
        .iter()
        .all(|event| event.goal_capture == Some(WatchGoalCapture::NotCapturedDisabled)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM coordinator_watch_goal_captures")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(creation(
        &db,
        &transcript,
        "installed-fails",
        GoalControlInstallation::Installed
    )
    .await
    .is_err());
    assert_eq!(
        db.pending_coordinator_watch_events(64).await.unwrap().len(),
        3
    );
}

#[tokio::test]
async fn saved_goal_is_frozen_not_admission_or_execution_completion_and_dedup_never_enriches() {
    let (db, transcript, owner) = fixture().await;
    create_goal(&db, &transcript, &owner).await;
    creation(
        &db,
        &transcript,
        "first",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    let first = db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(first.outcome, WatchOutcome::Completed);
    assert!(matches!(
        &first.goal_capture,
        Some(WatchGoalCapture::CapturedSavedGoal {
            owner_version: 1,
            state: WatchGoalState::Underway,
            usage: WatchGoalUsage::UnavailableNotCaptured,
            ..
        })
    ));
    let blocked = mutation(
        &owner,
        1,
        "block",
        GoalChange::SetDisposition {
            goal_id: GoalId::parse("saved-goal").unwrap(),
            disposition: GoalDisposition::Blocked(
                GoalText::parse("  exact blocked reason  ").unwrap(),
            ),
        },
    );
    db.apply_conversation_goal(&blocked, &transcript, GoalTimestampUs::parse(11).unwrap())
        .await
        .unwrap();
    creation(
        &db,
        &transcript,
        "first",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    creation(
        &db,
        &transcript,
        "second",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    creation(
        &db,
        &transcript,
        "third",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    let pending = db.pending_coordinator_watch_events(64).await.unwrap();
    assert_eq!(pending.len(), 3);
    assert_eq!(
        db.coordinator_watch_event_presentations(std::slice::from_ref(&first.event_id))
            .await
            .unwrap()
            .remove(&first.event_id)
            .unwrap()
            .goal_capture,
        first.goal_capture
    );
    for event in pending
        .iter()
        .filter(|event| event.source_occurrence_id != "first")
    {
        assert_eq!(event.outcome, WatchOutcome::Completed);
        assert!(
            matches!(&event.goal_capture, Some(WatchGoalCapture::CapturedSavedGoal { owner_version: 2, state: WatchGoalState::Blocked { reason }, .. }) if reason == "  exact blocked reason  ")
        );
    }
    assert!(sqlx::query(
        "UPDATE coordinator_watch_goal_captures SET objective='rewrite' WHERE event_id=?"
    )
    .bind(&first.event_id)
    .execute(db.pool())
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM coordinator_watch_goal_captures WHERE event_id=?")
            .bind(&first.event_id)
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE coordinator_watch_events SET goal_capture_kind='disabled' WHERE event_id=?"
    )
    .bind(&first.event_id)
    .execute(db.pool())
    .await
    .is_err());
}

#[tokio::test]
async fn no_goal_disabled_and_legacy_are_distinct_and_later_installation_does_not_backfill() {
    let (db, transcript, owner) = fixture().await;
    creation(
        &db,
        &transcript,
        "disabled",
        GoalControlInstallation::Disabled,
    )
    .await
    .unwrap();
    creation(
        &db,
        &transcript,
        "no-goal",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    let watches: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) VALUES ('legacy',?,'creation','old',0,?,'completed',1)")
        .bind(watches).bind(&transcript).execute(db.pool()).await.unwrap();
    create_goal(&db, &transcript, &owner).await;
    creation(
        &db,
        &transcript,
        "disabled",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    let pending = db.pending_coordinator_watch_events(64).await.unwrap();
    assert!(pending
        .iter()
        .any(|e| e.goal_capture == Some(WatchGoalCapture::LegacyNotCaptured)));
    assert!(pending
        .iter()
        .any(|e| e.goal_capture == Some(WatchGoalCapture::NotCapturedDisabled)));
    assert!(pending.iter().any(|e| matches!(
        &e.goal_capture,
        Some(WatchGoalCapture::CapturedNoGoal {
            owner_version: 0,
            ..
        })
    )));
    assert_eq!(pending.len(), 3);
}

#[tokio::test]
async fn standalone_occurrences_are_per_version_and_retained_enrollment_not_execution_and_failure_is_atomic(
) {
    let (db, transcript, owner) = fixture().await;
    let entry = create_goal(&db, &transcript, &owner).await;
    assert!(db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());
    let mut tx = db.pool().begin().await.unwrap();
    settled_change_tx(
        &mut tx,
        GoalControlInstallation::Installed,
        &entry,
        &transcript,
    )
    .await
    .unwrap();
    settled_change_tx(
        &mut tx,
        GoalControlInstallation::Installed,
        &entry,
        &transcript,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        db.pending_coordinator_watch_events(64).await.unwrap().len(),
        1
    );
    assert_eq!(
        db.pending_coordinator_watch_events(64).await.unwrap()[0].outcome,
        WatchOutcome::GoalChanged
    );
    db.unwatch_product_conversation(&owner).await.unwrap();
    db.watch_product_conversation(&owner).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    settled_change_tx(
        &mut tx,
        GoalControlInstallation::Installed,
        &entry,
        &transcript,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM coordinator_watch_events WHERE terminal_kind='goal_changed'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    sqlx::query("CREATE TRIGGER fail_goal_watch_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'fixture capture failure'); END")
        .execute(db.pool()).await.unwrap();
    assert!(creation(
        &db,
        &transcript,
        "must-rollback",
        GoalControlInstallation::Installed
    )
    .await
    .is_err());
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM coordinator_watch_events WHERE source_occurrence_id='must-rollback'").fetch_one(db.pool()).await.unwrap(), 0);
    sqlx::query("DROP TRIGGER fail_goal_watch_capture")
        .execute(db.pool())
        .await
        .unwrap();
    creation(
        &db,
        &transcript,
        "must-rollback",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn installed_settled_admission_rolls_back_goal_and_retry_result_on_append_failure() {
    let (db, transcript, owner) = fixture().await;
    let db = db.install_goal_controls_for_tests().await.unwrap();
    let create = mutation(
        &owner,
        0,
        "atomic-create",
        GoalChange::Create {
            goal_id: GoalId::parse("atomic-goal").unwrap(),
            objective: GoalText::parse("atomic objective").unwrap(),
        },
    );
    sqlx::query("CREATE TRIGGER fail_goal_watch_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'fixture capture failure'); END")
        .execute(db.pool()).await.unwrap();
    assert!(db
        .apply_conversation_goal(&create, &transcript, GoalTimestampUs::parse(10).unwrap())
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_goal_history")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_goal_ids")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_goal_actors")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DROP TRIGGER fail_goal_watch_capture")
        .execute(db.pool())
        .await
        .unwrap();
    let entry = db
        .apply_conversation_goal(&create, &transcript, GoalTimestampUs::parse(10).unwrap())
        .await
        .unwrap();
    assert_eq!(
        db.apply_conversation_goal(&create, &transcript, GoalTimestampUs::parse(11).unwrap())
            .await
            .unwrap(),
        entry
    );
    assert_eq!(
        db.pending_coordinator_watch_events(64).await.unwrap().len(),
        1
    );
    let stale = mutation(
        &owner,
        0,
        "stale",
        GoalChange::SetDisposition {
            goal_id: GoalId::parse("atomic-goal").unwrap(),
            disposition: GoalDisposition::Underway,
        },
    );
    assert!(db
        .apply_conversation_goal(&stale, &transcript, GoalTimestampUs::parse(12).unwrap())
        .await
        .is_err());
    assert_eq!(
        db.pending_coordinator_watch_events(64).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn stop_does_not_invent_goal_change_and_later_cancel_failure_cannot_undo_latch() {
    use crate::conversation_goals::control::{ControlCommand, ControlRequest};
    let (db, transcript, owner) = fixture().await;
    create_goal(&db, &transcript, &owner).await;
    let db = db.install_goal_controls_for_tests().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_goal_watch_capture BEFORE INSERT ON coordinator_watch_goal_captures BEGIN SELECT RAISE(ABORT,'fixture capture failure'); END")
        .execute(db.pool()).await.unwrap();
    let request = ControlRequest {
        owner: owner.clone(),
        principal_id: uuid::Uuid::new_v4(),
        request_id: uuid::Uuid::new_v4(),
        command: ControlCommand::Stop,
        expected_control_version: 0,
    };
    db.apply_goal_control(&request, GoalTimestampUs::parse(11).unwrap())
        .await
        .unwrap();
    assert!(db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM conversation_goal_history WHERE owner_id=? ORDER BY version DESC LIMIT 1").bind(owner.as_str()).fetch_one(db.pool()).await.unwrap(), "underway");
    let mut tx = db.pool().begin().await.unwrap();
    assert!(record_steering_event_tx(
        &mut tx,
        MessageExecutionSource::Steering,
        "actual-cancel",
        &transcript,
        "Cancelled",
        None,
        db.goal_control_installation()
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
    assert!(db
        .ordinary_goal_control(&owner)
        .await
        .unwrap()
        .unwrap()
        .stopped());
    assert!(db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DROP TRIGGER fail_goal_watch_capture")
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    record_steering_event_tx(
        &mut tx,
        MessageExecutionSource::Steering,
        "actual-cancel",
        &transcript,
        "Cancelled",
        None,
        db.goal_control_installation(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let events = db.pending_coordinator_watch_events(64).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, WatchOutcome::Cancelled);
    assert!(matches!(
        &events[0].goal_capture,
        Some(WatchGoalCapture::CapturedSavedGoal {
            state: WatchGoalState::Underway,
            ..
        })
    ));
}

#[tokio::test]
async fn captured_facts_survive_source_deletion_and_follow_event_lifetime() {
    let (db, transcript, owner) = fixture().await;
    create_goal(&db, &transcript, &owner).await;
    creation(
        &db,
        &transcript,
        "saved-before-delete",
        GoalControlInstallation::Installed,
    )
    .await
    .unwrap();
    let before = db
        .pending_coordinator_watch_events(1)
        .await
        .unwrap()
        .remove(0);
    db.delete_conversation(&transcript).await.unwrap();
    assert!(db.conversation_goal(&owner).await.unwrap().is_none());
    let mut presentations = db
        .coordinator_watch_event_presentations(&[before.event_id.clone()])
        .await
        .unwrap();
    let after = presentations
        .remove(&before.event_id)
        .expect("retained event presentation");
    assert_eq!(after.goal_capture, before.goal_capture);
    assert!(matches!(
        after.goal_capture,
        Some(WatchGoalCapture::CapturedSavedGoal { .. })
    ));
    sqlx::query("DELETE FROM coordinator_watch_events WHERE event_id=?")
        .bind(&after.event_id)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM coordinator_watch_goal_captures WHERE event_id=?"
        )
        .bind(&after.event_id)
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn capture_schema_is_source_delete_safe_and_cannot_commit_partial_or_nonlocal_goal_change() {
    let (db, transcript, owner) = fixture().await;
    let foreign_tables: Vec<String> = sqlx::query_scalar(
        "SELECT \"table\" FROM pragma_foreign_key_list('coordinator_watch_goal_captures')",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(foreign_tables, ["coordinator_watch_events"]);
    let watch: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id=?",
    )
    .bind(owner.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,goal_capture_kind) VALUES ('missing-capture',?,'goal_change','owner',1,?,'goal_changed',10,'captured')")
        .bind(watch).bind(&transcript).execute(&mut *tx).await.unwrap();
    assert!(tx.commit().await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM coordinator_watch_events WHERE event_id='missing-capture'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
    assert!(sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) VALUES ('unmatched',?,'creation','owner',0,?,'goal_changed',10)")
        .bind(watch).bind(&transcript).execute(db.pool()).await.is_err());
    assert!(sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,route_kind,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,goal_capture_kind) VALUES ('not-local',NULL,'mandatory_close_failure','goal_change','owner',1,?,'goal_changed',10,'captured')")
        .bind(&transcript).execute(db.pool()).await.is_err());
}
