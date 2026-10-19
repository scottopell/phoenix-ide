use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::ConvState;

use super::{GoalDbError, GoalExecutionBinding, GoalMutation, GoalTx, Result};

pub(super) async fn require_target_tx(
    tx: &mut GoalTx<'_>,
    owner: &ProductConversationId,
    expected_transcript: &str,
) -> Result<ConvState> {
    let admitted =
        crate::close_foundation::require_product_conversation_admission_tx(tx, expected_transcript)
            .await?;
    if &admitted != owner {
        return Err(GoalDbError::TargetChanged);
    }
    let mut aggregate = crate::Database::get_ordinary_product_conversation_on(tx, owner).await?;
    if aggregate.latest_transcript_row_id != expected_transcript {
        return Err(GoalDbError::TargetChanged);
    }
    aggregate
        .segments
        .pop()
        .map(|segment| segment.transcript_row.conversation.state)
        .ok_or(GoalDbError::TargetChanged)
}

pub(super) async fn require_settled_tx(
    tx: &mut GoalTx<'_>,
    mutation: &GoalMutation,
    expected_transcript: &str,
) -> Result<()> {
    let state = require_target_tx(tx, &mutation.owner, expected_transcript).await?;
    if !matches!(state, ConvState::Idle) {
        return Err(GoalDbError::Busy);
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT
         EXISTS(SELECT 1 FROM durable_turns t JOIN conversations c ON c.id=t.conversation_id
             WHERE c.product_conversation_id=?1 AND (t.owns_conversation=1 OR t.terminal_kind IS NULL))
         OR EXISTS(SELECT 1 FROM direct_turn_terminal_obligations o
             JOIN durable_turns t ON t.turn_id=o.turn_id JOIN conversations c ON c.id=t.conversation_id
             WHERE c.product_conversation_id=?1)
         OR EXISTS(SELECT 1 FROM steering_messages s JOIN conversations c ON c.id=s.conversation_id
             WHERE c.product_conversation_id=?1)
         OR EXISTS(SELECT 1 FROM automatic_continuation_admissions
             WHERE product_conversation_id=?1 AND phase NOT IN ('message_settled','superseded','failed'))
         OR EXISTS(SELECT 1 FROM product_continuation_reservations WHERE product_conversation_id=?1)
         OR EXISTS(SELECT 1 FROM continuation_dispatch_intents i
             JOIN conversations c ON c.id=i.parent_conversation_id OR c.id=i.successor_conversation_id
             WHERE c.product_conversation_id=?1)
         OR EXISTS(SELECT 1 FROM product_creation_jobs WHERE product_conversation_id=?1
             AND status NOT IN ('published','failed','cancelled'))",
    ).bind(mutation.owner.as_str()).fetch_one(&mut **tx).await?;
    if pending != 0 {
        return Err(GoalDbError::Busy);
    }
    Ok(())
}

pub(super) async fn require_live_report_tx(
    tx: &mut GoalTx<'_>,
    binding: &GoalExecutionBinding,
    expected_transcript: &str,
) -> Result<()> {
    require_target_tx(tx, &binding.owner, expected_transcript).await?;
    let live: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM durable_turns
         WHERE turn_id=? AND workflow_id=? AND conversation_id=?
           AND disposition='Runtime' AND owns_conversation=1 AND terminal_kind IS NULL)",
    )
    .bind(binding.execution.turn_id())
    .bind(binding.execution.workflow_id())
    .bind(expected_transcript)
    .fetch_one(&mut **tx)
    .await?;
    if live == 0 {
        return Err(GoalDbError::BindingConflict);
    }
    Ok(())
}
