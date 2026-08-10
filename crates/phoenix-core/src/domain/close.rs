use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::work_scope::{RuntimeRole, WorkScopeId};

const GIT_PATH_IDENTITY_CODEC_VERSION: &str = "git_path_bytes_hex_v1";
const OPAQUE_IDENTITY_CODEC_VERSION: &str = "opaque_string_v1";
const GIT_OID_HEX_LEN: usize = 40;
const GIT_OID_HEX_LEN_SHA256: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CloseAttemptId(String);

impl CloseAttemptId {
    /// # Errors
    /// Returns [`CloseAttemptIdError`] when the identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, CloseAttemptIdError> {
        let value = value.into();
        if value.is_empty() {
            Err(CloseAttemptIdError)
        } else {
            Ok(Self(value))
        }
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for CloseAttemptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for CloseAttemptId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseAttemptIdError;
impl fmt::Display for CloseAttemptIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("close attempt id cannot be empty")
    }
}
impl std::error::Error for CloseAttemptIdError {}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TranscriptConversationId(String);

impl TranscriptConversationId {
    /// # Errors
    /// Returns [`TranscriptConversationIdError`] when the supplied row identifier is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, TranscriptConversationIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TranscriptConversationIdError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TranscriptConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TranscriptConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptConversationIdError;

impl fmt::Display for TranscriptConversationIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("transcript conversation id cannot be empty")
    }
}

impl std::error::Error for TranscriptConversationIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosePhase {
    AwaitingBlockerResolution,
    AwaitingStopWorkConfirmation,
    SettlingActiveWork,
    CancelRequestedDuringSettlement,
    AwaitingRetirementInspection,
    AwaitingLossConfirmation,
    RetirementRequested,
    NeedsRepair,
    Completed,
}

impl ClosePhase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingBlockerResolution => "awaiting_blocker_resolution",
            Self::AwaitingStopWorkConfirmation => "awaiting_stop_work_confirmation",
            Self::SettlingActiveWork => "settling_active_work",
            Self::CancelRequestedDuringSettlement => "cancel_requested_during_settlement",
            Self::AwaitingRetirementInspection => "awaiting_retirement_inspection",
            Self::AwaitingLossConfirmation => "awaiting_loss_confirmation",
            Self::RetirementRequested => "retirement_requested",
            Self::NeedsRepair => "needs_repair",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "awaiting_blocker_resolution" => Self::AwaitingBlockerResolution,
            "awaiting_stop_work_confirmation" => Self::AwaitingStopWorkConfirmation,
            "settling_active_work" => Self::SettlingActiveWork,
            "cancel_requested_during_settlement" => Self::CancelRequestedDuringSettlement,
            "awaiting_retirement_inspection" => Self::AwaitingRetirementInspection,
            "awaiting_loss_confirmation" => Self::AwaitingLossConfirmation,
            "retirement_requested" => Self::RetirementRequested,
            "needs_repair" => Self::NeedsRepair,
            "completed" => Self::Completed,
            _ => return None,
        })
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::AwaitingBlockerResolution => matches!(
                next,
                Self::AwaitingStopWorkConfirmation | Self::SettlingActiveWork | Self::Completed
            ),
            Self::AwaitingStopWorkConfirmation => {
                matches!(next, Self::SettlingActiveWork | Self::Completed)
            }
            Self::SettlingActiveWork => matches!(
                next,
                Self::CancelRequestedDuringSettlement | Self::AwaitingRetirementInspection
            ),
            Self::CancelRequestedDuringSettlement => next == Self::Completed,
            Self::AwaitingRetirementInspection => matches!(
                next,
                Self::AwaitingLossConfirmation | Self::RetirementRequested | Self::Completed
            ),
            Self::AwaitingLossConfirmation => matches!(
                next,
                Self::AwaitingRetirementInspection | Self::RetirementRequested | Self::Completed
            ),
            Self::RetirementRequested => matches!(next, Self::NeedsRepair | Self::Completed),
            Self::NeedsRepair => matches!(next, Self::RetirementRequested | Self::Completed),
            Self::Completed => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseCompletionOutcome {
    Archived,
    Cancelled,
}

impl CloseCompletionOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archived => "archived",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "archived" => Self::Archived,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossCategory {
    StagedTrackedPaths,
    UnstagedTrackedPaths,
    UntrackedNonIgnoredPaths,
    InitializedSubmoduleState,
    DetachedUnreachableCommits,
}

impl LossCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StagedTrackedPaths => "staged_tracked_paths",
            Self::UnstagedTrackedPaths => "unstaged_tracked_paths",
            Self::UntrackedNonIgnoredPaths => "untracked_non_ignored_paths",
            Self::InitializedSubmoduleState => "initialized_submodule_state",
            Self::DetachedUnreachableCommits => "detached_unreachable_commits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetiredResourceKind {
    Worktree,
    BashProcessGroup,
    TmuxServer,
    PtySession,
    BrowserSession,
    EquivalentLiveResource,
}

impl RetiredResourceKind {
    #[must_use]
    pub fn admits_identity_kind(self, identity: &LossItemIdentity) -> bool {
        match self {
            Self::Worktree => matches!(identity, LossItemIdentity::GitPath(_)),
            Self::BashProcessGroup
            | Self::TmuxServer
            | Self::PtySession
            | Self::BrowserSession
            | Self::EquivalentLiveResource => matches!(identity, LossItemIdentity::Opaque(_)),
        }
    }
}

impl RetiredResourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::BashProcessGroup => "bash_process_group",
            Self::TmuxServer => "tmux_server",
            Self::PtySession => "pty_session",
            Self::BrowserSession => "browser_session",
            Self::EquivalentLiveResource => "equivalent_live_resource",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementFailureReason {
    RemovalFailed,
    StillSharedByLiveOwner,
    ResidualProcessAlive,
    IdentityNotProven,
    ManualRepairRequired,
}

impl RetirementFailureReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RemovalFailed => "removal_failed",
            Self::StillSharedByLiveOwner => "still_shared_by_live_owner",
            Self::ResidualProcessAlive => "residual_process_alive",
            Self::IdentityNotProven => "identity_not_proven",
            Self::ManualRepairRequired => "manual_repair_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitPathIdentity(Vec<u8>);

impl GitPathIdentity {
    /// # Panics
    /// Panics when `bytes` is empty or contains NUL because neither is a Git path identity.
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        assert!(!bytes.is_empty(), "Git path identity cannot be empty");
        assert!(
            !bytes.contains(&0),
            "Git path identity cannot contain a NUL byte"
        );
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn codec(&self) -> &'static str {
        GIT_PATH_IDENTITY_CODEC_VERSION
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let payload = self.0.iter().fold(
            String::with_capacity(self.0.len() * 2),
            |mut output, byte| {
                use std::fmt::Write as _;
                write!(output, "{byte:02x}").expect("writing to String cannot fail");
                output
            },
        );
        format!("{GIT_PATH_IDENTITY_CODEC_VERSION}:{payload}")
    }

    /// Decodes the exact versioned representation.
    ///
    /// # Errors
    ///
    /// Returns an error when the codec prefix is absent or the payload is not valid
    /// lowercase hexadecimal bytes.
    pub fn decode_exact(value: &str) -> Result<Self, GitPathIdentityError> {
        let (codec, payload) = value
            .split_once(':')
            .ok_or(GitPathIdentityError::MissingCodecPrefix)?;
        if codec != GIT_PATH_IDENTITY_CODEC_VERSION {
            return Err(GitPathIdentityError::UnsupportedCodec(codec.to_string()));
        }
        if payload.is_empty() {
            return Err(GitPathIdentityError::Empty);
        }
        if payload.len() % 2 != 0
            || payload
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(GitPathIdentityError::InvalidHex);
        }
        let hex_value = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let bytes = payload
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
            .collect::<Option<Vec<_>>>()
            .ok_or(GitPathIdentityError::InvalidHex)?;
        if bytes.contains(&0) {
            return Err(GitPathIdentityError::ContainsNul);
        }
        Ok(Self(bytes))
    }
}

impl Serialize for GitPathIdentity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for GitPathIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::decode_exact(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitPathIdentityError {
    MissingCodecPrefix,
    UnsupportedCodec(String),
    Empty,
    InvalidHex,
    ContainsNul,
}

impl fmt::Display for GitPathIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCodecPrefix => f.write_str("git path identity missing codec prefix"),
            Self::UnsupportedCodec(codec) => {
                write!(f, "unsupported git path identity codec: {codec}")
            }
            Self::Empty => f.write_str("git path identity cannot be empty"),
            Self::InvalidHex => f.write_str("invalid lowercase-hex git path identity payload"),
            Self::ContainsNul => f.write_str("git path identity cannot contain a NUL byte"),
        }
    }
}

impl std::error::Error for GitPathIdentityError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct GitOidIdentity(String);

impl<'de> Deserialize<'de> for GitOidIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse_hex(value).map_err(serde::de::Error::custom)
    }
}

impl GitOidIdentity {
    /// Parses a SHA-1 or SHA-256 Git object identifier and normalizes it to lowercase.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not exactly 40 or 64 hexadecimal characters.
    pub fn parse_hex(value: impl Into<String>) -> Result<Self, GitOidIdentityError> {
        let value = value.into();
        let is_valid_len = matches!(value.len(), GIT_OID_HEX_LEN | GIT_OID_HEX_LEN_SHA256);
        if !is_valid_len {
            return Err(GitOidIdentityError::InvalidLength(value.len()));
        }
        if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GitOidIdentityError::NonHex);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOidIdentityError {
    InvalidLength(usize),
    NonHex,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueIdentity(String);

impl<'de> Deserialize<'de> for OpaqueIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl OpaqueIdentity {
    /// Parses a non-empty opaque resource identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied identity is empty.
    pub fn parse(value: impl Into<String>) -> Result<Self, OpaqueIdentityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(OpaqueIdentityError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn codec(&self) -> &'static str {
        OPAQUE_IDENTITY_CODEC_VERSION
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpaqueIdentityError {
    Empty,
}

impl fmt::Display for OpaqueIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("opaque identity cannot be empty"),
        }
    }
}

impl std::error::Error for OpaqueIdentityError {}

impl fmt::Display for GitOidIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(len) => write!(
                f,
                "git oid identity must be {GIT_OID_HEX_LEN} or {GIT_OID_HEX_LEN_SHA256} hex chars, got {len}"
            ),
            Self::NonHex => f.write_str("git oid identity must be lowercase/uppercase ASCII hex"),
        }
    }
}

impl std::error::Error for GitOidIdentityError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "identity_kind", content = "identity", rename_all = "snake_case")]
pub enum LossItemIdentity {
    GitPath(GitPathIdentity),
    GitOid(GitOidIdentity),
    Opaque(OpaqueIdentity),
}

impl LossItemIdentity {
    #[must_use]
    pub fn identity_kind(&self) -> &'static str {
        match self {
            Self::GitPath(_) => "git_path",
            Self::GitOid(_) => "git_oid",
            Self::Opaque(_) => "opaque",
        }
    }

    #[must_use]
    pub fn codec(&self) -> &'static str {
        match self {
            Self::GitPath(identity) => identity.codec(),
            Self::GitOid(_) => "hex",
            Self::Opaque(identity) => identity.codec(),
        }
    }

    #[must_use]
    pub fn value(&self) -> String {
        match self {
            Self::GitPath(identity) => identity.encode(),
            Self::GitOid(identity) => identity.as_hex().to_string(),
            Self::Opaque(identity) => identity.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsenceBasis {
    SameAttemptPriorRetirement,
    PreexistingExactIdentityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "proof_kind", rename_all = "snake_case")]
pub enum RetirementOutcome {
    Retired,
    AbsenceAdopted {
        absence_basis: AbsenceBasis,
    },
    Residual {
        residual_reason: RetirementFailureReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseMemberRole {
    Root,
    Intermediate,
    Latest,
    RootLatest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturedConversationStateKind {
    Idle,
    LlmRequesting,
    ToolExecuting,
    CancellingTool,
    AwaitingSubAgents,
    CancellingSubAgents,
    Error,
    AwaitingContinuation,
    RecoverableContinuationFailure,
    AwaitingRecovery,
    AwaitingTaskApproval,
    AwaitingUserResponse,
    AwaitingCommissionReviewApproval,
    ContextExhausted,
    HandedOff,
    Terminal,
    Completed,
    Failed,
    Provisioning,
    CreationFailed,
    CreationCancelled,
    SeededLlmRequesting,
}

impl CapturedConversationStateKind {
    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "idle" => Self::Idle,
            "llm_requesting" => Self::LlmRequesting,
            "tool_executing" => Self::ToolExecuting,
            "cancelling_tool" => Self::CancellingTool,
            "awaiting_sub_agents" => Self::AwaitingSubAgents,
            "cancelling_sub_agents" => Self::CancellingSubAgents,
            "error" => Self::Error,
            "awaiting_continuation" => Self::AwaitingContinuation,
            "recoverable_continuation_failure" => Self::RecoverableContinuationFailure,
            "awaiting_recovery" => Self::AwaitingRecovery,
            "awaiting_task_approval" => Self::AwaitingTaskApproval,
            "awaiting_user_response" => Self::AwaitingUserResponse,
            "awaiting_commission_review_approval" => Self::AwaitingCommissionReviewApproval,
            "context_exhausted" => Self::ContextExhausted,
            "handed_off" => Self::HandedOff,
            "terminal" => Self::Terminal,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "provisioning" => Self::Provisioning,
            "creation_failed" => Self::CreationFailed,
            "creation_cancelled" => Self::CreationCancelled,
            "seeded_llm_requesting" => Self::SeededLlmRequesting,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseAttemptMember {
    pub attempt_id: CloseAttemptId,
    pub conversation_id: TranscriptConversationId,
    pub role: CloseMemberRole,
    pub continuation_ordinal: u32,
    pub captured_continued_in_conv_id: Option<TranscriptConversationId>,
    pub captured_state_kind: CapturedConversationStateKind,
    pub captured_runtime_role: RuntimeRole,
    pub captured_work_scope_id: Option<WorkScopeId>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseAttemptScope {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub captured_worktree: Option<GitPathIdentity>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseRetirementSnapshot {
    inspection_generation: String,
    inspection_fingerprint: String,
}

impl CloseRetirementSnapshot {
    /// # Errors
    /// Returns [`CloseRetirementSnapshotError`] when either exact snapshot identity is empty.
    pub fn parse(
        inspection_generation: impl Into<String>,
        inspection_fingerprint: impl Into<String>,
    ) -> Result<Self, CloseRetirementSnapshotError> {
        let inspection_generation = inspection_generation.into();
        let inspection_fingerprint = inspection_fingerprint.into();
        if inspection_generation.is_empty() || inspection_fingerprint.is_empty() {
            return Err(CloseRetirementSnapshotError);
        }
        Ok(Self {
            inspection_generation,
            inspection_fingerprint,
        })
    }

    #[must_use]
    pub fn generation(&self) -> &str {
        &self.inspection_generation
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.inspection_fingerprint
    }
}

impl<'de> Deserialize<'de> for CloseRetirementSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireSnapshot {
            inspection_generation: String,
            inspection_fingerprint: String,
        }

        let wire = WireSnapshot::deserialize(deserializer)?;
        Self::parse(wire.inspection_generation, wire.inspection_fingerprint)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseRetirementSnapshotError;

impl fmt::Display for CloseRetirementSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("close retirement snapshot identities must be non-empty")
    }
}

impl std::error::Error for CloseRetirementSnapshotError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRetirementTarget {
    pub scope: WorkScopeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseObligation {
    attempt_id: CloseAttemptId,
    product_conversation_id: ProductConversationId,
    phase: ClosePhase,
    snapshot: Option<CloseRetirementSnapshot>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    close_outcome: Option<CloseCompletionOutcome>,
}

impl CloseObligation {
    /// # Errors
    /// Returns an error when phase-specific evidence or completion fields disagree.
    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        attempt_id: CloseAttemptId,
        product_conversation_id: ProductConversationId,
        phase: ClosePhase,
        snapshot: Option<CloseRetirementSnapshot>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
        close_outcome: Option<CloseCompletionOutcome>,
    ) -> Result<Self, CloseObligationError> {
        let requires_snapshot = matches!(
            phase,
            ClosePhase::AwaitingLossConfirmation
                | ClosePhase::RetirementRequested
                | ClosePhase::NeedsRepair
        );
        let admits_optional_prior_snapshot = phase == ClosePhase::AwaitingRetirementInspection;
        let forbids_snapshot =
            !requires_snapshot && !admits_optional_prior_snapshot && phase != ClosePhase::Completed;
        let is_completed = phase == ClosePhase::Completed;
        let archived_completion_missing_snapshot =
            close_outcome == Some(CloseCompletionOutcome::Archived) && snapshot.is_none();
        if (requires_snapshot && snapshot.is_none())
            || (forbids_snapshot && snapshot.is_some())
            || (is_completed != completed_at.is_some())
            || (is_completed != close_outcome.is_some())
            || archived_completion_missing_snapshot
        {
            return Err(CloseObligationError);
        }
        Ok(Self {
            attempt_id,
            product_conversation_id,
            phase,
            snapshot,
            created_at,
            updated_at,
            completed_at,
            close_outcome,
        })
    }

    #[must_use]
    pub fn attempt_id(&self) -> &CloseAttemptId {
        &self.attempt_id
    }
    #[must_use]
    pub fn product_conversation_id(&self) -> &ProductConversationId {
        &self.product_conversation_id
    }
    #[must_use]
    pub fn phase(&self) -> ClosePhase {
        self.phase
    }
    #[must_use]
    pub fn snapshot(&self) -> Option<&CloseRetirementSnapshot> {
        self.snapshot.as_ref()
    }
    #[must_use]
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    #[must_use]
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    #[must_use]
    pub fn completed_at(&self) -> Option<DateTime<Utc>> {
        self.completed_at
    }
    #[must_use]
    pub fn close_outcome(&self) -> Option<CloseCompletionOutcome> {
        self.close_outcome
    }
}

impl<'de> Deserialize<'de> for CloseObligation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            attempt_id: CloseAttemptId,
            product_conversation_id: ProductConversationId,
            phase: ClosePhase,
            snapshot: Option<CloseRetirementSnapshot>,
            created_at: DateTime<Utc>,
            updated_at: DateTime<Utc>,
            completed_at: Option<DateTime<Utc>>,
            close_outcome: Option<CloseCompletionOutcome>,
        }
        let w = Wire::deserialize(deserializer)?;
        Self::parse(
            w.attempt_id,
            w.product_conversation_id,
            w.phase,
            w.snapshot,
            w.created_at,
            w.updated_at,
            w.completed_at,
            w.close_outcome,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseObligationError;
impl fmt::Display for CloseObligationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("close obligation phase evidence mismatch")
    }
}
impl std::error::Error for CloseObligationError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseInspection {
    pub attempt_id: CloseAttemptId,
    pub target: CloseRetirementTarget,
    pub snapshot: CloseRetirementSnapshot,
    pub inspected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "category", content = "identity", rename_all = "snake_case")]
pub enum CloseLossItem {
    StagedTrackedPath(GitPathIdentity),
    UnstagedTrackedPath(GitPathIdentity),
    UntrackedNonIgnoredPath(GitPathIdentity),
    InitializedSubmoduleState(GitPathIdentity),
    DetachedUnreachableCommit(GitOidIdentity),
}

impl CloseLossItem {
    #[must_use]
    pub fn category(&self) -> LossCategory {
        match self {
            Self::StagedTrackedPath(_) => LossCategory::StagedTrackedPaths,
            Self::UnstagedTrackedPath(_) => LossCategory::UnstagedTrackedPaths,
            Self::UntrackedNonIgnoredPath(_) => LossCategory::UntrackedNonIgnoredPaths,
            Self::InitializedSubmoduleState(_) => LossCategory::InitializedSubmoduleState,
            Self::DetachedUnreachableCommit(_) => LossCategory::DetachedUnreachableCommits,
        }
    }

    #[must_use]
    pub fn identity(&self) -> LossItemIdentity {
        match self {
            Self::StagedTrackedPath(identity)
            | Self::UnstagedTrackedPath(identity)
            | Self::UntrackedNonIgnoredPath(identity)
            | Self::InitializedSubmoduleState(identity) => {
                LossItemIdentity::GitPath(identity.clone())
            }
            Self::DetachedUnreachableCommit(identity) => LossItemIdentity::GitOid(identity.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseInspectionLoss {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub item: CloseLossItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetiredResourceIdentity {
    kind: RetiredResourceKind,
    identity: LossItemIdentity,
}
impl RetiredResourceIdentity {
    /// # Errors
    /// Returns [`RetiredResourceIdentityError`] when the resource kind cannot own the identity type.
    pub fn parse(
        kind: RetiredResourceKind,
        identity: LossItemIdentity,
    ) -> Result<Self, RetiredResourceIdentityError> {
        if kind.admits_identity_kind(&identity) {
            Ok(Self { kind, identity })
        } else {
            Err(RetiredResourceIdentityError)
        }
    }
    #[must_use]
    pub fn kind(&self) -> RetiredResourceKind {
        self.kind
    }
    #[must_use]
    pub fn identity(&self) -> &LossItemIdentity {
        &self.identity
    }
}
impl<'de> Deserialize<'de> for RetiredResourceIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            kind: RetiredResourceKind,
            identity: LossItemIdentity,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::parse(wire.kind, wire.identity).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiredResourceIdentityError;
impl fmt::Display for RetiredResourceIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("retired resource kind and identity mismatch")
    }
}
impl std::error::Error for RetiredResourceIdentityError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseOwnedResourceInventory {
    pub worktree: Option<GitPathIdentity>,
    pub bash_process_groups: std::collections::BTreeSet<OpaqueIdentity>,
    pub tmux_servers: std::collections::BTreeSet<OpaqueIdentity>,
    pub pty_sessions: std::collections::BTreeSet<OpaqueIdentity>,
    pub browser_sessions: std::collections::BTreeSet<OpaqueIdentity>,
    pub equivalent_live_resources: std::collections::BTreeSet<OpaqueIdentity>,
}

impl CloseOwnedResourceInventory {
    fn opaque(kind: RetiredResourceKind, identity: OpaqueIdentity) -> RetiredResourceIdentity {
        RetiredResourceIdentity::parse(kind, LossItemIdentity::Opaque(identity))
            .expect("opaque owned-resource identity pairing is structural")
    }

    /// # Panics
    /// This cannot panic: each typed field is paired with its structurally fixed resource kind.
    #[must_use]
    pub fn resources(&self) -> Vec<RetiredResourceIdentity> {
        self.worktree
            .iter()
            .cloned()
            .map(|identity| {
                RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree,
                    LossItemIdentity::GitPath(identity),
                )
                .expect("worktree identity pairing is structural")
            })
            .chain(
                self.bash_process_groups
                    .iter()
                    .cloned()
                    .map(|identity| Self::opaque(RetiredResourceKind::BashProcessGroup, identity)),
            )
            .chain(
                self.tmux_servers
                    .iter()
                    .cloned()
                    .map(|identity| Self::opaque(RetiredResourceKind::TmuxServer, identity)),
            )
            .chain(
                self.pty_sessions
                    .iter()
                    .cloned()
                    .map(|identity| Self::opaque(RetiredResourceKind::PtySession, identity)),
            )
            .chain(
                self.browser_sessions
                    .iter()
                    .cloned()
                    .map(|identity| Self::opaque(RetiredResourceKind::BrowserSession, identity)),
            )
            .chain(
                self.equivalent_live_resources
                    .iter()
                    .cloned()
                    .map(|identity| {
                        Self::opaque(RetiredResourceKind::EquivalentLiveResource, identity)
                    }),
            )
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseExpectedRetirementResource {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRetiredResource {
    pub attempt_id: CloseAttemptId,
    pub scope: WorkScopeId,
    pub snapshot: CloseRetirementSnapshot,
    pub resource: RetiredResourceIdentity,
    pub outcome: RetirementOutcome,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    fn transcript_conversation_id_is_distinct_and_round_trips() {
        let id = TranscriptConversationId::parse("leaf-1").unwrap();
        assert_eq!(id.as_str(), "leaf-1");
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""leaf-1""#);
        assert_eq!(
            serde_json::from_str::<TranscriptConversationId>(r#""leaf-1""#).unwrap(),
            id
        );
        assert!(TranscriptConversationId::parse(" \t").is_err());
    }

    #[test]
    fn close_completion_outcome_db_strings_round_trip() {
        assert_eq!(CloseCompletionOutcome::Archived.as_str(), "archived");
        assert_eq!(CloseCompletionOutcome::Cancelled.as_str(), "cancelled");
        assert_eq!(
            CloseCompletionOutcome::from_db_str("archived"),
            Some(CloseCompletionOutcome::Archived)
        );
        assert_eq!(
            CloseCompletionOutcome::from_db_str("cancelled"),
            Some(CloseCompletionOutcome::Cancelled)
        );
        assert_eq!(CloseCompletionOutcome::from_db_str("nope"), None);
    }

    #[test]
    fn close_phase_db_strings_and_transitions_follow_bedrock() {
        let ordered = [
            (
                ClosePhase::AwaitingBlockerResolution,
                "awaiting_blocker_resolution",
            ),
            (
                ClosePhase::AwaitingStopWorkConfirmation,
                "awaiting_stop_work_confirmation",
            ),
            (ClosePhase::SettlingActiveWork, "settling_active_work"),
            (
                ClosePhase::CancelRequestedDuringSettlement,
                "cancel_requested_during_settlement",
            ),
            (
                ClosePhase::AwaitingRetirementInspection,
                "awaiting_retirement_inspection",
            ),
            (
                ClosePhase::AwaitingLossConfirmation,
                "awaiting_loss_confirmation",
            ),
            (ClosePhase::RetirementRequested, "retirement_requested"),
            (ClosePhase::NeedsRepair, "needs_repair"),
            (ClosePhase::Completed, "completed"),
        ];
        for (phase, raw) in ordered {
            assert_eq!(phase.as_str(), raw);
            assert_eq!(ClosePhase::from_db_str(raw), Some(phase));
        }
        assert_eq!(ClosePhase::from_db_str("nope"), None);

        let phases = ordered.iter().map(|(phase, _)| *phase).collect::<Vec<_>>();
        for &from in &phases {
            for &to in &phases {
                let allowed = from.can_transition_to(to);
                let expected = match from {
                    ClosePhase::AwaitingBlockerResolution => matches!(
                        to,
                        ClosePhase::AwaitingStopWorkConfirmation
                            | ClosePhase::SettlingActiveWork
                            | ClosePhase::Completed
                    ),
                    ClosePhase::AwaitingStopWorkConfirmation => {
                        matches!(to, ClosePhase::SettlingActiveWork | ClosePhase::Completed)
                    }
                    ClosePhase::SettlingActiveWork => matches!(
                        to,
                        ClosePhase::CancelRequestedDuringSettlement
                            | ClosePhase::AwaitingRetirementInspection
                    ),
                    ClosePhase::CancelRequestedDuringSettlement => to == ClosePhase::Completed,
                    ClosePhase::AwaitingRetirementInspection => matches!(
                        to,
                        ClosePhase::AwaitingLossConfirmation
                            | ClosePhase::RetirementRequested
                            | ClosePhase::Completed
                    ),
                    ClosePhase::AwaitingLossConfirmation => matches!(
                        to,
                        ClosePhase::AwaitingRetirementInspection
                            | ClosePhase::RetirementRequested
                            | ClosePhase::Completed
                    ),
                    ClosePhase::RetirementRequested => {
                        matches!(to, ClosePhase::NeedsRepair | ClosePhase::Completed)
                    }
                    ClosePhase::NeedsRepair => {
                        matches!(to, ClosePhase::RetirementRequested | ClosePhase::Completed)
                    }
                    ClosePhase::Completed => false,
                };
                assert_eq!(
                    allowed,
                    expected,
                    "unexpected transition permission from {} to {}",
                    from.as_str(),
                    to.as_str()
                );
            }
        }
    }

    #[test]
    fn enums_expose_exact_persisted_strings() {
        assert_eq!(
            LossCategory::StagedTrackedPaths.as_str(),
            "staged_tracked_paths"
        );
        assert_eq!(
            LossCategory::UnstagedTrackedPaths.as_str(),
            "unstaged_tracked_paths"
        );
        assert_eq!(
            LossCategory::UntrackedNonIgnoredPaths.as_str(),
            "untracked_non_ignored_paths"
        );
        assert_eq!(
            LossCategory::InitializedSubmoduleState.as_str(),
            "initialized_submodule_state"
        );
        assert_eq!(
            LossCategory::DetachedUnreachableCommits.as_str(),
            "detached_unreachable_commits"
        );

        assert_eq!(RetiredResourceKind::Worktree.as_str(), "worktree");
        assert_eq!(
            RetiredResourceKind::BashProcessGroup.as_str(),
            "bash_process_group"
        );
        assert_eq!(RetiredResourceKind::TmuxServer.as_str(), "tmux_server");
        assert_eq!(RetiredResourceKind::PtySession.as_str(), "pty_session");
        assert_eq!(
            RetiredResourceKind::BrowserSession.as_str(),
            "browser_session"
        );
        assert_eq!(
            RetiredResourceKind::EquivalentLiveResource.as_str(),
            "equivalent_live_resource"
        );

        assert_eq!(
            RetirementFailureReason::RemovalFailed.as_str(),
            "removal_failed"
        );
        assert_eq!(
            RetirementFailureReason::StillSharedByLiveOwner.as_str(),
            "still_shared_by_live_owner"
        );
        assert_eq!(
            RetirementFailureReason::ResidualProcessAlive.as_str(),
            "residual_process_alive"
        );
        assert_eq!(
            RetirementFailureReason::IdentityNotProven.as_str(),
            "identity_not_proven"
        );
        assert_eq!(
            RetirementFailureReason::ManualRepairRequired.as_str(),
            "manual_repair_required"
        );

        assert_eq!(
            serde_json::to_value(AbsenceBasis::SameAttemptPriorRetirement).unwrap(),
            serde_json::json!("same_attempt_prior_retirement")
        );
        assert_eq!(
            serde_json::to_value(AbsenceBasis::PreexistingExactIdentityEvidence).unwrap(),
            serde_json::json!("preexisting_exact_identity_evidence")
        );
        assert_eq!(
            serde_json::to_value(CloseMemberRole::Root).unwrap(),
            serde_json::json!("root")
        );
        assert_eq!(
            serde_json::to_value(CloseMemberRole::Intermediate).unwrap(),
            serde_json::json!("intermediate")
        );
        assert_eq!(
            serde_json::to_value(CloseMemberRole::Latest).unwrap(),
            serde_json::json!("latest")
        );
        assert_eq!(
            serde_json::to_value(CloseMemberRole::RootLatest).unwrap(),
            serde_json::json!("root_latest")
        );
    }

    #[test]
    fn git_path_identity_codec_round_trips_non_utf8_losslessly() {
        let identity = GitPathIdentity::from_bytes(vec![0x66, 0x6f, 0x80, 0x2f, 0xff]);
        let encoded = identity.encode();
        assert_eq!(encoded, "git_path_bytes_hex_v1:666f802fff");
        let decoded = GitPathIdentity::decode_exact(&encoded).unwrap();
        assert_eq!(decoded.as_bytes(), identity.as_bytes());
    }

    #[test]
    fn git_path_identity_rejects_embedded_nul() {
        assert_eq!(
            GitPathIdentity::decode_exact("git_path_bytes_hex_v1:610062"),
            Err(GitPathIdentityError::ContainsNul)
        );
    }

    #[test]
    #[should_panic(expected = "Git path identity cannot contain a NUL byte")]
    fn git_path_identity_constructor_rejects_embedded_nul() {
        let _ = GitPathIdentity::from_bytes(b"a\0b".to_vec());
    }

    #[test]
    fn git_path_identity_prevents_lossy_collisions() {
        let left = GitPathIdentity::from_bytes(b"a\x80".to_vec());
        let right = GitPathIdentity::from_bytes("a\u{fffd}".as_bytes().to_vec());
        assert_eq!(
            String::from_utf8_lossy(left.as_bytes()),
            String::from_utf8_lossy(right.as_bytes())
        );
        assert_ne!(left.as_bytes(), right.as_bytes());
        assert_ne!(left.encode(), right.encode());
    }

    #[test]
    fn git_oid_identity_accepts_only_hex_lengths() {
        let oid = GitOidIdentity::parse_hex("A123456789012345678901234567890123456789").unwrap();
        assert_eq!(oid.as_hex(), "a123456789012345678901234567890123456789");
        assert!(matches!(
            GitOidIdentity::parse_hex("abc"),
            Err(GitOidIdentityError::InvalidLength(3))
        ));
        assert_eq!(
            GitOidIdentity::parse_hex("z123456789012345678901234567890123456789"),
            Err(GitOidIdentityError::NonHex)
        );
    }

    #[test]
    fn git_oid_identity_deserialization_preserves_validation() {
        let oid: GitOidIdentity =
            serde_json::from_str(r#""A123456789012345678901234567890123456789""#).unwrap();
        assert_eq!(oid.as_hex(), "a123456789012345678901234567890123456789");

        assert!(serde_json::from_str::<GitOidIdentity>(r#""abc""#).is_err());
        assert!(serde_json::from_str::<GitOidIdentity>(
            r#""z123456789012345678901234567890123456789""#,
        )
        .is_err());
    }

    #[test]
    fn opaque_identity_round_trips() {
        let identity = OpaqueIdentity::parse("browser-session:abc").unwrap();
        assert_eq!(identity.codec(), "opaque_string_v1");
        assert_eq!(identity.as_str(), "browser-session:abc");
        assert_eq!(OpaqueIdentity::parse(""), Err(OpaqueIdentityError::Empty));
        assert!(serde_json::from_str::<OpaqueIdentity>(r#""""#).is_err());
    }
    #[test]
    fn loss_item_identity_serializes_with_exact_shape() {
        let path = LossItemIdentity::GitPath(GitPathIdentity::from_bytes(b"src/lib.rs".to_vec()));
        assert_eq!(path.identity_kind(), "git_path");
        assert_eq!(path.codec(), "git_path_bytes_hex_v1");
        assert_eq!(
            serde_json::to_value(path).unwrap(),
            serde_json::json!({
                "identity_kind": "git_path",
                "identity": "git_path_bytes_hex_v1:7372632f6c69622e7273"
            })
        );

        let oid = LossItemIdentity::GitOid(
            GitOidIdentity::parse_hex("1234567890123456789012345678901234567890").unwrap(),
        );
        assert_eq!(oid.identity_kind(), "git_oid");
        assert_eq!(oid.codec(), "hex");
        assert_eq!(oid.value(), "1234567890123456789012345678901234567890");
    }

    #[test]
    fn close_loss_item_couples_category_to_identity_correctly() {
        let path = GitPathIdentity::from_bytes(b"src/lib.rs".to_vec());
        let oid = GitOidIdentity::parse_hex("1234567890123456789012345678901234567890").unwrap();

        let staged = CloseLossItem::StagedTrackedPath(path.clone());
        assert_eq!(staged.category(), LossCategory::StagedTrackedPaths);
        assert_eq!(staged.identity(), LossItemIdentity::GitPath(path.clone()));

        let detached = CloseLossItem::DetachedUnreachableCommit(oid.clone());
        assert_eq!(
            detached.category(),
            LossCategory::DetachedUnreachableCommits
        );
        assert_eq!(detached.identity(), LossItemIdentity::GitOid(oid));
    }

    #[test]
    fn retired_resource_kind_restricts_identity_kinds() {
        let git_path = LossItemIdentity::GitPath(GitPathIdentity::from_bytes(b"wt".to_vec()));
        let opaque = LossItemIdentity::Opaque(OpaqueIdentity::parse("browser:1").unwrap());
        let git_oid = LossItemIdentity::GitOid(
            GitOidIdentity::parse_hex("1234567890123456789012345678901234567890").unwrap(),
        );

        assert!(RetiredResourceKind::Worktree.admits_identity_kind(&git_path));
        assert!(!RetiredResourceKind::Worktree.admits_identity_kind(&opaque));
        assert!(!RetiredResourceKind::Worktree.admits_identity_kind(&git_oid));
        assert!(RetiredResourceKind::BrowserSession.admits_identity_kind(&opaque));
        assert!(!RetiredResourceKind::BrowserSession.admits_identity_kind(&git_path));
    }

    #[test]
    fn retirement_outcome_serializes_as_typed_shape() {
        let retired = serde_json::to_value(RetirementOutcome::Retired).unwrap();
        assert_eq!(retired, serde_json::json!({"proof_kind": "retired"}));

        let absence = serde_json::to_value(RetirementOutcome::AbsenceAdopted {
            absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
        })
        .unwrap();
        assert_eq!(
            absence,
            serde_json::json!({
                "proof_kind": "absence_adopted",
                "absence_basis": "same_attempt_prior_retirement"
            })
        );

        let residual = serde_json::to_value(RetirementOutcome::Residual {
            residual_reason: RetirementFailureReason::ManualRepairRequired,
        })
        .unwrap();
        assert_eq!(
            residual,
            serde_json::json!({
                "proof_kind": "residual",
                "residual_reason": "manual_repair_required"
            })
        );
    }

    #[test]
    fn close_obligation_completion_fields_are_structural() {
        let attempt_id = CloseAttemptId::parse("attempt-1").unwrap();
        let product_conversation_id = ProductConversationId::parse("root-1").unwrap();
        let now = Utc::now();

        let snapshot = CloseRetirementSnapshot::parse("generation-1", "fingerprint-1").unwrap();
        assert!(CloseObligation::parse(
            attempt_id.clone(),
            product_conversation_id.clone(),
            ClosePhase::Completed,
            Some(snapshot),
            now,
            now,
            Some(now),
            Some(CloseCompletionOutcome::Archived),
        )
        .is_ok());
        assert!(CloseObligation::parse(
            attempt_id.clone(),
            product_conversation_id.clone(),
            ClosePhase::Completed,
            None,
            now,
            now,
            Some(now),
            Some(CloseCompletionOutcome::Archived),
        )
        .is_err());
        assert!(CloseObligation::parse(
            attempt_id.clone(),
            product_conversation_id.clone(),
            ClosePhase::Completed,
            None,
            now,
            now,
            Some(now),
            Some(CloseCompletionOutcome::Cancelled),
        )
        .is_ok());
        assert!(CloseObligation::parse(
            attempt_id.clone(),
            product_conversation_id.clone(),
            ClosePhase::Completed,
            None,
            now,
            now,
            Some(now),
            None,
        )
        .is_err());
        assert!(CloseObligation::parse(
            attempt_id,
            product_conversation_id,
            ClosePhase::AwaitingBlockerResolution,
            None,
            now,
            now,
            None,
            Some(CloseCompletionOutcome::Archived),
        )
        .is_err());
    }

    #[test]
    fn snapshot_and_target_structs_preserve_exact_fields() {
        let snapshot = CloseRetirementSnapshot::parse("g1", "fp1").unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            serde_json::json!({
                "inspection_generation": "g1",
                "inspection_fingerprint": "fp1"
            })
        );
        assert_eq!(snapshot.generation(), "g1");
        assert_eq!(snapshot.fingerprint(), "fp1");
        assert!(CloseRetirementSnapshot::parse("", "fp1").is_err());
        assert!(serde_json::from_str::<CloseRetirementSnapshot>(
            r#"{"inspection_generation":"g1","inspection_fingerprint":""}"#,
        )
        .is_err());

        let target = CloseRetirementTarget {
            scope: WorkScopeId::parse("scope-1").unwrap(),
        };
        assert_eq!(
            serde_json::to_value(&target).unwrap(),
            serde_json::json!({
                "scope": "scope-1"
            })
        );
    }
}
