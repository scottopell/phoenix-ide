use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::kill_signal::KillSignal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeContractHandleKind {
    Bash,
    TmuxWindow,
}

impl WakeContractHandleKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::TmuxWindow => "tmux_window",
        }
    }

    /// # Errors
    ///
    /// Returns an error when `value` is not a persisted enum value.
    pub fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "bash" => Ok(Self::Bash),
            "tmux_window" => Ok(Self::TmuxWindow),
            other => Err(format!("invalid wake contract handle kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeContractStatus {
    Pending,
    Fired,
    Cancelled,
    Expired,
    Forgotten,
}

impl WakeContractStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fired => "fired",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Forgotten => "forgotten",
        }
    }

    /// # Errors
    ///
    /// Returns an error when `value` is not a persisted enum value.
    pub fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "fired" => Ok(Self::Fired),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "forgotten" => Ok(Self::Forgotten),
            other => Err(format!("invalid wake contract status: {other}")),
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeTerminalCause {
    Fired,
    Cancelled,
    Expired,
    Forgotten,
}

impl WakeTerminalCause {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Forgotten => "forgotten",
        }
    }

    /// # Errors
    ///
    /// Returns an error when `value` is not a persisted enum value.
    pub fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "fired" => Ok(Self::Fired),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "forgotten" => Ok(Self::Forgotten),
            other => Err(format!("invalid wake terminal cause: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeForgottenReason {
    HandleMissing,
    RuntimeUnrecoverableAfterRestart,
}

impl WakeForgottenReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HandleMissing => "handle_missing",
            Self::RuntimeUnrecoverableAfterRestart => "runtime_unrecoverable_after_restart",
        }
    }

    /// # Errors
    ///
    /// Returns an error when `value` is not a persisted enum value.
    pub fn from_db(value: &str) -> Result<Self, String> {
        match value {
            "handle_missing" => Ok(Self::HandleMissing),
            "runtime_unrecoverable_after_restart" => Ok(Self::RuntimeUnrecoverableAfterRestart),
            other => Err(format!("invalid wake forgotten reason: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeBashObservedStatus {
    Exited,
    Killed,
    KillPendingKernel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeTmuxObservedStatus {
    ExitMarkerObserved,
    WindowKilled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WakeDeadlineSeconds(u16);

impl WakeDeadlineSeconds {
    pub const MIN: u16 = 1;
    pub const MAX: u16 = 1800;

    /// # Errors
    ///
    /// Returns an error when `value` is outside the supported deadline range.
    pub fn new(value: u16) -> Result<Self, String> {
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(format!(
                "wake deadline seconds must be in {}..={}, got {value}",
                Self::MIN,
                Self::MAX
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for WakeDeadlineSeconds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeContractHandle {
    Bash { handle_id: String },
    TmuxWindow { handle_id: String },
}

impl WakeContractHandle {
    #[must_use]
    pub fn kind(&self) -> WakeContractHandleKind {
        match self {
            Self::Bash { .. } => WakeContractHandleKind::Bash,
            Self::TmuxWindow { .. } => WakeContractHandleKind::TmuxWindow,
        }
    }

    #[must_use]
    pub fn handle_id(&self) -> &str {
        match self {
            Self::Bash { handle_id } | Self::TmuxWindow { handle_id } => handle_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeContract {
    pub id: String,
    pub current_conversation_id: String,
    pub registration_work_scope: crate::work_scope::WorkScope,
    pub handle: WakeContractHandle,
    pub registering_tool_use_id: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: WakeContractStatus,
    pub terminal_cause: Option<WakeTerminalCause>,
    pub forgotten_reason: Option<WakeForgottenReason>,
    pub terminal_payload: Option<WakeTerminalPayload>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeTail {
    pub ordinal: i64,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeBashFiredPayload {
    pub status: WakeBashObservedStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub signal_number: Option<i64>,
    pub kill_signal_sent: Option<KillSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeTmuxFiredPayload {
    pub status: WakeTmuxObservedStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeTerminalPayload {
    Bash { bash: WakeBashFiredPayload },
    TmuxWindow { tmux_window: WakeTmuxFiredPayload },
}

impl WakeTerminalPayload {
    #[must_use]
    pub fn kind(&self) -> WakeContractHandleKind {
        match self {
            Self::Bash { .. } => WakeContractHandleKind::Bash,
            Self::TmuxWindow { .. } => WakeContractHandleKind::TmuxWindow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRegistrationReceipt {
    pub contract_id: String,
    pub handle: WakeContractHandle,
    pub expires_at: DateTime<Utc>,
    pub registering_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WakeRegisteredHandle {
    Bash { id: String },
    TmuxWindow { id: String },
}

/// Private live-edge payload emitted only after durable registration is visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct WakeContractRegistered {
    pub conversation_id: String,
    pub contract_id: String,
    pub handle: WakeRegisteredHandle,
    pub expires_at: DateTime<Utc>,
    pub registering_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum WakeInboxCause {
    Fired {
        terminal_payload: WakeTerminalPayload,
        tails: Vec<WakeTail>,
        auto_resume: bool,
    },
    Cancelled {
        auto_resume: bool,
    },
    Expired {
        auto_resume: bool,
    },
    Forgotten {
        forgotten_reason: WakeForgottenReason,
        auto_resume: bool,
    },
}

impl WakeInboxCause {
    #[must_use]
    pub fn auto_resume(&self) -> bool {
        match self {
            Self::Cancelled { auto_resume }
            | Self::Expired { auto_resume }
            | Self::Forgotten { auto_resume, .. }
            | Self::Fired { auto_resume, .. } => *auto_resume,
        }
    }

    #[must_use]
    pub fn terminal_cause(&self) -> WakeTerminalCause {
        match self {
            Self::Fired { .. } => WakeTerminalCause::Fired,
            Self::Cancelled { .. } => WakeTerminalCause::Cancelled,
            Self::Expired { .. } => WakeTerminalCause::Expired,
            Self::Forgotten { .. } => WakeTerminalCause::Forgotten,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeInboxItem {
    pub inbox_id: i64,
    pub contract_id: String,
    pub conversation_id: String,
    pub receipt: WakeRegistrationReceipt,
    pub cause: WakeInboxCause,
    pub delivered_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeTerminalOutcome {
    Fired {
        terminal_payload: WakeTerminalPayload,
        tails: Vec<WakeTail>,
        resolved_at: DateTime<Utc>,
    },
    Cancelled {
        resolved_at: DateTime<Utc>,
    },
    Expired {
        resolved_at: DateTime<Utc>,
    },
    Forgotten {
        forgotten_reason: WakeForgottenReason,
        resolved_at: DateTime<Utc>,
    },
}

impl WakeTerminalOutcome {
    #[must_use]
    pub fn status(&self) -> WakeContractStatus {
        match self {
            Self::Fired { .. } => WakeContractStatus::Fired,
            Self::Cancelled { .. } => WakeContractStatus::Cancelled,
            Self::Expired { .. } => WakeContractStatus::Expired,
            Self::Forgotten { .. } => WakeContractStatus::Forgotten,
        }
    }

    #[must_use]
    pub fn terminal_cause(&self) -> WakeTerminalCause {
        match self {
            Self::Fired { .. } => WakeTerminalCause::Fired,
            Self::Cancelled { .. } => WakeTerminalCause::Cancelled,
            Self::Expired { .. } => WakeTerminalCause::Expired,
            Self::Forgotten { .. } => WakeTerminalCause::Forgotten,
        }
    }

    #[must_use]
    pub fn forgotten_reason(&self) -> Option<WakeForgottenReason> {
        match self {
            Self::Forgotten {
                forgotten_reason, ..
            } => Some(*forgotten_reason),
            Self::Fired { .. } | Self::Cancelled { .. } | Self::Expired { .. } => None,
        }
    }

    #[must_use]
    pub fn terminal_payload(&self) -> Option<&WakeTerminalPayload> {
        match self {
            Self::Fired {
                terminal_payload, ..
            } => Some(terminal_payload),
            Self::Cancelled { .. } | Self::Expired { .. } | Self::Forgotten { .. } => None,
        }
    }

    #[must_use]
    pub fn tails(&self) -> &[WakeTail] {
        match self {
            Self::Fired { tails, .. } => tails,
            Self::Cancelled { .. } | Self::Expired { .. } | Self::Forgotten { .. } => &[],
        }
    }

    #[must_use]
    pub fn resolved_at(&self) -> DateTime<Utc> {
        match self {
            Self::Fired { resolved_at, .. }
            | Self::Cancelled { resolved_at }
            | Self::Expired { resolved_at }
            | Self::Forgotten { resolved_at, .. } => *resolved_at,
        }
    }

    #[must_use]
    pub fn auto_resume(&self) -> bool {
        !matches!(self, Self::Cancelled { .. })
    }

    #[must_use]
    pub fn inbox_cause(&self) -> WakeInboxCause {
        match self {
            Self::Fired {
                terminal_payload,
                tails,
                ..
            } => WakeInboxCause::Fired {
                terminal_payload: terminal_payload.clone(),
                tails: tails.clone(),
                auto_resume: true,
            },
            Self::Cancelled { .. } => WakeInboxCause::Cancelled { auto_resume: false },
            Self::Expired { .. } => WakeInboxCause::Expired { auto_resume: true },
            Self::Forgotten {
                forgotten_reason, ..
            } => WakeInboxCause::Forgotten {
                forgotten_reason: *forgotten_reason,
                auto_resume: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_seconds_reject_out_of_range_values() {
        assert!(WakeDeadlineSeconds::new(0).is_err());
        assert!(WakeDeadlineSeconds::new(1801).is_err());
        assert_eq!(WakeDeadlineSeconds::new(600).unwrap().get(), 600);
    }

    #[test]
    fn deadline_seconds_deserialization_enforces_constructor_range() {
        assert!(serde_json::from_str::<WakeDeadlineSeconds>("0").is_err());
        assert!(serde_json::from_str::<WakeDeadlineSeconds>("1801").is_err());
        assert_eq!(
            serde_json::from_str::<WakeDeadlineSeconds>("600")
                .unwrap()
                .get(),
            600
        );
    }
}
