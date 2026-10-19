use std::{collections::HashMap, fmt::Write as _, str::FromStr};

use phoenix_core::domain::instance_identity::PeerBaseUrl;
use sqlx::QueryBuilder;

use super::{
    consumer::decode_notification, invalid, Database, DbResult, RemoteNotification,
    RemoteSubscription, Row, Sqlite, Transaction,
};

/// Durable received facts paired with optional source-issued navigation paths.
pub type RemoteNotificationPresentations =
    HashMap<String, (RemoteNotification, Option<(String, String)>)>;

impl Database {
    /// Persist source-generated credential-free navigation for a staged notification.
    ///
    /// # Errors
    ///
    /// Returns an error when the staged notification binding is missing, the paths are
    /// inconsistent with its exact source identity, or `SQLite` persistence fails.
    pub async fn save_remote_notification_navigation(
        &self,
        remote: &RemoteSubscription,
        admission_id: &str,
        stable_path: &str,
        source_path: &str,
    ) -> Result<(), crate::DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::save_remote_notification_navigation_tx(
            &mut tx,
            remote,
            admission_id,
            stable_path,
            source_path,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn save_remote_notification_navigation_tx(
        tx: &mut Transaction<'_, Sqlite>,
        remote: &RemoteSubscription,
        admission_id: &str,
        stable_path: &str,
        source_path: &str,
    ) -> DbResult<()> {
        let transcript: String = sqlx::query_scalar("SELECT COALESCE(f.source_transcript_id, g.source_transcript_id) FROM federation_remote_admissions a JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id) LEFT JOIN federation_received_facts f USING(admission_id) LEFT JOIN federation_received_gaps g USING(admission_id) WHERE a.admission_id = ?1 AND a.destination_instance_id = ?2 AND a.subscription_id = ?3 AND s.product_id = ?4 AND a.authorization_requested = 1 AND a.suppressed = 0")
            .bind(admission_id).bind(remote.reference.instance_id.to_string()).bind(remote.subscription_id.to_string()).bind(remote.reference.product_conversation_id.as_str()).fetch_optional(&mut **tx).await?.ok_or_else(|| invalid("staged delivery unavailable for navigation"))?;
        let transcript = encoded_component(&transcript);
        let expected_source = format!("/c/{transcript}?source_transcript={transcript}");
        let origin = PeerBaseUrl::from_str("https://subscription.invalid")
            .map_err(|error| invalid(&error.to_string()))?;
        for path in [stable_path, source_path] {
            let url = origin
                .navigation_url(path)
                .map_err(|error| invalid(&error.to_string()))?;
            if url.as_url().as_str() != format!("https://subscription.invalid{path}") {
                return Err(invalid(
                    "source navigation is not a credential-free destination locator",
                ));
            }
        }
        if source_path != expected_source {
            return Err(invalid("source evidence navigation changed identity"));
        }
        if !stable_path
            .strip_prefix("/c/")
            .is_some_and(|component| !component.is_empty() && !component.contains(['/', '?', '#']))
        {
            return Err(invalid(
                "stable navigation is not a canonical conversation route",
            ));
        }
        let changed = sqlx::query("INSERT INTO federation_remote_navigation(admission_id,stable_path,source_path) VALUES (?1,?2,?3) ON CONFLICT(admission_id) DO UPDATE SET stable_path = excluded.stable_path WHERE source_path = excluded.source_path")
            .bind(admission_id).bind(stable_path).bind(source_path).execute(&mut **tx).await?.rows_affected();
        if changed != 1 {
            return Err(invalid("source evidence navigation changed identity"));
        }
        Ok(())
    }

    /// Read exact received event IDs in bounded SQL batches without per-event queries.
    /// Missing IDs are omitted; missing source locators remain explicitly absent.
    ///
    /// # Errors
    /// Returns an error if the query fails or persisted received facts cannot be decoded.
    pub async fn remote_notification_presentations(
        &self,
        admission_ids: &[String],
    ) -> DbResult<RemoteNotificationPresentations> {
        let mut presentations = HashMap::new();
        for ids in admission_ids.chunks(400) {
            let mut query = QueryBuilder::<Sqlite>::new("SELECT a.*, s.product_id, f.event_id, f.source_transcript_id AS fact_transcript, f.outcome, f.occurred_at_us, g.source_transcript_id AS gap_transcript, g.current_outcome, n.stable_path, n.source_path FROM federation_remote_admissions a JOIN federation_remote_subscriptions s USING(destination_instance_id,subscription_id) LEFT JOIN federation_received_facts f USING(admission_id) LEFT JOIN federation_received_gaps g USING(admission_id) LEFT JOIN federation_remote_navigation n USING(admission_id) WHERE a.admission_id IN (");
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");
            for row in query.build().fetch_all(&self.pool).await? {
                let notification = decode_notification(&row)?;
                let locator = row
                    .try_get::<Option<String>, _>("stable_path")?
                    .zip(row.try_get::<Option<String>, _>("source_path")?);
                presentations.insert(notification.admission_id.clone(), (notification, locator));
            }
        }
        Ok(presentations)
    }
}

pub(super) fn encoded_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}
