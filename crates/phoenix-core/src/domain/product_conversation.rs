use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ProductConversationId(String);

impl<'de> Deserialize<'de> for ProductConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl ProductConversationId {
    /// # Errors
    /// Returns [`ProductConversationIdError`] when the supplied identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProductConversationIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProductConversationIdError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProductConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProductConversationId {
    type Err = ProductConversationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationIdError {
    Empty,
}

impl fmt::Display for ProductConversationIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("product conversation id cannot be empty"),
        }
    }
}

impl std::error::Error for ProductConversationIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrdinaryProductConversationLifecycle {
    Open,
    History,
}

impl OrdinaryProductConversationLifecycle {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::History => "history",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "open" => Self::Open,
            "history" => Self::History,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProductConversation {
    Ordinary {
        id: ProductConversationId,
        lifecycle: OrdinaryProductConversationLifecycle,
    },
    Coordinator {
        id: ProductConversationId,
    },
}

impl ProductConversation {
    #[must_use]
    pub fn ordinary(
        id: ProductConversationId,
        lifecycle: OrdinaryProductConversationLifecycle,
    ) -> Self {
        Self::Ordinary { id, lifecycle }
    }

    #[must_use]
    pub fn coordinator(id: ProductConversationId) -> Self {
        Self::Coordinator { id }
    }

    #[must_use]
    pub fn id(&self) -> &ProductConversationId {
        match self {
            Self::Ordinary { id, .. } | Self::Coordinator { id } => id,
        }
    }

    #[must_use]
    pub fn kind(&self) -> ProductConversationKind {
        match self {
            Self::Ordinary { .. } => ProductConversationKind::Ordinary,
            Self::Coordinator { .. } => ProductConversationKind::Coordinator,
        }
    }

    #[must_use]
    pub fn ordinary_lifecycle(&self) -> Option<OrdinaryProductConversationLifecycle> {
        match self {
            Self::Ordinary { lifecycle, .. } => Some(*lifecycle),
            Self::Coordinator { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductConversationKind {
    Ordinary,
    Coordinator,
}

impl ProductConversationKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Coordinator => "coordinator",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "ordinary" => Self::Ordinary,
            "coordinator" => Self::Coordinator,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_conversation_id_rejects_empty() {
        assert_eq!(
            ProductConversationId::parse(""),
            Err(ProductConversationIdError::Empty)
        );
        assert_eq!(
            ProductConversationId::parse(" \t"),
            Err(ProductConversationIdError::Empty)
        );
        assert!(serde_json::from_str::<ProductConversationId>(r#""""#).is_err());
    }

    #[test]
    fn product_conversation_id_round_trips() {
        let id = ProductConversationId::parse("root-1").unwrap();
        assert_eq!(id.as_str(), "root-1");
        assert_eq!(id.to_string(), "root-1");
        assert_eq!(ProductConversationId::from_str("root-1").unwrap(), id);
    }

    #[test]
    fn coordinator_has_no_ordinary_lifecycle() {
        let coordinator =
            ProductConversation::coordinator(ProductConversationId::parse("pc-1").unwrap());
        assert_eq!(coordinator.kind(), ProductConversationKind::Coordinator);
        assert_eq!(coordinator.ordinary_lifecycle(), None);
    }

    #[test]
    fn ordinary_keeps_lifecycle_structurally() {
        let ordinary = ProductConversation::ordinary(
            ProductConversationId::parse("pc-2").unwrap(),
            OrdinaryProductConversationLifecycle::Open,
        );
        assert_eq!(ordinary.kind(), ProductConversationKind::Ordinary);
        assert_eq!(
            ordinary.ordinary_lifecycle(),
            Some(OrdinaryProductConversationLifecycle::Open)
        );
    }
}
