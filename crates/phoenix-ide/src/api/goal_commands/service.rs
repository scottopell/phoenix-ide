use std::sync::Arc;

use phoenix_db::workflow::LocalAuthorityResult;

use phoenix_core::domain::conversation_goal::{GoalHistoryEntry, GoalTimestampUs, GoalVersion};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::user_commands::{
    GoalUserChange, GoalUserCommand, GoalUserReceipt,
};

use super::goal_error;
use crate::api::{auth::OwnerAuthenticated, handlers::AppError};
use crate::runtime::RuntimeManager;

pub(super) struct GoalCommandService {
    runtime: Arc<RuntimeManager>,
    #[cfg(test)]
    read_publication_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    receipt_lookup_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    command_publication_barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl GoalCommandService {
    pub(super) fn new(runtime: Arc<RuntimeManager>) -> Self {
        Self {
            runtime,
            #[cfg(test)]
            read_publication_barrier: None,
            #[cfg(test)]
            receipt_lookup_barrier: None,
            #[cfg(test)]
            command_publication_barrier: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_read_publication_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.read_publication_barrier = Some(barrier);
        self
    }

    #[cfg(test)]
    pub(super) fn with_receipt_lookup_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.receipt_lookup_barrier = Some(barrier);
        self
    }

    #[cfg(test)]
    pub(super) fn with_command_publication_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.command_publication_barrier = Some(barrier);
        self
    }

    fn publication_result<T>(&self, result: Result<T, AppError>) -> Result<T, AppError> {
        let _publication = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        result
    }

    pub(super) async fn current(
        &self,
        owner: &ProductConversationId,
    ) -> Result<Option<GoalHistoryEntry>, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .conversation_goal_current_entry(owner)
            .await
            .map_err(goal_error);
        #[cfg(test)]
        if let Some(barrier) = &self.read_publication_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        self.publication_result(result)
    }

    pub(super) async fn history(
        &self,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<Vec<GoalHistoryEntry>, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .conversation_goal_history(owner, after)
            .await
            .map_err(goal_error);
        #[cfg(test)]
        if let Some(barrier) = &self.read_publication_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        self.publication_result(result)
    }

    pub(super) fn established_receipt(
        &self,
        outcome: LocalAuthorityResult<
            Result<GoalUserReceipt, phoenix_db::conversation_goals::GoalDbError>,
        >,
    ) -> Result<GoalUserReceipt, AppError> {
        match outcome {
            LocalAuthorityResult::DurableFactEstablished(result) => result.map_err(goal_error),
            LocalAuthorityResult::DurableFactUnclassified => {
                self.runtime
                    .signal_fatal_local_authority("goal_command_result");
                Err(AppError::Internal(
                    "goal command durable result is unclassified".into(),
                ))
            }
        }
    }

    pub(super) async fn command(
        &self,
        owner: ProductConversationId,
        request_id: uuid::Uuid,
        expected_version: GoalVersion,
        change: GoalUserChange,
        authenticated_owner: OwnerAuthenticated,
    ) -> Result<GoalUserReceipt, AppError> {
        let result = self
            .command_unpublished(
                owner,
                request_id,
                expected_version,
                change,
                authenticated_owner,
            )
            .await;
        #[cfg(test)]
        if let Some(barrier) = &self.command_publication_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        self.publication_result(result)
    }

    async fn command_unpublished(
        &self,
        owner: ProductConversationId,
        request_id: uuid::Uuid,
        expected_version: GoalVersion,
        change: GoalUserChange,
        _authenticated_owner: OwnerAuthenticated,
    ) -> Result<GoalUserReceipt, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let db = self.runtime.db();
        let instance = db
            .instance_id()
            .await
            .map_err(crate::api::product_conversations::db_to_app)?;
        let command = GoalUserCommand {
            owner: owner.clone(),
            principal_id: uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                instance.to_string().as_bytes(),
            ),
            request_id,
            expected_version,
            change,
        };
        let ordinary = db
            .get_ordinary_product_conversation(&owner)
            .await
            .map_err(crate::api::product_conversations::db_to_app)?;
        let transcript = ordinary.latest_transcript_row_id;
        #[cfg(test)]
        if let Some(barrier) = &self.receipt_lookup_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        if let Some(receipt) = db
            .retained_user_goal_command(&command, &transcript)
            .await
            .map_err(goal_error)?
        {
            return Ok(receipt);
        }
        let clock = || {
            GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
                .map_err(phoenix_db::conversation_goals::GoalDbError::from)
        };
        let Some(_product_guard) = self
            .runtime
            .try_lock_product_message_admission(owner.as_str())
            .await
        else {
            return self.established_receipt(
                db.reject_busy_user_goal_command(&command, &transcript, clock, || {
                    self.runtime
                        .signal_fatal_local_authority("goal_user_command_write");
                })
                .await,
            );
        };
        // Resolve again under product admission: continuation can change the latest transcript.
        let transcript = db
            .get_ordinary_product_conversation(&owner)
            .await
            .map_err(crate::api::product_conversations::db_to_app)?
            .latest_transcript_row_id;
        let Some(_acceptance_guard) = self.runtime.try_lock_message_acceptance(&transcript).await
        else {
            return self.established_receipt(
                db.reject_busy_user_goal_command(&command, &transcript, clock, || {
                    self.runtime
                        .signal_fatal_local_authority("goal_user_command_write");
                })
                .await,
            );
        };
        self.established_receipt(
            db.apply_user_goal_command(&command, &transcript, clock, || {
                self.runtime
                    .signal_fatal_local_authority("goal_user_command_write");
            })
            .await,
        )
    }
}
