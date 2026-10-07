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
