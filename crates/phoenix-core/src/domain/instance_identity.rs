use rustls_pki_types::pem::PemObject as _;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum InstanceIdParseError {
    #[error("invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("instance identity must be UUIDv4")]
    NotVersionFour,
    #[error("instance identity must use the RFC 4122 variant")]
    NotRfc4122,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(uuid::Uuid);

impl InstanceId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for InstanceId {
    type Err = InstanceIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(value)?;
        if uuid.get_variant() != uuid::Variant::RFC4122 {
            return Err(InstanceIdParseError::NotRfc4122);
        }
        if uuid.get_version_num() != 4 {
            return Err(InstanceIdParseError::NotVersionFour);
        }
        Ok(Self(uuid))
    }
}

impl serde::Serialize for InstanceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for InstanceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerBaseUrl(url::Url);

#[derive(Debug, thiserror::Error)]
pub enum PeerBaseUrlError {
    #[error("invalid peer URL: {0}")]
    Invalid(#[from] url::ParseError),
    #[error("peer URL must use HTTPS")]
    NotHttps,
    #[error("peer URL must not contain credentials, query, or fragment")]
    ContainsAmbientData,
    #[error("peer URL must be an origin without a path")]
    ContainsPath,
}

impl FromStr for PeerBaseUrl {
    type Err = PeerBaseUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = url::Url::parse(value)?;
        if url.scheme() != "https" {
            return Err(PeerBaseUrlError::NotHttps);
        }
        let Some(host) = url.host_str() else {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        };
        if !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if url.port() == Some(0) {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(PeerBaseUrlError::ContainsAmbientData);
        }
        if url.path() != "/" {
            return Err(PeerBaseUrlError::ContainsPath);
        }
        Ok(Self(url))
    }
}

impl fmt::Display for PeerBaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationQueryDatabaseEndpoint(url::Url);

impl FederationQueryDatabaseEndpoint {
    #[must_use]
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }
}

impl PeerBaseUrl {
    /// Return the normalized ASCII hostname.
    ///
    /// # Panics
    /// Panics only if this value bypassed `PeerBaseUrl` construction.
    #[must_use]
    pub fn host(&self) -> &str {
        self.0
            .host_str()
            .expect("validated HTTPS origin always has a host")
    }

    /// Return the explicit or HTTPS-default port.
    ///
    /// # Panics
    /// Panics only if this value bypassed `PeerBaseUrl` construction.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.0
            .port_or_known_default()
            .expect("HTTPS always has a known default port")
    }

    /// Return the closed endpoint for the bounded remote database query operation.
    ///
    /// # Panics
    /// Panics only if the fixed compile-time route is not a valid URL path.
    #[must_use]
    pub fn query_database_endpoint(&self) -> FederationQueryDatabaseEndpoint {
        FederationQueryDatabaseEndpoint(
            self.0
                .join("/api/federation/peer/query-database")
                .expect("fixed federation route is a valid URL path"),
        )
    }

    /// Reconstruct a peer origin from normalized relational columns.
    ///
    /// # Errors
    /// Returns an error if the persisted host and port do not form a bare HTTPS origin.
    pub fn from_host_port(host: &str, port: u16) -> Result<Self, PeerBaseUrlError> {
        Self::from_str(&format!("https://{host}:{port}"))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PeerBearerCredential(String);

#[derive(Debug, thiserror::Error)]
#[error("invalid peer bearer credential")]
pub struct PeerBearerCredentialError;

impl PeerBearerCredential {
    /// Parse a receiver-issued federation bearer credential.
    ///
    /// # Errors
    /// Returns an error when the credential is outside the reserved token namespace or shape.
    pub fn parse(value: String) -> Result<Self, PeerBearerCredentialError> {
        let encoded = value
            .strip_prefix("phx_peer_")
            .ok_or(PeerBearerCredentialError)?;
        if encoded.len() != 43
            || !encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PeerBearerCredentialError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

const MAX_PEER_CA_CERTIFICATE_PEM_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerTlsTrust {
    PlatformRoots,
    PrivateCa {
        certificate_pem: PeerCaCertificatePem,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerCaCertificatePem(String);

impl PeerCaCertificatePem {
    /// Parse one bounded public CA certificate in PEM form.
    ///
    /// # Errors
    /// Returns an error when the value is empty, oversized, malformed, or contains anything other
    /// than one certificate.
    pub fn parse(value: impl Into<String>) -> Result<Self, PeerCaCertificatePemError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PeerCaCertificatePemError::Empty);
        }
        if value.len() > MAX_PEER_CA_CERTIFICATE_PEM_BYTES {
            return Err(PeerCaCertificatePemError::TooLarge);
        }
        let trimmed = value.trim();
        let Some(after_begin) = trimmed.strip_prefix("-----BEGIN CERTIFICATE-----") else {
            return Err(PeerCaCertificatePemError::CertificateCount);
        };
        if !trimmed.ends_with("-----END CERTIFICATE-----") || after_begin.contains("-----BEGIN ") {
            return Err(PeerCaCertificatePemError::CertificateCount);
        }
        let certificates = rustls_pki_types::CertificateDer::pem_slice_iter(trimmed.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PeerCaCertificatePemError::Invalid)?;
        if certificates.len() != 1 {
            return Err(PeerCaCertificatePemError::CertificateCount);
        }
        let [certificate]: [_; 1] = certificates
            .try_into()
            .map_err(|_| PeerCaCertificatePemError::CertificateCount)?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(certificate)
            .map_err(|_| PeerCaCertificatePemError::Invalid)?;
        Ok(Self(format!("{trimmed}\n")))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl serde::Serialize for PeerCaCertificatePem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> serde::Deserialize<'de> for PeerCaCertificatePem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PeerCaCertificatePemError {
    #[error("peer CA certificate PEM must not be empty")]
    Empty,
    #[error("peer CA certificate PEM exceeds 32 KiB")]
    TooLarge,
    #[error("peer CA certificate PEM is invalid")]
    Invalid,
    #[error("peer CA certificate PEM must contain exactly one certificate")]
    CertificateCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationCredentialVerifier([u8; 32]);

impl FederationCredentialVerifier {
    #[must_use]
    pub fn from_bearer(bearer: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self(Sha256::digest(bearer).into())
    }
}

impl fmt::Display for FederationCredentialVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_base_url_requires_a_bare_https_origin() {
        assert!(PeerBaseUrl::from_str("https://peer.example").is_ok());
        let endpoint = PeerBaseUrl::from_str("https://peer.example:8443")
            .unwrap()
            .query_database_endpoint();
        assert_eq!(
            endpoint.as_url().as_str(),
            "https://peer.example:8443/api/federation/peer/query-database"
        );
        for value in [
            "http://peer.example",
            "https://user@peer.example",
            "https://peer.example/path",
            "https://peer.example/?query=yes",
            "https://peer.example/#fragment",
            "https://peer.example:0",
        ] {
            assert!(PeerBaseUrl::from_str(value).is_err(), "{value}");
        }
    }

    #[test]
    fn peer_bearer_credential_requires_issued_shape() {
        assert!(PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).is_ok());
        for value in [
            "owner-password".to_string(),
            "phx_peer_short".to_string(),
            format!("phx_peer_{}=", "a".repeat(42)),
        ] {
            assert!(PeerBearerCredential::parse(value).is_err());
        }
    }

    #[test]
    fn peer_private_ca_requires_one_bounded_certificate() {
        let temp = tempfile::tempdir().unwrap();
        let paths = phoenix_tls::ensure_ca(temp.path()).unwrap();
        let certificate = std::fs::read_to_string(paths.cert_path).unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(&certificate).unwrap().expose(),
            certificate
        );
        assert_eq!(
            PeerCaCertificatePem::parse(" ").unwrap_err(),
            PeerCaCertificatePemError::Empty
        );
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}{certificate}")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        let private_key = std::fs::read_to_string(paths.key_path).unwrap();
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}{private_key}")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        assert_eq!(
            PeerCaCertificatePem::parse(format!("{certificate}not PEM")).unwrap_err(),
            PeerCaCertificatePemError::CertificateCount
        );
        assert_eq!(
            PeerCaCertificatePem::parse(
                "-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----"
            )
            .unwrap_err(),
            PeerCaCertificatePemError::Invalid
        );
    }

    #[test]
    fn instance_identity_rejects_non_v4_uuids() {
        for value in [
            "d9428888-122b-11e1-b85c-61cd3cbb3210",
            "21f7f8de-8051-5b89-8680-0195ef798b6a",
        ] {
            assert!(matches!(
                InstanceId::from_str(value),
                Err(InstanceIdParseError::NotVersionFour)
            ));
        }
        for value in [
            "00000000-0000-0000-0000-000000000000",
            "00000000-0000-4000-0000-000000000000",
        ] {
            assert!(matches!(
                InstanceId::from_str(value),
                Err(InstanceIdParseError::NotRfc4122)
            ));
        }
        assert!(InstanceId::from_str(&InstanceId::new().to_string()).is_ok());
    }
}
