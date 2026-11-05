//! One-shot reminder intent and time eligibility.
//! REQ-REM-001, REQ-REM-003, REQ-REM-004, REQ-REM-006.

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use super::product_conversation::ProductConversationId;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReminderValueError {
    #[error("reminder identifier must not be blank")]
    BlankId,
    #[error("reminder text must contain 1 through 4096 characters")]
    InvalidText,
    #[error("reminder time must be nonnegative Unix microseconds")]
    InvalidTime,
    #[error("reminder revision must be a positive SQLite integer")]
    InvalidRevision,
    #[error("reminder timezone must be an IANA timezone name")]
    InvalidTimezone,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(transparent)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderId(String);

impl ReminderId {
    /// # Errors
    /// Rejects blank identifiers.
    pub fn parse(value: impl Into<String>) -> Result<Self, ReminderValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ReminderValueError::BlankId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReminderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(transparent)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderText(String);

impl ReminderText {
    /// # Errors
    /// Rejects blank or overlong text without changing the saved message.
    pub fn parse(value: impl Into<String>) -> Result<Self, ReminderValueError> {
        let value = value.into();
        if value.trim().is_empty() || value.chars().count() > 4096 {
            return Err(ReminderValueError::InvalidText);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReminderText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(transparent)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderTimezone(String);

impl ReminderTimezone {
    /// # Errors
    /// Rejects values that do not identify an IANA timezone.
    pub fn parse(value: impl Into<String>) -> Result<Self, ReminderValueError> {
        let value = value.into();
        value
            .parse::<chrono_tz::Tz>()
            .map_err(|_| ReminderValueError::InvalidTimezone)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ReminderTimezone {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, TS)]
#[serde(transparent)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderTimestampUs(#[ts(type = "number")] i64);

impl ReminderTimestampUs {
    /// # Errors
    /// Rejects timestamps outside the supported nonnegative `SQLite` range.
    pub const fn parse(value: i64) -> Result<Self, ReminderValueError> {
        if value < 0 {
            return Err(ReminderValueError::InvalidTime);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReminderTimestampUs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(transparent)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderRevision(#[ts(type = "number")] i64);

impl ReminderRevision {
    pub const INITIAL: Self = Self(1);

    /// # Errors
    /// Rejects nonpositive revisions.
    pub const fn parse(value: i64) -> Result<Self, ReminderValueError> {
        if value < 1 {
            return Err(ReminderValueError::InvalidRevision);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ReminderRevision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReminderState {
    Scheduled,
    Due {
        occurred_at_us: ReminderTimestampUs,
    },
    Cancelled {
        cancelled_at_us: ReminderTimestampUs,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct Reminder {
    pub id: ReminderId,
    #[ts(type = "string")]
    pub product_conversation_id: ProductConversationId,
    pub text: ReminderText,
    pub due_at_us: ReminderTimestampUs,
    pub timezone: ReminderTimezone,
    pub revision: ReminderRevision,
    pub created_at_us: ReminderTimestampUs,
    pub state: ReminderState,
}

impl Reminder {
    #[must_use]
    pub fn input_message_id(&self) -> String {
        format!("reminder:{}:{}", self.id.as_str(), self.revision.get())
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderNotification {
    pub reminder: Reminder,
    pub target: ReminderNotificationTarget,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderNotificationTarget {
    pub id: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub kind: ReminderNotificationTargetKind,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReminderNotificationTargetKind {
    Ordinary,
    Coordinator,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_values_reject_invalid_deserialized_states() {
        assert!(serde_json::from_str::<ReminderTimestampUs>("-1").is_err());
        assert!(serde_json::from_str::<ReminderRevision>("0").is_err());
        assert!(serde_json::from_str::<ReminderText>("\"  \"").is_err());
        assert!(ReminderText::parse("x".repeat(4097)).is_err());
        assert!(ReminderTimezone::parse("Eastern").is_err());
        assert!(ReminderTimezone::parse("America/New_York").is_ok());
    }
}
