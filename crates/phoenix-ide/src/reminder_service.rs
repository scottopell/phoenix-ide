//! REQ-REM-001/002/003/006: trusted conversation binding and committed receipts.
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use chrono::Utc;
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::reminder::{Reminder, ReminderTimestampUs};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{CreateReminderInput, RescheduleReminderInput};
use phoenix_tools::reminders::{ReminderOperation, ReminderService};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::runtime::{AdmittedOperation, RuntimeManager};

pub(crate) struct ReminderWritePass {
    owner: Option<AdmittedOperation>,
    reason: &'static str,
}

impl ReminderWritePass {
    pub(crate) fn new(owner: AdmittedOperation, reason: &'static str) -> Self {
        Self {
            owner: Some(owner),
            reason,
        }
    }

    pub(crate) fn complete<T>(
        mut self,
        result: LocalAuthorityResult<phoenix_db::DbResult<T>>,
    ) -> phoenix_db::DbResult<T> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                self.owner.take();
                result
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(phoenix_db::DbError::Serialization(
                    "Reminder commit could not be classified; local authority closed".into(),
                ))
            }
        }
    }
}

impl Drop for ReminderWritePass {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.close(self.reason);
        }
    }
}

pub(crate) fn tool(
    runtime: &Arc<RuntimeManager>,
    transcript_id: String,
) -> Arc<dyn phoenix_tools::Tool> {
    Arc::new(phoenix_tools::RemindersTool::new(Arc::new(
        HostReminderService {
            runtime: Arc::downgrade(runtime),
            transcript_id,
        },
    )))
}

struct HostReminderService {
    runtime: Weak<RuntimeManager>,
    transcript_id: String,
}

#[derive(serde::Serialize)]
struct AgentReminderReceipt<'a> {
    id: &'a phoenix_core::domain::reminder::ReminderId,
    text: &'a phoenix_core::domain::reminder::ReminderText,
    scheduled_at: String,
    timezone: &'a phoenix_core::domain::reminder::ReminderTimezone,
    revision: phoenix_core::domain::reminder::ReminderRevision,
    state: phoenix_core::domain::reminder::ReminderState,
    notification_capability: &'static str,
}

pub(crate) fn receipt(reminder: &Reminder) -> Result<Value, String> {
    let instant = chrono::DateTime::from_timestamp_micros(reminder.due_at_us.get())
        .ok_or("reminder date is outside supported display range")?;
    let zone: chrono_tz::Tz = reminder
        .timezone
        .as_str()
        .parse()
        .map_err(|_| "invalid reminder timezone")?;
    serde_json::to_value(AgentReminderReceipt {
        id: &reminder.id, text: &reminder.text,
        scheduled_at: instant.with_timezone(&zone).to_rfc3339(),
        timezone: &reminder.timezone, revision: reminder.revision, state: reminder.state,
        notification_capability: "In-app reminder and ordinary agent input. Connected browser notifications follow permission, settings and focus policy. Closed-app phone alerts are unavailable.",
    }).map_err(|error| error.to_string())
}

fn change_receipt(change: phoenix_db::ReminderChange) -> Result<Value, String> {
    use phoenix_db::ReminderChange;
    match change {
        ReminderChange::Updated { reminder } => receipt(&reminder),
        ReminderChange::NotFound => Err(
            "Reminder not found in this conversation. Use op=list to inspect saved reminders."
                .into(),
        ),
        ReminderChange::RevisionConflict { current } => Err(format!(
            "The reminder changed. Use its current expected_revision to retry: {}",
            receipt(&current)?
        )),
        ReminderChange::AlreadyDue { current } => Err(format!(
            "This reminder is already due and cannot be retracted: {}",
            receipt(&current)?
        )),
        ReminderChange::AlreadyCancelled { current } => Err(format!(
            "This reminder is already cancelled: {}",
            receipt(&current)?
        )),
    }
}

fn create_fingerprint(operation: &ReminderOperation) -> Result<String, String> {
    use std::fmt::Write as _;
    Ok(
        Sha256::digest(serde_json::to_vec(operation).map_err(|e| e.to_string())?)
            .iter()
            .fold(String::with_capacity(64), |mut output, byte| {
                let _ = write!(output, "{byte:02x}");
                output
            }),
    )
}

impl HostReminderService {
    async fn create(
        &self,
        runtime: &RuntimeManager,
        source_call: &SourceToolCall,
        text: phoenix_core::domain::reminder::ReminderText,
        when: phoenix_tools::reminders::ReminderWhen,
        now: chrono::DateTime<Utc>,
    ) -> Result<Value, String> {
        let authority = runtime
            .acquire_local_authority_pass()
            .map_err(|()| "Reminder creation is unavailable after local authority was closed")?;
        let db = runtime.db();
        let now_us =
            ReminderTimestampUs::parse(now.timestamp_micros()).map_err(|e| e.to_string())?;
        let fingerprint = create_fingerprint(&ReminderOperation::Create {
            text: text.clone(),
            when: when.clone(),
        })?;
        if let Some(existing) = db
            .reminder_for_create_call(
                &self.transcript_id,
                &source_call.message_id,
                &source_call.tool_use_id,
                &fingerprint,
            )
            .await
            .map_err(|e| e.to_string())?
        {
            return receipt(&existing);
        }
        let (due_at_us, timezone) = when.resolve(now)?;
        let input = CreateReminderInput {
            source_transcript_id: self.transcript_id.clone(),
            source_message_id: source_call.message_id.clone(),
            source_tool_use_id: source_call.tool_use_id.clone(),
            request_fingerprint: fingerprint.clone(),
            text,
            due_at_us,
            timezone,
            now_us,
        };
        let command = ReminderWritePass::new(authority, "reminder_create_unclassified");
        let reminder = command
            .complete(create_command(db, &input).await)
            .map_err(|error| error.to_string())?;
        let receipt = receipt(&reminder)?;
        runtime.kick_direct_turn_worker();
        Ok(receipt)
    }
}

async fn create_command(
    db: &phoenix_db::Database,
    input: &CreateReminderInput,
) -> LocalAuthorityResult<phoenix_db::DbResult<Reminder>> {
    let result = match db.create_reminder(input).await {
        Ok(reminder) => reminder,
        Err(
            error @ (phoenix_db::DbError::ReminderInvalid(_)
            | phoenix_db::DbError::ReminderRequestConflict),
        ) => return LocalAuthorityResult::DurableFactEstablished(Err(error)),
        Err(error) => match db
            .reminder_for_create_call(
                &input.source_transcript_id,
                &input.source_message_id,
                &input.source_tool_use_id,
                &input.request_fingerprint,
            )
            .await
        {
            Ok(Some(existing)) => existing,
            Ok(None) => return LocalAuthorityResult::DurableFactEstablished(Err(error)),
            Err(error @ phoenix_db::DbError::ReminderRequestConflict) => {
                return LocalAuthorityResult::DurableFactEstablished(Err(error))
            }
            Err(probe) => {
                tracing::error!(%error, %probe, "reminder create commit could not be classified");
                return LocalAuthorityResult::DurableFactUnclassified;
            }
        },
    };
    LocalAuthorityResult::DurableFactEstablished(Ok(result))
}

#[async_trait]
impl ReminderService for HostReminderService {
    async fn execute(
        &self,
        operation: ReminderOperation,
        source_call: &SourceToolCall,
    ) -> Result<Value, String> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or("runtime is no longer available")?;
        let db = runtime.db();
        let now = Utc::now();
        let now_us =
            ReminderTimestampUs::parse(now.timestamp_micros()).map_err(|e| e.to_string())?;
        let result = match operation {
            ReminderOperation::Create { text, when } => {
                self.create(&runtime, source_call, text, when, now).await?
            }
            ReminderOperation::List {} => {
                let reminders = db
                    .list_reminders(&self.transcript_id, 10)
                    .await
                    .map_err(|e| e.to_string())?;
                json!({"current_utc":now.to_rfc3339(),"reminders":reminders.iter().map(receipt).collect::<Result<Vec<_>, _>>()?,"limit_per_state":10})
            }
            ReminderOperation::Reschedule {
                reminder_id,
                expected_revision,
                when,
            } => {
                let (due_at_us, timezone) = when.resolve(now)?;
                change_receipt(
                    reschedule(
                        &runtime,
                        &RescheduleReminderInput {
                            source_transcript_id: self.transcript_id.clone(),
                            reminder_id,
                            expected_revision,
                            due_at_us,
                            timezone,
                            now_us,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?,
                )?
            }
            ReminderOperation::Cancel {
                reminder_id,
                expected_revision,
            } => change_receipt(
                cancel(
                    &runtime,
                    &self.transcript_id,
                    &reminder_id,
                    expected_revision,
                    now_us,
                )
                .await
                .map_err(|e| e.to_string())?,
            )?,
        };
        Ok(result)
    }
}

pub(crate) async fn reschedule(
    runtime: &RuntimeManager,
    input: &RescheduleReminderInput,
) -> phoenix_db::DbResult<phoenix_db::ReminderChange> {
    let authority = runtime
        .acquire_local_authority_pass()
        .map_err(|()| phoenix_db::DbError::ReminderInvalid("local authority is closed".into()))?;
    let db = runtime.db();
    let command = ReminderWritePass::new(authority, "reminder_reschedule_unclassified");
    let result = command.complete(classify_management_result(
        db.reschedule_reminder(input).await,
    ))?;
    if matches!(result, phoenix_db::ReminderChange::Updated { .. }) {
        runtime.kick_direct_turn_worker();
    }
    Ok(result)
}

fn classify_management_result(
    result: phoenix_db::DbResult<phoenix_db::ReminderChange>,
) -> LocalAuthorityResult<phoenix_db::DbResult<phoenix_db::ReminderChange>> {
    match result {
        Ok(change) => LocalAuthorityResult::DurableFactEstablished(Ok(change)),
        Err(
            error @ (phoenix_db::DbError::ReminderInvalid(_)
            | phoenix_db::DbError::ConversationNotFound(_)
            | phoenix_db::DbError::CloseAdmissionFenced(_)
            | phoenix_db::DbError::ProductConversationUnavailable(_)),
        ) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
        Err(error) => {
            tracing::error!(%error, "reminder management returned no classified command result");
            LocalAuthorityResult::DurableFactUnclassified
        }
    }
}

pub(crate) async fn cancel(
    runtime: &RuntimeManager,
    transcript_id: &str,
    reminder_id: &phoenix_core::domain::reminder::ReminderId,
    expected_revision: phoenix_core::domain::reminder::ReminderRevision,
    now_us: ReminderTimestampUs,
) -> phoenix_db::DbResult<phoenix_db::ReminderChange> {
    let authority = runtime
        .acquire_local_authority_pass()
        .map_err(|()| phoenix_db::DbError::ReminderInvalid("local authority is closed".into()))?;
    let db = runtime.db();
    let command = ReminderWritePass::new(authority, "reminder_cancel_unclassified");
    let result = command.complete(classify_management_result(
        db.cancel_reminder(transcript_id, reminder_id, expected_revision, now_us)
            .await,
    ))?;
    if matches!(result, phoenix_db::ReminderChange::Updated { .. }) {
        runtime.kick_direct_turn_worker();
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::db_schema::SourceToolCall;
    use phoenix_core::domain::reminder::{ReminderText, ReminderTimezone};
    use phoenix_tools::reminders::ReminderWhen;

    #[tokio::test]
    async fn replay_returns_saved_relative_deadline_before_clock_resolution() {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("reminder-replay", "replay", "/tmp", true, None, None)
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
        let operation = ReminderOperation::Create {
            text: ReminderText::parse("Check Safari").unwrap(),
            when: ReminderWhen::After { seconds: 10 },
        };
        let fingerprint = create_fingerprint(&operation).unwrap();
        let saved = db
            .create_reminder(&CreateReminderInput {
                source_transcript_id: source.id.clone(),
                source_message_id: "assistant-message".into(),
                source_tool_use_id: "call".into(),
                request_fingerprint: fingerprint,
                text: ReminderText::parse("Check Safari").unwrap(),
                due_at_us: ReminderTimestampUs::parse(20).unwrap(),
                timezone: ReminderTimezone::parse("UTC").unwrap(),
                now_us: ReminderTimestampUs::parse(10).unwrap(),
            })
            .await
            .unwrap();
        let service = HostReminderService {
            runtime: Arc::downgrade(&runtime),
            transcript_id: source.id,
        };
        let replay = service
            .execute(
                operation,
                &SourceToolCall {
                    message_id: "assistant-message".into(),
                    tool_use_id: "call".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(replay["id"], saved.id.as_str());
        assert_eq!(replay["scheduled_at"], "1970-01-01T00:00:00.000020+00:00");
        let conflict = ReminderOperation::Create {
            text: ReminderText::parse("Different").unwrap(),
            when: ReminderWhen::After { seconds: 10 },
        };
        assert!(service
            .execute(
                conflict,
                &SourceToolCall {
                    message_id: "assistant-message".into(),
                    tool_use_id: "call".into()
                }
            )
            .await
            .unwrap_err()
            .contains("different"));
    }
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    use phoenix_core::domain::reminder::{
        ReminderId, ReminderRevision, ReminderText, ReminderTimezone,
    };
    use phoenix_db::ReminderChange;

    #[test]
    fn untyped_management_failure_is_unclassified() {
        assert!(matches!(
            classify_management_result(Err(phoenix_db::DbError::Serialization(
                "incomplete stored shape".into()
            ))),
            LocalAuthorityResult::DurableFactUnclassified
        ));
    }

    #[tokio::test]
    async fn reminder_abandoned_mutation_commit_closes_local_authority() {
        for operation in [
            AbandonedWrite::Create,
            AbandonedWrite::Reschedule,
            AbandonedWrite::Cancel,
            AbandonedWrite::Promote,
        ] {
            abandoned_commit_closes_authority(operation).await;
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum AbandonedWrite {
        Create,
        Reschedule,
        Cancel,
        Promote,
    }

    async fn abandoned_commit_closes_authority(operation: AbandonedWrite) {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let original = saved(&db).await;
        let runtime = manager(&db);
        let (commit_entered, entered) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        {
            let mut connection = db.pool().acquire().await.unwrap();
            let mut handle = connection.lock_handle().await.unwrap();
            let mut commit_entered = Some(commit_entered);
            handle.set_commit_hook(move || {
                if let Some(entered) = commit_entered.take() {
                    let _ = entered.send(());
                    resume.recv().unwrap();
                }
                true
            });
        }
        let worker_runtime = runtime.clone();
        let original_id = original.id.clone();
        let task = tokio::spawn(async move {
            match operation {
                AbandonedWrite::Create => {
                    HostReminderService {
                        runtime: Arc::downgrade(&worker_runtime),
                        transcript_id: "mutation-source".into(),
                    }
                    .create(
                        &worker_runtime,
                        &SourceToolCall {
                            message_id: "new-assistant".into(),
                            tool_use_id: "create".into(),
                        },
                        ReminderText::parse("new reminder").unwrap(),
                        phoenix_tools::reminders::ReminderWhen::After { seconds: 10 },
                        chrono::DateTime::from_timestamp_micros(10).unwrap(),
                    )
                    .await
                    .unwrap();
                }
                AbandonedWrite::Reschedule => {
                    reschedule(
                        &worker_runtime,
                        &RescheduleReminderInput {
                            source_transcript_id: "mutation-source".into(),
                            reminder_id: original_id,
                            expected_revision: original.revision,
                            due_at_us: time(40),
                            timezone: original.timezone,
                            now_us: time(11),
                        },
                    )
                    .await
                    .unwrap();
                }
                AbandonedWrite::Cancel => {
                    cancel(
                        &worker_runtime,
                        "mutation-source",
                        &original_id,
                        original.revision,
                        time(11),
                    )
                    .await
                    .unwrap();
                }
                AbandonedWrite::Promote => {
                    crate::reminder_delivery::deliver_pass(&worker_runtime, time(30)).await;
                }
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let fenced = runtime.acquire_local_authority_pass().is_err();
        release.send(()).unwrap();
        let rows = db.list_reminders("mutation-source", 100).await.unwrap();
        match operation {
            AbandonedWrite::Create => assert_eq!(rows.len(), 2),
            AbandonedWrite::Reschedule => assert_eq!(rows[0].due_at_us, time(40)),
            AbandonedWrite::Cancel => assert!(matches!(
                rows[0].state,
                phoenix_core::domain::reminder::ReminderState::Cancelled { .. }
            )),
            AbandonedWrite::Promote => assert!(matches!(
                rows[0].state,
                phoenix_core::domain::reminder::ReminderState::Due { .. }
            )),
        }
        assert!(
            fenced,
            "{operation:?} lost its typed SQLite command result without fencing admission"
        );
    }

    fn time(value: i64) -> ReminderTimestampUs {
        ReminderTimestampUs::parse(value).unwrap()
    }
    fn manager(db: &phoenix_db::Database) -> Arc<RuntimeManager> {
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
    async fn saved(db: &phoenix_db::Database) -> Reminder {
        db.create_conversation("mutation-source", "mutation", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_reminder(&CreateReminderInput {
            source_transcript_id: "mutation-source".into(),
            source_message_id: "assistant".into(),
            source_tool_use_id: "create".into(),
            request_fingerprint: "a".repeat(64),
            text: ReminderText::parse("check").unwrap(),
            due_at_us: time(20),
            timezone: ReminderTimezone::parse("UTC").unwrap(),
            now_us: time(10),
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reminder_create_preflight_failures_do_not_close_local_authority() {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        saved(&db).await;
        let runtime = manager(&db);
        let service = HostReminderService {
            runtime: Arc::downgrade(&runtime),
            transcript_id: "mutation-source".into(),
        };
        let source_call = SourceToolCall {
            message_id: "new-assistant".into(),
            tool_use_id: "create".into(),
        };
        assert!(service
            .create(
                &runtime,
                &source_call,
                ReminderText::parse("check").unwrap(),
                phoenix_tools::reminders::ReminderWhen::After { seconds: 0 },
                Utc::now()
            )
            .await
            .is_err());
        assert!(runtime.acquire_local_authority_pass().is_ok());
        db.pool().close().await;
        assert!(service
            .create(
                &runtime,
                &source_call,
                ReminderText::parse("check").unwrap(),
                phoenix_tools::reminders::ReminderWhen::After { seconds: 10 },
                Utc::now()
            )
            .await
            .is_err());
        assert!(runtime.acquire_local_authority_pass().is_ok());
    }
    #[tokio::test]
    async fn reminder_completed_mutations_keep_local_authority_open() {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let original = saved(&db).await;
        let runtime = manager(&db);
        let input = RescheduleReminderInput {
            source_transcript_id: "mutation-source".into(),
            reminder_id: original.id.clone(),
            expected_revision: original.revision,
            due_at_us: time(40),
            timezone: original.timezone,
            now_us: time(11),
        };
        let ReminderChange::Updated { reminder } = reschedule(&runtime, &input).await.unwrap()
        else {
            panic!("reschedule must commit")
        };
        assert!(runtime.acquire_local_authority_pass().is_ok());
        assert!(matches!(
            reschedule(&runtime, &input).await.unwrap(),
            ReminderChange::RevisionConflict { .. }
        ));
        assert!(runtime.acquire_local_authority_pass().is_ok());
        let ReminderChange::Updated {
            reminder: cancelled,
        } = cancel(
            &runtime,
            "mutation-source",
            &reminder.id,
            reminder.revision,
            time(12),
        )
        .await
        .unwrap()
        else {
            panic!("cancel must commit")
        };
        assert!(runtime.acquire_local_authority_pass().is_ok());
        assert!(matches!(
            cancel(&runtime, "mutation-source", &reminder.id, reminder.revision, time(12)).await.unwrap(),
            ReminderChange::AlreadyCancelled { current } if current == cancelled
        ));
        assert!(runtime.acquire_local_authority_pass().is_ok());
    }

    #[tokio::test]
    async fn cancel_without_a_classified_result_fences_even_when_rows_remain_readable() {
        unclassified_management_write_fences(false).await;
    }

    #[tokio::test]
    async fn reschedule_without_a_classified_result_fences_even_when_rows_remain_readable() {
        unclassified_management_write_fences(true).await;
    }

    async fn unclassified_management_write_fences(rescheduling: bool) {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let original = saved(&db).await;
        let runtime = manager(&db);
        {
            let mut connection = db.pool().acquire().await.unwrap();
            let mut handle = connection.lock_handle().await.unwrap();
            let mut allow_commit = false;
            handle.set_commit_hook(move || std::mem::replace(&mut allow_commit, true));
        }
        let result = if rescheduling {
            reschedule(
                &runtime,
                &RescheduleReminderInput {
                    source_transcript_id: "mutation-source".into(),
                    reminder_id: original.id.clone(),
                    expected_revision: original.revision,
                    due_at_us: time(40),
                    timezone: original.timezone.clone(),
                    now_us: time(11),
                },
            )
            .await
        } else {
            cancel(
                &runtime,
                "mutation-source",
                &original.id,
                original.revision,
                time(11),
            )
            .await
        };
        assert!(result.is_err());
        assert!(runtime.acquire_local_authority_pass().is_err());
        assert_eq!(
            db.get_reminder("mutation-source", &original.id)
                .await
                .unwrap(),
            Some(original)
        );
    }

    #[tokio::test]
    async fn reminder_management_closes_authority_when_the_database_is_unavailable() {
        let db = phoenix_db::Database::open_in_memory().await.unwrap();
        let original = saved(&db).await;
        let cancel_runtime = manager(&db);
        let reschedule_runtime = manager(&db);
        db.pool().close().await;
        assert!(cancel(
            &cancel_runtime,
            "mutation-source",
            &original.id,
            original.revision,
            time(11)
        )
        .await
        .is_err());
        assert!(cancel_runtime.acquire_local_authority_pass().is_err());
        let input = RescheduleReminderInput {
            source_transcript_id: "mutation-source".into(),
            reminder_id: ReminderId::parse(original.id.as_str()).unwrap(),
            expected_revision: ReminderRevision::INITIAL,
            due_at_us: time(40),
            timezone: original.timezone,
            now_us: time(11),
        };
        assert!(reschedule(&reschedule_runtime, &input).await.is_err());
        assert!(reschedule_runtime.acquire_local_authority_pass().is_err());
    }
}
