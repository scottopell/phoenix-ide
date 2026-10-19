use std::collections::HashMap;

use sqlx::QueryBuilder;

use super::{consumer::decode_notification, *};

/// Durable received facts paired with optional source-issued navigation paths.
pub type RemoteNotificationPresentations =
    HashMap<String, (RemoteNotification, Option<(String, String)>)>;

impl Database {
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
