use std::str::FromStr;

use chrono::Utc;
use phoenix_core::domain::instance_identity::{
    InstanceId, PeerBaseUrl, PeerBearerCredential, PeerCaCertificatePem, PeerTlsTrust,
};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

pub struct FederationPeerConnection {
    pub peer_instance_id: InstanceId,
    pub peer_display_name: String,
    pub base_url: PeerBaseUrl,
    pub bearer_credential: PeerBearerCredential,
    pub tls_trust: PeerTlsTrust,
    pub created_at: chrono::DateTime<Utc>,
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
        tls_trust: &PeerTlsTrust,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = Utc::now().timestamp_micros();
        let tls_ca_certificate_pem = match tls_trust {
            PeerTlsTrust::PlatformRoots => None,
            PeerTlsTrust::PrivateCa { certificate_pem } => Some(certificate_pem.expose()),
        };
        sqlx::query(
            "INSERT INTO federation_peer_connections
                 (peer_instance_id, peer_display_name, host, port, bearer_credential,
                  tls_ca_certificate_pem, created_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(peer_instance_id) DO UPDATE SET
                 peer_display_name = excluded.peer_display_name,
                 host = excluded.host,
                 port = excluded.port,
                 bearer_credential = excluded.bearer_credential,
                 tls_ca_certificate_pem = excluded.tls_ca_certificate_pem,
                 created_at_us = excluded.created_at_us",
        )
        .bind(peer_instance_id.to_string())
        .bind(peer_display_name)
        .bind(base_url.host())
        .bind(i64::from(base_url.port()))
        .bind(bearer_credential.expose())
        .bind(tls_ca_certificate_pem)
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
            "SELECT peer_instance_id, peer_display_name, host, port, bearer_credential,
                    tls_ca_certificate_pem, created_at_us
             FROM federation_peer_connections WHERE peer_instance_id = ?1",
        )
        .bind(peer_instance_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let id: String = row.try_get("peer_instance_id")?;
            let host: String = row.try_get("host")?;
            let port: i64 = row.try_get("port")?;
            let bearer: String = row.try_get("bearer_credential")?;
            let ca_certificate: Option<String> = row.try_get("tls_ca_certificate_pem")?;
            let created_at_us: i64 = row.try_get("created_at_us")?;
            Ok(FederationPeerConnection {
                peer_instance_id: InstanceId::from_str(&id)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                peer_display_name: row.try_get("peer_display_name")?,
                base_url: PeerBaseUrl::from_host_port(
                    &host,
                    u16::try_from(port)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?,
                bearer_credential: PeerBearerCredential::parse(bearer)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                tls_trust: match ca_certificate {
                    Some(certificate) => PeerTlsTrust::PrivateCa {
                        certificate_pem: PeerCaCertificatePem::parse(certificate)
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                    },
                    None => PeerTlsTrust::PlatformRoots,
                },
                created_at: chrono::DateTime::from_timestamp_micros(created_at_us).ok_or_else(
                    || DbError::Serialization("peer creation timestamp is out of range".into()),
                )?,
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
    async fn schema_rejects_malformed_peer_identity_and_origins() {
        let db = Database::open_in_memory().await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let bearer = format!("phx_peer_{}", "a".repeat(43));
        for id in [
            "not-a-uuid",
            "00000000-0000-0000-0000-000000000000",
            "21f7f8de-8051-5b89-8680-0195ef798b6a",
            "00000000-0000-4000-0000-000000000000",
        ] {
            assert!(sqlx::query(
                "INSERT INTO federation_peer_connections
                     (peer_instance_id, peer_display_name, host, port, bearer_credential, created_at_us)
                 VALUES (?1, 'peer', 'peer.example', 443, ?2, 1)",
            )
            .bind(id)
            .bind(&bearer)
            .execute(db.pool())
            .await
            .is_err(), "{id}");
        }
        for host in ["", "[", "%2F", "peer example", "peer.example\n"] {
            assert!(sqlx::query(
                "INSERT INTO federation_peer_connections
                     (peer_instance_id, peer_display_name, host, port, bearer_credential, created_at_us)
                 VALUES (?1, 'peer', ?2, 443, ?3, 1)",
            )
            .bind(InstanceId::new().to_string())
            .bind(host)
            .bind(&bearer)
            .execute(db.pool())
            .await
            .is_err(), "{host:?}");
        }
    }

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

        let temp = tempfile::tempdir().unwrap();
        let ca_paths = phoenix_tls::ensure_ca(temp.path()).unwrap();
        let private_ca = PeerTlsTrust::PrivateCa {
            certificate_pem: PeerCaCertificatePem::parse(
                std::fs::read_to_string(ca_paths.cert_path).unwrap(),
            )
            .unwrap(),
        };
        db.save_federation_peer_connection(
            peer,
            "peer",
            &first_url,
            &first_token,
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
        db.save_federation_peer_connection(
            peer,
            "renamed",
            &second_url,
            &second_token,
            &private_ca,
        )
        .await
        .unwrap();
        let saved = db.federation_peer_connection(peer).await.unwrap().unwrap();
        assert_eq!(saved.peer_instance_id, peer);
        assert_eq!(saved.peer_display_name, "renamed");
        assert_eq!(saved.base_url, second_url);
        assert_eq!(saved.bearer_credential.expose(), second_token.expose());
        assert_eq!(saved.tls_trust, private_ca);
        assert!(saved.created_at.timestamp_micros() >= 0);
        assert!(db.remove_federation_peer_connection(peer).await.unwrap());
        assert!(db.federation_peer_connection(peer).await.unwrap().is_none());
    }
}
