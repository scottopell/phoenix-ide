use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Opaque durable identifier for a Git repository authority row.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GitRepositoryId(String);

impl GitRepositoryId {
    /// # Errors
    /// Returns [`GitRepositoryIdError`] when the supplied identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, GitRepositoryIdError> {
        let value = value.into();
        if value.trim().is_empty() {
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
    fn parse_rejects_empty_or_blank() {
        assert_eq!(GitRepositoryId::parse(""), Err(GitRepositoryIdError::Empty));
        assert_eq!(
            GitRepositoryId::parse("  \t"),
            Err(GitRepositoryIdError::Empty)
        );
    }

    #[test]
    fn parse_accepts_opaque_non_empty_text() {
        let id = GitRepositoryId::parse("repo-opaque").unwrap();
        assert_eq!(id.as_str(), "repo-opaque");
    }
}
