use serde::{Deserialize, Deserializer, Serialize};

use super::{GoalError, GoalExecution, GoalFence, GoalText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GoalRequestLimit(i64);

impl GoalRequestLimit {
    /// # Errors
    /// Rejects zero, negative values and values outside `SQLite`'s integer range.
    pub fn parse(value: i64) -> Result<Self, GoalError> {
        if value <= 0 {
            return Err(GoalError::Range);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for GoalRequestLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoalRequestId(GoalText);

impl GoalRequestId {
    /// # Errors
    /// Rejects blank logical request identities.
    pub fn parse(value: impl Into<String>) -> Result<Self, GoalError> {
        Ok(Self(GoalText::parse(value)?))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GoalRequestCause {
    Initial,
    AfterCheckpoint { predecessor: GoalRequestId },
    Summary { operation: GoalText },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalRequestPurpose {
    Normal,
    Summary,
}

impl GoalRequestCause {
    #[must_use]
    pub const fn purpose(&self) -> GoalRequestPurpose {
        match self {
            Self::Initial | Self::AfterCheckpoint { .. } => GoalRequestPurpose::Normal,
            Self::Summary { .. } => GoalRequestPurpose::Summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalRequestIntent {
    pub id: GoalRequestId,
    pub goal: GoalFence,
    pub execution: GoalExecution,
    generation: i64,
    pub cause: GoalRequestCause,
}

impl GoalRequestIntent {
    /// # Errors
    /// Rejects negative execution generations and an unobserved goal version.
    pub fn new(
        id: GoalRequestId,
        goal: GoalFence,
        execution: GoalExecution,
        generation: i64,
        cause: GoalRequestCause,
    ) -> Result<Self, GoalError> {
        if generation < 0 || goal.version.get() == 0 {
            return Err(GoalError::Range);
        }
        Ok(Self {
            id,
            goal,
            execution,
            generation,
            cause,
        })
    }

    #[must_use]
    pub const fn generation(&self) -> i64 {
        self.generation
    }
}

impl<'de> Deserialize<'de> for GoalRequestIntent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            id: GoalRequestId,
            goal: GoalFence,
            execution: GoalExecution,
            generation: i64,
            cause: GoalRequestCause,
        }
        let fields = Fields::deserialize(deserializer)?;
        Self::new(
            fields.id,
            fields.goal,
            fields.execution,
            fields.generation,
            fields.cause,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_limit_rejects_absence_encodings_and_preserves_sqlite_maximum() {
        for invalid in [i64::MIN, -1, 0] {
            assert!(GoalRequestLimit::parse(invalid).is_err());
            assert!(serde_json::from_str::<GoalRequestLimit>(&invalid.to_string()).is_err());
        }
        for valid in [1, 17, i64::MAX] {
            let limit = GoalRequestLimit::parse(valid).unwrap();
            assert_eq!(limit.get(), valid);
            assert_eq!(
                serde_json::from_str::<GoalRequestLimit>(&serde_json::to_string(&limit).unwrap())
                    .unwrap(),
                limit
            );
        }
        assert!(serde_json::from_str::<GoalRequestLimit>("9223372036854775808").is_err());
    }

    #[test]
    fn cause_structurally_determines_normal_or_summary_purpose() {
        assert_eq!(
            GoalRequestCause::Initial.purpose(),
            GoalRequestPurpose::Normal
        );
        assert_eq!(
            GoalRequestCause::AfterCheckpoint {
                predecessor: GoalRequestId::parse("first").unwrap()
            }
            .purpose(),
            GoalRequestPurpose::Normal
        );
        assert_eq!(
            GoalRequestCause::Summary {
                operation: GoalText::parse("handoff").unwrap()
            }
            .purpose(),
            GoalRequestPurpose::Summary
        );
    }
}
