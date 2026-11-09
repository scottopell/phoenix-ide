//! Internal frozen-pending checks and closed outcomes for the unexposed native command.

use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::task_approval::ReviewedProposal;
use phoenix_core::task_source::TaskSource;
use phoenix_db::task_approval_inspection::{
    NativeTaskApprovalAvailability, PendingTaskInspection, PendingTaskSnapshot, PendingTaskStatus,
};
use phoenix_db::{Database, DbError, MessageTargetAdmission, ProductConversationAdmission};

#[derive(Debug, Clone)]
pub(crate) struct FrozenPendingTask {
    owner: ProductConversationId,
    transcript_id: String,
    pending: PendingTaskSnapshot,
}

/// A transient observation, never a capability: full admission must be repeated at the effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTaskAdmissionReadiness {
    PendingMatchesRequiresFullAdmission,
    StalePending,
    TargetChanged,
    Unavailable,
}

impl FrozenPendingTask {
    #[allow(dead_code)]
    pub(crate) fn from_inspection(
        inspection: PendingTaskInspection,
    ) -> Result<Self, NativeTaskAdmissionReadiness> {
        let PendingTaskStatus::Pending(pending) = inspection.pending else {
            return Err(NativeTaskAdmissionReadiness::StalePending);
        };
        if pending.native_approval_availability()
            != NativeTaskApprovalAvailability::CapturedProposalRequiresAdmission
            || pending.reviewed_proposal.is_none()
        {
            return Err(NativeTaskAdmissionReadiness::Unavailable);
        }
        Ok(Self {
            owner: inspection.owner,
            transcript_id: inspection.current_transcript_id,
            pending,
        })
    }

    pub(crate) fn transcript_id(&self) -> &str {
        &self.transcript_id
    }

    pub(crate) async fn check_at_actor(
        &self,
        db: &Database,
        state: &ConvState,
    ) -> Result<NativeTaskAdmissionReadiness, String> {
        use NativeTaskAdmissionReadiness::{
            PendingMatchesRequiresFullAdmission, StalePending, TargetChanged, Unavailable,
        };
        let ConvState::AwaitingTaskApproval {
            task_file,
            title,
            priority,
            plan,
            reviewed_proposal,
        } = state
        else {
            return Ok(StalePending);
        };
        let Some(reviewed) = reviewed_proposal else {
            return Ok(Unavailable);
        };
        let aggregate = match db.get_ordinary_product_conversation(&self.owner).await {
            Ok(aggregate) => aggregate,
            Err(DbError::ConversationNotFound(_)) => return Ok(TargetChanged),
            Err(error) => return Err(error.to_string()),
        };
        if aggregate.latest_transcript_row_id != self.transcript_id {
            return Ok(TargetChanged);
        }
        let Some(member) = aggregate
            .segments
            .iter()
            .find(|segment| segment.transcript_row.conversation.id == self.transcript_id)
        else {
            return Ok(TargetChanged);
        };
        let conversation = &member.transcript_row.conversation;
        if conversation.product_conversation_id != self.owner {
            return Ok(TargetChanged);
        }
        if conversation.runtime_role == phoenix_core::work_scope::RuntimeRole::SubAgent
            || !matches!(
                db.message_target_admission(&self.transcript_id)
                    .await
                    .map_err(|error| error.to_string())?,
                MessageTargetAdmission::Aggregate(ProductConversationAdmission::Accepted { .. })
                    | MessageTargetAdmission::StandaloneAvailable
            )
        {
            return Ok(Unavailable);
        }
        if task_file != &self.pending.task_file
            || title != &self.pending.title
            || priority != &self.pending.priority
            || plan != &self.pending.plan
            || reviewed_proposal != &self.pending.reviewed_proposal
        {
            return Ok(StalePending);
        }
        let Some(source) = TaskSource::detect(task_file) else {
            return Ok(Unavailable);
        };
        if reviewed.occurrence.assistant_message_id.trim().is_empty()
            || reviewed.occurrence.tool_use_id.trim().is_empty()
            || reviewed != &ReviewedProposal::capture(reviewed.occurrence.clone(), &source, plan)
        {
            return Ok(Unavailable);
        }
        Ok(PendingMatchesRequiresFullAdmission)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeTaskApprovalOutcome {
    Committed(phoenix_db::native_task_approval::NativeTaskApprovalReceipt),
    NotApplied(NativeTaskApprovalRefusal),
    Unsupported(NativeTaskApprovalUnsupported),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTaskApprovalRefusal {
    PendingChanged,
    TargetChanged,
    SourceOrRequestRefused,
    RequestConflict,
    BeforeGitRefused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeTaskApprovalUnsupported {
    FollowUp,
    DetachedCheckpoint,
    InstalledGoalControls,
}

impl NativeTaskApprovalOutcome {
    pub(crate) fn preflight_error(
        error: phoenix_db::native_task_approval::NativeTaskApprovalError,
    ) -> Self {
        use phoenix_db::native_task_approval::NativeTaskApprovalError;
        tracing::debug!(%error, "native task approval preflight refused");
        match error {
            NativeTaskApprovalError::RequestConflict => {
                Self::NotApplied(NativeTaskApprovalRefusal::RequestConflict)
            }
            NativeTaskApprovalError::Database(
                DbError::MessageConflict(_) | DbError::ConversationNotFound(_),
            ) => Self::NotApplied(NativeTaskApprovalRefusal::SourceOrRequestRefused),
            NativeTaskApprovalError::Database(_) => Self::Unavailable,
        }
    }
}

impl FrozenPendingTask {
    pub(crate) fn from_command(
        command: &phoenix_db::native_task_approval::NativeTaskApprovalCommand,
    ) -> Self {
        Self {
            owner: command.owner.clone(),
            transcript_id: command.current_transcript_id.clone(),
            pending: command.pending.clone(),
        }
    }
}
