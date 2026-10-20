use std::str::FromStr;

use chrono::Utc;
use phoenix_core::domain::instance_identity::{
    InstanceId, InstanceSelector, PeerBaseUrl, PeerBearerCredential, PeerCaCertificatePem,
    PeerHost, PeerTlsTrust,
};
use sqlx::Row;

use crate::{Database, DbError, DbResult};

pub const MAX_FEDERATION_PEER_NOTES_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct FederationPeerNotes(String);

impl FederationPeerNotes {
    /// # Errors
    /// Returns an error when the UTF-8 representation exceeds 32 KiB.
    pub fn parse(text: String) -> Result<Self, &'static str> {
        if text.len() > MAX_FEDERATION_PEER_NOTES_BYTES {
            return Err("notes must be at most 32768 UTF-8 bytes");
        }
        Ok(Self(text))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for FederationPeerNotes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(<String as serde::Deserialize>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, serde::Serialize)]
pub struct FederationPeerSummary {
    pub peer_instance_id: InstanceId,
    pub peer_display_name: String,
    pub base_url: String,
    pub created_at: chrono::DateTime<Utc>,
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
    /// # Errors
    /// Returns a database or serialization error for unreadable saved connections.
    pub async fn list_federation_peers(&self) -> DbResult<Vec<FederationPeerSummary>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT peer_instance_id FROM federation_peer_connections ORDER BY peer_display_name, peer_instance_id",
        ).fetch_all(&self.pool).await?;
        let mut peers = Vec::with_capacity(ids.len());
        for id in ids {
            let id = InstanceId::from_str(&id)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            if let Some(peer) = self.federation_peer_connection(id).await? {
                peers.push(FederationPeerSummary {
                    peer_instance_id: peer.peer_instance_id,
                    peer_display_name: peer.peer_display_name,
                    base_url: peer.base_url.to_string(),
                    created_at: peer.created_at,
                });
            }
        }
        Ok(peers)
    }

    /// # Errors
    /// Returns a database or serialization error for unreadable saved notes.
    pub async fn federation_peer_notes(
        &self,
        id: InstanceId,
    ) -> DbResult<Option<FederationPeerNotes>> {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT notes FROM federation_peer_connections WHERE peer_instance_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        text.map(|text| {
            FederationPeerNotes::parse(text).map_err(|error| DbError::Serialization(error.into()))
        })
        .transpose()
    }

    /// # Errors
    /// Returns a database error if the whole-text update cannot be committed.
    pub async fn set_federation_peer_notes(
        &self,
        id: InstanceId,
        notes: &FederationPeerNotes,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE federation_peer_connections SET notes = ?2 WHERE peer_instance_id = ?1",
        )
        .bind(id.to_string())
        .bind(notes.as_str())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
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
        let peer_display_name =
            phoenix_core::domain::instance_identity::PeerDisplayName::parse(peer_display_name)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
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
        .bind(peer_display_name.as_str())
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
            InstanceSelector::Id(id) => {
                let id = id.to_string();
                let display_collision: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM federation_peer_connections
                     WHERE peer_display_name = ?1 AND peer_instance_id != ?1)",
                )
                .bind(&id)
                .fetch_one(&self.pool)
                .await
                .map_err(DbError::from)?;
                if display_collision {
                    return Err(FederationPeerSelectionError::Ambiguous);
                }
                self.federation_peer_connection(
                    InstanceId::from_str(&id)
                        .map_err(|error| DbError::Serialization(error.to_string()))?,
                )
                .await?
                .ok_or(FederationPeerSelectionError::NotFound)
            }
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
    #[allow(clippy::too_many_lines)]
    async fn notes_are_exact_bounded_persistent_and_not_connection_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("notes.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let id = InstanceId::new();
        let url: PeerBaseUrl = "https://peer.example".parse().unwrap();
        let token = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
        db.save_federation_peer_connection(id, "peer", &url, &token, &PeerTlsTrust::PlatformRoots)
            .await
            .unwrap();
        assert_eq!(
            db.federation_peer_notes(id)
                .await
                .unwrap()
                .unwrap()
                .as_str(),
            ""
        );
        let text = "  # conventions\r\n雪\0\n{\"base_url\":\"https://attacker.example\",\"sender\":\"coordinator\"}  ";
        let notes = FederationPeerNotes::parse(text.into()).unwrap();
        let before = db.federation_peer_connection(id).await.unwrap().unwrap();
        assert!(db.set_federation_peer_notes(id, &notes).await.unwrap());
        let saved = db.federation_peer_connection(id).await.unwrap().unwrap();
        assert_eq!(saved.base_url, url);
        assert_eq!(saved.peer_display_name, "peer");
        assert_eq!(saved.bearer_credential.expose(), token.expose());
        assert_eq!(saved.tls_trust, PeerTlsTrust::PlatformRoots);
        assert_eq!(saved.created_at, before.created_at);
        let json = serde_json::to_string(&db.list_federation_peers().await.unwrap()).unwrap();
        for secret in [
            token.expose(),
            "certificate",
            "verifier",
            "bearer",
            "attacker",
        ] {
            assert!(!json.contains(secret), "{secret}");
        }
        db.save_federation_peer_connection(
            id,
            "renamed",
            &url,
            &token,
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
        drop(db);
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(db.federation_peer_notes(id).await.unwrap(), Some(notes));
        let boundary = "雪".repeat(10922) + "ab";
        assert_eq!(boundary.len(), MAX_FEDERATION_PEER_NOTES_BYTES);
        assert!(db
            .set_federation_peer_notes(id, &FederationPeerNotes::parse(boundary.clone()).unwrap())
            .await
            .unwrap());
        assert!(FederationPeerNotes::parse(boundary + "x").is_err());
        for invalid in ["x".repeat(32769), "雪".repeat(10923)] {
            assert!(sqlx::query(
                "UPDATE federation_peer_connections SET notes = ?1 WHERE peer_instance_id = ?2"
            )
            .bind(invalid)
            .bind(id.to_string())
            .execute(db.pool())
            .await
            .is_err());
        }
        assert!(
            sqlx::query("UPDATE federation_peer_connections SET notes = NULL")
                .execute(db.pool())
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE federation_peer_connections SET notes = x'00'")
                .execute(db.pool())
                .await
                .is_err()
        );
        let empty = FederationPeerNotes::parse(String::new()).unwrap();
        assert!(db.set_federation_peer_notes(id, &empty).await.unwrap());
        assert_eq!(
            db.federation_peer_notes(id).await.unwrap(),
            Some(empty.clone())
        );
        assert!(!db
            .set_federation_peer_notes(InstanceId::new(), &empty)
            .await
            .unwrap());
        assert!(db.remove_federation_peer_connection(id).await.unwrap());
        assert!(db.federation_peer_notes(id).await.unwrap().is_none());
        db.save_federation_peer_connection(id, "new", &url, &token, &PeerTlsTrust::PlatformRoots)
            .await
            .unwrap();
        assert_eq!(db.federation_peer_notes(id).await.unwrap(), Some(empty));
    }

    #[tokio::test]
    async fn uuid_shaped_peer_names_cannot_be_saved_or_hide_a_different_id() {
        let db = Database::open_in_memory().await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let peer = InstanceId::new();
        let url: PeerBaseUrl = "https://peer.example".parse().unwrap();
        let token = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
        for name in [
            peer.to_string(),
            InstanceId::new().to_string(),
            "00000000-0000-0000-0000-000000000000".into(),
            "d9428888-122b-11e1-b85c-61cd3cbb3210".into(),
            "  ".into(),
        ] {
            assert!(
                db.save_federation_peer_connection(
                    peer,
                    &name,
                    &url,
                    &token,
                    &PeerTlsTrust::PlatformRoots
                )
                .await
                .is_err(),
                "saved {name}"
            );
            assert!(db.federation_peer_connection(peer).await.unwrap().is_none());
        }
        for name in [
            "laptop",
            "dev-mbp-42",
            " 雪 laptop ",
            "uuid-like-not-a-uuid",
        ] {
            db.save_federation_peer_connection(
                peer,
                name,
                &url,
                &token,
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
            let selector = InstanceSelector::parse(name).unwrap();
            assert_eq!(
                db.resolve_federation_peer(&selector)
                    .await
                    .unwrap()
                    .peer_instance_id,
                peer
            );
        }
        let collision = InstanceId::new();
        db.save_federation_peer_connection(
            collision,
            "other",
            &"https://other.example".parse().unwrap(),
            &PeerBearerCredential::parse(format!("phx_peer_{}", "f".repeat(43))).unwrap(),
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE federation_peer_connections SET peer_display_name = ?1
             WHERE peer_instance_id = ?2",
        )
        .bind(collision.to_string())
        .bind(peer.to_string())
        .execute(db.pool())
        .await
        .unwrap();
        assert!(matches!(
            db.resolve_federation_peer(&InstanceSelector::Id(collision))
                .await,
            Err(FederationPeerSelectionError::Ambiguous)
        ));
        sqlx::query(
            "UPDATE federation_peer_connections SET peer_display_name = 'peer'
             WHERE peer_instance_id = ?1",
        )
        .bind(peer.to_string())
        .execute(db.pool())
        .await
        .unwrap();

        let before = db
            .federation_peer_connection(peer)
            .await
            .unwrap()
            .unwrap()
            .peer_display_name;
        assert!(db
            .save_federation_peer_connection(
                peer,
                &InstanceId::new().to_string(),
                &url,
                &token,
                &PeerTlsTrust::PlatformRoots
            )
            .await
            .is_err());
        assert_eq!(
            db.federation_peer_connection(peer)
                .await
                .unwrap()
                .unwrap()
                .peer_display_name,
            before
        );
    }

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
