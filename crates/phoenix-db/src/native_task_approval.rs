//! Private approval-transaction foundation; no runtime or tool capability is exported.

use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{Message, SourceToolCall};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_core::domain::task_approval::ReviewedProposal;
use phoenix_core::task_handoff::ApprovedTaskSnapshot;
use phoenix_core::task_source::Priority;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

use crate::task_approval_inspection::{
    proposal_availability_tx, require_executing_source, NativeTaskApprovalAvailability,
    PendingTaskSnapshot,
};
use crate::{Database, DbError, DbResult};

const APPROVE_TASK_TOOL_NAME: &str = "approve_task";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NativeApprovalPlacement {
    #[allow(dead_code)] // Constructed by the future owning actor, not an exported DB service.
    ContinueHere,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NativeApprovalIntent {
    #[allow(dead_code)] // Constructed by the future owning actor, not an exported DB service.
    ApproveTaskAndRequestContinuation,
}

#[derive(Debug, Clone)]
pub(super) struct NativeTaskApprovalCommand {
    pub source_transcript_id: String,
    pub source: SourceToolCall,
    pub owner: ProductConversationId,
    pub current_transcript_id: String,
    pub pending: PendingTaskSnapshot,
    pub placement: NativeApprovalPlacement,
    pub intent: NativeApprovalIntent,
}

#[derive(Clone, Copy)]
pub(super) enum ApprovalSettlement<'a> {
    User(&'a Message, &'a ConvState, DateTime<Utc>),
    // The owning pre-Git actor has not been integrated; this variant is internal only.
    #[allow(dead_code)]
    Native(
        &'a Message,
        &'a ConvState,
        DateTime<Utc>,
        &'a NativeTaskApprovalCommand,
    ),
}

impl<'a> ApprovalSettlement<'a> {
    pub(super) fn state(self) -> (&'a Message, &'a ConvState, DateTime<Utc>) {
        match self {
            Self::User(message, state, at) | Self::Native(message, state, at, _) => {
                (message, state, at)
            }
        }
    }

    pub(super) fn native(self) -> Option<&'a NativeTaskApprovalCommand> {
        match self {
            Self::User(..) => None,
            Self::Native(_, _, _, command) => Some(command),
        }
    }
}

// Fixed-field serde structs, not caller-provided JSON/maps, define the internal encoding.
#[derive(Serialize)]
struct ApprovalInput<'a> {
    version: u8,
    owner: &'a ProductConversationId,
    current_transcript_id: &'a str,
    task_file: &'a str,
    title: &'a str,
    priority: Priority,
    plan: &'a str,
    reviewed_proposal: &'a ReviewedProposal,
    placement: NativeApprovalPlacement,
    intent: NativeApprovalIntent,
}

impl NativeTaskApprovalCommand {
    fn input(&self) -> DbResult<ApprovalInput<'_>> {
        Ok(ApprovalInput {
            version: 1,
            owner: &self.owner,
            current_transcript_id: &self.current_transcript_id,
            task_file: &self.pending.task_file,
            title: &self.pending.title,
            priority: self.pending.priority,
            plan: &self.pending.plan,
            reviewed_proposal: self.pending.reviewed_proposal.as_ref().ok_or_else(|| {
                conflict("native approval requires a captured proposal; inspect and re-propose")
            })?,
            placement: self.placement,
            intent: self.intent,
        })
    }

    fn input_value(&self) -> DbResult<serde_json::Value> {
        serde_json::to_value(self.input()?)
            .map_err(|error| DbError::Serialization(error.to_string()))
    }

    fn fingerprint(&self) -> DbResult<Vec<u8>> {
        #[derive(Serialize)]
        struct Identity<'a> {
            source_transcript_id: &'a str,
            source: &'a SourceToolCall,
            input: ApprovalInput<'a>,
        }
        let bytes = serde_json::to_vec(&Identity {
            source_transcript_id: &self.source_transcript_id,
            source: &self.source,
            input: self.input()?,
        })
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        let mut hash = Sha256::new();
        hash.update(b"phoenix.native-task-approval.v1\0");
        hash.update(bytes);
        Ok(hash.finalize().to_vec())
    }
}

fn conflict(message: &str) -> DbError {
    DbError::MessageConflict(message.to_owned())
}

/// Returns true only for an exact historical receipt after current/live source proof.
/// No caller can use this module as a receipt lookup API.
#[allow(clippy::too_many_lines)]
pub(super) async fn prepare_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeTaskApprovalCommand,
    target: &str,
    approved: &ApprovedTaskSnapshot,
    message: &Message,
    next_state: &ConvState,
) -> DbResult<bool> {
    crate::product_conversation_rename::require_current_gc_tx(tx, &command.source_transcript_id)
        .await?;
    let state_json: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id=?1")
        .bind(&command.source_transcript_id)
        .fetch_one(&mut **tx)
        .await?;
    let source_state: ConvState = serde_json::from_str(&state_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    require_executing_source(
        &source_state,
        &command.source,
        APPROVE_TASK_TOOL_NAME,
        &command.input_value()?,
    )?;
    if command.current_transcript_id != target {
        return Err(conflict(
            "native approval target does not match the command",
        ));
    }
    let fingerprint = command.fingerprint()?;
    let existing: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT request_sha256 FROM native_task_approval_receipts
         WHERE source_transcript_id=?1 AND source_message_id=?2 AND source_tool_use_id=?3",
    )
    .bind(&command.source_transcript_id)
    .bind(&command.source.message_id)
    .bind(&command.source.tool_use_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(existing) = existing {
        return if existing == fingerprint {
            Ok(true)
        } else {
            Err(conflict(
                "native approval source call was committed with a different request",
            ))
        };
    }

    if command.current_transcript_id != target
        || message.conversation_id != target
        || !matches!(next_state, ConvState::LlmRequesting { .. })
        || approved.title != command.pending.title
        || approved.priority != command.pending.priority
        || approved.plan != command.pending.plan
        || approved.artifact_body != command.pending.plan
    {
        return Err(conflict(
            "native approval settlement does not match the frozen request",
        ));
    }
    let aggregate = Database::get_ordinary_product_conversation_on(tx, &command.owner).await?;
    if aggregate.latest_transcript_row_id != target {
        return Err(conflict(
            "native approval target CURRENT changed; inspect again",
        ));
    }
    let current = aggregate
        .segments
        .iter()
        .find(|segment| {
            segment.transcript_row.conversation.id == aggregate.latest_transcript_row_id
        })
        .ok_or_else(|| conflict("native approval target transcript is missing"))?;
    let state = &current.transcript_row.conversation.state;
    let ConvState::AwaitingTaskApproval {
        task_file,
        title,
        priority,
        plan,
        reviewed_proposal,
    } = state
    else {
        return Err(conflict(
            "native approval target no longer has the reviewed pending proposal",
        ));
    };
    if task_file != &command.pending.task_file
        || title != &command.pending.title
        || priority != &command.pending.priority
        || plan != &command.pending.plan
        || reviewed_proposal != &command.pending.reviewed_proposal
        || proposal_availability_tx(tx, target, state).await?
            != NativeTaskApprovalAvailability::CapturedProposalRequiresAdmission
    {
        return Err(conflict(
            "native approval pending proposal changed or lacks its canonical witness",
        ));
    }
    let reviewed = command.input()?.reviewed_proposal;
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM native_task_approval_receipts
         WHERE target_transcript_id=?1 AND proposal_message_id=?2 AND proposal_tool_use_id=?3)",
    )
    .bind(target)
    .bind(&reviewed.occurrence.assistant_message_id)
    .bind(&reviewed.occurrence.tool_use_id)
    .fetch_one(&mut **tx)
    .await?;
    if duplicate {
        return Err(conflict(
            "native approval proposal was already committed by another source call",
        ));
    }
    Ok(false)
}

pub(super) async fn insert_receipt_tx(
    tx: &mut Transaction<'_, Sqlite>,
    command: &NativeTaskApprovalCommand,
    approval_message: &Message,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO native_task_approval_source_calls
         (source_transcript_id, source_message_id, source_tool_use_id) VALUES (?1, ?2, ?3)
         ON CONFLICT DO NOTHING",
    )
    .bind(&command.source_transcript_id)
    .bind(&command.source.message_id)
    .bind(&command.source.tool_use_id)
    .execute(&mut **tx)
    .await?;
    let reviewed = command.input()?.reviewed_proposal;
    sqlx::query(
        "INSERT INTO native_task_approval_receipts
         (source_transcript_id, source_message_id, source_tool_use_id, request_sha256,
          target_product_conversation_id, target_transcript_id, proposal_message_id,
          proposal_tool_use_id, approval_message_id, continuation_requested, committed_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)",
    )
    .bind(&command.source_transcript_id)
    .bind(&command.source.message_id)
    .bind(&command.source.tool_use_id)
    .bind(command.fingerprint()?)
    .bind(command.owner.as_str())
    .bind(&command.current_transcript_id)
    .bind(&reviewed.occurrence.assistant_message_id)
    .bind(&reviewed.occurrence.tool_use_id)
    .bind(&approval_message.message_id)
    .bind(Utc::now().timestamp_micros())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Additional native commit-error witness, never the user settlement's older-state test alone.
pub(super) async fn receipt_established(
    db: &Database,
    command: &NativeTaskApprovalCommand,
    approved: &ApprovedTaskSnapshot,
    approval_message: &Message,
) -> DbResult<bool> {
    let row = sqlx::query(
        "SELECT r.request_sha256, o.task_title, o.approved_title, o.approved_priority,
                o.approved_plan, o.approved_task_file, o.approved_artifact_body, m.content
         FROM native_task_approval_receipts r
         JOIN native_task_approval_source_calls s USING
              (source_transcript_id, source_message_id, source_tool_use_id)
         JOIN conversation_approved_task_objectives o ON o.conversation_id=r.target_transcript_id
         JOIN messages m ON m.conversation_id=r.target_transcript_id AND m.message_id=r.approval_message_id
         WHERE r.source_transcript_id=?1 AND r.source_message_id=?2 AND r.source_tool_use_id=?3
           AND r.target_product_conversation_id=?4 AND r.target_transcript_id=?5
           AND r.approval_message_id=?6 AND r.continuation_requested=1",
    )
    .bind(&command.source_transcript_id)
    .bind(&command.source.message_id)
    .bind(&command.source.tool_use_id)
    .bind(command.owner.as_str())
    .bind(&command.current_transcript_id)
    .bind(&approval_message.message_id)
    .fetch_optional(&db.pool)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    Ok(
        row.get::<Vec<u8>, _>("request_sha256") == command.fingerprint()?
            && row.get::<String, _>("task_title") == approved.task_title
            && row.get::<String, _>("approved_title") == approved.title
            && row.get::<String, _>("approved_priority")
                == serde_json::to_string(&approved.priority)
                    .map_err(|error| DbError::Serialization(error.to_string()))?
            && row.get::<String, _>("approved_plan") == approved.plan
            && row.get::<String, _>("approved_task_file") == approved.task_file
            && row.get::<String, _>("approved_artifact_body") == approved.artifact_body
            && row.get::<String, _>("content")
                == approval_message
                    .content
                    .to_stored_json()
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
    )
}

#[cfg(test)]
mod tests;
