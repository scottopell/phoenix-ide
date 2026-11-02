use crate::{
    federation_enrollment::federation_send_enrollment_id_tx, Database, DbError, DbResult,
    FederationSendAuthority,
};
use phoenix_core::domain::{
    close::TranscriptConversationId,
    federation_subscription::{
        FederatedTerminalFact, GapReason, SubscriptionAuthorization, SubscriptionCurrentState,
        SubscriptionCursor, SubscriptionDelivery, SubscriptionEnd, SubscriptionGap,
        SubscriptionReference, SubscriptionReplay, SubscriptionSnapshot, SubscriptionState,
        TerminalFactOccurrence, TerminalFactOccurrenceKind, TerminalFactOutcome,
        MAX_DISCONNECTED_AGE_US, MAX_UNACKNOWLEDGED_EVENTS, MAX_UNACKNOWLEDGED_PAYLOAD_BYTES,
        REPLAY_BATCH, TERMINAL_EVIDENCE_LIFETIME_US,
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
    SubscriptionCursor::try_from(value).map_err(invalid)
}
fn uuid(value: &str) -> DbResult<Uuid> {
    Uuid::parse_str(value).map_err(|error| invalid(&error.to_string()))
}
fn replay_batch_sql() -> DbResult<i64> {
    i64::try_from(REPLAY_BATCH).map_err(|error| invalid(&error.to_string()))
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
            let payload_bytes = fact
                .transcript_id
                .len()
                .checked_add(fact.outcome.as_str().len())
                .and_then(|bytes| bytes.checked_add(96))
                .and_then(|bytes| i64::try_from(bytes).ok())
                .ok_or_else(|| invalid("terminal payload size overflow"))?;
            sqlx::query("INSERT INTO federation_terminal_payloads(event_id,source_transcript_id,outcome,context_exhausted,occurred_at_us,payload_bytes) VALUES (?1,?2,?3,?4,?5,?6)")
                .bind(event).bind(fact.transcript_id).bind(fact.outcome.as_str()).bind(fact.context_exhausted).bind(fact.occurred_at_us).bind(payload_bytes).execute(&mut **tx).await?;
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
    publish_tx(tx, subscription).await?;
    let row = sqlx::query("SELECT product_id, acknowledged_through, last_sequence, last_contact_at_us FROM federation_subscriptions WHERE id = ?1 AND state = 'active'")
        .bind(subscription).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(());
    };
    let acknowledged: i64 = row.try_get("acknowledged_through")?;
    let produced: i64 = row.try_get("last_sequence")?;
    let last_contact: Option<i64> = row.try_get("last_contact_at_us")?;
    let totals = sqlx::query("SELECT COUNT(*) AS event_count, COALESCE(SUM(p.payload_bytes), 0) AS payload_bytes FROM federation_terminal_identities i JOIN federation_terminal_payloads p USING(event_id) WHERE i.subscription_id = ?1 AND i.sequence > ?2 AND i.continuation_state = 'none'")
        .bind(subscription).bind(acknowledged).fetch_one(&mut **tx).await?;
    let event_count: i64 = totals.try_get("event_count")?;
    let payload_bytes: i64 = totals.try_get("payload_bytes")?;
    let reason = if event_count
        > i64::try_from(MAX_UNACKNOWLEDGED_EVENTS).map_err(|error| invalid(&error.to_string()))?
    {
        Some(GapReason::EventCount)
    } else if payload_bytes
        > i64::try_from(MAX_UNACKNOWLEDGED_PAYLOAD_BYTES)
            .map_err(|error| invalid(&error.to_string()))?
    {
        Some(GapReason::PayloadBytes)
    } else if produced > acknowledged
        && last_contact
            .is_some_and(|contact| now.saturating_sub(contact) >= MAX_DISCONNECTED_AGE_US)
    {
        Some(GapReason::DisconnectedAge)
    } else {
        None
    };
    let Some(reason) = reason else {
        return Ok(());
    };
    let terminal_evidence_expires_at_us = now
        .checked_add(TERMINAL_EVIDENCE_LIFETIME_US)
        .ok_or_else(|| invalid("terminal evidence expiry overflow"))?;
    let product = ProductConversationId::parse(row.try_get::<String, _>("product_id")?)
        .map_err(|error| invalid(&error.to_string()))?;
    let snapshot = snapshot_tx(tx, &product).await?;
    let outcome = match snapshot.state {
        SubscriptionCurrentState::Nonterminal => None,
        SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
    };
    sqlx::query("INSERT INTO federation_subscription_gaps(subscription_id,last_acknowledged,last_produced,reason,failed_at_us,terminal_evidence_expires_at_us,snapshot_transcript_id,snapshot_outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
        .bind(subscription).bind(acknowledged).bind(produced).bind(reason.as_str()).bind(now).bind(terminal_evidence_expires_at_us).bind(snapshot.transcript_id.as_str()).bind(outcome).execute(&mut **tx).await?;
    sqlx::query("UPDATE federation_subscriptions SET state = 'failed_gap' WHERE id = ?1 AND state = 'active'")
        .bind(subscription).execute(&mut **tx).await?;
    Ok(())
}

async fn gap_tx(tx: &mut Transaction<'_, Sqlite>, subscription: &str) -> DbResult<SubscriptionGap> {
    let row = sqlx::query("SELECT last_acknowledged,last_produced,reason,failed_at_us,terminal_evidence_expires_at_us,snapshot_transcript_id,snapshot_outcome FROM federation_subscription_gaps WHERE subscription_id = ?1")
        .bind(subscription).fetch_one(&mut **tx).await?;
    let outcome: Option<String> = row.try_get("snapshot_outcome")?;
    Ok(SubscriptionGap {
        last_acknowledged: cursor(row.try_get("last_acknowledged")?)?,
        last_produced: cursor(row.try_get("last_produced")?)?,
        reason: GapReason::try_from(row.try_get::<&str, _>("reason")?).map_err(invalid)?,
        failed_at_us: row.try_get("failed_at_us")?,
        terminal_evidence_expires_at_us: row.try_get("terminal_evidence_expires_at_us")?,
        snapshot: SubscriptionSnapshot {
            transcript_id: TranscriptConversationId::parse(
                row.try_get::<String, _>("snapshot_transcript_id")?,
            )
            .map_err(|error| invalid(&error.to_string()))?,
            state: match outcome {
                Some(outcome) => SubscriptionCurrentState::Terminal {
                    outcome: TerminalFactOutcome::try_from(outcome.as_str()).map_err(invalid)?,
                },
                None => SubscriptionCurrentState::Nonterminal,
            },
        },
    })
}

async fn release_terminal_evidence_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
) -> DbResult<()> {
    sqlx::query(
        "DELETE FROM federation_delivery_authorizations
         WHERE subscription_id = ?1
           AND kind = 'fact'
           AND event_id IN (
               SELECT i.event_id
               FROM federation_terminal_identities i
               JOIN federation_subscription_gaps g ON g.subscription_id = i.subscription_id
               WHERE i.subscription_id = ?1
                 AND i.sequence > g.last_acknowledged
                 AND i.sequence <= g.last_produced
           )",
    )
    .bind(subscription.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE federation_terminal_identities
         SET evidence_release_reason = 'expired'
         WHERE subscription_id = ?1
           AND evidence_release_reason IS NULL
           AND event_id IN (SELECT event_id FROM federation_terminal_payloads)
           AND sequence > (
               SELECT last_acknowledged FROM federation_subscription_gaps
               WHERE subscription_id = ?1
           )
           AND sequence <= (
               SELECT last_produced FROM federation_subscription_gaps
               WHERE subscription_id = ?1
           )",
    )
    .bind(subscription.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM federation_terminal_payloads
         WHERE event_id IN (
             SELECT i.event_id
             FROM federation_terminal_identities i
             JOIN federation_subscription_gaps g ON g.subscription_id = i.subscription_id
             WHERE i.subscription_id = ?1
               AND i.sequence > g.last_acknowledged
               AND i.sequence <= g.last_produced
         )",
    )
    .bind(subscription.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn replay_failed_gap_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
    after: SubscriptionCursor,
    now: i64,
) -> DbResult<SubscriptionGap> {
    let gap = gap_tx(tx, &subscription.to_string()).await?;
    if after < gap.last_acknowledged || after > gap.last_produced {
        return Err(invalid(
            "terminal gap replay cursor is outside the frozen prefix",
        ));
    }
    if now >= gap.terminal_evidence_expires_at_us {
        release_terminal_evidence_tx(tx, subscription).await?;
    }
    Ok(gap)
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
                    .map_err(invalid)?,
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
        authority: &FederationSendAuthority,
        subscription: Uuid,
        product: &ProductConversationId,
        now: i64,
    ) -> DbResult<SubscriptionSnapshot> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let enrollment_id = federation_send_enrollment_id_tx(&mut tx, authority).await?;
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
            sqlx::query("INSERT INTO federation_subscriptions(id,enrollment_id,product_id,created_at_us,last_contact_at_us) VALUES (?1,?2,?3,?4,?4)").bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).bind(now).execute(&mut *tx).await?;
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
        authority: &FederationSendAuthority,
        subscription: Uuid,
        product: &ProductConversationId,
        after: SubscriptionCursor,
        now: i64,
    ) -> DbResult<SubscriptionReplay> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let enrollment_id = federation_send_enrollment_id_tx(&mut tx, authority).await?;
        let row = sqlx::query("SELECT state, acknowledged_through FROM federation_subscriptions WHERE id = ?1 AND enrollment_id = ?2 AND product_id = ?3")
            .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).fetch_optional(&mut *tx).await?.ok_or_else(|| invalid("subscription unavailable"))?;
        let state: String = row.try_get("state")?;
        let id = subscription.to_string();
        if state == "failed_gap" {
            let gap = replay_failed_gap_tx(&mut tx, subscription, after, now).await?;
            tx.commit().await?;
            return Ok(SubscriptionReplay::Gap { gap });
        }
        if state != "active" {
            tx.commit().await?;
            return Ok(SubscriptionReplay::Ended {
                reason: end(&state)?,
            });
        }
        retain_tx(&mut tx, &id, now).await?;
        let row = sqlx::query(
            "SELECT state, acknowledged_through FROM federation_subscriptions WHERE id = ?1",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        let state: String = row.try_get("state")?;
        let mut acknowledged: i64 = row.try_get("acknowledged_through")?;
        if state == "failed_gap" {
            let gap = replay_failed_gap_tx(&mut tx, subscription, after, now).await?;
            tx.commit().await?;
            return Ok(SubscriptionReplay::Gap { gap });
        }
        if after.get() != acknowledged {
            let next = acknowledged
                .checked_add(1)
                .ok_or_else(|| invalid("cursor overflow"))?;
            let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_delivery_authorizations a JOIN federation_terminal_identities i ON i.event_id = a.event_id WHERE a.subscription_id = ?1 AND a.kind = 'fact' AND a.after_cursor = ?2 AND a.through_cursor = ?3 AND i.subscription_id = a.subscription_id AND i.sequence = a.through_cursor)")
                .bind(&id).bind(acknowledged).bind(after.get()).fetch_one(&mut *tx).await?;
            if after.get() != next || !authorized {
                return Err(invalid("acknowledgement is not the next authorized fact"));
            }
            sqlx::query(
                "UPDATE federation_subscriptions SET acknowledged_through = ?2 WHERE id = ?1",
            )
            .bind(&id)
            .bind(after.get())
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM federation_delivery_authorizations WHERE subscription_id = ?1 AND kind = 'fact' AND through_cursor <= ?2")
                .bind(&id).bind(after.get()).execute(&mut *tx).await?;
            sqlx::query("UPDATE federation_terminal_identities SET evidence_release_reason = 'settled' WHERE subscription_id = ?1 AND sequence <= ?2 AND evidence_release_reason IS NULL AND event_id IN (SELECT event_id FROM federation_terminal_payloads)")
                .bind(&id).bind(after.get()).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1 AND sequence <= ?2)")
                .bind(&id).bind(after.get()).execute(&mut *tx).await?;
            acknowledged = after.get();
        }
        sqlx::query("UPDATE federation_subscriptions SET last_contact_at_us = ?2 WHERE id = ?1")
            .bind(&id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query("SELECT i.event_id, i.sequence, o.occurrence_kind, o.occurrence_id, o.generation AS occurrence_generation, p.source_transcript_id,p.outcome,p.occurred_at_us FROM federation_terminal_identities i JOIN federation_terminal_occurrences o ON o.occurrence_kind = i.occurrence_kind AND o.occurrence_id = i.occurrence_id AND o.generation = i.generation JOIN federation_terminal_payloads p ON p.event_id = i.event_id WHERE i.subscription_id = ?1 AND i.sequence > ?2 AND i.continuation_state = 'none' AND i.sequence < COALESCE((SELECT MIN(blocked.sequence) FROM federation_terminal_identities blocked JOIN federation_terminal_payloads blocked_payload USING(event_id) WHERE blocked.subscription_id = i.subscription_id AND blocked.continuation_state = 'awaiting'), 9223372036854775807) AND NOT EXISTS (SELECT 1 FROM federation_subscriptions s JOIN close_obligations o ON o.product_conversation_id = s.product_id WHERE s.id = i.subscription_id AND o.phase != 'completed') ORDER BY i.sequence LIMIT ?3")
            .bind(&id).bind(acknowledged).bind(replay_batch_sql()?).fetch_all(&mut *tx).await?;
        let events = rows
            .iter()
            .map(|row| {
                Ok(FederatedTerminalFact {
                    event_id: uuid(row.try_get("event_id")?)?,
                    sequence: cursor(row.try_get("sequence")?)?,
                    occurrence: TerminalFactOccurrence::new(
                        TerminalFactOccurrenceKind::try_from(
                            row.try_get::<&str, _>("occurrence_kind")?,
                        )
                        .map_err(invalid)?,
                        row.try_get::<String, _>("occurrence_id")?,
                        u64::try_from(row.try_get::<i64, _>("occurrence_generation")?)
                            .map_err(|_| invalid("negative terminal occurrence generation"))?,
                    )
                    .map_err(invalid)?,
                    source_transcript_id: TranscriptConversationId::parse(
                        row.try_get::<String, _>("source_transcript_id")?,
                    )
                    .map_err(|error| invalid(&error.to_string()))?,
                    outcome: TerminalFactOutcome::try_from(row.try_get::<&str, _>("outcome")?)
                        .map_err(invalid)?,
                    occurred_at_us: row.try_get("occurred_at_us")?,
                })
            })
            .collect::<DbResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(SubscriptionReplay::Facts { events })
    }

    /// End the exact subscription or fence its absent Create without retracting receipts.
    ///
    /// # Errors
    /// Returns an error for a mismatched caller/binding or failed transaction.
    pub async fn unsubscribe_federation_subscription(
        &self,
        authority: &FederationSendAuthority,
        subscription: Uuid,
        product: &ProductConversationId,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let enrollment_id = federation_send_enrollment_id_tx(&mut tx, authority).await?;
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

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
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
        state: SubscriptionState::try_from(row.try_get::<&str, _>("state")?).map_err(invalid)?,
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

    /// Settle a source revocation and discard work that can no longer be authorized.
    ///
    /// # Errors
    /// Returns an error if the transaction fails.
    pub async fn settle_remote_subscription_revocation(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let matched: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3)")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).fetch_one(&mut *tx).await?;
        if !matched {
            return Err(invalid("remote subscription binding unavailable"));
        }
        sqlx::query("UPDATE federation_remote_subscriptions SET state = 'revoked' WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND state = 'active'")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM federation_remote_navigation WHERE admission_id IN (SELECT admission_id FROM federation_remote_admissions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND accepted_transcript_id IS NULL)")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE federation_remote_admissions SET suppression_reason = 'trust_revoked', authorization_requested = 0, authorization_id = NULL WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND kind = 'fact' AND accepted_transcript_id IS NULL AND suppression_reason IS NULL")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM federation_pending_unsubscribes WHERE destination_instance_id = ?1 AND subscription_id = ?2")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

mod authorization;
mod consumer;
mod presentation;
pub use authorization::RemoteAuthorizationRequest;
pub use consumer::{
    RemoteNotification, RemoteNotificationContent, RemoteNotificationDisposition,
    RemoteSuppressionReason,
};
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
    /// Find the active or pending-unsubscribe subscription for exact management.
    ///
    /// # Errors
    /// Returns an error if the query fails or the binding cannot be decoded.
    pub async fn remote_subscription_for_management(
        &self,
        reference: &SubscriptionReference,
    ) -> DbResult<Option<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND product_id = ?2 AND (state = 'active' OR EXISTS (SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id)) ORDER BY pending_unsubscribe DESC, subscription_id DESC LIMIT 1").bind(reference.instance_id.to_string()).bind(reference.product_conversation_id.as_str()).fetch_optional(&self.pool).await?.as_ref().map(decode_remote).transpose()
    }
    /// Select at most four subscriptions with active, unsubscribe, or authorization work.
    ///
    /// # Errors
    /// Returns an error if the query fails or a binding cannot be decoded.
    pub async fn next_remote_subscription_sync_batch(
        &self,
        limit: i64,
    ) -> DbResult<Vec<RemoteSubscription>> {
        sqlx::query("SELECT *, EXISTS(SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) AS pending_unsubscribe FROM federation_remote_subscriptions WHERE state = 'active' OR EXISTS (SELECT 1 FROM federation_pending_unsubscribes u WHERE u.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND u.subscription_id = federation_remote_subscriptions.subscription_id) OR (state = 'failed_gap' AND EXISTS (SELECT 1 FROM federation_remote_admissions a WHERE a.destination_instance_id = federation_remote_subscriptions.destination_instance_id AND a.subscription_id = federation_remote_subscriptions.subscription_id AND a.accepted_transcript_id IS NULL AND a.suppression_reason IS NULL)) ORDER BY last_sync_at_us, destination_instance_id,subscription_id LIMIT ?1").bind(limit.clamp(1,4)).fetch_all(&self.pool).await?.iter().map(decode_remote).collect()
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
