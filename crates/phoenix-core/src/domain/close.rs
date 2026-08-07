use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProductConversationId(String);

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
                Self::AwaitingStopWorkConfirmation | Self::SettlingActiveWork
            ),
            Self::AwaitingStopWorkConfirmation => next == Self::SettlingActiveWork,
            Self::SettlingActiveWork => matches!(
                next,
                Self::CancelRequestedDuringSettlement | Self::AwaitingRetirementInspection
            ),
            Self::CancelRequestedDuringSettlement => next == Self::Completed,
            Self::AwaitingRetirementInspection => matches!(
                next,
                Self::AwaitingLossConfirmation | Self::RetirementRequested
            ),
            Self::AwaitingLossConfirmation => matches!(
                next,
                Self::AwaitingRetirementInspection | Self::RetirementRequested
            ),
            Self::RetirementRequested => matches!(next, Self::NeedsRepair | Self::Completed),
            Self::NeedsRepair => matches!(next, Self::RetirementRequested | Self::Completed),
            Self::Completed => false,
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "failure_reason", rename_all = "snake_case")]
pub enum RetirementOutcome {
    Retired,
    AbsenceAdopted,
    Residual(RetirementFailureReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseObligation {
    pub attempt_id: String,
    pub product_conversation_id: ProductConversationId,
    pub phase: ClosePhase,
    pub inspection_generation: Option<String>,
    pub inspection_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseInspection {
    pub attempt_id: String,
    pub scope: String,
    pub generation: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseInspectionLoss {
    pub attempt_id: String,
    pub scope: String,
    pub generation: String,
    pub category: LossCategory,
    pub item_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRetiredResource {
    pub attempt_id: String,
    pub scope: String,
    pub resource_kind: RetiredResourceKind,
    pub resource_identity: String,
    pub outcome: RetirementOutcome,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseTombstone {
    pub product_conversation_id: ProductConversationId,
    pub kind: CloseTombstoneKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseTombstoneKind {
    Root,
    Continuation,
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
    }

    #[test]
    fn product_conversation_id_round_trips() {
        let id = ProductConversationId::parse("root-1").unwrap();
        assert_eq!(id.as_str(), "root-1");
        assert_eq!(id.to_string(), "root-1");
        assert_eq!(ProductConversationId::from_str("root-1").unwrap(), id);
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

        assert!(ClosePhase::AwaitingBlockerResolution
            .can_transition_to(ClosePhase::AwaitingStopWorkConfirmation));
        assert!(
            ClosePhase::AwaitingBlockerResolution.can_transition_to(ClosePhase::SettlingActiveWork)
        );
        assert!(!ClosePhase::AwaitingBlockerResolution.can_transition_to(ClosePhase::Completed));
        assert!(ClosePhase::AwaitingStopWorkConfirmation
            .can_transition_to(ClosePhase::SettlingActiveWork));
        assert!(ClosePhase::SettlingActiveWork
            .can_transition_to(ClosePhase::CancelRequestedDuringSettlement));
        assert!(ClosePhase::SettlingActiveWork
            .can_transition_to(ClosePhase::AwaitingRetirementInspection));
        assert!(
            ClosePhase::CancelRequestedDuringSettlement.can_transition_to(ClosePhase::Completed)
        );
        assert!(ClosePhase::AwaitingRetirementInspection
            .can_transition_to(ClosePhase::AwaitingLossConfirmation));
        assert!(ClosePhase::AwaitingRetirementInspection
            .can_transition_to(ClosePhase::RetirementRequested));
        assert!(ClosePhase::AwaitingLossConfirmation
            .can_transition_to(ClosePhase::AwaitingRetirementInspection));
        assert!(
            ClosePhase::AwaitingLossConfirmation.can_transition_to(ClosePhase::RetirementRequested)
        );
        assert!(ClosePhase::RetirementRequested.can_transition_to(ClosePhase::NeedsRepair));
        assert!(ClosePhase::RetirementRequested.can_transition_to(ClosePhase::Completed));
        assert!(ClosePhase::NeedsRepair.can_transition_to(ClosePhase::RetirementRequested));
        assert!(ClosePhase::NeedsRepair.can_transition_to(ClosePhase::Completed));
        assert!(!ClosePhase::Completed.can_transition_to(ClosePhase::Completed));
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
    }

    #[test]
    fn retirement_outcome_serializes_as_typed_shape() {
        let retired = serde_json::to_value(RetirementOutcome::Retired).unwrap();
        assert_eq!(retired, serde_json::json!({"outcome": "retired"}));

        let absence = serde_json::to_value(RetirementOutcome::AbsenceAdopted).unwrap();
        assert_eq!(absence, serde_json::json!({"outcome": "absence_adopted"}));

        let residual = serde_json::to_value(RetirementOutcome::Residual(
            RetirementFailureReason::ManualRepairRequired,
        ))
        .unwrap();
        assert_eq!(
            residual,
            serde_json::json!({
                "outcome": "residual",
                "failure_reason": "manual_repair_required"
            })
        );
    }
}
