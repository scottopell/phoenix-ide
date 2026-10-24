//! Goal application boundary; no HTTP, tool or automatic-follow-up adapter is installed.

use phoenix_core::domain::conversation_goal::{
    GoalActor, GoalChange, GoalHistoryEntry, GoalMutation, GoalSnapshot, GoalTimestampUs,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::GoalDbError;

use crate::runtime::RuntimeManager;

#[allow(dead_code, reason = "goal read adapter is not installed yet")]
pub(crate) async fn get(
    runtime: &RuntimeManager,
    owner: &ProductConversationId,
) -> Result<Option<GoalSnapshot>, GoalDbError> {
    runtime
        .db()
        .get_ordinary_product_conversation(owner)
        .await?;
    runtime.db().conversation_goal(owner).await
}

#[allow(
    dead_code,
    reason = "trusted goal command adapters are not installed yet"
)]
pub(crate) async fn apply(
    runtime: &RuntimeManager,
    mutation: &GoalMutation,
    at: GoalTimestampUs,
) -> Result<GoalHistoryEntry, GoalDbError> {
    if let Some(accepted) = runtime
        .db()
        .accepted_conversation_goal_mutation(mutation)
        .await?
    {
        return Ok(accepted);
    }
    match &mutation.change {
        GoalChange::Report { .. } => {
            let GoalActor::OwningAgent(source) = &mutation.attribution.requested_by else {
                return Err(GoalDbError::BindingConflict);
            };
            runtime
                .db()
                .apply_conversation_goal(mutation, source.transcript_id.as_str(), at)
                .await
        }
        GoalChange::Create { .. } | GoalChange::SetDisposition { .. } => {
            let _product = runtime
                .try_lock_product_message_admission(mutation.owner.as_str())
                .await
                .ok_or(GoalDbError::Busy)?;
            let owner = runtime
                .db()
                .get_ordinary_product_conversation(&mutation.owner)
                .await?;
            let _acceptance = runtime
                .try_lock_message_acceptance(&owner.latest_transcript_row_id)
                .await
                .ok_or(GoalDbError::Busy)?;
            runtime
                .db()
                .apply_conversation_goal(mutation, &owner.latest_transcript_row_id, at)
                .await
        }
    }
}

#[cfg(test)]
mod tests;
