use crate::{Database, DbError, DbResult};
use phoenix_core::domain::{
    close::TranscriptConversationId,
    federation_subscription::{
        FederatedTerminalFact, SubscriptionAuthorization, SubscriptionCurrentState,
        SubscriptionCursor, SubscriptionDelivery, SubscriptionEnd, SubscriptionGap,
        SubscriptionReference, SubscriptionReplay, SubscriptionSnapshot, SubscriptionState,
        TerminalFactOutcome, MAX_UNACKNOWLEDGED, REPLAY_BATCH, RETENTION_US,
    },
    instance_identity::InstanceId,
    product_conversation::ProductConversationId,
};
use serde::Serialize;
use sqlx::{Row, Sqlite, Transaction};
use uuid::Uuid;

fn invalid(message: &str) -> DbError {
    DbError::Serialization(message.to_owned())
}
fn cursor(value: i64) -> DbResult<SubscriptionCursor> {
    SubscriptionCursor::try_from(value).map_err(|error| invalid(&error.to_string()))
}
fn uuid(value: &str) -> DbResult<Uuid> {
    Uuid::parse_str(value).map_err(|error| invalid(&error.to_string()))
}
fn end(value: &str) -> DbResult<SubscriptionEnd> {
    match value {
        "unsubscribed" => Ok(SubscriptionEnd::Unsubscribed),
        "revoked" => Ok(SubscriptionEnd::Revoked),
        "source_closed" => Ok(SubscriptionEnd::SourceClosed),
        _ => Err(invalid("invalid subscription end")),
    }
}

pub(crate) struct ClassifiedTerminal<'a> {
    pub occurrence_kind: &'a str,
    pub occurrence_id: &'a str,
    pub generation: i64,
    pub transcript_id: &'a str,
    pub outcome: TerminalFactOutcome,
    pub context_exhausted: bool,
    pub occurred_at_us: i64,
}

pub(crate) async fn append_terminal_tx(
    tx: &mut Transaction<'_, Sqlite>,
    fact: ClassifiedTerminal<'_>,
) -> DbResult<()> {
    let row = sqlx::query("SELECT c.product_conversation_id, c.state FROM conversations c JOIN product_conversations p ON p.id = c.product_conversation_id WHERE c.id = ?1 AND c.parent_conversation_id IS NULL AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open'")
        .bind(fact.transcript_id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(());
    };
    let projection =
        serde_json::from_str::<phoenix_core::domain::sm_state::ConvState>(row.try_get("state")?)
            .map_err(|error| invalid(&error.to_string()))?;
    if SubscriptionCurrentState::from_projection(&projection, Some(fact.outcome))
        == SubscriptionCurrentState::Nonterminal
    {
        return Ok(());
    }
    let product: String = row.try_get("product_conversation_id")?;
    let first = sqlx::query("INSERT INTO federation_terminal_occurrences(occurrence_kind,occurrence_id,generation,product_id) VALUES (?1,?2,?3,?4) ON CONFLICT DO NOTHING")
        .bind(fact.occurrence_kind).bind(fact.occurrence_id).bind(fact.generation).bind(&product).execute(&mut **tx).await?.rows_affected();
    if first == 0 {
        return Ok(());
    }
    sqlx::query("INSERT INTO federation_terminal_current(product_id, source_transcript_id, outcome) VALUES (?1, ?2, ?3) ON CONFLICT(product_id) DO UPDATE SET source_transcript_id = excluded.source_transcript_id, outcome = excluded.outcome")
        .bind(&product).bind(fact.transcript_id).bind(fact.outcome.as_str()).execute(&mut **tx).await?;
    let awaiting: bool = fact.context_exhausted && fact.outcome == TerminalFactOutcome::Failed && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM automatic_continuation_admissions WHERE predecessor_conversation_id = ?1 AND phase NOT IN ('failed','superseded'))")
        .bind(fact.transcript_id).fetch_one(&mut **tx).await?;
    let subscriptions: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM federation_subscriptions WHERE product_id = ?1 AND state = 'active'",
    )
    .bind(product)
    .fetch_all(&mut **tx)
    .await?;
    for subscription in subscriptions {
        let event = Uuid::new_v4().to_string();
        let inserted = sqlx::query("INSERT INTO federation_terminal_identities(event_id, subscription_id, occurrence_kind, occurrence_id, generation, continuation_state) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(subscription_id, occurrence_kind, occurrence_id, generation) DO NOTHING")
            .bind(&event).bind(&subscription).bind(fact.occurrence_kind).bind(fact.occurrence_id).bind(fact.generation).bind(if awaiting { "awaiting" } else { "none" }).execute(&mut **tx).await?.rows_affected();
        if inserted == 1 {
            sqlx::query("INSERT INTO federation_terminal_payloads(event_id,source_transcript_id,outcome,context_exhausted,occurred_at_us) VALUES (?1,?2,?3,?4,?5)")
                .bind(event).bind(fact.transcript_id).bind(fact.outcome.as_str()).bind(fact.context_exhausted).bind(fact.occurred_at_us).execute(&mut **tx).await?;
        }
        retain_tx(tx, &subscription, fact.occurred_at_us).await?;
    }
    Ok(())
}

async fn assign_sequence_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: &str,
    event: &str,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE federation_subscriptions SET last_sequence = last_sequence + 1 WHERE id = ?1",
    )
    .bind(subscription)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE federation_terminal_identities SET sequence = (SELECT last_sequence FROM federation_subscriptions WHERE id = ?1) WHERE event_id = ?2 AND sequence IS NULL")
        .bind(subscription).bind(event).execute(&mut **tx).await?;
    Ok(())
}

async fn publish_tx(tx: &mut Transaction<'_, Sqlite>, subscription: &str) -> DbResult<()> {
    let ready: Vec<String> = sqlx::query_scalar("SELECT i.event_id FROM federation_terminal_identities i JOIN federation_terminal_payloads p ON p.event_id = i.event_id WHERE i.subscription_id = ?1 AND i.sequence IS NULL AND i.continuation_state = 'none' ORDER BY p.occurred_at_us, i.rowid")
        .bind(subscription).fetch_all(&mut **tx).await?;
    for event in ready {
        assign_sequence_tx(tx, subscription, &event).await?;
    }
    Ok(())
}

async fn retain_tx(tx: &mut Transaction<'_, Sqlite>, subscription: &str, now: i64) -> DbResult<()> {
    let rows = sqlx::query("SELECT i.event_id, i.sequence, p.occurred_at_us FROM federation_terminal_identities i JOIN federation_terminal_payloads p ON p.event_id = i.event_id WHERE i.subscription_id = ?1 AND NOT EXISTS (SELECT 1 FROM federation_delivery_authorizations a WHERE a.event_id = i.event_id) ORDER BY p.occurred_at_us DESC, i.rowid DESC")
        .bind(subscription).fetch_all(&mut **tx).await?;
    for (index, row) in rows.into_iter().enumerate() {
        let time: i64 = row.try_get("occurred_at_us")?;
        if index < MAX_UNACKNOWLEDGED && time > now.saturating_sub(RETENTION_US) {
            continue;
        }
        let event: String = row.try_get("event_id")?;
        let sequence: Option<i64> = row.try_get("sequence")?;
        if sequence.is_none() {
            assign_sequence_tx(tx, subscription, &event).await?;
        }
        sqlx::query("UPDATE federation_subscriptions SET pruned_through = MAX(pruned_through, (SELECT sequence FROM federation_terminal_identities WHERE event_id = ?2)) WHERE id = ?1")
            .bind(subscription).bind(&event).execute(&mut **tx).await?;
        sqlx::query("DELETE FROM federation_terminal_payloads WHERE event_id = ?1")
            .bind(event)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn snapshot_tx(
    tx: &mut Transaction<'_, Sqlite>,
    product: &ProductConversationId,
) -> DbResult<SubscriptionSnapshot> {
    let row = sqlx::query("SELECT c.id, c.state, f.outcome, (EXISTS (SELECT 1 FROM durable_turns t WHERE t.conversation_id = c.id AND t.terminal_kind IS NULL) OR EXISTS (SELECT 1 FROM steering_execution_occurrences e WHERE e.conversation_id = c.id) OR EXISTS (SELECT 1 FROM steering_messages m WHERE m.conversation_id = c.id) OR EXISTS (SELECT 1 FROM automatic_continuation_admissions a WHERE a.predecessor_conversation_id = c.id AND a.phase NOT IN ('failed','superseded'))) AS pending FROM product_conversations p JOIN conversations c ON c.product_conversation_id = p.id LEFT JOIN federation_terminal_current f ON f.product_id = p.id AND f.source_transcript_id = c.id WHERE p.id = ?1 AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open' AND c.parent_conversation_id IS NULL AND c.continued_in_conv_id IS NULL")
        .bind(product.as_str()).fetch_optional(&mut **tx).await?.ok_or_else(|| DbError::ProductConversationUnavailable(product.clone()))?;
    let outcome: Option<String> = row.try_get("outcome")?;
    Ok(SubscriptionSnapshot {
        transcript_id: TranscriptConversationId::parse(row.try_get::<String, _>("id")?)
            .map_err(|error| invalid(&error.to_string()))?,
        state: if row.try_get::<bool, _>("pending")? {
            SubscriptionCurrentState::Nonterminal
        } else {
            let projection = serde_json::from_str::<phoenix_core::domain::sm_state::ConvState>(
                row.try_get("state")?,
            )
            .map_err(|error| invalid(&error.to_string()))?;
            SubscriptionCurrentState::from_projection(
                &projection,
                outcome
                    .as_deref()
                    .map(TerminalFactOutcome::try_from)
                    .transpose()
                    .map_err(|error| invalid(&error.to_string()))?,
            )
        },
    })
}

impl Database {
    /// Enroll a future-only subscription or recover the exact active create receipt.
    ///
    /// # Errors
    /// Returns an error for an inactive enrollment, changed binding, closed source, or failed transaction.
    pub async fn create_federation_subscription(
        &self,
        enrollment_id: Uuid,
        caller: InstanceId,
        subscription: Uuid,
        product: &ProductConversationId,
        now: i64,
    ) -> DbResult<SubscriptionSnapshot> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_enrollment_tx(&mut tx, enrollment_id, caller).await?;
        let existing = sqlx::query(
            "SELECT enrollment_id, product_id FROM federation_subscriptions WHERE id = ?1",
        )
        .bind(subscription.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = existing {
            if row.try_get::<String, _>("enrollment_id")? != enrollment_id.to_string()
                || row.try_get::<String, _>("product_id")? != product.as_str()
            {
                return Err(invalid("subscription binding mismatch"));
            }
            let state: String =
                sqlx::query_scalar("SELECT state FROM federation_subscriptions WHERE id = ?1")
                    .bind(subscription.to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if state != "active" {
                return Err(invalid("subscription ended"));
            }
        } else {
            sqlx::query("INSERT INTO federation_subscriptions(id,enrollment_id,product_id,created_at_us) VALUES (?1,?2,?3,?4)").bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).bind(now).execute(&mut *tx).await?;
        }
        let snapshot = snapshot_tx(&mut tx, product).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// Acknowledge the caller's admitted cursor and replay a bounded retained batch.
    ///
    /// # Errors
    /// Returns an error for a changed binding, invalid cursor, inactive enrollment, or failed transaction.
    pub async fn replay_federation_subscription(
        &self,
        enrollment_id: Uuid,
        caller: InstanceId,
        subscription: Uuid,
        product: &ProductConversationId,
        after: SubscriptionCursor,
        now: i64,
    ) -> DbResult<SubscriptionReplay> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_enrollment_tx(&mut tx, enrollment_id, caller).await?;
        let row = sqlx::query("SELECT state, last_sequence FROM federation_subscriptions WHERE id = ?1 AND enrollment_id = ?2 AND product_id = ?3")
            .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).fetch_optional(&mut *tx).await?.ok_or_else(|| invalid("subscription unavailable"))?;
        let state: String = row.try_get("state")?;
        if after.get() > row.try_get::<i64, _>("last_sequence")? {
            return Err(invalid("cursor exceeds published history"));
        }
        let id = subscription.to_string();
        sqlx::query("UPDATE federation_subscriptions SET acknowledged_through = MAX(acknowledged_through, ?2) WHERE id = ?1").bind(&id).bind(after.get()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM federation_delivery_authorizations WHERE subscription_id = ?1 AND through_cursor <= ?2").bind(&id).bind(after.get()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1 AND sequence <= ?2)").bind(&id).bind(after.get()).execute(&mut *tx).await?;
        if state != "active" {
            tx.commit().await?;
            return Ok(SubscriptionReplay::Ended {
                reason: end(&state)?,
            });
        }
        publish_tx(&mut tx, &id).await?;
        retain_tx(&mut tx, &id, now).await?;
        let pruned: i64 = sqlx::query_scalar("SELECT MAX(pruned_through,acknowledged_through) FROM federation_subscriptions WHERE id = ?1").bind(&id).fetch_one(&mut *tx).await?;
        let replay = if after.get() < pruned {
            SubscriptionReplay::Gap {
                gap: SubscriptionGap {
                    requested_after: after,
                    resume_after: cursor(pruned)?,
                    snapshot: snapshot_tx(&mut tx, product).await?,
                },
            }
        } else {
            let rows = sqlx::query("SELECT i.event_id, i.sequence, p.source_transcript_id,p.outcome,p.occurred_at_us FROM federation_terminal_identities i JOIN federation_terminal_payloads p ON p.event_id = i.event_id WHERE i.subscription_id = ?1 AND i.sequence > ?2 AND i.continuation_state = 'none' AND i.sequence < COALESCE((SELECT MIN(blocked.sequence) FROM federation_terminal_identities blocked JOIN federation_terminal_payloads blocked_payload USING(event_id) WHERE blocked.subscription_id = i.subscription_id AND blocked.continuation_state = 'awaiting'), 9223372036854775807) AND NOT EXISTS (SELECT 1 FROM federation_subscriptions s JOIN close_obligations o ON o.product_conversation_id = s.product_id WHERE s.id = i.subscription_id AND o.phase != 'completed') ORDER BY i.sequence LIMIT ?3")
                .bind(&id).bind(after.get()).bind(i64::try_from(REPLAY_BATCH).expect("replay batch fits in SQLite integer")).fetch_all(&mut *tx).await?;
            let events = rows
                .iter()
                .map(|row| {
                    Ok(FederatedTerminalFact {
                        event_id: uuid(row.try_get("event_id")?)?,
                        sequence: cursor(row.try_get("sequence")?)?,
                        source_transcript_id: TranscriptConversationId::parse(
                            row.try_get::<String, _>("source_transcript_id")?,
                        )
                        .map_err(|error| invalid(&error.to_string()))?,
                        outcome: TerminalFactOutcome::try_from(row.try_get::<&str, _>("outcome")?)
                            .map_err(|error| invalid(&error.to_string()))?,
                        occurred_at_us: row.try_get("occurred_at_us")?,
                    })
                })
                .collect::<DbResult<Vec<_>>>()?;
            SubscriptionReplay::Facts { events }
        };
        tx.commit().await?;
        Ok(replay)
    }

    /// End the exact subscription or fence its absent Create without retracting receipts.
    ///
    /// # Errors
    /// Returns an error for a mismatched caller/binding or failed transaction.
    pub async fn unsubscribe_federation_subscription(
        &self,
        enrollment_id: Uuid,
        caller: InstanceId,
        subscription: Uuid,
        product: &ProductConversationId,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_enrollment_tx(&mut tx, enrollment_id, caller).await?;
        let existing = sqlx::query(
            "SELECT enrollment_id, product_id FROM federation_subscriptions WHERE id = ?1",
        )
        .bind(subscription.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            if existing.try_get::<String, _>("enrollment_id")? != enrollment_id.to_string()
                || existing.try_get::<String, _>("product_id")? != product.as_str()
            {
                return Err(invalid("subscription binding mismatch"));
            }
            sqlx::query("UPDATE federation_subscriptions SET state = 'unsubscribed' WHERE id = ?1 AND state = 'active'").bind(subscription.to_string()).execute(&mut *tx).await?;
        } else {
            sqlx::query("INSERT INTO federation_subscription_create_fences(id,enrollment_id,product_id) VALUES (?1,?2,?3) ON CONFLICT(id) DO NOTHING")
                .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).execute(&mut *tx).await?;
            let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_subscription_create_fences WHERE id = ?1 AND enrollment_id = ?2 AND product_id = ?3)")
                .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).fetch_one(&mut *tx).await?;
            if !matches {
                return Err(invalid("subscription create fence binding mismatch"));
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn require_enrollment_tx(
    tx: &mut Transaction<'_, Sqlite>,
    enrollment_id: Uuid,
    caller: InstanceId,
) -> DbResult<()> {
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_enrollments WHERE id = ?1 AND caller_instance_id = ?2 AND revoked_at_us IS NULL)").bind(enrollment_id.to_string()).bind(caller.to_string()).fetch_one(&mut **tx).await?;
    if !active {
        return Err(invalid("enrollment no longer active"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteSubscription {
    pub reference: SubscriptionReference,
    pub subscription_id: Uuid,
    pub state: SubscriptionState,
    pub admitted_cursor: SubscriptionCursor,
    pub enrolled: bool,
    pub pending_unsubscribe: bool,
}
fn decode_remote(row: &sqlx::sqlite::SqliteRow) -> DbResult<RemoteSubscription> {
    Ok(RemoteSubscription {
        reference: SubscriptionReference {
            instance_id: row
                .try_get::<String, _>("destination_instance_id")?
                .parse::<InstanceId>()
                .map_err(|error| invalid(&error.to_string()))?,
            product_conversation_id: ProductConversationId::parse(
                row.try_get::<String, _>("product_id")?,
            )
            .map_err(|error| invalid(&error.to_string()))?,
        },
        subscription_id: uuid(row.try_get("subscription_id")?)?,
        state: SubscriptionState::try_from(row.try_get::<&str, _>("state")?)
            .map_err(|error| invalid(&error.to_string()))?,
        admitted_cursor: cursor(row.try_get("admitted_cursor")?)?,
        enrolled: row.try_get("enrolled")?,
        pending_unsubscribe: row.try_get("pending_unsubscribe")?,
    })
}
impl Database {
    /// Save a create retry identity before contacting the source.
    ///
    /// # Errors
    /// Returns an error for changed/ended bindings, pending unsubscribe, or failed persistence.
    pub async fn save_remote_subscription(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT INTO federation_remote_subscriptions(destination_instance_id,subscription_id,product_id) VALUES (?1,?2,?3) ON CONFLICT(destination_instance_id,subscription_id) DO NOTHING").bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).execute(&mut *tx).await?;
        let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3 AND state = 'active')")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).fetch_one(&mut *tx).await?;
        if !matches {
            return Err(invalid("remote subscription binding changed or ended"));
        }
        tx.commit().await?;
        Ok(())
    }
    /// Record successful source enrollment for an active retry identity.
    ///
    /// # Errors
    /// Returns an error if the write fails.
    pub async fn mark_remote_subscription_enrolled(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
    ) -> DbResult<()> {
        sqlx::query("UPDATE federation_remote_subscriptions SET enrolled = 1 WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3 AND state = 'active'").bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).execute(&self.pool).await?;
        Ok(())
    }
    /// List saved source subscriptions, optionally restricted to an instance.
    ///
    /// # Errors
    /// Returns an error if the query fails or persisted bindings cannot be decoded.
    pub async fn remote_subscriptions(
        &self,
        destination: Option<InstanceId>,
        limit: i64,
    ) -> DbResult<Vec<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE (?1 IS NULL OR destination_instance_id = ?1) ORDER BY destination_instance_id,subscription_id LIMIT ?2").bind(destination.map(|id| id.to_string())).bind(limit.clamp(1,1024)).fetch_all(&self.pool).await?.iter().map(decode_remote).collect()
    }
    /// Stop unrequested delivery without discarding in-flight authorization identities.
    ///
    /// # Errors
    /// Returns an error if the write fails.
    pub async fn end_remote_subscription(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
        reason: SubscriptionEnd,
    ) -> DbResult<()> {
        let state = match reason {
            SubscriptionEnd::Unsubscribed => "unsubscribed",
            SubscriptionEnd::Revoked => "revoked",
            SubscriptionEnd::SourceClosed => "source_closed",
        };
        sqlx::query("UPDATE federation_remote_subscriptions SET state = ?4 WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3 AND state = 'active'").bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).bind(state).execute(&self.pool).await?;
        Ok(())
    }
}

mod authorization;
mod consumer;
mod presentation;
pub use consumer::{RemoteNotification, RemoteNotificationContent, RemoteNotificationDisposition};
pub use presentation::RemoteNotificationPresentations;
#[cfg(test)]
mod tests;

impl Database {
    /// Look up one exact instance-qualified subscription retry identity.
    ///
    /// # Errors
    /// Returns an error if the query fails or the binding cannot be decoded.
    pub async fn remote_subscription_by_id(
        &self,
        destination: InstanceId,
        subscription: Uuid,
    ) -> DbResult<Option<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND subscription_id = ?2").bind(destination.to_string()).bind(subscription.to_string()).fetch_optional(&self.pool).await?.as_ref().map(decode_remote).transpose()
    }
    /// Find the active saved subscription for an instance-qualified product.
    ///
    /// # Errors
    /// Returns an error if the query fails or the binding cannot be decoded.
    pub async fn remote_subscription_for_product(
        &self,
        reference: &SubscriptionReference,
    ) -> DbResult<Option<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND product_id = ?2 AND state = 'active'").bind(reference.instance_id.to_string()).bind(reference.product_conversation_id.as_str()).fetch_optional(&self.pool).await?.as_ref().map(decode_remote).transpose()
    }
    /// Select at most four subscriptions with active, unsubscribe, or authorization work.
    ///
    /// # Errors
    /// Returns an error if the query fails or a binding cannot be decoded.
    pub async fn next_remote_subscription_sync_batch(
        &self,
        limit: i64,
    ) -> DbResult<Vec<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE state = 'active' OR EXISTS (SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) OR EXISTS (SELECT 1 FROM federation_remote_admissions a WHERE a.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND a.subscription_id = federation_remote_subscriptions.subscription_id AND a.accepted_transcript_id IS NULL AND a.suppressed = 0 AND a.authorization_requested = 1) ORDER BY last_sync_at_us, destination_instance_id,subscription_id LIMIT ?1").bind(limit.clamp(1,4)).fetch_all(&self.pool).await?.iter().map(decode_remote).collect()
    }
    /// Rotate the bounded synchronization queue after an attempted visit.
    ///
    /// # Errors
    /// Returns an error if the timestamp write fails.
    pub async fn touch_remote_subscription(&self, remote: &RemoteSubscription) -> DbResult<()> {
        sqlx::query("UPDATE federation_remote_subscriptions SET last_sync_at_us = ?3 WHERE destination_instance_id = ?1 AND subscription_id = ?2").bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(chrono::Utc::now().timestamp_micros()).execute(&self.pool).await?;
        Ok(())
    }
}

pub(crate) async fn suppress_summary_failure_tx(
    tx: &mut Transaction<'_, Sqlite>,
    transcript_id: &str,
) -> DbResult<()> {
    sqlx::query("UPDATE federation_subscriptions SET pruned_through = MAX(pruned_through, COALESCE((SELECT MAX(i.sequence) FROM federation_terminal_identities i JOIN federation_terminal_payloads p USING(event_id) WHERE i.subscription_id = federation_subscriptions.id AND i.occurrence_kind = 'continuation_summary' AND p.source_transcript_id = ?1),0))").bind(transcript_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE federation_terminal_identities SET continuation_state = 'suppressed' WHERE occurrence_kind = 'continuation_summary' AND NOT EXISTS (SELECT 1 FROM federation_delivery_authorizations a WHERE a.event_id = federation_terminal_identities.event_id) AND event_id IN (SELECT event_id FROM federation_terminal_payloads WHERE source_transcript_id = ?1)").bind(transcript_id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE continuation_state = 'suppressed')").execute(&mut **tx).await?;
    Ok(())
}
