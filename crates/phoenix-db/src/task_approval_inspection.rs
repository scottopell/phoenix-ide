//! Read-only pending-task inspection from a proven in-flight Coordinator call.

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::task_approval::ReviewedProposal;
use phoenix_core::task_source::Priority;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

use crate::{Database, DbError, DbResult};

pub const INSPECT_PENDING_TASK_TOOL_NAME: &str = "inspect_pending_task";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectPendingTaskInput {
    pub target: ProductConversationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingTaskInspection {
    pub owner: ProductConversationId,
    pub current_transcript_id: String,
    pub pending: PendingTaskStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PendingTaskStatus {
    NoPending,
    Pending(PendingTaskSnapshot),
}

/// An owned projection of one persisted `AwaitingTaskApproval` aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTaskSnapshot {
    pub task_file: String,
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub reviewed_proposal: Option<ReviewedProposal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTaskApprovalAvailability {
    /// Provenance was captured; mutation, Stop and lifecycle admission are still required.
    CapturedProposalRequiresAdmission,
    /// Re-propose the task to capture canonical provenance before native approval.
    UnavailableRepropose,
}

impl PendingTaskSnapshot {
    #[must_use]
    pub fn native_approval_availability(&self) -> NativeTaskApprovalAvailability {
        if self.reviewed_proposal.is_some() {
            NativeTaskApprovalAvailability::CapturedProposalRequiresAdmission
        } else {
            NativeTaskApprovalAvailability::UnavailableRepropose
        }
    }
}

impl Database {
    /// Inspects the explicit owner's CURRENT member in the same read snapshot as source proof.
    /// This grants no mutation authority and does not consult or alter the runtime Stop latch.
    ///
    /// # Errors
    /// Rejects non-current Coordinators, sources other than the actual executing occurrence,
    /// changed canonical payloads, unavailable targets and database/serialization failures.
    pub async fn inspect_pending_task_from_coordinator(
        &self,
        source_transcript: &str,
        source_call: &SourceToolCall,
        input: &InspectPendingTaskInput,
    ) -> DbResult<PendingTaskInspection> {
        let mut connection = self.pool.acquire().await?;
        let mut tx = connection.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, source_transcript)
            .await?;
        let state_json: String =
            sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
                .bind(source_transcript)
                .fetch_one(&mut *tx)
                .await?;
        let source_state: ConvState = serde_json::from_str(&state_json)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        require_executing_source(&source_state, source_call, input)?;

        let aggregate = Self::get_ordinary_product_conversation_on(&mut tx, &input.target).await?;
        let current = aggregate
            .segments
            .iter()
            .find(|segment| {
                segment.transcript_row.conversation.id == aggregate.latest_transcript_row_id
            })
            .ok_or_else(|| {
                DbError::ConversationNotFound(aggregate.latest_transcript_row_id.clone())
            })?;
        let pending = if let ConvState::AwaitingTaskApproval {
            task_file,
            title,
            priority,
            plan,
            reviewed_proposal,
        } = &current.transcript_row.conversation.state
        {
            PendingTaskStatus::Pending(PendingTaskSnapshot {
                task_file: task_file.clone(),
                title: title.clone(),
                priority: *priority,
                plan: plan.clone(),
                reviewed_proposal: reviewed_proposal.clone(),
            })
        } else {
            PendingTaskStatus::NoPending
        };
        let inspection = PendingTaskInspection {
            owner: input.target.clone(),
            current_transcript_id: aggregate.latest_transcript_row_id,
            pending,
        };
        tx.commit().await?;
        Ok(inspection)
    }
}

fn invalid_source() -> DbError {
    DbError::MessageConflict(
        "pending-task inspection requires the exact executing source occurrence and payload".into(),
    )
}

fn require_executing_source(
    state: &ConvState,
    source: &SourceToolCall,
    input: &InspectPendingTaskInput,
) -> DbResult<()> {
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        assistant_message,
        ..
    } = state
    else {
        return Err(invalid_source());
    };
    if source.message_id.trim().is_empty()
        || source.tool_use_id.trim().is_empty()
        || assistant_message.message_id != source.message_id
        || current_tool.id != source.tool_use_id
        || current_tool.name() != INSPECT_PENDING_TASK_TOOL_NAME
        || remaining_tools
            .iter()
            .any(|tool| tool.id == source.tool_use_id)
        || completed_results
            .iter()
            .any(|result| result.tool_use_id() == source.tool_use_id)
    {
        return Err(invalid_source());
    }
    let requested =
        serde_json::to_value(input).map_err(|error| DbError::Serialization(error.to_string()))?;
    let executing = current_tool.input.to_value();
    let mut occurrences = assistant_message.content.iter().filter_map(|block| {
        if let ContentBlock::ToolUse { id, name, input } = block {
            (id == &source.tool_use_id).then_some((name, input))
        } else {
            None
        }
    });
    let Some((name, canonical)) = occurrences.next() else {
        return Err(invalid_source());
    };
    if occurrences.next().is_some()
        || name != INSPECT_PENDING_TASK_TOOL_NAME
        || canonical != &executing
        || canonical != &requested
    {
        return Err(invalid_source());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
