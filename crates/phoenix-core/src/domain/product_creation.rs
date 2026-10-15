//! `ProductConversation` creation provisioning vocabulary.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
#[serde(rename_all = "snake_case")]
pub enum ProductCreationProvisioningStage {
    ValidateIntent,
    ResolveRepository,
    ResolveStartingPin,
    MaterializeWorktree,
    FinalizeAttachments,
    CommitMetadata,
    BootstrapInitialTurn,
    FinalizePublication,
}

impl ProductCreationProvisioningStage {
    #[must_use]
    pub fn predecessor(self) -> Option<Self> {
        match self {
            Self::ValidateIntent => None,
            Self::ResolveRepository => Some(Self::ValidateIntent),
            Self::ResolveStartingPin => Some(Self::ResolveRepository),
            Self::MaterializeWorktree => Some(Self::ResolveStartingPin),
            Self::FinalizeAttachments => Some(Self::MaterializeWorktree),
            Self::CommitMetadata => Some(Self::FinalizeAttachments),
            Self::BootstrapInitialTurn => Some(Self::CommitMetadata),
            Self::FinalizePublication => Some(Self::BootstrapInitialTurn),
        }
    }

    #[must_use]
    pub fn rank(self) -> u8 {
        match self.predecessor() {
            None => 0,
            Some(previous) => previous.rank() + 1,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValidateIntent => "validate_intent",
            Self::ResolveRepository => "resolve_repository",
            Self::ResolveStartingPin => "resolve_starting_pin",
            Self::MaterializeWorktree => "materialize_worktree",
            Self::FinalizeAttachments => "finalize_attachments",
            Self::CommitMetadata => "commit_metadata",
            Self::BootstrapInitialTurn => "bootstrap_initial_turn",
            Self::FinalizePublication => "finalize_publication",
        }
    }
}
