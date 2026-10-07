use std::fmt;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum InstanceIdParseError {
    #[error("invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("instance identity must be UUIDv4")]
    NotVersionFour,
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
    /// Resolve one fixed federation API path against this peer origin.
    ///
    /// # Errors
    /// Returns a URL parse error if the supplied path is not a valid relative reference.
    pub fn endpoint(&self, path: &str) -> Result<url::Url, url::ParseError> {
        self.0.join(path)
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
            "00000000-0000-0000-0000-000000000000",
            "d9428888-122b-11e1-b85c-61cd3cbb3210",
            "21f7f8de-8051-5b89-8680-0195ef798b6a",
        ] {
            assert!(matches!(
                InstanceId::from_str(value),
                Err(InstanceIdParseError::NotVersionFour)
            ));
        }
        assert!(InstanceId::from_str(&InstanceId::new().to_string()).is_ok());
    }
}
