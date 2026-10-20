use std::sync::Arc;

use phoenix_core::domain::{
    conversation_goal::{
        GoalCommandDisposition as UserGoalDisposition, GoalHistoryEntry, GoalId, GoalText,
        GoalTimestampUs, GoalVersion,
    },
    product_conversation::ProductConversationId,
};
use phoenix_db::conversation_goals::{
    user_commands::{GoalUserChange, GoalUserCommand, GoalUserReceipt},
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalStateRequest {
    pub request_id: Uuid,
    pub expected_version: GoalVersion,
    pub goal_id: GoalId,
    pub disposition: UserGoalDisposition,
}

#[derive(Default)]
pub(crate) struct GoalCommandService {
    runtime: Option<Arc<RuntimeManager>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GoalCommandServiceError {
    #[error("authenticated goal commands are not installed")]
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

impl GoalCommandService {
    pub(crate) fn from_installed_runtime(
        runtime: Arc<RuntimeManager>,
    ) -> Result<Self, GoalCommandServiceError> {
        if runtime.db().goal_control_installation()
            != phoenix_db::conversation_goals::control::GoalControlInstallation::Installed
        {
            return Err(GoalCommandServiceError::Disabled);
        }
        Ok(Self {
            runtime: Some(runtime),
        })
    }

    pub(crate) async fn current(
        &self,
        _authenticated_owner: OwnerAuthenticated,
        owner: ProductConversationId,
    ) -> Result<Option<GoalHistoryEntry>, GoalCommandServiceError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(GoalCommandServiceError::Disabled)?;
        Ok(runtime.db().conversation_goal_current_entry(&owner).await?)
    }

    pub(crate) async fn history(
        &self,
        _authenticated_owner: OwnerAuthenticated,
        owner: ProductConversationId,
        after: GoalVersion,
    ) -> Result<Vec<GoalHistoryEntry>, GoalCommandServiceError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(GoalCommandServiceError::Disabled)?;
        Ok(runtime
            .db()
            .ordinary_conversation_goal_history(&owner, after)
            .await?)
    }

    pub(crate) async fn create(
        &self,
        authenticated: OwnerAuthenticated,
        owner: ProductConversationId,
        request: GoalCreateRequest,
    ) -> Result<GoalUserReceipt, GoalCommandServiceError> {
        self.apply(
            authenticated,
            owner,
            request.request_id,
            request.expected_version,
            GoalUserChange::Create {
                objective: request.objective,
            },
        )
        .await
    }

    pub(crate) async fn state(
        &self,
        authenticated: OwnerAuthenticated,
        owner: ProductConversationId,
        request: GoalStateRequest,
    ) -> Result<GoalUserReceipt, GoalCommandServiceError> {
        let disposition = request.disposition.into();
        self.apply(
            authenticated,
            owner,
            request.request_id,
            request.expected_version,
            GoalUserChange::State {
                goal_id: request.goal_id,
                disposition,
            },
        )
        .await
    }

    async fn apply(
        &self,
        _authenticated: OwnerAuthenticated,
        owner: ProductConversationId,
        request_id: Uuid,
        expected_version: GoalVersion,
        change: GoalUserChange,
    ) -> Result<GoalUserReceipt, GoalCommandServiceError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(GoalCommandServiceError::Disabled)?;
        let db = runtime.db();
        let principal_id = Uuid::parse_str(&db.instance_id().await?.to_string())?;
        let input = GoalUserCommand {
            owner,
            principal_id,
            request_id,
            expected_version,
            change,
        };
        let at = GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
            .map_err(|_| GoalCommandServiceError::Clock)?;
        let target = db.get_ordinary_product_conversation(&input.owner).await?;
        if let Some(receipt) = db
            .retained_user_goal_command(&input, &target.latest_transcript_row_id)
            .await?
        {
            return Ok(receipt);
        }
        let Some(_product) = runtime
            .try_lock_product_message_admission(input.owner.as_str())
            .await
        else {
            return Ok(db
                .reject_busy_user_goal_command(&input, &target.latest_transcript_row_id, at)
                .await?);
        };
        let target = db.get_ordinary_product_conversation(&input.owner).await?;
        let Some(_acceptance) = runtime
            .try_lock_message_acceptance(&target.latest_transcript_row_id)
            .await
        else {
            return Ok(db
                .reject_busy_user_goal_command(&input, &target.latest_transcript_row_id, at)
                .await?);
        };
        Ok(db
            .apply_user_goal_command(&input, &target.latest_transcript_row_id, at)
            .await?)
    }
}

#[cfg(test)]
mod tests;
