use super::{
    end, federation_send_enrollment_id_tx, invalid, uuid, Database, DbResult,
    FederatedTerminalFact, FederationSendAuthority, ProductConversationId, RemoteSubscription, Row,
    Sqlite, SubscriptionAuthorization, SubscriptionCurrentState, SubscriptionDelivery,
    SubscriptionEnd, SubscriptionGap, SubscriptionReference, Transaction, Uuid,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuthorizationRequest {
    Active,
    TerminalAttempt { attempt: u8 },
    TerminalExhausted,
}

const TERMINAL_AUTHORIZATION_ATTEMPTS: i64 = 3;

impl Database {
    /// Serialize a delivery authorization against source Close, revoke, and unsubscribe.
    /// Existing authorization identities do not outlive source Close.
    ///
    /// # Errors
    /// Returns an error for changed identities, invalid ranges, or failed persistence.
    pub async fn authorize_federation_delivery(
        &self,
        authority: &FederationSendAuthority,
        subscription: Uuid,
        product: &ProductConversationId,
        delivery: &SubscriptionDelivery,
        observed_at_us: i64,
    ) -> DbResult<SubscriptionAuthorization> {
        let (after, through) = delivery.bounds().map_err(invalid)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let enrollment_id = federation_send_enrollment_id_tx(&mut tx, authority).await?;
        let row = sqlx::query("SELECT s.state, e.revoked_at_us, p.ordinary_lifecycle, EXISTS(SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = s.product_id AND o.phase != 'completed') AS closing FROM federation_subscriptions s JOIN federation_enrollments e ON e.id = s.enrollment_id JOIN product_conversations p ON p.id = s.product_id WHERE s.id = ?1 AND s.enrollment_id = ?2 AND s.product_id = ?3")
            .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).fetch_optional(&mut *tx).await?.ok_or_else(|| invalid("subscription binding unavailable"))?;
        let (kind, event, transcript, outcome, gap_reason, gap_failed_at) = match delivery {
            SubscriptionDelivery::Fact { fact } => (
                "fact",
                Some(fact.event_id.to_string()),
                None,
                None,
                None,
                None,
            ),
            SubscriptionDelivery::Gap { gap } => (
                "gap",
                None,
                Some(gap.snapshot.transcript_id.as_str()),
                match gap.snapshot.state {
                    SubscriptionCurrentState::Nonterminal => None,
                    SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
                },
                Some(gap.reason.as_str()),
                Some(gap.failed_at_us),
            ),
        };
        let existing = sqlx::query("SELECT authorization_id, event_id, gap_transcript_id, gap_outcome, gap_reason, gap_failed_at_us FROM federation_delivery_authorizations WHERE subscription_id = ?1 AND after_cursor = ?2 AND through_cursor = ?3 AND kind = ?4")
            .bind(subscription.to_string()).bind(after.get()).bind(through.get()).bind(kind).fetch_optional(&mut *tx).await?;
        let state: &str = row.try_get("state")?;
        if state == "source_closed" || row.try_get::<&str, _>("ordinary_lifecycle")? == "history" {
            return Ok(SubscriptionAuthorization::Rejected {
                reason: SubscriptionEnd::SourceClosed,
            });
        }
        if state == "failed_gap" {
            if let SubscriptionDelivery::Fact { fact } = delivery {
                let gap = sqlx::query(
                    "SELECT last_acknowledged, last_produced,
                            terminal_evidence_expires_at_us
                     FROM federation_subscription_gaps
                     WHERE subscription_id = ?1",
                )
                .bind(subscription.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| invalid("failed-gap subscription is missing its tombstone"))?;
                let last_acknowledged: i64 = gap.try_get("last_acknowledged")?;
                let last_produced: i64 = gap.try_get("last_produced")?;
                let expires_at_us: i64 = gap.try_get("terminal_evidence_expires_at_us")?;
                if fact.sequence.get() <= last_acknowledged || fact.sequence.get() > last_produced {
                    return Ok(SubscriptionAuthorization::Refresh);
                }
                if observed_at_us >= expires_at_us {
                    release_terminal_evidence_tx(&mut tx, subscription).await?;
                    tx.commit().await?;
                    return Ok(SubscriptionAuthorization::EvidenceExpired);
                }
            }
        }
        if let Some(existing) = existing {
            if existing.try_get::<Option<String>, _>("event_id")? != event
                || existing
                    .try_get::<Option<String>, _>("gap_transcript_id")?
                    .as_deref()
                    != transcript
                || existing
                    .try_get::<Option<String>, _>("gap_outcome")?
                    .as_deref()
                    != outcome
                || existing
                    .try_get::<Option<String>, _>("gap_reason")?
                    .as_deref()
                    != gap_reason
                || existing.try_get::<Option<i64>, _>("gap_failed_at_us")? != gap_failed_at
            {
                return Err(invalid("authorized delivery identity changed"));
            }
            match delivery {
                SubscriptionDelivery::Fact { fact } => {
                    if !validate_fact_tx(&mut tx, subscription, fact).await? {
                        return Err(invalid("authorized fact identity changed"));
                    }
                }
                SubscriptionDelivery::Gap { gap } => {
                    if !validate_gap_tx(&mut tx, subscription, gap).await? {
                        return Err(invalid("authorized gap identity changed"));
                    }
                }
            }
            return Ok(SubscriptionAuthorization::Authorized {
                authorization_id: uuid(existing.try_get("authorization_id")?)?,
            });
        }
        let rejection = if state == "failed_gap" {
            None
        } else if state != "active" {
            Some(end(state)?)
        } else if row.try_get::<Option<i64>, _>("revoked_at_us")?.is_some() {
            Some(SubscriptionEnd::Revoked)
        } else {
            None
        };
        if let Some(reason) = rejection {
            return Ok(SubscriptionAuthorization::Rejected { reason });
        }
        if row.try_get::<bool, _>("closing")? {
            return Ok(SubscriptionAuthorization::Refresh);
        }
        match delivery {
            SubscriptionDelivery::Fact { fact } => {
                if !validate_fact_tx(&mut tx, subscription, fact).await? {
                    return Ok(SubscriptionAuthorization::Refresh);
                }
            }
            SubscriptionDelivery::Gap { gap } => {
                if !validate_gap_tx(&mut tx, subscription, gap).await? {
                    return Ok(SubscriptionAuthorization::Refresh);
                }
            }
        }
        let authorization_id = Uuid::new_v4();
        sqlx::query("INSERT INTO federation_delivery_authorizations(authorization_id,subscription_id,after_cursor,through_cursor,kind,event_id,gap_transcript_id,gap_outcome,gap_reason,gap_failed_at_us) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")
            .bind(authorization_id.to_string()).bind(subscription.to_string()).bind(after.get()).bind(through.get()).bind(kind).bind(event).bind(transcript).bind(outcome).bind(gap_reason).bind(gap_failed_at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(SubscriptionAuthorization::Authorized { authorization_id })
    }

    /// Persist the attempted delivery identity before requesting source authorization.
    ///
    /// # Errors
    /// Returns an error if the staged delivery is ineligible or the write fails.
    pub async fn request_remote_notification_authorization(
        &self,
        remote: &RemoteSubscription,
        admission_id: &str,
    ) -> DbResult<RemoteAuthorizationRequest> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT a.kind, a.accepted_transcript_id, a.suppression_reason, a.authorization_id, a.terminal_settlement_attempts, s.state FROM federation_remote_admissions a JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id) WHERE a.admission_id = ?1 AND a.destination_instance_id = ?2 AND a.subscription_id = ?3")
            .bind(admission_id)
            .bind(remote.reference.instance_id.to_string())
            .bind(remote.subscription_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| invalid("staged delivery no longer authorizable"))?;
        if row.try_get::<String, _>("kind")? != "fact"
            || row
                .try_get::<Option<String>, _>("accepted_transcript_id")?
                .is_some()
            || row
                .try_get::<Option<String>, _>("suppression_reason")?
                .is_some()
            || row
                .try_get::<Option<String>, _>("authorization_id")?
                .is_some()
        {
            return Err(invalid("staged delivery no longer authorizable"));
        }
        let state: String = row.try_get("state")?;
        let request = if state == "active" {
            sqlx::query("UPDATE federation_remote_admissions SET authorization_requested = 1 WHERE admission_id = ?1")
                .bind(admission_id)
                .execute(&mut *tx)
                .await?;
            RemoteAuthorizationRequest::Active
        } else if state == "failed_gap" {
            let is_next: bool = sqlx::query_scalar("SELECT ?1 = (SELECT admission_id FROM federation_remote_admissions WHERE destination_instance_id = ?2 AND subscription_id = ?3 AND kind = 'fact' AND accepted_transcript_id IS NULL AND suppression_reason IS NULL ORDER BY after_cursor, through_cursor LIMIT 1)")
                .bind(admission_id)
                .bind(remote.reference.instance_id.to_string())
                .bind(remote.subscription_id.to_string())
                .fetch_one(&mut *tx)
                .await?;
            if !is_next {
                return Err(invalid("staged delivery is not the next prefix fact"));
            }
            let attempts: i64 = row.try_get("terminal_settlement_attempts")?;
            if attempts >= TERMINAL_AUTHORIZATION_ATTEMPTS {
                let updated = sqlx::query("UPDATE federation_remote_admissions SET suppression_reason = 'source_unreachable', authorization_requested = 0, authorization_id = NULL WHERE admission_id = ?1 AND accepted_transcript_id IS NULL AND suppression_reason IS NULL AND terminal_settlement_attempts = ?2")
                    .bind(admission_id)
                    .bind(attempts)
                    .execute(&mut *tx)
                    .await?;
                if updated.rows_affected() != 1 {
                    return Err(invalid(
                        "authorization exhaustion raced terminal settlement",
                    ));
                }
                tx.commit().await?;
                return Ok(RemoteAuthorizationRequest::TerminalExhausted);
            }
            let attempt = attempts + 1;
            let updated = sqlx::query("UPDATE federation_remote_admissions SET authorization_requested = 1, terminal_settlement_attempts = ?2 WHERE admission_id = ?1 AND accepted_transcript_id IS NULL AND suppression_reason IS NULL AND terminal_settlement_attempts = ?3")
                .bind(admission_id)
                .bind(attempt)
                .bind(attempts)
                .execute(&mut *tx)
                .await?;
            if updated.rows_affected() != 1 {
                return Err(invalid("authorization request raced terminal settlement"));
            }
            RemoteAuthorizationRequest::TerminalAttempt {
                attempt: u8::try_from(attempt).map_err(|error| invalid(&error.to_string()))?,
            }
        } else {
            return Err(invalid("staged delivery no longer authorizable"));
        };
        tx.commit().await?;
        Ok(request)
    }

    /// Persist a source authorization receipt and its navigation atomically.
    ///
    /// # Errors
    /// Returns an error for a mismatched receipt, missing locator, or failed write.
    pub async fn confirm_remote_authorization(
        &self,
        remote: &RemoteSubscription,
        admission_id: &str,
        authorization_id: Uuid,
        stable_path: &str,
        source_path: &str,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::save_remote_notification_navigation_tx(
            &mut tx,
            remote,
            admission_id,
            stable_path,
            source_path,
        )
        .await?;
        let changed = sqlx::query("UPDATE federation_remote_admissions SET authorization_id = ?4 WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND kind = 'fact' AND authorization_requested = 1 AND suppression_reason IS NULL AND (authorization_id IS NULL OR authorization_id = ?4) AND EXISTS(SELECT 1 FROM federation_remote_subscriptions s WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id AND s.subscription_id = federation_remote_admissions.subscription_id AND s.state IN ('active','failed_gap'))")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(authorization_id.to_string()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(invalid(
                "authorization receipt does not match staged delivery",
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Suppress/reset an unconfirmed attempt without advancing the admitted cursor.
    ///
    /// # Errors
    /// Returns an error if the transaction fails.
    pub async fn suppress_remote_notification(
        &self,
        remote: &RemoteSubscription,
        admission_id: &str,
        reason: super::consumer::RemoteSuppressionReason,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM federation_remote_navigation WHERE admission_id = ?1 AND EXISTS (SELECT 1 FROM federation_remote_admissions a WHERE a.admission_id = ?1 AND a.destination_instance_id = ?2 AND a.subscription_id = ?3 AND a.accepted_transcript_id IS NULL AND a.authorization_id IS NULL)")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).execute(&mut *tx).await?;
        let changed = sqlx::query("UPDATE federation_remote_admissions SET suppression_reason = ?4, authorization_requested = 0 WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND kind = 'fact' AND accepted_transcript_id IS NULL AND authorization_id IS NULL AND suppression_reason IS NULL")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(reason.as_str()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(invalid("staged delivery is not suppressible"));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Durably stop new local attempts and retain the exact remote unsubscribe identity.
    ///
    /// # Errors
    /// Returns an error for a missing binding or failed transaction.
    pub async fn begin_remote_unsubscribe(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_remote_subscriptions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND product_id = ?3)")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).fetch_one(&mut *tx).await?;
        if !exists {
            return Err(invalid("remote subscription unavailable"));
        }
        sqlx::query("INSERT INTO federation_pending_unsubscribes(destination_instance_id,subscription_id) VALUES (?1,?2) ON CONFLICT DO NOTHING")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM federation_remote_navigation WHERE admission_id IN (SELECT admission_id FROM federation_remote_admissions WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND accepted_transcript_id IS NULL)")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE federation_remote_admissions SET suppression_reason = 'subscription_ended', authorization_requested = 0, authorization_id = NULL WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND kind = 'fact' AND accepted_transcript_id IS NULL AND suppression_reason IS NULL")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE federation_remote_subscriptions SET state = 'unsubscribed' WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND state = 'active'")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Clear a pending unsubscribe after its exact source confirmation is received.
    ///
    /// # Errors
    /// Returns an error if the write fails.
    pub async fn complete_remote_unsubscribe(
        &self,
        reference: &SubscriptionReference,
        subscription: Uuid,
    ) -> DbResult<()> {
        sqlx::query("DELETE FROM federation_pending_unsubscribes WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND EXISTS(SELECT 1 FROM federation_remote_subscriptions s WHERE s.destination_instance_id = ?1 AND s.subscription_id = ?2 AND s.product_id = ?3)")
            .bind(reference.instance_id.to_string()).bind(subscription.to_string()).bind(reference.product_conversation_id.as_str()).execute(&self.pool).await?;
        Ok(())
    }
}

async fn validate_gap_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
    gap: &SubscriptionGap,
) -> DbResult<bool> {
    let outcome = match gap.snapshot.state {
        SubscriptionCurrentState::Nonterminal => None,
        SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
    };
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_subscription_gaps WHERE subscription_id = ?1 AND last_acknowledged = ?2 AND last_produced = ?3 AND reason = ?4 AND failed_at_us = ?5 AND terminal_evidence_expires_at_us = ?6 AND snapshot_transcript_id = ?7 AND snapshot_outcome IS ?8)")
        .bind(subscription.to_string())
        .bind(gap.last_acknowledged.get())
        .bind(gap.last_produced.get())
        .bind(gap.reason.as_str())
        .bind(gap.failed_at_us)
        .bind(gap.terminal_evidence_expires_at_us)
        .bind(gap.snapshot.transcript_id.as_str())
        .bind(outcome)
        .fetch_one(&mut **tx)
        .await
        .map_err(Into::into)
}

async fn validate_fact_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
    fact: &FederatedTerminalFact,
) -> DbResult<bool> {
    let generation = i64::try_from(fact.occurrence.generation)
        .map_err(|_| invalid("terminal fact occurrence generation overflow"))?;
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_terminal_identities i JOIN federation_terminal_payloads p USING(event_id) JOIN federation_terminal_occurrences o ON o.occurrence_id = i.occurrence_id WHERE i.subscription_id = ?1 AND i.event_id = ?2 AND i.sequence = ?3 AND i.continuation_state = 'none' AND p.source_transcript_id = ?4 AND p.outcome = ?5 AND p.occurred_at_us = ?6 AND o.occurrence_kind = ?7 AND o.occurrence_id = ?8 AND o.generation = ?9)")
        .bind(subscription.to_string())
        .bind(fact.event_id.to_string())
        .bind(fact.sequence.get())
        .bind(fact.source_transcript_id.as_str())
        .bind(fact.outcome.as_str())
        .bind(fact.occurred_at_us)
        .bind(fact.occurrence.kind.as_str())
        .bind(fact.occurrence.id.as_str())
        .bind(generation)
        .fetch_one(&mut **tx)
        .await?;
    Ok(valid)
}

async fn release_terminal_evidence_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
) -> DbResult<()> {
    sqlx::query("DELETE FROM federation_delivery_authorizations WHERE subscription_id = ?1")
        .bind(subscription.to_string())
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE federation_terminal_identities
         SET evidence_release_reason = 'expired'
         WHERE subscription_id = ?1
           AND evidence_release_reason IS NULL
           AND event_id IN (SELECT event_id FROM federation_terminal_payloads)",
    )
    .bind(subscription.to_string())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM federation_terminal_payloads
         WHERE event_id IN (
             SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1
         )",
    )
    .bind(subscription.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
