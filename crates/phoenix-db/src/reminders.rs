//! REQ-REM-001, REQ-REM-002, REQ-REM-004, REQ-REM-005, REQ-REM-006,
//! REQ-REM-007, REQ-REM-009: reminder intent, eligibility and owner-scoped reads.

use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::reminder::{
    Reminder, ReminderId, ReminderRevision, ReminderState, ReminderText, ReminderTimestampUs,
    ReminderTimezone,
};
use serde::Serialize;
use sqlx::{Row, Sqlite, Transaction};

use crate::{Database, DbError, DbResult};

pub struct CreateReminderInput {
    pub source_transcript_id: String,
    pub source_message_id: String,
    pub source_tool_use_id: String,
    pub request_fingerprint: String,
    pub text: ReminderText,
    pub due_at_us: ReminderTimestampUs,
    pub timezone: ReminderTimezone,
    pub now_us: ReminderTimestampUs,
}

pub struct RescheduleReminderInput {
    pub source_transcript_id: String,
    pub reminder_id: ReminderId,
    pub expected_revision: ReminderRevision,
    pub due_at_us: ReminderTimestampUs,
    pub timezone: ReminderTimezone,
    pub now_us: ReminderTimestampUs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReminderChange {
    Updated { reminder: Reminder },
    NotFound,
    RevisionConflict { current: Reminder },
    AlreadyDue { current: Reminder },
    AlreadyCancelled { current: Reminder },
}

#[derive(Debug, Clone)]
pub struct DueReminder {
    pub reminder: Reminder,
    pub transcript_id: String,
}

#[derive(Debug, Clone)]
pub struct DueReminderCursor {
    pub due_at_us: ReminderTimestampUs,
    pub reminder_id: ReminderId,
}

fn invalid(error: impl std::fmt::Display) -> DbError {
    DbError::ReminderInvalid(error.to_string())
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> DbResult<Reminder> {
    let timestamp = |field| ReminderTimestampUs::parse(row.try_get(field)?).map_err(invalid);
    let state: String = row.try_get("state")?;
    let state = match state.as_str() {
        "scheduled" => ReminderState::Scheduled,
        "due" => ReminderState::Due {
            occurred_at_us: timestamp("occurred_at_us")?,
        },
        "cancelled" => ReminderState::Cancelled {
            cancelled_at_us: timestamp("cancelled_at_us")?,
        },
        _ => return Err(DbError::Serialization("invalid reminder state".into())),
    };
    Ok(Reminder {
        id: ReminderId::parse(row.try_get::<String, _>("id")?).map_err(invalid)?,
        product_conversation_id: ProductConversationId::parse(
            row.try_get::<String, _>("product_conversation_id")?,
        )
        .map_err(invalid)?,
        text: ReminderText::parse(row.try_get::<String, _>("text")?).map_err(invalid)?,
        due_at_us: timestamp("due_at_us")?,
        timezone: ReminderTimezone::parse(row.try_get::<String, _>("timezone")?)
            .map_err(invalid)?,
        revision: ReminderRevision::parse(row.try_get("revision")?).map_err(invalid)?,
        created_at_us: timestamp("created_at_us")?,
        state,
    })
}

async fn owner_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
) -> DbResult<ProductConversationId> {
    let product: Option<String> = sqlx::query_scalar(
        "SELECT c.product_conversation_id FROM conversations c
         JOIN product_conversations p ON p.id = c.product_conversation_id
         WHERE c.id = ?1 AND c.parent_conversation_id IS NULL
           AND ((p.kind = 'ordinary' AND c.runtime_role = 'user')
             OR (p.kind = 'coordinator' AND c.runtime_role = 'coordinator' AND c.coordinator_head = 1))",
    )
    .bind(transcript_id)
    .fetch_optional(&mut **tx)
    .await?;
    ProductConversationId::parse(
        product.ok_or_else(|| invalid("reminders require a top-level conversation"))?,
    )
    .map_err(invalid)
}

async fn owned_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
    id: &ReminderId,
) -> DbResult<Option<Reminder>> {
    let owner = owner_tx(tx, transcript_id).await?;
    sqlx::query("SELECT * FROM reminders WHERE id = ?1 AND product_conversation_id = ?2")
        .bind(id.as_str())
        .bind(owner.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .as_ref()
        .map(decode)
        .transpose()
}

fn changed_state(current: Reminder, expected: ReminderRevision) -> Option<ReminderChange> {
    match current.state {
        ReminderState::Due { .. } => Some(ReminderChange::AlreadyDue { current }),
        ReminderState::Cancelled { .. } => Some(ReminderChange::AlreadyCancelled { current }),
        ReminderState::Scheduled if current.revision != expected => {
            Some(ReminderChange::RevisionConflict { current })
        }
        ReminderState::Scheduled => None,
    }
}

const ELIGIBLE: &str =
    "((p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open') OR p.kind = 'coordinator')";

impl Database {
    /// Read an existing create receipt before resolving a retried relative deadline.
    /// # Errors
    /// Rejects changed request identity or invalid owners, and returns database errors.
    pub async fn reminder_for_create_call(
        &self,
        transcript_id: &str,
        source_message_id: &str,
        tool_use_id: &str,
        request_fingerprint: &str,
    ) -> DbResult<Option<Reminder>> {
        let mut tx = self.pool.begin().await?;
        owner_tx(&mut tx, transcript_id).await?;
        let row = sqlx::query(
            "SELECT * FROM reminders WHERE source_transcript_id = ?1 AND source_message_id = ?2 AND source_tool_use_id = ?3",
        )
        .bind(transcript_id)
        .bind(source_message_id)
        .bind(tool_use_id)
        .fetch_optional(&mut *tx)
        .await?;
        let result = if let Some(row) = row {
            if row.try_get::<String, _>("create_request_fingerprint")? != request_fingerprint {
                return Err(DbError::ReminderRequestConflict);
            }
            Some(decode(&row)?)
        } else {
            None
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Read one reminder within the caller's stable conversation.
    /// # Errors
    /// Returns owner validation or database errors.
    pub async fn get_reminder(
        &self,
        transcript_id: &str,
        id: &ReminderId,
    ) -> DbResult<Option<Reminder>> {
        let mut tx = self.pool.begin().await?;
        let reminder = owned_tx(&mut tx, transcript_id, id).await?;
        tx.commit().await?;
        Ok(reminder)
    }

    /// Persist a reminder or return the exact original operation's reminder.
    /// # Errors
    /// Rejects changed replay input, invalid dates, unavailable owners, or database failures.
    pub async fn create_reminder(&self, input: &CreateReminderInput) -> DbResult<Reminder> {
        if input.source_message_id.trim().is_empty()
            || input.source_tool_use_id.trim().is_empty()
            || input.request_fingerprint.len() != 64
            || !input
                .request_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(
                "create requires a tool identity and SHA-256 request fingerprint",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        owner_tx(&mut tx, &input.source_transcript_id).await?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM reminders WHERE source_transcript_id = ?1 AND source_message_id = ?2 AND source_tool_use_id = ?3",
        )
        .bind(&input.source_transcript_id)
        .bind(&input.source_message_id)
        .bind(&input.source_tool_use_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let fingerprint: String = row.try_get("create_request_fingerprint")?;
            if fingerprint != input.request_fingerprint {
                return Err(DbError::ReminderRequestConflict);
            }
            let existing = decode(&row)?;
            tx.commit().await?;
            return Ok(existing);
        }
        if input.due_at_us <= input.now_us {
            return Err(invalid("choose a reminder time in the future"));
        }
        let owner = crate::close_foundation::require_product_conversation_admission_tx(
            &mut tx,
            &input.source_transcript_id,
        )
        .await?;
        let id = uuid::Uuid::new_v4().to_string();
        let row = sqlx::query(
            "INSERT INTO reminders (id, product_conversation_id, source_transcript_id,
                source_message_id, source_tool_use_id, create_request_fingerprint, text, due_at_us, timezone,
                revision, created_at_us, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, 'scheduled') RETURNING *",
        )
        .bind(id)
        .bind(owner.as_str())
        .bind(&input.source_transcript_id)
        .bind(&input.source_message_id)
        .bind(&input.source_tool_use_id)
        .bind(&input.request_fingerprint)
        .bind(input.text.as_str())
        .bind(input.due_at_us.get())
        .bind(input.timezone.as_str())
        .bind(input.now_us.get())
        .fetch_one(&mut *tx)
        .await?;
        let reminder = decode(&row)?;
        tx.commit().await?;
        Ok(reminder)
    }

    /// Read at most `per_state_limit` reminders independently for each state, owned by the caller.
    /// # Errors
    /// Returns owner validation or database errors.
    pub async fn list_reminders(
        &self,
        transcript_id: &str,
        per_state_limit: u32,
    ) -> DbResult<Vec<Reminder>> {
        let mut tx = self.pool.begin().await?;
        let owner = owner_tx(&mut tx, transcript_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM (
                SELECT * FROM reminders WHERE product_conversation_id = ?1 AND state = 'scheduled'
                ORDER BY due_at_us, id LIMIT ?2
             ) UNION ALL SELECT * FROM (
                SELECT * FROM reminders WHERE product_conversation_id = ?1 AND state = 'due'
                ORDER BY occurred_at_us DESC, id LIMIT ?2
             ) UNION ALL SELECT * FROM (
                SELECT * FROM reminders WHERE product_conversation_id = ?1 AND state = 'cancelled'
                ORDER BY cancelled_at_us DESC, id LIMIT ?2
             )",
        )
        .bind(owner.as_str())
        .bind(i64::from(per_state_limit.clamp(1, 100)))
        .fetch_all(&mut *tx)
        .await?;
        let reminders = rows.iter().map(decode).collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(reminders)
    }

    /// Replace a scheduled deadline under its observed revision.
    /// # Errors
    /// Returns invalid date, lifecycle admission, or database errors.
    pub async fn reschedule_reminder(
        &self,
        input: &RescheduleReminderInput,
    ) -> DbResult<ReminderChange> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(current) =
            owned_tx(&mut tx, &input.source_transcript_id, &input.reminder_id).await?
        else {
            return Ok(ReminderChange::NotFound);
        };
        if let Some(outcome) = changed_state(current, input.expected_revision) {
            return Ok(outcome);
        }
        if input.due_at_us <= input.now_us {
            return Err(invalid("choose a reminder time in the future"));
        }
        crate::close_foundation::require_product_conversation_admission_tx(
            &mut tx,
            &input.source_transcript_id,
        )
        .await?;
        let row = sqlx::query(
            "UPDATE reminders SET due_at_us = ?2, timezone = ?3, revision = revision + 1
             WHERE id = ?1 AND revision < 9223372036854775807 RETURNING *",
        )
        .bind(input.reminder_id.as_str())
        .bind(input.due_at_us.get())
        .bind(input.timezone.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("reminder revision exhausted"))?;
        let reminder = decode(&row)?;
        tx.commit().await?;
        Ok(ReminderChange::Updated { reminder })
    }

    /// Cancel a scheduled reminder without retracting an already due occurrence.
    /// # Errors
    /// Returns caller validation or database errors.
    pub async fn cancel_reminder(
        &self,
        transcript_id: &str,
        id: &ReminderId,
        expected_revision: ReminderRevision,
        now_us: ReminderTimestampUs,
    ) -> DbResult<ReminderChange> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(current) = owned_tx(&mut tx, transcript_id, id).await? else {
            return Ok(ReminderChange::NotFound);
        };
        if let Some(outcome) = changed_state(current, expected_revision) {
            return Ok(outcome);
        }
        let row = sqlx::query(
            "UPDATE reminders SET state = 'cancelled', cancelled_at_us = ?2
             WHERE id = ?1 RETURNING *",
        )
        .bind(id.as_str())
        .bind(now_us.get())
        .fetch_one(&mut *tx)
        .await?;
        let reminder = decode(&row)?;
        tx.commit().await?;
        Ok(ReminderChange::Updated { reminder })
    }

    /// Freeze a bounded batch of elapsed scheduled reminders into visible due facts.
    /// # Errors
    /// Returns database errors. A repeated call does not change an existing occurrence.
    pub async fn promote_due_reminders(
        &self,
        now_us: ReminderTimestampUs,
        limit: u32,
    ) -> DbResult<Vec<Reminder>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows = sqlx::query(
            "UPDATE reminders SET state = 'due', occurred_at_us = ?1
             WHERE id IN (SELECT id FROM reminders WHERE state = 'scheduled' AND due_at_us <= ?1
                 ORDER BY due_at_us, id LIMIT ?2) RETURNING *",
        )
        .bind(now_us.get())
        .bind(i64::from(limit.clamp(1, 100)))
        .fetch_all(&mut *tx)
        .await?;
        let due = rows.iter().map(decode).collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(due)
    }

    /// Discover due inputs not already owned by the ordinary turn/steering journals.
    /// # Errors
    /// Returns database or persisted-shape errors.
    pub async fn pending_due_reminders(
        &self,
        after: Option<&DueReminderCursor>,
        limit: u32,
    ) -> DbResult<Vec<DueReminder>> {
        let sql = format!(
            "SELECT r.*, c.id AS current_transcript_id FROM reminders r
             JOIN product_conversations p ON p.id = r.product_conversation_id
             JOIN conversations c ON c.product_conversation_id = p.id
               AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
               AND ((p.kind = 'ordinary' AND c.runtime_role = 'user')
                 OR (p.kind = 'coordinator' AND c.runtime_role = 'coordinator' AND c.coordinator_head = 1))
             WHERE r.state = 'due' AND {ELIGIBLE}
               AND (?1 IS NULL OR (r.due_at_us, r.id) > (?1, ?2))
               AND NOT EXISTS (SELECT 1 FROM durable_turns t JOIN conversations owner ON owner.id = t.conversation_id
                 WHERE owner.product_conversation_id = r.product_conversation_id
                   AND t.client_turn_key = 'reminder:' || r.id || ':' || r.revision)
               AND NOT EXISTS (SELECT 1 FROM steering_acceptance_receipts receipt JOIN conversations owner ON owner.id = receipt.conversation_id
                 WHERE owner.product_conversation_id = r.product_conversation_id
                   AND receipt.message_id = 'reminder:' || r.id || ':' || r.revision)
             ORDER BY r.due_at_us, r.id LIMIT ?3"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(after.map(|cursor| cursor.due_at_us.get()))
            .bind(after.map(|cursor| cursor.reminder_id.as_str()))
            .bind(i64::from(limit.clamp(1, 100)))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|row| {
                Ok(DueReminder {
                    reminder: decode(row)?,
                    transcript_id: row.try_get("current_transcript_id")?,
                })
            })
            .collect()
    }

    /// Read a bounded recent due feed, including occurrences already accepted by input admission.
    /// # Errors
    /// Returns database or persisted-shape errors.
    pub async fn reminder_notification_feed(
        &self,
        since: ReminderTimestampUs,
        after: Option<&DueReminderCursor>,
        limit: u32,
    ) -> DbResult<Vec<phoenix_core::domain::reminder::ReminderNotification>> {
        use phoenix_core::domain::reminder::{
            ReminderNotification, ReminderNotificationTarget, ReminderNotificationTargetKind,
        };
        let sql = format!(
            "SELECT r.*, c.id AS target_id, c.slug AS target_slug, c.title AS target_title, p.kind AS target_kind
             FROM reminders r JOIN product_conversations p ON p.id = r.product_conversation_id
             JOIN conversations c ON c.product_conversation_id = p.id
               AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL
               AND ((p.kind = 'ordinary' AND c.runtime_role = 'user')
                 OR (p.kind = 'coordinator' AND c.runtime_role = 'coordinator' AND c.coordinator_head = 1))
             WHERE r.state = 'due' AND {ELIGIBLE} AND r.occurred_at_us >= ?1
               AND (?2 IS NULL OR (r.due_at_us, r.id) > (?2, ?3))
             ORDER BY r.due_at_us, r.id LIMIT ?4"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(since.get())
            .bind(after.map(|c| c.due_at_us.get()))
            .bind(after.map(|c| c.reminder_id.as_str()))
            .bind(i64::from(limit.clamp(1, 100)))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|row| {
                Ok(ReminderNotification {
                    reminder: decode(row)?,
                    target: ReminderNotificationTarget {
                        id: row.try_get("target_id")?,
                        slug: row.try_get("target_slug")?,
                        title: row.try_get("target_title")?,
                        kind: match row.try_get::<String, _>("target_kind")?.as_str() {
                            "ordinary" => ReminderNotificationTargetKind::Ordinary,
                            "coordinator" => ReminderNotificationTargetKind::Coordinator,
                            _ => return Err(invalid("invalid reminder target kind")),
                        },
                    },
                })
            })
            .collect()
    }

    /// Read the next eligible scheduled deadline; elapsed deadlines remain discoverable.
    /// # Errors
    /// Returns database or persisted timestamp errors.
    pub async fn next_reminder_deadline(&self) -> DbResult<Option<ReminderTimestampUs>> {
        let sql = format!(
            "SELECT MIN(r.due_at_us) FROM reminders r
             JOIN product_conversations p ON p.id = r.product_conversation_id
             WHERE r.state = 'scheduled' AND {ELIGIBLE}"
        );
        let value: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(&self.pool)
            .await?;
        value
            .map(ReminderTimestampUs::parse)
            .transpose()
            .map_err(invalid)
    }
}

#[cfg(test)]
mod tests;
