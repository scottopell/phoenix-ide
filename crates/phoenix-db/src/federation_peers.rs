use std::str::FromStr;

use chrono::Utc;
use phoenix_core::domain::instance_identity::{InstanceId, PeerBaseUrl, PeerBearerCredential};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

pub struct FederationPeerConnection {
    pub peer_instance_id: InstanceId,
    pub peer_display_name: String,
    pub base_url: PeerBaseUrl,
    pub bearer_credential: PeerBearerCredential,
}

impl Database {
    /// Persist or replace one caller-side directional peer connection.
    ///
    /// # Errors
    /// Returns a database error if the connection cannot be committed.
    pub async fn save_federation_peer_connection(
        &self,
        peer_instance_id: InstanceId,
        peer_display_name: &str,
        base_url: &PeerBaseUrl,
        bearer_credential: &PeerBearerCredential,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now().timestamp_micros();
        sqlx::query(
            "INSERT INTO federation_peer_connections
                 (peer_instance_id, peer_display_name, base_url, bearer_credential, created_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(peer_instance_id) DO UPDATE SET
                 peer_display_name = excluded.peer_display_name,
                 base_url = excluded.base_url,
                 bearer_credential = excluded.bearer_credential,
                 created_at_us = excluded.created_at_us",
        )
        .bind(peer_instance_id.to_string())
        .bind(peer_display_name)
        .bind(base_url.to_string())
        .bind(bearer_credential.expose())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Read one caller-side directional peer connection.
    ///
    /// # Errors
    /// Returns a database or serialization error if persisted data is malformed.
    pub async fn federation_peer_connection(
        &self,
        peer_instance_id: InstanceId,
    ) -> DbResult<Option<FederationPeerConnection>> {
        let row = sqlx::query(
            "SELECT peer_instance_id, peer_display_name, base_url, bearer_credential
             FROM federation_peer_connections WHERE peer_instance_id = ?1",
        )
        .bind(peer_instance_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let id: String = row.try_get("peer_instance_id")?;
            let base_url: String = row.try_get("base_url")?;
            let bearer: String = row.try_get("bearer_credential")?;
            Ok(FederationPeerConnection {
                peer_instance_id: InstanceId::from_str(&id)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                peer_display_name: row.try_get("peer_display_name")?,
                base_url: PeerBaseUrl::from_str(&base_url)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                bearer_credential: PeerBearerCredential::parse(bearer)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            })
        })
        .transpose()
    }

    /// Remove one caller-side directional peer connection.
    ///
    /// # Errors
    /// Returns a database error if removal cannot be committed.
    pub async fn remove_federation_peer_connection(
        &self,
        peer_instance_id: InstanceId,
    ) -> DbResult<bool> {
        let result =
            sqlx::query("DELETE FROM federation_peer_connections WHERE peer_instance_id = ?1")
                .bind(peer_instance_id.to_string())
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn peer_connection_round_trips_and_replaces_by_instance() {
        let db = Database::open_in_memory().await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let peer = InstanceId::new();
        let first_url = PeerBaseUrl::from_str("https://peer.example").unwrap();
        let second_url = PeerBaseUrl::from_str("https://renamed.example").unwrap();
        let first_token =
            PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
        let second_token =
            PeerBearerCredential::parse(format!("phx_peer_{}", "b".repeat(43))).unwrap();

        db.save_federation_peer_connection(peer, "peer", &first_url, &first_token)
            .await
            .unwrap();
        db.save_federation_peer_connection(peer, "renamed", &second_url, &second_token)
            .await
            .unwrap();
        let saved = db.federation_peer_connection(peer).await.unwrap().unwrap();
        assert_eq!(saved.peer_instance_id, peer);
        assert_eq!(saved.peer_display_name, "renamed");
        assert_eq!(saved.base_url, second_url);
        assert_eq!(saved.bearer_credential.expose(), second_token.expose());
        assert!(db.remove_federation_peer_connection(peer).await.unwrap());
        assert!(db.federation_peer_connection(peer).await.unwrap().is_none());
    }
}
