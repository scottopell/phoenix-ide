use super::{
    cursor, invalid, replay_batch_sql, uuid, Database, DbResult, FederatedTerminalFact, GapReason,
    InstanceId, ProductConversationId, RemoteSubscription, Row, Serialize, Sqlite,
    SubscriptionCurrentState, SubscriptionCursor, SubscriptionDelivery, SubscriptionEnd,
    SubscriptionGap, SubscriptionReference, SubscriptionReplay, SubscriptionSnapshot,
    TerminalFactOccurrence, TerminalFactOccurrenceKind, TerminalFactOutcome, Transaction,
    TranscriptConversationId, Uuid, REPLAY_BATCH,
};

pub type RemoteNotificationContent = SubscriptionDelivery;
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum RemoteNotificationDisposition {
    Pending,
    Authorizing,
    Authorized {
        authorization_id: Uuid,
    },
    Suppressed {
        reason: RemoteSuppressionReason,
    },
    Accepted {
        transcript_id: TranscriptConversationId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSuppressionReason {
    AuthorizationDenied,
    TrustRevoked,
    SourceUnreachable,
    EvidenceExpired,
    SubscriptionEnded,
}

impl RemoteSuppressionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationDenied => "authorization_denied",
            Self::TrustRevoked => "trust_revoked",
            Self::SourceUnreachable => "source_unreachable",
            Self::EvidenceExpired => "evidence_expired",
            Self::SubscriptionEnded => "subscription_ended",
        }
    }

    fn parse(value: &str) -> DbResult<Self> {
        match value {
            "authorization_denied" => Ok(Self::AuthorizationDenied),
            "trust_revoked" => Ok(Self::TrustRevoked),
            "source_unreachable" => Ok(Self::SourceUnreachable),
            "evidence_expired" => Ok(Self::EvidenceExpired),
            "subscription_ended" => Ok(Self::SubscriptionEnded),
            _ => Err(super::DbError::Serialization(format!(
                "invalid remote suppression reason: {value}"
            ))),
        }
    }
}
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RemoteNotification {
    pub admission_id: String,
    pub reference: SubscriptionReference,
    pub subscription_id: Uuid,
    pub content: RemoteNotificationContent,
    pub disposition: RemoteNotificationDisposition,
}

impl Database {
    /// Stage a bounded source replay without acknowledging any unaccepted delivery.
    ///
    /// # Errors
    /// Returns an error for changed bindings, cursor/payload mismatch, or failed persistence.
    #[allow(clippy::too_many_lines)]
    pub async fn stage_remote_replay(
        &self,
        remote: &RemoteSubscription,
        replay: &SubscriptionReplay,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = sqlx::query("SELECT admitted_cursor,state FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3")
            .bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(remote.reference.product_conversation_id.as_str()).fetch_optional(&mut *tx).await?;
        let Some(current) = current else {
            return Err(invalid("remote subscription cursor or lifecycle changed"));
        };
        if current.try_get::<i64, _>("admitted_cursor")? != remote.admitted_cursor.get() {
            return Err(invalid("remote subscription cursor changed"));
        }
        let state: &str = current.try_get("state")?;
        match replay {
            SubscriptionReplay::Facts { events } => {
                if state != "active" {
                    return Err(invalid("remote subscription lifecycle changed"));
                }
                if events.len() > REPLAY_BATCH {
                    return Err(invalid("replay exceeds event budget"));
                }
                let mut after = remote.admitted_cursor.get();
                for fact in events {
                    if fact.sequence.get()
                        != after
                            .checked_add(1)
                            .ok_or_else(|| invalid("cursor overflow"))?
                        || fact.occurred_at_us < 0
                        || fact.source_transcript_id.as_str().len() > 1024
                        || fact.occurrence.id.is_empty()
                        || fact.occurrence.id.len() > 1024
                    {
                        return Err(invalid("invalid fact order or bounds"));
                    }
                    let generation = i64::try_from(fact.occurrence.generation)
                        .map_err(|_| invalid("invalid fact order or bounds"))?;
                    let admission_id = format!(
                        "federation:{}:fact:{}",
                        remote.reference.instance_id, fact.event_id
                    );
                    stage_admission_tx(
                        &mut tx,
                        remote,
                        &admission_id,
                        "fact",
                        after,
                        fact.sequence.get(),
                    )
                    .await?;
                    sqlx::query("INSERT INTO federation_received_facts(admission_id,destination_instance_id,event_id,occurrence_kind,occurrence_id,occurrence_generation,source_transcript_id,outcome,occurred_at_us) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(admission_id) DO NOTHING")
                        .bind(&admission_id).bind(remote.reference.instance_id.to_string()).bind(fact.event_id.to_string()).bind(fact.occurrence.kind.as_str()).bind(&fact.occurrence.id).bind(generation).bind(fact.source_transcript_id.as_str()).bind(fact.outcome.as_str()).bind(fact.occurred_at_us).execute(&mut *tx).await?;
                    let stored = sqlx::query("SELECT occurrence_kind,occurrence_id,occurrence_generation,source_transcript_id,outcome,occurred_at_us FROM federation_received_facts WHERE admission_id = ?1").bind(admission_id).fetch_one(&mut *tx).await?;
                    if stored.try_get::<String, _>("occurrence_kind")?
                        != fact.occurrence.kind.as_str()
                        || stored.try_get::<String, _>("occurrence_id")? != fact.occurrence.id
                        || stored.try_get::<i64, _>("occurrence_generation")? != generation
                        || stored.try_get::<String, _>("source_transcript_id")?
                            != fact.source_transcript_id.as_str()
                        || stored.try_get::<String, _>("outcome")? != fact.outcome.as_str()
                        || stored.try_get::<i64, _>("occurred_at_us")? != fact.occurred_at_us
                    {
                        return Err(invalid("event identity changed payload"));
                    }
                    after = fact.sequence.get();
                }
            }
            SubscriptionReplay::Gap { gap } => {
                if !matches!(state, "active" | "failed_gap")
                    || remote.admitted_cursor < gap.last_acknowledged
                    || remote.admitted_cursor > gap.last_produced
                    || gap.last_produced <= gap.last_acknowledged
                    || gap.failed_at_us < 0
                    || gap.terminal_evidence_expires_at_us <= gap.failed_at_us
                    || gap.snapshot.transcript_id.as_str().len() > 1024
                {
                    return Err(invalid("invalid terminal gap tombstone"));
                }
                let id = format!(
                    "federation:{}:gap:{}",
                    remote.reference.instance_id, remote.subscription_id
                );
                stage_admission_tx(
                    &mut tx,
                    remote,
                    &id,
                    "gap",
                    gap.last_acknowledged.get(),
                    gap.last_produced.get(),
                )
                .await?;
                let outcome = match gap.snapshot.state {
                    SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
                    SubscriptionCurrentState::Nonterminal => None,
                };
                sqlx::query("INSERT INTO federation_received_gaps(admission_id,last_acknowledged,last_produced,reason,failed_at_us,terminal_evidence_expires_at_us,source_transcript_id,current_outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(admission_id) DO NOTHING")
                    .bind(&id).bind(gap.last_acknowledged.get()).bind(gap.last_produced.get()).bind(gap.reason.as_str()).bind(gap.failed_at_us).bind(gap.terminal_evidence_expires_at_us).bind(gap.snapshot.transcript_id.as_str()).bind(outcome).execute(&mut *tx).await?;
                let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_received_gaps WHERE admission_id = ?1 AND last_acknowledged = ?2 AND last_produced = ?3 AND reason = ?4 AND failed_at_us = ?5 AND terminal_evidence_expires_at_us = ?6 AND source_transcript_id = ?7 AND current_outcome IS ?8)")
                    .bind(&id).bind(gap.last_acknowledged.get()).bind(gap.last_produced.get()).bind(gap.reason.as_str()).bind(gap.failed_at_us).bind(gap.terminal_evidence_expires_at_us).bind(gap.snapshot.transcript_id.as_str()).bind(outcome).fetch_one(&mut *tx).await?;
                if !matches {
                    return Err(invalid("gap identity changed payload"));
                }
                sqlx::query("UPDATE federation_remote_subscriptions SET state = 'failed_gap' WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND state = 'active'")
                    .bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).execute(&mut *tx).await?;
            }
            SubscriptionReplay::Ended { reason } => {
                let state = match reason {
                    SubscriptionEnd::Unsubscribed => "unsubscribed",
                    SubscriptionEnd::Revoked => "revoked",
                    SubscriptionEnd::SourceClosed => "source_closed",
                };
                sqlx::query("UPDATE federation_remote_subscriptions SET state = ?3 WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND state = 'active'").bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(state).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// Read at most one replay batch of pending or recoverable authorization attempts.
    ///
    /// # Errors
    /// Returns an error if the query fails or persisted payloads cannot be decoded.
    pub async fn pending_remote_notifications(
        &self,
        remote: &RemoteSubscription,
    ) -> DbResult<Vec<RemoteNotification>> {
        self.remote_notifications(remote, SubscriptionCursor::START, true)
            .await
    }
    /// Read received fact/gap history strictly after the supplied cursor.
    ///
    /// # Errors
    /// Returns an error if the query fails or persisted payloads cannot be decoded.
    pub async fn remote_notification_history(
        &self,
        remote: &RemoteSubscription,
        after: SubscriptionCursor,
    ) -> DbResult<Vec<RemoteNotification>> {
        self.remote_notifications(remote, after, false).await
    }
    async fn remote_notifications(
        &self,
        remote: &RemoteSubscription,
        after: SubscriptionCursor,
        pending_only: bool,
    ) -> DbResult<Vec<RemoteNotification>> {
        let rows = sqlx::query("SELECT a.*, s.product_id, f.event_id, f.occurrence_kind, f.occurrence_id, f.occurrence_generation, f.source_transcript_id AS fact_transcript, f.outcome, f.occurred_at_us, g.last_acknowledged, g.last_produced, g.reason AS gap_reason, g.failed_at_us, g.terminal_evidence_expires_at_us, g.source_transcript_id AS gap_transcript, g.current_outcome FROM federation_remote_admissions a JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id) LEFT JOIN federation_received_facts f ON f.admission_id = a.admission_id LEFT JOIN federation_received_gaps g ON g.admission_id = a.admission_id WHERE a.destination_instance_id = ?1 AND a.subscription_id = ?2 AND (a.kind = 'gap' OR a.through_cursor > ?4) AND (?5 = 0 OR (a.accepted_transcript_id IS NULL AND a.suppression_reason IS NULL AND ((a.kind = 'fact' AND s.state IN ('active','failed_gap')) OR (a.kind = 'gap' AND s.state = 'failed_gap' AND NOT EXISTS (SELECT 1 FROM federation_remote_admissions prefix WHERE prefix.destination_instance_id = a.destination_instance_id AND prefix.subscription_id = a.subscription_id AND prefix.kind = 'fact' AND prefix.accepted_transcript_id IS NULL AND prefix.suppression_reason IS NULL))))) ORDER BY CASE a.kind WHEN 'fact' THEN 0 ELSE 1 END, a.after_cursor, a.through_cursor LIMIT ?3")
            .bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(replay_batch_sql()?).bind(after.get()).bind(pending_only).fetch_all(&self.pool).await?;
        rows.iter().map(decode_notification).collect()
    }
}
pub(super) fn decode_notification(row: &sqlx::sqlite::SqliteRow) -> DbResult<RemoteNotification> {
    let kind: &str = row.try_get("kind")?;
    let content = if kind == "fact" {
        RemoteNotificationContent::Fact {
            fact: FederatedTerminalFact {
                event_id: uuid(row.try_get("event_id")?)?,
                sequence: cursor(row.try_get("through_cursor")?)?,
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
                    row.try_get::<String, _>("fact_transcript")?,
                )
                .map_err(|error| invalid(&error.to_string()))?,
                outcome: TerminalFactOutcome::try_from(row.try_get::<&str, _>("outcome")?)
                    .map_err(invalid)?,
                occurred_at_us: row.try_get("occurred_at_us")?,
            },
        }
    } else {
        let outcome: Option<String> = row.try_get("current_outcome")?;
        RemoteNotificationContent::Gap {
            gap: SubscriptionGap {
                last_acknowledged: cursor(row.try_get("last_acknowledged")?)?,
                last_produced: cursor(row.try_get("last_produced")?)?,
                reason: GapReason::try_from(row.try_get::<&str, _>("gap_reason")?)
                    .map_err(invalid)?,
                failed_at_us: row.try_get("failed_at_us")?,
                terminal_evidence_expires_at_us: row.try_get("terminal_evidence_expires_at_us")?,
                snapshot: SubscriptionSnapshot {
                    transcript_id: TranscriptConversationId::parse(
                        row.try_get::<String, _>("gap_transcript")?,
                    )
                    .map_err(|error| invalid(&error.to_string()))?,
                    state: match outcome {
                        Some(outcome) => SubscriptionCurrentState::Terminal {
                            outcome: TerminalFactOutcome::try_from(outcome.as_str())
                                .map_err(invalid)?,
                        },
                        None => SubscriptionCurrentState::Nonterminal,
                    },
                },
            },
        }
    };
    let accepted: Option<String> = row.try_get("accepted_transcript_id")?;
    let disposition = match accepted {
        Some(transcript) => RemoteNotificationDisposition::Accepted {
            transcript_id: TranscriptConversationId::parse(transcript)
                .map_err(|error| invalid(&error.to_string()))?,
        },
        None => match row.try_get::<Option<String>, _>("suppression_reason")? {
            Some(reason) => RemoteNotificationDisposition::Suppressed {
                reason: RemoteSuppressionReason::parse(&reason)?,
            },
            None => match row.try_get::<Option<String>, _>("authorization_id")? {
                Some(id) => RemoteNotificationDisposition::Authorized {
                    authorization_id: uuid(&id)?,
                },
                None if row.try_get::<bool, _>("authorization_requested")? => {
                    RemoteNotificationDisposition::Authorizing
                }
                None => RemoteNotificationDisposition::Pending,
            },
        },
    };
    Ok(RemoteNotification {
        admission_id: row.try_get("admission_id")?,
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
        content,
        disposition,
    })
}

async fn stage_admission_tx(
    tx: &mut Transaction<'_, Sqlite>,
    remote: &RemoteSubscription,
    id: &str,
    kind: &str,
    after: i64,
    through: i64,
) -> DbResult<()> {
    sqlx::query("INSERT INTO federation_remote_admissions(admission_id,destination_instance_id,subscription_id,kind,after_cursor,through_cursor) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(admission_id) DO NOTHING")
        .bind(id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(kind).bind(after).bind(through).execute(&mut **tx).await?;
    let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_remote_admissions WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND kind = ?4 AND after_cursor = ?5 AND through_cursor = ?6)")
        .bind(id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(kind).bind(after).bind(through).fetch_one(&mut **tx).await?;
    if !matches {
        return Err(invalid("event identity changed subscription or sequence"));
    }
    Ok(())
}
