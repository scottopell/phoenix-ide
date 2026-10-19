use super::{
    end, invalid, snapshot_tx, uuid, Database, DbResult, FederatedTerminalFact, InstanceId,
    ProductConversationId, RemoteSubscription, Row, Sqlite, SubscriptionAuthorization,
    SubscriptionCurrentState, SubscriptionDelivery, SubscriptionEnd, SubscriptionReference,
    Transaction, Uuid,
};

impl Database {
    /// Serialize a delivery authorization against source Close, revoke, and unsubscribe.
    /// Exact unacknowledged receipts remain replayable after the subscription ends.
    ///
    /// # Errors
    /// Returns an error for changed identities, invalid ranges, or failed persistence.
    pub async fn authorize_federation_delivery(
        &self,
        enrollment_id: Uuid,
        caller: InstanceId,
        subscription: Uuid,
        product: &ProductConversationId,
        delivery: &SubscriptionDelivery,
    ) -> DbResult<SubscriptionAuthorization> {
        let (after, through) = delivery.bounds().map_err(invalid)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT s.state, e.revoked_at_us, p.ordinary_lifecycle, EXISTS(SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = s.product_id AND o.phase != 'completed') AS closing FROM federation_subscriptions s JOIN federation_enrollments e ON e.id = s.enrollment_id JOIN product_conversations p ON p.id = s.product_id WHERE s.id = ?1 AND s.enrollment_id = ?2 AND s.product_id = ?3 AND e.caller_instance_id = ?4")
            .bind(subscription.to_string()).bind(enrollment_id.to_string()).bind(product.as_str()).bind(caller.to_string()).fetch_optional(&mut *tx).await?.ok_or_else(|| invalid("subscription binding unavailable"))?;
        let (kind, event, transcript, outcome) = match delivery {
            SubscriptionDelivery::Fact { fact } => {
                ("fact", Some(fact.event_id.to_string()), None, None)
            }
            SubscriptionDelivery::Gap { gap } => (
                "gap",
                None,
                Some(gap.snapshot.transcript_id.as_str()),
                match gap.snapshot.state {
                    SubscriptionCurrentState::Nonterminal => None,
                    SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
                },
            ),
        };
        let existing = sqlx::query("SELECT authorization_id, event_id, gap_transcript_id, gap_outcome FROM federation_delivery_authorizations WHERE subscription_id = ?1 AND after_cursor = ?2 AND through_cursor = ?3 AND kind = ?4")
            .bind(subscription.to_string()).bind(after.get()).bind(through.get()).bind(kind).fetch_optional(&mut *tx).await?;
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
            {
                return Err(invalid("authorized delivery identity changed"));
            }
            if let SubscriptionDelivery::Fact { fact } = delivery {
                if !validate_fact_tx(&mut tx, subscription, fact).await? {
                    return Err(invalid("authorized fact identity changed"));
                }
            }
            return Ok(SubscriptionAuthorization::Authorized {
                authorization_id: uuid(existing.try_get("authorization_id")?)?,
            });
        }
        let state: &str = row.try_get("state")?;
        let rejection = if state != "active" {
            Some(end(state)?)
        } else if row.try_get::<Option<i64>, _>("revoked_at_us")?.is_some() {
            Some(SubscriptionEnd::Revoked)
        } else if row.try_get::<&str, _>("ordinary_lifecycle")? == "history" {
            Some(SubscriptionEnd::SourceClosed)
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
                let pruned: i64 = sqlx::query_scalar("SELECT MAX(pruned_through, acknowledged_through) FROM federation_subscriptions WHERE id = ?1").bind(subscription.to_string()).fetch_one(&mut *tx).await?;
                if gap.resume_after.get() != pruned
                    || gap.snapshot != snapshot_tx(&mut tx, product).await?
                {
                    return Ok(SubscriptionAuthorization::Refresh);
                }
            }
        }
        let authorization_id = Uuid::new_v4();
        sqlx::query("INSERT INTO federation_delivery_authorizations(authorization_id,subscription_id,after_cursor,through_cursor,kind,event_id,gap_transcript_id,gap_outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
            .bind(authorization_id.to_string()).bind(subscription.to_string()).bind(after.get()).bind(through.get()).bind(kind).bind(event).bind(transcript).bind(outcome).execute(&mut *tx).await?;
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
    ) -> DbResult<()> {
        let changed = sqlx::query("UPDATE federation_remote_admissions SET authorization_requested = 1 WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND accepted_transcript_id IS NULL AND suppressed = 0 AND (authorization_requested = 1 OR EXISTS(SELECT 1 FROM federation_remote_subscriptions s WHERE s.destination_instance_id = federation_remote_admissions.destination_instance_id AND s.subscription_id = federation_remote_admissions.subscription_id AND s.state = 'active'))")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).execute(&self.pool).await?.rows_affected();
        if changed != 1 {
            return Err(invalid("staged delivery no longer authorizable"));
        }
        Ok(())
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
        let changed = sqlx::query("UPDATE federation_remote_admissions SET authorization_id = ?4 WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND authorization_requested = 1 AND suppressed = 0 AND (authorization_id IS NULL OR authorization_id = ?4)")
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
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM federation_remote_navigation WHERE admission_id = ?1 AND EXISTS (SELECT 1 FROM federation_remote_admissions a WHERE a.admission_id = ?1 AND a.destination_instance_id = ?2 AND a.subscription_id = ?3 AND a.accepted_transcript_id IS NULL AND a.authorization_id IS NULL)")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).execute(&mut *tx).await?;
        sqlx::query("UPDATE federation_remote_admissions SET suppressed = 1, authorization_requested = 0 WHERE admission_id = ?1 AND destination_instance_id = ?2 AND subscription_id = ?3 AND accepted_transcript_id IS NULL AND authorization_id IS NULL")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).execute(&mut *tx).await?;
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
        sqlx::query("UPDATE federation_remote_admissions SET suppressed = 1, authorization_requested = 0, authorization_id = NULL WHERE destination_instance_id = ?1 AND subscription_id = ?2 AND accepted_transcript_id IS NULL")
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

async fn validate_fact_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subscription: Uuid,
    fact: &FederatedTerminalFact,
) -> DbResult<bool> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_terminal_identities i JOIN federation_terminal_payloads p USING(event_id) WHERE i.subscription_id = ?1 AND i.event_id = ?2 AND i.sequence = ?3 AND i.continuation_state = 'none' AND p.source_transcript_id = ?4 AND p.outcome = ?5 AND p.occurred_at_us = ?6)")
        .bind(subscription.to_string()).bind(fact.event_id.to_string()).bind(fact.sequence.get()).bind(fact.source_transcript_id.as_str()).bind(fact.outcome.as_str()).bind(fact.occurred_at_us).fetch_one(&mut **tx).await?;
    Ok(valid)
}
