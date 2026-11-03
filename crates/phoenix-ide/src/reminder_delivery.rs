//! REQ-REM-004/005/007: clock facts submitted through ordinary input admission.
use std::{sync::Arc, time::Duration};

use phoenix_core::domain::{db_schema::InputOrigin, reminder::ReminderTimestampUs};
use phoenix_db::DueReminderCursor;

use crate::{
    runtime::RuntimeManager,
    send_chat_service::{
        MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
    },
};

pub(crate) async fn deliver_pass(runtime: &Arc<RuntimeManager>, now: ReminderTimestampUs) {
    let db = runtime.db();
    if let Err(error) = db.promote_due_reminders(now, 100).await {
        tracing::warn!(%error, "reminder clock promotion failed; persisted facts will be rediscovered");
    }
    let service = SendChatApplicationService::new(db.clone(), runtime.clone());
    let mut cursor = None;
    loop {
        let rows = match db.pending_due_reminders(cursor.as_ref(), 16).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error,"due reminder discovery failed");
                return;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = Some(DueReminderCursor {
                due_at_us: row.reminder.due_at_us,
                reminder_id: row.reminder.id.clone(),
            });
            let request = SendChatRequest {
                conversation_id: row.transcript_id,
                origin: InputOrigin::ScheduledReminder {
                    reminder_id: row.reminder.id.clone(),
                },
                text: row.reminder.text.as_str().to_owned(),
                message_id: row.reminder.input_message_id(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            };
            match service.send(request).await {
                Ok(
                    SendChatOutcome::Delivered
                    | SendChatOutcome::AlreadyPersisted
                    | SendChatOutcome::QueuedAsSteering,
                ) => {}
                Ok(SendChatOutcome::Rejected { message, code }) => {
                    tracing::debug!(reminder_id=%row.reminder.id.as_str(),code,%message,"reminder admission refused; retrying through ordinary admission");
                }
                Err(error) => {
                    tracing::warn!(reminder_id=%row.reminder.id.as_str(),%error,"reminder admission failed; retrying");
                }
            }
        }
    }
}

pub(crate) async fn next_wait(
    runtime: &RuntimeManager,
    now: ReminderTimestampUs,
    maximum: Duration,
) -> Duration {
    match runtime.db().next_reminder_deadline().await {
        Ok(Some(deadline)) if deadline <= now => {
            maximum.min(crate::runtime::direct_turn_worker::ERROR_RETRY_INTERVAL)
        }
        Ok(Some(deadline)) => maximum.min(Duration::from_micros(
            u64::try_from(deadline.get() - now.get()).unwrap_or_default(),
        )),
        Ok(None) => maximum,
        Err(error) => {
            tracing::warn!(%error,"reminder deadline discovery failed");
            maximum
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::{
        reminder::{Reminder, ReminderState, ReminderText, ReminderTimezone},
        sm_state::ConvState,
    };
    use phoenix_db::{CreateReminderInput, Database};

    fn time(value: i64) -> ReminderTimestampUs {
        ReminderTimestampUs::parse(value).unwrap()
    }
    fn manager(db: &Database) -> Arc<RuntimeManager> {
        Arc::new(RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ))
    }
    async fn save(db: &Database, transcript: &str, call: &str) -> Reminder {
        db.create_reminder(&CreateReminderInput {
            source_transcript_id: transcript.into(),
            source_message_id: "assistant-message".into(),
            source_tool_use_id: call.into(),
            request_fingerprint: "a".repeat(64),
            text: ReminderText::parse("Check Safari and capture storage screenshots.").unwrap(),
            due_at_us: time(20),
            timezone: ReminderTimezone::parse("America/New_York").unwrap(),
            now_us: time(10),
        })
        .await
        .unwrap()
    }
    async fn count(db: &Database, reminder: &Reminder) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM durable_turns WHERE client_turn_key=?1")
            .bind(reminder.input_message_id())
            .fetch_one(db.pool())
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn due_fact_survives_model_failure_and_restart_with_one_standard_admission() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("reminder-direct", "direct", "/tmp", true, None, None)
            .await
            .unwrap();
        let reminder = save(&db, &source.id, "create").await;
        // Crash after the durable clock fact and before ordinary admission.
        db.promote_due_reminders(time(21), 100).await.unwrap();
        let runtime = manager(&db);
        deliver_pass(&runtime, time(30)).await;
        assert_eq!(count(&db, &reminder).await, 1);
        assert!(db
            .pending_due_reminders(None, 100)
            .await
            .unwrap()
            .is_empty());
        let due = db
            .get_reminder(&source.id, &reminder.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            due.state,
            ReminderState::Due {
                occurred_at_us: time(21)
            }
        );
        let loaded: String = sqlx::query_scalar(
            "SELECT origin_reminder_id FROM durable_turns WHERE client_turn_key=?1",
        )
        .bind(reminder.input_message_id())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(loaded, reminder.id.as_str());
        let repo = db.workflow_repository();
        let accepted = repo
            .load_owning_authoritative_turn(&phoenix_workflow::ConversationAuthority(
                source.id.clone(),
            ))
            .await
            .unwrap()
            .unwrap();
        repo.terminate_authoritative_turn(phoenix_workflow::TurnCommand::Cancel {
            turn_id: accepted.id,
            expected_generation: accepted.generation,
        })
        .await
        .unwrap();
        let stopped = repo
            .load_authoritative_turn(accepted.id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stopped.lifecycle,
            phoenix_workflow::TurnLifecycle::Terminal {
                terminal: phoenix_workflow::TurnTerminal::Cancelled,
                ..
            }
        ));
        let restarted = manager(&db);
        deliver_pass(&restarted, time(100)).await;
        assert_eq!(count(&db, &reminder).await, 1);
    }
    #[tokio::test]
    async fn busy_uses_standard_steering_and_removed_input_is_not_recreated() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("reminder-steering", "queue", "/tmp", true, None, None)
            .await
            .unwrap();
        let reminder = save(&db, &source.id, "create").await;
        db.update_conversation_state(&source.id, &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        let runtime = manager(&db);
        runtime
            .inject_handle_for_test(&source.id, ConvState::LlmRequesting { attempt: 1 })
            .await;
        deliver_pass(&runtime, time(30)).await;
        let queue = db.get_steering_queue(&source.id).await.unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].message_id, reminder.input_message_id());
        assert_eq!(
            queue[0].origin,
            InputOrigin::ScheduledReminder {
                reminder_id: reminder.id.clone()
            }
        );
        db.remove_steering_entries(&source.id, &[reminder.input_message_id()])
            .await
            .unwrap();
        assert!(db.get_steering_queue(&source.id).await.unwrap().is_empty());
        deliver_pass(&manager(&db), time(100)).await;
        assert!(db.get_steering_queue(&source.id).await.unwrap().is_empty());
        assert!(db
            .pending_due_reminders(None, 100)
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn reminder_routes_to_stable_conversation_successor() {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("reminder-old", "old", "/tmp", true, None, None)
            .await
            .unwrap();
        let reminder = save(&db, &source.id, "create").await;
        db.update_conversation_state(
            &source.id,
            &ConvState::ContextExhausted {
                summary: "Continue".into(),
            },
        )
        .await
        .unwrap();
        let next = match db.continue_conversation(&source.id).await.unwrap() {
            phoenix_db::ContinueOutcome::Created(next) => next,
            other @ (phoenix_db::ContinueOutcome::AlreadyContinued(_)
            | phoenix_db::ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("{other:?}")
            }
        };
        deliver_pass(&manager(&db), time(30)).await;
        let target: String = sqlx::query_scalar(
            "SELECT conversation_id FROM durable_turns WHERE client_turn_key=?1",
        )
        .bind(reminder.input_message_id())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(target, next.id);
        assert_eq!(count(&db, &reminder).await, 1);
    }
    #[tokio::test]
    async fn awaiting_user_and_approval_preserve_due_until_ordinary_admission_accepts() {
        let db = Database::open_in_memory().await.unwrap();
        let mut reminders = Vec::new();
        for (name, state) in [
            (
                "pending-question",
                ConvState::AwaitingUserResponse {
                    questions: vec![],
                    tool_use_id: "question".into(),
                    request_authority:
                        phoenix_core::domain::sm_state::QuestionRequestAuthority::new(),
                },
            ),
            (
                "pending-approval",
                ConvState::AwaitingTaskApproval {
                    task_file: "tasks/12345-p1-ready--example.md".into(),
                    title: "Example".into(),
                    priority: phoenix_core::task_source::Priority::P1,
                    plan: "Review".into(),
                },
            ),
        ] {
            let source = db
                .create_conversation(name, name, "/tmp", true, None, None)
                .await
                .unwrap();
            let reminder = save(&db, &source.id, "create").await;
            db.update_conversation_state(&source.id, &state)
                .await
                .unwrap();
            reminders.push((source, reminder));
        }
        let runtime = manager(&db);
        deliver_pass(&runtime, time(30)).await;
        assert_eq!(db.pending_due_reminders(None, 100).await.unwrap().len(), 2);
        for (source, reminder) in &reminders {
            assert_eq!(count(&db, reminder).await, 0);
            assert!(db.get_steering_queue(&source.id).await.unwrap().is_empty());
        }
        for (source, _) in &reminders {
            db.update_conversation_state(&source.id, &ConvState::Idle)
                .await
                .unwrap();
        }
        deliver_pass(&runtime, time(40)).await;
        deliver_pass(&manager(&db), time(50)).await;
        assert!(db
            .pending_due_reminders(None, 100)
            .await
            .unwrap()
            .is_empty());
        for (_, reminder) in &reminders {
            assert_eq!(count(&db, reminder).await, 1);
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use phoenix_core::domain::reminder::{ReminderText, ReminderTimezone};

    fn time(value: i64) -> ReminderTimestampUs {
        ReminderTimestampUs::parse(value).unwrap()
    }
    #[tokio::test]
    async fn next_deadline_wait_is_bounded_and_elapsed_deadlines_use_existing_retry_cadence() {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        db.create_conversation("deadline-wait", "wait", "/tmp", true, None, None)
            .await
            .unwrap();
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let maximum = Duration::from_secs(5);
        assert_eq!(next_wait(&runtime, time(10), maximum).await, maximum);
        db.create_reminder(&phoenix_db::CreateReminderInput {
            source_transcript_id: "deadline-wait".into(),
            source_message_id: "assistant-message".into(),
            source_tool_use_id: "create".into(),
            request_fingerprint: "a".repeat(64),
            text: ReminderText::parse("check").unwrap(),
            due_at_us: time(2_000_000),
            timezone: ReminderTimezone::parse("UTC").unwrap(),
            now_us: time(10),
        })
        .await
        .unwrap();
        assert_eq!(
            next_wait(&runtime, time(1_000_000), maximum).await,
            Duration::from_secs(1)
        );
        let retry = crate::runtime::direct_turn_worker::ERROR_RETRY_INTERVAL;
        assert_eq!(next_wait(&runtime, time(2_000_000), maximum).await, retry);
        assert_eq!(next_wait(&runtime, time(3_000_000), maximum).await, retry);
        assert_eq!(
            next_wait(&runtime, time(10), Duration::from_millis(20)).await,
            Duration::from_millis(20)
        );
        assert_eq!(
            next_wait(&runtime, time(3_000_000), Duration::from_millis(20)).await,
            Duration::from_millis(20)
        );
    }
}
