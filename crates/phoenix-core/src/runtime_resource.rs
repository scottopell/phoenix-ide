//! Durable, normalized identity for Phoenix-owned live resources.
//!
//! `RuntimeResourceInstanceId` is allocated before admission. Typed locator and
//! process-birth fields are intentionally separate from Close display identity:
//! recovery consumes these facts directly and never reparses an opaque string.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::work_scope::WorkScopeId;

/// Durable admission boundary owned by the application persistence layer.
/// Engines invoke it before publishing a newly created resource to their
/// process-local registry; engine crates therefore never depend on `phoenix-db`.
#[async_trait]
pub trait RuntimeResourceAdmissionSink: Send + Sync {
    async fn admit_runtime_resource(
        &self,
        admission: RuntimeResourceAdmission,
    ) -> Result<(), String>;
}

pub type RuntimeResourceAdmissionAuthority = Arc<dyn RuntimeResourceAdmissionSink>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeResourceInstanceId(String);

impl RuntimeResourceInstanceId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// # Errors
    /// Returns [`RuntimeResourceInstanceIdError`] for a blank identifier.
    pub fn parse(value: impl Into<String>) -> Result<Self, RuntimeResourceInstanceIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RuntimeResourceInstanceIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RuntimeResourceInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RuntimeResourceInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeResourceInstanceIdError;

impl fmt::Display for RuntimeResourceInstanceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("runtime resource instance id cannot be empty")
    }
}

impl std::error::Error for RuntimeResourceInstanceIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceKind {
    Bash,
    Tmux,
    Pty,
    Browser,
}

impl RuntimeResourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Tmux => "tmux",
            Self::Pty => "pty",
            Self::Browser => "browser",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bash" => Some(Self::Bash),
            "tmux" => Some(Self::Tmux),
            "pty" => Some(Self::Pty),
            "browser" => Some(Self::Browser),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceInstanceState {
    Live,
    RetirementPending,
    Retired,
    NeedsRepair,
}

impl RuntimeResourceInstanceState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::RetirementPending => "retirement_pending",
            Self::Retired => "retired",
            Self::NeedsRepair => "needs_repair",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "live" => Some(Self::Live),
            "retirement_pending" => Some(Self::RetirementPending),
            "retired" => Some(Self::Retired),
            "needs_repair" => Some(Self::NeedsRepair),
            _ => None,
        }
    }
}

/// Typed facts captured at resource admission. Exactly one kind-specific shape
/// is valid; persistence repeats this constraint in `SQLite` CHECK clauses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeResourceAdmission {
    pub instance_id: RuntimeResourceInstanceId,
    pub scope: WorkScopeId,
    pub kind: RuntimeResourceKind,
    pub launch_uuid: String,
    pub pid: Option<u32>,
    pub process_birth: Option<u128>,
    pub pgid: Option<i32>,
    pub tmux_socket_path: Option<String>,
    pub tmux_server_token: Option<String>,
    pub browser_session_key: Option<String>,
    pub browser_audience: Option<String>,
    pub browser_profile_path: Option<String>,
}

impl RuntimeResourceAdmission {
    /// # Errors
    /// Rejects blank authority values and fields that do not belong to `kind`.
    pub fn validate(self) -> Result<Self, RuntimeResourceAdmissionError> {
        let present =
            |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        let absent = |value: &Option<String>| !present(value);
        if self.launch_uuid.trim().is_empty() {
            return Err(RuntimeResourceAdmissionError::BlankLaunchUuid);
        }
        let process = self.pid.is_some() && self.process_birth.is_some();
        let no_process = self.pid.is_none() && self.process_birth.is_none();
        let valid = match self.kind {
            RuntimeResourceKind::Bash => {
                process
                    && self.pgid.is_some()
                    && absent(&self.tmux_socket_path)
                    && absent(&self.tmux_server_token)
                    && absent(&self.browser_session_key)
                    && absent(&self.browser_audience)
                    && absent(&self.browser_profile_path)
            }
            RuntimeResourceKind::Tmux => {
                no_process
                    && self.pgid.is_none()
                    && present(&self.tmux_socket_path)
                    && present(&self.tmux_server_token)
                    && absent(&self.browser_session_key)
                    && absent(&self.browser_audience)
                    && absent(&self.browser_profile_path)
            }
            RuntimeResourceKind::Pty => {
                process
                    && self.pgid.is_none()
                    && absent(&self.tmux_socket_path)
                    && absent(&self.tmux_server_token)
                    && absent(&self.browser_session_key)
                    && absent(&self.browser_audience)
                    && absent(&self.browser_profile_path)
            }
            RuntimeResourceKind::Browser => {
                process
                    && self.pgid.is_none()
                    && absent(&self.tmux_socket_path)
                    && absent(&self.tmux_server_token)
                    && present(&self.browser_session_key)
                    && present(&self.browser_audience)
                    && present(&self.browser_profile_path)
            }
        };
        if valid {
            Ok(self)
        } else {
            Err(RuntimeResourceAdmissionError::KindShape(self.kind))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeResourceAdmissionError {
    BlankLaunchUuid,
    KindShape(RuntimeResourceKind),
}

impl fmt::Display for RuntimeResourceAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankLaunchUuid => {
                formatter.write_str("runtime resource launch UUID cannot be blank")
            }
            Self::KindShape(kind) => write!(
                formatter,
                "runtime resource fields do not match {}",
                kind.as_str()
            ),
        }
    }
}

impl std::error::Error for RuntimeResourceAdmissionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash_admission() -> RuntimeResourceAdmission {
        RuntimeResourceAdmission {
            instance_id: RuntimeResourceInstanceId::parse("instance").unwrap(),
            scope: WorkScopeId::parse("scope").unwrap(),
            kind: RuntimeResourceKind::Bash,
            launch_uuid: "launch".into(),
            pid: Some(10),
            process_birth: Some(11),
            pgid: Some(10),
            tmux_socket_path: None,
            tmux_server_token: None,
            browser_session_key: None,
            browser_audience: None,
            browser_profile_path: None,
        }
    }

    #[test]
    fn admission_requires_its_kind_specific_authority() {
        assert!(bash_admission().validate().is_ok());
        let mut invalid = bash_admission();
        invalid.tmux_server_token = Some("token".into());
        assert_eq!(
            invalid.validate(),
            Err(RuntimeResourceAdmissionError::KindShape(
                RuntimeResourceKind::Bash
            ))
        );
    }

    #[test]
    fn ids_reject_blank_values() {
        assert!(RuntimeResourceInstanceId::parse(" ").is_err());
        assert!(!RuntimeResourceInstanceId::new().as_str().is_empty());
    }
}
