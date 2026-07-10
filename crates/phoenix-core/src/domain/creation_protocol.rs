//! Durable conversation-creation protocol vocabulary.
//!
//! These types describe authority and lifecycle state. The pure transition
//! rules live in `phoenix-state-machine`; persistence and side effects live in
//! the database and runtime crates.

use serde::{Deserialize, Serialize};

/// Monotonic logical time used by the protocol.
///
/// Production adapters convert timestamps to this ordered millisecond value;
/// deterministic simulations advance it explicitly.
pub type CreationTime = u64;

/// Stable identity of a worker process participating in creation provisioning.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CreationWorkerId(pub String);

/// Opaque authority minted for one claimed generation of a creation job.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CreationClaimToken(pub String);

/// Authority required to report results for a claimed creation job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationClaim {
    pub worker_id: CreationWorkerId,
    pub generation: u64,
    pub token: CreationClaimToken,
    pub lease_until: CreationTime,
}

/// Kind of durable creation accepted for a conversation shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreationKind {
    InitialTurn { message_id: String },
    SeededEmpty,
}

/// Restart boundary within one provisioning generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CreationStage {
    ValidateIntent,
    ResolveRepository,
    ReserveResources,
    MaterializeWorktree,
    FinalizeAttachments,
    ExpandInitialMessage,
    CommitMetadata,
    BootstrapInitialTurn,
    Finalize,
}

impl CreationStage {
    /// The next durable stage, or `None` after finalization.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::ValidateIntent => Some(Self::ResolveRepository),
            Self::ResolveRepository => Some(Self::ReserveResources),
            Self::ReserveResources => Some(Self::MaterializeWorktree),
            Self::MaterializeWorktree => Some(Self::FinalizeAttachments),
            Self::FinalizeAttachments => Some(Self::ExpandInitialMessage),
            Self::ExpandInitialMessage => Some(Self::CommitMetadata),
            Self::CommitMetadata => Some(Self::BootstrapInitialTurn),
            Self::BootstrapInitialTurn => Some(Self::Finalize),
            Self::Finalize => None,
        }
    }
}

/// Why a creation attempt could not continue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationError {
    pub kind: String,
    pub message: String,
}

/// Current durable lifecycle of a creation job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreationStatus {
    Accepted,
    Claimed(CreationClaim),
    RetryScheduled {
        next_attempt_at: CreationTime,
        last_error: CreationError,
    },
    Cancelling,
    Cancelled,
    DeletionPending,
    Ready,
    Failed(CreationError),
}

impl CreationStatus {
    /// Whether provisioning may never resume from this status.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Cancelled | Self::Ready | Self::Failed(_))
    }

    /// Whether the conversation is omitted from normal user-facing listings.
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        matches!(self, Self::DeletionPending)
    }
}

/// Durable state interpreted by both production and deterministic simulation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationProtocolState {
    pub kind: CreationKind,
    pub status: CreationStatus,
    pub stage: CreationStage,
    pub attempt: u32,
    pub generation: u64,
}

impl CreationProtocolState {
    #[must_use]
    pub const fn accepted(kind: CreationKind) -> Self {
        Self {
            kind,
            status: CreationStatus::Accepted,
            stage: CreationStage::ValidateIntent,
            attempt: 0,
            generation: 0,
        }
    }
}
