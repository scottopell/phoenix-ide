use super::*;

fn time(value: i64) -> ReminderTimestampUs {
    ReminderTimestampUs::parse(value).unwrap()
}

fn input(transcript_id: &str, call: &str, deadline: i64) -> CreateReminderInput {
    CreateReminderInput {
        source_transcript_id: transcript_id.into(),
        source_tool_use_id: call.into(),
        request_fingerprint: "a".repeat(64),
        text: ReminderText::parse("Check Safari Web Inspector and capture storage screenshots.")
            .unwrap(),
        due_at_us: time(deadline),
        timezone: ReminderTimezone::parse("America/New_York").unwrap(),
        now_us: time(10),
    }
}

async fn source(db: &Database, name: &str) -> crate::Conversation {
    db.create_conversation(name, name, "/tmp", true, None, None)
        .await
        .unwrap()
}

async fn file_db(path: &str) -> Database {
    let db = Database::open(path).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    db
}

#[tokio::test]
async fn reminders_create_replay_preserves_original_deadline_and_rejects_conflict() {
    let db = Database::open_in_memory().await.unwrap();
    let source = source(&db, "reminder-source").await;
    let original = db
        .create_reminder(&input(&source.id, "call", 20))
        .await
        .unwrap();
    let mut retry = input(&source.id, "call", 500);
    retry.now_us = time(100);
    assert_eq!(db.create_reminder(&retry).await.unwrap(), original);
    assert_eq!(
        db.reminder_for_create_call(&source.id, "call", &retry.request_fingerprint)
            .await
            .unwrap(),
        Some(original.clone())
    );
    retry.request_fingerprint = "b".repeat(64);
    assert!(matches!(
        db.create_reminder(&retry).await,
        Err(DbError::ReminderRequestConflict)
    ));
    assert!(matches!(
        db.reminder_for_create_call(&source.id, "call", &retry.request_fingerprint)
            .await,
        Err(DbError::ReminderRequestConflict)
    ));
    assert_eq!(
        db.list_reminders(&source.id, 100).await.unwrap(),
        vec![original]
    );
}

#[tokio::test]
async fn reminders_reject_past_deadlines_and_cross_conversation_mutation() {
    let db = Database::open_in_memory().await.unwrap();
    let owner = source(&db, "reminder-owner").await;
    let other = source(&db, "reminder-other").await;
    assert!(matches!(
        db.create_reminder(&input(&owner.id, "past", 10)).await,
        Err(DbError::ReminderInvalid(_))
    ));
    let reminder = db
        .create_reminder(&input(&owner.id, "future", 20))
        .await
        .unwrap();
    assert!(db
        .get_reminder(&other.id, &reminder.id)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        db.cancel_reminder(&other.id, &reminder.id, reminder.revision, time(11))
            .await
            .unwrap(),
        ReminderChange::NotFound
    ));
    assert_eq!(
        db.get_reminder(&owner.id, &reminder.id).await.unwrap(),
        Some(reminder)
    );
}

#[tokio::test]
async fn reminders_coordinator_owns_reminders_and_subagent_role_is_rejected() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let reminder = db
        .create_reminder(&input(&coordinator.id, "coordinator", 20))
        .await
        .unwrap();
    assert_eq!(
        reminder.product_conversation_id,
        coordinator.product_conversation_id
    );
    db.promote_due_reminders(time(20), 100).await.unwrap();
    assert_eq!(
        db.pending_due_reminders(None, 100).await.unwrap()[0].transcript_id,
        coordinator.id
    );

    let parent = source(&db, "reminder-actor-parent").await;
    sqlx::query(
        "INSERT INTO conversations (id, product_conversation_id, slug, title,
        parent_conversation_id, user_initiated, state, state_kind, state_updated_at,
        created_at, updated_at, archived, transcript_generation, model, effort,
        llm_language, cm_kind, work_scope_id, runtime_role)
        SELECT 'reminder-child', product_conversation_id, 'reminder-child', 'Reminder child',
        id, 0, state, state_kind, state_updated_at, created_at, updated_at, 0, 1,
        model, effort, llm_language, cm_kind, work_scope_id, 'sub_agent'
        FROM conversations WHERE id = ?1",
    )
    .bind(&parent.id)
    .execute(db.pool())
    .await
    .unwrap();
    assert!(matches!(
        db.create_reminder(&input("reminder-child", "child", 20))
            .await,
        Err(DbError::ReminderInvalid(_))
    ));
}

#[tokio::test]
async fn reminders_reschedule_invalidates_old_deadline_and_due_is_immutable() {
    let db = Database::open_in_memory().await.unwrap();
    let source = source(&db, "reminder-reschedule").await;
    let reminder = db
        .create_reminder(&input(&source.id, "call", 20))
        .await
        .unwrap();
    let change = RescheduleReminderInput {
        source_transcript_id: source.id.clone(),
        reminder_id: reminder.id.clone(),
        expected_revision: reminder.revision,
        due_at_us: time(30),
        timezone: ReminderTimezone::parse("UTC").unwrap(),
        now_us: time(12),
    };
    let ReminderChange::Updated { reminder: moved } =
        db.reschedule_reminder(&change).await.unwrap()
    else {
        panic!("reschedule should commit");
    };
    assert_eq!(moved.revision.get(), 2);
    assert_eq!(db.next_reminder_deadline().await.unwrap(), Some(time(30)));
    assert!(matches!(
        db.reschedule_reminder(&change).await.unwrap(),
        ReminderChange::RevisionConflict { .. }
    ));
    assert!(db
        .promote_due_reminders(time(20), 100)
        .await
        .unwrap()
        .is_empty());
    let due = db.promote_due_reminders(time(30), 100).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(
        due[0].input_message_id(),
        format!("reminder:{}:2", reminder.id.as_str())
    );
    assert!(db
        .promote_due_reminders(time(50), 100)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        db.cancel_reminder(&source.id, &reminder.id, moved.revision, time(40))
            .await
            .unwrap(),
        ReminderChange::AlreadyDue { .. }
    ));
    assert!(matches!(
        db.reschedule_reminder(&RescheduleReminderInput {
            expected_revision: moved.revision,
            due_at_us: time(50),
            ..change
        })
        .await
        .unwrap(),
        ReminderChange::AlreadyDue { .. }
    ));
    assert!(
        sqlx::query("UPDATE reminders SET text = 'changed' WHERE id = ?1")
            .bind(reminder.id.as_str())
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE reminders SET state = 'scheduled', occurred_at_us = NULL WHERE id = ?1"
    )
    .bind(reminder.id.as_str())
    .execute(db.pool())
    .await
    .is_err());
}

#[tokio::test]
async fn reminders_cancel_wins_before_due_and_is_idempotently_visible() {
    let db = Database::open_in_memory().await.unwrap();
    let source = source(&db, "reminder-cancel").await;
    let reminder = db
        .create_reminder(&input(&source.id, "call", 20))
        .await
        .unwrap();
    assert!(matches!(
        db.cancel_reminder(
            &source.id,
            &reminder.id,
            ReminderRevision::parse(2).unwrap(),
            time(12)
        )
        .await
        .unwrap(),
        ReminderChange::RevisionConflict { .. }
    ));
    assert!(matches!(
        db.cancel_reminder(&source.id, &reminder.id, reminder.revision, time(12))
            .await
            .unwrap(),
        ReminderChange::Updated {
            reminder: Reminder {
                state: ReminderState::Cancelled { .. },
                ..
            }
        }
    ));
    assert!(matches!(
        db.cancel_reminder(&source.id, &reminder.id, reminder.revision, time(13))
            .await
            .unwrap(),
        ReminderChange::AlreadyCancelled { .. }
    ));
    assert!(db
        .promote_due_reminders(time(30), 100)
        .await
        .unwrap()
        .is_empty());
    assert!(db.next_reminder_deadline().await.unwrap().is_none());
}

#[tokio::test]
async fn reminders_steering_acceptance_prevents_rediscovery_after_queue_removal() {
    let db = Database::open_in_memory().await.unwrap();
    let source = source(&db, "reminder-admitted").await;
    let reminder = db
        .create_reminder(&input(&source.id, "call", 20))
        .await
        .unwrap();
    db.promote_due_reminders(time(20), 100).await.unwrap();
    let due = db.pending_due_reminders(None, 100).await.unwrap();
    assert_eq!(due.len(), 1);
    sqlx::query("INSERT INTO steering_acceptance_receipts (conversation_id, message_id, request_fingerprint) VALUES (?1, ?2, ?3)")
        .bind(&source.id).bind(reminder.input_message_id()).bind("accepted-payload")
        .execute(db.pool()).await.unwrap();
    let live_queue_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages WHERE conversation_id = ?1")
            .bind(&source.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(live_queue_count, 0);
    assert!(db
        .pending_due_reminders(None, 100)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        db.get_reminder(&source.id, &reminder.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ReminderState::Due { .. }
    ));
}

#[tokio::test]
async fn reminders_due_survives_reopen_with_no_client_and_preserves_other_reminders() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reminders.db");
    let path = path.to_str().unwrap();
    let db = file_db(path).await;
    let owner = source(&db, "reminder-reopen").await;
    let other = source(&db, "reminder-preserve").await;
    let reminder = db
        .create_reminder(&input(&owner.id, "due", 20))
        .await
        .unwrap();
    let later = db
        .create_reminder(&input(&other.id, "later", 100))
        .await
        .unwrap();
    db.pool.close().await;
    let db = file_db(path).await;
    let due = db.promote_due_reminders(time(80), 1).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, reminder.id);
    db.pool.close().await;
    let db = file_db(path).await;
    let pending = db.pending_due_reminders(None, 100).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].reminder.state,
        ReminderState::Due {
            occurred_at_us: time(80)
        }
    );
    assert_eq!(
        db.list_reminders(&other.id, 100).await.unwrap(),
        vec![later]
    );
    assert_eq!(db.next_reminder_deadline().await.unwrap(), Some(time(100)));
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn reminders_schema_rejects_invalid_shapes_and_deletion_preserves_other_owner() {
    let db = Database::open_in_memory().await.unwrap();
    let owner = source(&db, "reminder-delete").await;
    let other = source(&db, "reminder-keep").await;
    let reminder = db
        .create_reminder(&input(&owner.id, "delete", 20))
        .await
        .unwrap();
    let kept = db
        .create_reminder(&input(&other.id, "keep", 30))
        .await
        .unwrap();
    for mutation in [
        "state = 'due'",
        "due_at_us = -1, revision = 2",
        "revision = 0",
        "due_at_us = 0.5, revision = 2",
        "text = ''",
        "product_conversation_id = 'other'",
    ] {
        let sql = format!("UPDATE reminders SET {mutation} WHERE id = ?1");
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(reminder.id.as_str())
                .execute(db.pool())
                .await
                .is_err(),
            "accepted mutation {mutation}"
        );
    }
    db.delete_conversation(&owner.id).await.unwrap();
    assert_eq!(db.list_reminders(&other.id, 100).await.unwrap(), vec![kept]);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reminders WHERE id = ?1")
        .bind(reminder.id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn reminders_keyset_discovery_advances_past_unaccepted_recipients() {
    let db = Database::open_in_memory().await.unwrap();
    let first_owner = source(&db, "reminder-page-first").await;
    let second_owner = source(&db, "reminder-page-second").await;
    db.create_reminder(&input(&first_owner.id, "first", 20))
        .await
        .unwrap();
    db.create_reminder(&input(&second_owner.id, "second", 30))
        .await
        .unwrap();
    db.promote_due_reminders(time(40), 100).await.unwrap();
    let first = db.pending_due_reminders(None, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    let cursor = DueReminderCursor {
        due_at_us: first[0].reminder.due_at_us,
        reminder_id: first[0].reminder.id.clone(),
    };
    let second = db.pending_due_reminders(Some(&cursor), 1).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].transcript_id, second_owner.id);
    assert_eq!(
        db.pending_due_reminders(None, 1).await.unwrap()[0].transcript_id,
        first_owner.id
    );
}

#[tokio::test]
async fn reminders_real_sqlite_cancel_and_due_race_has_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let path = path.to_str().unwrap();
    let observer = file_db(path).await;
    let canceller = file_db(path).await;
    let owner = source(&observer, "reminder-race").await;
    let reminder = observer
        .create_reminder(&input(&owner.id, "race", 20))
        .await
        .unwrap();
    let (due, cancelled) = tokio::join!(
        observer.promote_due_reminders(time(20), 100),
        canceller.cancel_reminder(&owner.id, &reminder.id, reminder.revision, time(20)),
    );
    let due = due.unwrap();
    let cancelled = cancelled.unwrap();
    match observer
        .get_reminder(&owner.id, &reminder.id)
        .await
        .unwrap()
        .unwrap()
        .state
    {
        ReminderState::Due { .. } => {
            assert_eq!(due.len(), 1);
            assert!(matches!(cancelled, ReminderChange::AlreadyDue { .. }));
        }
        ReminderState::Cancelled { .. } => {
            assert!(due.is_empty());
            assert!(matches!(cancelled, ReminderChange::Updated { .. }));
        }
        ReminderState::Scheduled => panic!("race must commit one terminal outcome"),
    }
}

#[tokio::test]
async fn reminders_cancellation_accepts_a_backward_clock_observation() {
    let db = Database::open_in_memory().await.unwrap();
    let source = source(&db, "reminder-clock-backwards").await;
    let reminder = db
        .create_reminder(&input(&source.id, "call", 20))
        .await
        .unwrap();
    let outcome = db
        .cancel_reminder(&source.id, &reminder.id, reminder.revision, time(5))
        .await
        .unwrap();
    let ReminderChange::Updated { reminder } = outcome else {
        panic!("cancellation should commit");
    };
    assert_eq!(reminder.created_at_us, time(10));
    assert_eq!(
        reminder.state,
        ReminderState::Cancelled {
            cancelled_at_us: time(5)
        }
    );
}
