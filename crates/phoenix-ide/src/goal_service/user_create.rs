use std::sync::Arc;

use phoenix_core::domain::{
    conversation_goal::{GoalText, GoalTimestampUs, GoalVersion},
    product_conversation::ProductConversationId,
};
use phoenix_db::conversation_goals::{
    user_create::{GoalCreateCommand, GoalCreateReceipt},
    GoalDbError,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{api::auth::OwnerAuthenticated, runtime::RuntimeManager};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalCreateRequest {
    pub request_id: Uuid,
    pub expected_version: GoalVersion,
    pub objective: GoalText,
}

#[derive(Default)]
pub(crate) struct GoalCreateService {
    runtime: Option<Arc<RuntimeManager>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GoalCreateServiceError {
    #[error("authenticated goal Create is not installed")]
    Disabled,
    #[error(transparent)]
    Storage(#[from] crate::db::DbError),
    #[error(transparent)]
    Goal(#[from] GoalDbError),
    #[error(transparent)]
    HostIdentity(#[from] uuid::Error),
    #[error("host clock is outside the supported goal timestamp range")]
    Clock,
}

impl GoalCreateService {
    pub(crate) async fn create(
        &self,
        _authenticated: OwnerAuthenticated,
        owner: ProductConversationId,
        request: GoalCreateRequest,
    ) -> Result<GoalCreateReceipt, GoalCreateServiceError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(GoalCreateServiceError::Disabled)?;
        let db = runtime.db();
        let principal_id = Uuid::parse_str(&db.instance_id().await?.to_string())?;
        let input = GoalCreateCommand {
            owner,
            principal_id,
            request_id: request.request_id,
            expected_version: request.expected_version,
            objective: request.objective,
        };
        let at = GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
            .map_err(|_| GoalCreateServiceError::Clock)?;
        let target = db.get_ordinary_product_conversation(&input.owner).await?;
        if let Some(receipt) = db
            .retained_user_goal_create(&input, &target.latest_transcript_row_id)
            .await?
        {
            return Ok(receipt);
        }
        let Some(_product) = runtime
            .try_lock_product_message_admission(input.owner.as_str())
            .await
        else {
            return Ok(db
                .reject_busy_user_goal_create(&input, &target.latest_transcript_row_id, at)
                .await?);
        };
        let target = db.get_ordinary_product_conversation(&input.owner).await?;
        let Some(_acceptance) = runtime
            .try_lock_message_acceptance(&target.latest_transcript_row_id)
            .await
        else {
            return Ok(db
                .reject_busy_user_goal_create(&input, &target.latest_transcript_row_id, at)
                .await?);
        };
        Ok(db
            .apply_user_goal_create(&input, &target.latest_transcript_row_id, at)
            .await?)
    }
}

#[cfg(test)]
mod tests;
