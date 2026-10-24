use std::str::FromStr;

use chrono::Utc;
use phoenix_core::domain::instance_identity::{
    InstanceId, InstanceSelector, PeerBaseUrl, PeerBearerCredential, PeerCaCertificatePem,
    PeerHost, PeerTlsTrust,
};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

#[derive(Debug, serde::Serialize)]
pub struct FederationPeerSummary {
    pub peer_instance_id: InstanceId,
    pub peer_display_name: String,
    #[serde(serialize_with = "serialize_peer_base_url")]
    pub base_url: PeerBaseUrl,
    pub created_at: chrono::DateTime<Utc>,
}

fn serialize_peer_base_url<S: serde::Serializer>(
    url: &PeerBaseUrl,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(url)
}

pub struct FederationPeerConnection {
    pub peer_instance_id: InstanceId,
    pub peer_display_name: String,
    pub base_url: PeerBaseUrl,
    pub bearer_credential: PeerBearerCredential,
    pub tls_trust: PeerTlsTrust,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum FederationPeerSelectionError {
    #[error("peer not found")]
    NotFound,
    #[error("peer display name is ambiguous")]
    Ambiguous,
    #[error(transparent)]
    Database(#[from] DbError),
}

impl Database {
    /// List advertised peer metadata without reading connection credentials.
    ///
    /// # Errors
    /// Returns an error for invalid stored metadata or a failed query.
    pub async fn federation_peer_summaries(&self) -> DbResult<Vec<FederationPeerSummary>> {
        let rows = sqlx::query("SELECT peer_instance_id, peer_display_name, domain_or_ipv4_host, ipv6_host, port, created_at_us FROM federation_peer_connections ORDER BY peer_instance_id")
            .fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let domain: Option<String> = row.try_get("domain_or_ipv4_host")?;
                let address: Option<Vec<u8>> = row.try_get("ipv6_host")?;
                let host = match (domain.as_deref(), address) {
                    (Some(domain), None) => PeerHost::DomainOrIpv4(domain),
                    (None, Some(bytes)) => PeerHost::Ipv6(std::net::Ipv6Addr::from(
                        <[u8; 16]>::try_from(bytes)
                            .map_err(|_| DbError::Serialization("invalid IPv6 peer host".into()))?,
                    )),
                    _ => return Err(DbError::Serialization("invalid peer host shape".into())),
                };
                let port: i64 = row.try_get("port")?;
                Ok(FederationPeerSummary {
                    peer_instance_id: row
                        .try_get::<String, _>("peer_instance_id")?
                        .parse()
                        .map_err(
                        |error: phoenix_core::domain::instance_identity::InstanceIdParseError| {
                            DbError::Serialization(error.to_string())
                        },
                    )?,
                    peer_display_name: row.try_get("peer_display_name")?,
                    base_url: PeerBaseUrl::from_host_port(
                        host,
                        u16::try_from(port)
                            .map_err(|error| DbError::Serialization(error.to_string()))?,
                    )
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                    created_at: chrono::DateTime::from_timestamp_micros(
                        row.try_get("created_at_us")?,
                    )
                    .ok_or_else(|| DbError::Serialization("invalid peer timestamp".into()))?,
                })
            })
            .collect()
    }

    /// Read the opaque owner note for an existing peer.
    ///
    /// # Errors
    /// Returns a database error when the read fails.
    pub async fn federation_peer_notes(&self, peer: InstanceId) -> DbResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT notes FROM federation_peer_connections WHERE peer_instance_id = ?1",
        )
        .bind(peer.to_string())
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Replace the opaque owner note; returns false for an unknown peer.
    ///
    /// # Errors
    /// Rejects notes over 32768 UTF-8 bytes and failed writes.
    pub async fn set_federation_peer_notes(&self, peer: InstanceId, notes: &str) -> DbResult<bool> {
        if notes.len() > 32768 {
            return Err(DbError::Serialization(
                "peer notes exceed 32768 UTF-8 bytes".into(),
            ));
        }
        Ok(sqlx::query(
            "UPDATE federation_peer_connections SET notes = ?2 WHERE peer_instance_id = ?1",
        )
        .bind(peer.to_string())
        .bind(notes)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

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
        let (domain_or_ipv4_host, ipv6_host) = match base_url.host() {
            PeerHost::DomainOrIpv4(host) => (Some(host.to_owned()), None),
            PeerHost::Ipv6(address) => (None, Some(address.octets().to_vec())),
        };
        sqlx::query(
            "INSERT INTO federation_peer_connections
                 (peer_instance_id, peer_display_name, domain_or_ipv4_host, ipv6_host, port,
                  bearer_credential, tls_ca_certificate_pem, created_at_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(peer_instance_id) DO UPDATE SET
                 peer_display_name = excluded.peer_display_name,
                 domain_or_ipv4_host = excluded.domain_or_ipv4_host,
                 ipv6_host = excluded.ipv6_host,
                 port = excluded.port,
                 bearer_credential = excluded.bearer_credential,
                 tls_ca_certificate_pem = excluded.tls_ca_certificate_pem,
                 created_at_us = excluded.created_at_us",
        )
        .bind(peer_instance_id.to_string())
        .bind(peer_display_name)
        .bind(domain_or_ipv4_host)
        .bind(ipv6_host)
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
            "SELECT peer_instance_id, peer_display_name, domain_or_ipv4_host, ipv6_host, port,
                    bearer_credential, tls_ca_certificate_pem, created_at_us
             FROM federation_peer_connections WHERE peer_instance_id = ?1",
        )
        .bind(peer_instance_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let id: String = row.try_get("peer_instance_id")?;
            let domain_or_ipv4_host: Option<String> = row.try_get("domain_or_ipv4_host")?;
            let ipv6_host: Option<Vec<u8>> = row.try_get("ipv6_host")?;
            let port: i64 = row.try_get("port")?;
            let bearer: String = row.try_get("bearer_credential")?;
            let ca_certificate: Option<String> = row.try_get("tls_ca_certificate_pem")?;
            let created_at_us: i64 = row.try_get("created_at_us")?;
            let ipv6_address = ipv6_host
                .map(|bytes| {
                    <[u8; 16]>::try_from(bytes)
                        .map(std::net::Ipv6Addr::from)
                        .map_err(|_| {
                            DbError::Serialization(
                                "peer IPv6 host must contain exactly 16 bytes".to_string(),
                            )
                        })
                })
                .transpose()?;
            let typed_host = match (domain_or_ipv4_host.as_deref(), ipv6_address) {
                (Some(host), None) => PeerHost::DomainOrIpv4(host),
                (None, Some(address)) => PeerHost::Ipv6(address),
                _ => {
                    return Err(DbError::Serialization(
                        "peer connection must contain exactly one typed host".to_string(),
                    ));
                }
            };
            let base_url = PeerBaseUrl::from_host_port(
                typed_host,
                u16::try_from(port).map_err(|error| DbError::Serialization(error.to_string()))?,
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            Ok(FederationPeerConnection {
                peer_instance_id: InstanceId::from_str(&id)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
                peer_display_name: row.try_get("peer_display_name")?,
                base_url,
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

    /// Resolve one exact peer identity or one unambiguous display name.
    ///
    /// # Errors
    /// Returns a database error for malformed persisted data, and a typed selection error when the
    /// selector is missing or ambiguous.
    pub async fn resolve_federation_peer(
        &self,
        selector: &InstanceSelector,
    ) -> Result<FederationPeerConnection, FederationPeerSelectionError> {
        match selector {
            InstanceSelector::Id(id) => self
                .federation_peer_connection(*id)
                .await?
                .ok_or(FederationPeerSelectionError::NotFound),
            InstanceSelector::DisplayName(name) => {
                let ids: Vec<String> = sqlx::query_scalar(
                    "SELECT peer_instance_id FROM federation_peer_connections
                     WHERE peer_display_name = ?1 ORDER BY peer_instance_id LIMIT 2",
                )
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .map_err(DbError::from)?;
                match ids.as_slice() {
                    [] => Err(FederationPeerSelectionError::NotFound),
                    [id] => self
                        .federation_peer_connection(
                            InstanceId::from_str(id)
                                .map_err(|error| DbError::Serialization(error.to_string()))?,
                        )
                        .await?
                        .ok_or(FederationPeerSelectionError::NotFound),
                    [_, _, ..] => Err(FederationPeerSelectionError::Ambiguous),
                }
            }
        }
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
    async fn peer_selector_requires_exact_id_or_unambiguous_display_name() {
        let db = Database::open_in_memory().await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let first = InstanceId::new();
        let second = InstanceId::new();
        for (id, url, token) in [
            (
                first,
                PeerBaseUrl::from_str("https://first.peer.example").unwrap(),
                PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap(),
            ),
            (
                second,
                PeerBaseUrl::from_str("https://second.peer.example").unwrap(),
                PeerBearerCredential::parse(format!("phx_peer_{}", "b".repeat(43))).unwrap(),
            ),
        ] {
            db.save_federation_peer_connection(
                id,
                "duplicate",
                &url,
                &token,
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
        }

        assert_eq!(
            db.resolve_federation_peer(&InstanceSelector::Id(first))
                .await
                .unwrap()
                .peer_instance_id,
            first
        );
        assert!(matches!(
            db.resolve_federation_peer(&InstanceSelector::DisplayName("missing".to_string()))
                .await,
            Err(FederationPeerSelectionError::NotFound)
        ));
        assert!(matches!(
            db.resolve_federation_peer(&InstanceSelector::DisplayName("duplicate".to_string()))
                .await,
            Err(FederationPeerSelectionError::Ambiguous)
        ));
    }

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
            assert!(
                sqlx::query(
                    "INSERT INTO federation_peer_connections
                     (peer_instance_id, peer_display_name, domain_or_ipv4_host, ipv6_host, port,
                      bearer_credential, created_at_us)
                 VALUES (?1, 'peer', 'peer.example', NULL, 443, ?2, 1)",
                )
                .bind(id)
                .bind(&bearer)
                .execute(db.pool())
                .await
                .is_err(),
                "{id}"
            );
        }
        for host in ["", "[", "%2F", "peer example", "peer.example\n"] {
            assert!(
                sqlx::query(
                    "INSERT INTO federation_peer_connections
                     (peer_instance_id, peer_display_name, domain_or_ipv4_host, ipv6_host, port,
                      bearer_credential, created_at_us)
                 VALUES (?1, 'peer', ?2, NULL, 443, ?3, 1)",
                )
                .bind(InstanceId::new().to_string())
                .bind(host)
                .bind(&bearer)
                .execute(db.pool())
                .await
                .is_err(),
                "{host:?}"
            );
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
        let second_url = PeerBaseUrl::from_str("https://[2001:db8::1]:8031").unwrap();
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
