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
        for value in [
            "http://peer.example",
            "https://user@peer.example",
            "https://peer.example/path",
            "https://peer.example/?query=yes",
            "https://peer.example/#fragment",
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
