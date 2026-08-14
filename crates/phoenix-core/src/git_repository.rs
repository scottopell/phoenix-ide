use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Opaque durable identifier for a Git repository authority row.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GitRepositoryId(String);

impl GitRepositoryId {
    /// # Errors
    /// Returns [`GitRepositoryIdError`] when the supplied identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, GitRepositoryIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(GitRepositoryIdError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GitRepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for GitRepositoryId {
    type Err = GitRepositoryIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for GitRepositoryId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRepositoryIdError {
    Empty,
}

impl fmt::Display for GitRepositoryIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("git repository id must not be empty"),
        }
    }
}

impl std::error::Error for GitRepositoryIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_only_empty_text() {
        assert_eq!(GitRepositoryId::parse(""), Err(GitRepositoryIdError::Empty));
    }

    #[test]
    fn parse_preserves_whitespace_only_legacy_identity() {
        let value = "  \t";
        let id = GitRepositoryId::parse(value).unwrap();
        assert_eq!(id.as_str().as_bytes(), value.as_bytes());
    }

    #[test]
    fn parse_accepts_opaque_non_empty_text() {
        let id = GitRepositoryId::parse("repo-opaque").unwrap();
        assert_eq!(id.as_str(), "repo-opaque");
    }

    #[test]
    fn parse_preserves_legacy_project_identity_bytes_exactly() {
        let id = GitRepositoryId::parse("  repo-opaque  ").unwrap();
        assert_eq!(id.as_str(), "  repo-opaque  ");
    }

    #[test]
    fn deserialize_rejects_empty_json_string() {
        let result: Result<GitRepositoryId, _> = serde_json::from_str(r#""""#);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_preserves_whitespace_only_json_string() {
        let id: GitRepositoryId = serde_json::from_str(r#""  \t  ""#).unwrap();
        assert_eq!(id.as_str().as_bytes(), b"  \t  ");
    }

    #[test]
    fn serde_roundtrip_matches_string_bytes_exactly() {
        let id = GitRepositoryId::parse("  repo-opaque  ").unwrap();

        let id_bytes = serde_json::to_vec(&id).unwrap();
        let string_bytes = serde_json::to_vec(id.as_str()).unwrap();
        assert_eq!(id_bytes, string_bytes);

        let decoded: GitRepositoryId = serde_json::from_slice(&id_bytes).unwrap();
        assert_eq!(decoded, id);
    }
}
