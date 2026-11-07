use std::sync::Arc;

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_db::conversation_goals::prepared::{GoalPreparationIntent, PreparedGoalAction};
use phoenix_db::workflow::LocalAuthorityResult;

use phoenix_core::domain::conversation_goal::{
    GoalAgentSource, GoalHistoryEntry, GoalPreparationSource, GoalSnapshot, GoalTimestampUs,
    GoalVersion,
};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::user_commands::{
    GoalUserChange, GoalUserCommand, GoalUserReceipt,
};
use phoenix_db::conversation_goals::{CoordinatorGoalWaitRequest, CoordinatorGoalWaitResult};

use super::goal_error;
use crate::api::{auth::OwnerAuthenticated, handlers::AppError};
use crate::runtime::{AdmittedOperation, RuntimeManager};

pub(super) struct CoordinatorGoalPreparationAuthorityPass {
    owner: Option<AdmittedOperation>,
}

impl CoordinatorGoalPreparationAuthorityPass {
    pub(super) fn new(owner: AdmittedOperation) -> Self {
        Self { owner: Some(owner) }
    }

    pub(super) fn complete<T>(
        mut self,
        result: LocalAuthorityResult<Result<T, phoenix_db::conversation_goals::GoalDbError>>,
    ) -> Result<T, AppError> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                self.owner.take();
                result.map_err(goal_error)
            }
            LocalAuthorityResult::DurableFactUnclassified => Err(AppError::Internal(
                "Goal preparation durability is unclassified; local authority closed".into(),
            )),
        }
    }
}

impl Drop for CoordinatorGoalPreparationAuthorityPass {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.close("coordinator_goal_preparation");
        }
    }
}

struct RemoteGoalApplyAuthorityPass {
    owner: Option<AdmittedOperation>,
}

impl RemoteGoalApplyAuthorityPass {
    fn new(owner: AdmittedOperation) -> Self {
        Self { owner: Some(owner) }
    }

    fn complete<T>(
        mut self,
        result: LocalAuthorityResult<Result<T, phoenix_db::conversation_goals::GoalDbError>>,
    ) -> Result<T, AppError> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                self.owner.take();
                result.map_err(goal_error)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                if let Some(owner) = self.owner.take() {
                    owner.close("remote_coordinator_goal_command_result");
                }
                Err(AppError::Internal(
                    "remote goal command durable result is unclassified".into(),
                ))
            }
        }
    }
}

impl Drop for RemoteGoalApplyAuthorityPass {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.close("remote_coordinator_goal_command_apply");
        }
    }
}

#[derive(Clone)]
pub(crate) struct GoalCommandService {
    runtime: Arc<RuntimeManager>,
    #[cfg(test)]
    read_publication_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    receipt_lookup_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    command_publication_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    remote_source_identity_barrier: Option<Arc<tokio::sync::Barrier>>,
}

impl GoalCommandService {
    pub(crate) fn new(runtime: Arc<RuntimeManager>) -> Self {
        Self {
            runtime,
            #[cfg(test)]
            read_publication_barrier: None,
            #[cfg(test)]
            receipt_lookup_barrier: None,
            #[cfg(test)]
            command_publication_barrier: None,
            #[cfg(test)]
            remote_source_identity_barrier: None,
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

    #[cfg(test)]
    pub(crate) fn with_remote_source_identity_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.remote_source_identity_barrier = Some(barrier);
        self
    }

    fn publication_result<T>(&self, result: Result<T, AppError>) -> Result<T, AppError> {
        let _publication = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        result
    }

    pub(crate) async fn admit_remote_source(
        &self,
        caller: &str,
        call: &SourceToolCall,
        turn: phoenix_core::domain::sm_event::DirectTurnTurnId,
        tool: &str,
    ) -> Result<(tokio::sync::OwnedMutexGuard<()>, GoalAgentSource), AppError> {
        let product: String =
            sqlx::query_scalar("SELECT product_conversation_id FROM conversations WHERE id=?")
                .bind(caller)
                .fetch_optional(self.runtime.db().pool())
                .await
                .map_err(|error| AppError::Internal(error.to_string()))?
                .ok_or_else(|| {
                    AppError::BadRequest("Coordinator product identity unavailable".into())
                })?;
        let product = ProductConversationId::parse(product)
            .map_err(|error| AppError::Internal(error.to_string()))?;
        #[cfg(test)]
        if let Some(barrier) = &self.remote_source_identity_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        let admission = self
            .runtime
            .try_lock_product_message_admission(product.as_str())
            .await
            .ok_or_else(|| {
                AppError::BadRequest("Coordinator message admission is closed".into())
            })?;
        let source = self
            .remote_source(caller, &product, call, turn, tool)
            .await?;
        Ok((admission, source))
    }

    async fn remote_source(
        &self,
        caller: &str,
        product: &ProductConversationId,
        call: &SourceToolCall,
        turn: phoenix_core::domain::sm_event::DirectTurnTurnId,
        tool: &str,
    ) -> Result<GoalAgentSource, AppError> {
        use phoenix_core::domain::conversation_goal::{GoalExecution, GoalText};
        use sqlx::Row as _;
        let db = self.runtime.db();
        db.require_current_coordinator(caller)
            .await
            .map_err(goal_error)?;
        let turn = i64::try_from(turn.0)
            .map_err(|_| AppError::BadRequest("durable turn ID out of range".into()))?;
        let row = sqlx::query("SELECT t.workflow_id,t.turn_id FROM conversations c JOIN durable_turns t ON t.conversation_id=c.id JOIN workflows w ON w.workflow_id=t.workflow_id AND w.generation=t.generation WHERE c.id=? AND c.product_conversation_id=? AND t.turn_id=? AND t.disposition='Runtime' AND t.terminal_kind IS NULL AND t.owns_conversation=1 AND w.status='Active'")
            .bind(caller).bind(product.as_str()).bind(turn).fetch_optional(db.pool()).await
            .map_err(|error| AppError::Internal(error.to_string()))?
            .ok_or_else(|| AppError::BadRequest("durable Coordinator turn provenance unavailable".into()))?;
        let content: Option<String> = sqlx::query_scalar("SELECT content FROM messages WHERE conversation_id=? AND message_id=? AND message_type='agent'")
            .bind(caller).bind(&call.message_id).fetch_optional(db.pool()).await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let valid = if let Some(content) = content {
            let blocks: Vec<phoenix_core::domain::llm_types::ContentBlock> =
                serde_json::from_str(&content)
                    .map_err(|error| AppError::Internal(error.to_string()))?;
            blocks.iter().any(|block| matches!(block, phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, name, .. } if id == &call.tool_use_id && name == tool))
        } else {
            let state: Option<String> =
                sqlx::query_scalar("SELECT state FROM conversations WHERE id=?")
                    .bind(caller)
                    .fetch_optional(db.pool())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
            matches!(state.and_then(|s| serde_json::from_str::<phoenix_db::ConvState>(&s).ok()),
                Some(phoenix_db::ConvState::ToolExecuting { current_tool, assistant_message, .. })
                if current_tool.name() == tool && current_tool.id == call.tool_use_id
                && assistant_message.message_id == call.message_id
                && assistant_message.content.iter().any(|block| matches!(block, phoenix_core::domain::llm_types::ContentBlock::ToolUse { id, name, .. } if id == &call.tool_use_id && name == tool)))
        };
        if !valid {
            return Err(AppError::BadRequest(
                "exact source tool-call provenance unavailable".into(),
            ));
        }
        Ok(GoalAgentSource {
            product_conversation_id: product.clone(),
            transcript_id: GoalText::parse(caller)
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
            execution: GoalExecution::parse(
                row.try_get("workflow_id")
                    .map_err(|e| AppError::Internal(e.to_string()))?,
                row.try_get("turn_id")
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
            message_id: GoalText::parse(call.message_id.clone())
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
            tool_use_id: GoalText::parse(call.tool_use_id.clone())
                .map_err(|e| AppError::BadRequest(e.to_string()))?,
        })
    }

    pub(crate) async fn prepare_remote(
        &self,
        authority: &phoenix_db::FederationSendAuthority,
        source: &phoenix_core::domain::conversation_goal::RemoteGoalAgentSource,
        owner: &ProductConversationId,
        intent: GoalPreparationIntent,
    ) -> Result<PreparedGoalAction, AppError> {
        let pass = CoordinatorGoalPreparationAuthorityPass::new(
            self.runtime
                .acquire_local_authority_pass()
                .map_err(|()| AppError::Internal("local authority is closed".into()))?,
        );
        let outcome = self
            .runtime
            .db()
            .prepare_remote_coordinator_goal_action(authority, source, owner, intent)
            .await;
        self.publication_result(pass.complete(outcome)?)
    }

    pub(crate) async fn apply_remote(
        &self,
        authority: &phoenix_db::FederationSendAuthority,
        reference: uuid::Uuid,
    ) -> Result<(PreparedGoalAction, Option<GoalSnapshot>, GoalUserReceipt), AppError> {
        let admission = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let db = self.runtime.db();
        let action = db
            .read_remote_coordinator_goal_preparation(authority, reference)
            .await
            .map_err(goal_error)?;
        let previous = if action.expected_version == GoalVersion::INITIAL {
            None
        } else {
            let after = GoalVersion::parse(action.expected_version.get() - 1)
                .map_err(|error| AppError::Internal(error.to_string()))?;
            let entry = db
                .conversation_goal_history(&action.owner, after)
                .await
                .map_err(goal_error)?
                .into_iter()
                .next()
                .filter(|entry| entry.snapshot().version() == action.expected_version)
                .ok_or_else(|| {
                    AppError::Internal(
                        "remote goal preparation previous snapshot is unavailable".into(),
                    )
                })?;
            Some(entry.snapshot().clone())
        };
        if let Some(receipt) = db
            .retained_remote_coordinator_goal_command(authority, reference)
            .await
            .map_err(goal_error)?
        {
            return self.publication_result(Ok((action, previous, receipt)));
        }
        let clock = || {
            GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
                .map_err(phoenix_db::conversation_goals::GoalDbError::from)
        };
        let lost = || {
            self.runtime
                .signal_fatal_local_authority("remote_coordinator_goal_command_write")
        };
        let Some(_product_guard) = self
            .runtime
            .try_lock_product_message_admission(action.owner.as_str())
            .await
        else {
            let pass = RemoteGoalApplyAuthorityPass::new(admission);
            let receipt = pass.complete(
                db.apply_remote_coordinator_goal_command(authority, reference, true, clock, lost)
                    .await,
            )?;
            return self.publication_result(Ok((action, previous, receipt)));
        };
        let transcript = db
            .get_ordinary_product_conversation(&action.owner)
            .await
            .map_err(crate::api::product_conversations::db_to_app)?
            .latest_transcript_row_id;
        let acceptance = self.runtime.try_lock_message_acceptance(&transcript).await;
        let pass = RemoteGoalApplyAuthorityPass::new(admission);
        let receipt = pass.complete(
            db.apply_remote_coordinator_goal_command(
                authority,
                reference,
                acceptance.is_none(),
                clock,
                lost,
            )
            .await,
        )?;
        self.publication_result(Ok((action, previous, receipt)))
    }

    pub(crate) async fn remote_prepared(
        &self,
        authority: &phoenix_db::FederationSendAuthority,
        reference: uuid::Uuid,
    ) -> Result<PreparedGoalAction, AppError> {
        let _pass = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .read_remote_coordinator_goal_preparation(authority, reference)
            .await
            .map_err(goal_error);
        self.publication_result(result)
    }

    pub(crate) async fn prepare_coordinator(
        &self,
        caller: &str,
        source_tool_call: &SourceToolCall,
        owner: &ProductConversationId,
        intent: GoalPreparationIntent,
        turn_id: phoenix_core::domain::sm_event::DirectTurnTurnId,
    ) -> Result<PreparedGoalAction, AppError> {
        let authority = CoordinatorGoalPreparationAuthorityPass::new(
            self.runtime
                .acquire_local_authority_pass()
                .map_err(|()| AppError::Internal("local authority is closed".into()))?,
        );
        let classified = self
            .runtime
            .db()
            .prepare_coordinator_goal_action(caller, owner, source_tool_call, turn_id, intent)
            .await;
        let result = match authority.complete(classified) {
            Ok(result) => self
                .runtime
                .db()
                .require_current_coordinator(caller)
                .await
                .map(|()| result)
                .map_err(goal_error),
            Err(error) => Err(error),
        };
        #[cfg(test)]
        if let Some(barrier) = &self.command_publication_barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        self.publication_result(result)
    }

    pub(crate) async fn prepared_snapshot(
        &self,
        caller: &str,
        reference: uuid::Uuid,
    ) -> Result<PreparedGoalAction, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .read_coordinator_goal_preparation(caller, reference)
            .await
            .map_err(goal_error);
        self.publication_result(result)
    }

    pub(crate) async fn authorize_coordinator(&self, caller: &str) -> Result<(), AppError> {
        self.runtime
            .db()
            .require_current_coordinator(caller)
            .await
            .map_err(goal_error)
    }

    pub(crate) async fn read_coordinator(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<(Option<GoalHistoryEntry>, Vec<GoalHistoryEntry>), AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .read_coordinator_goals(caller, owner, after)
            .await
            .map_err(goal_error);
        self.runtime
            .db()
            .require_current_coordinator(caller)
            .await
            .map_err(goal_error)?;
        self.publication_result(result)
    }

    pub(crate) async fn wait_coordinator(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        after: GoalVersion,
        request: CoordinatorGoalWaitRequest,
    ) -> Result<CoordinatorGoalWaitResult, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let result = self
            .runtime
            .db()
            .wait_for_coordinator_goals(caller, owner, after, request)
            .await
            .map_err(goal_error);
        self.runtime
            .db()
            .require_current_coordinator(caller)
            .await
            .map_err(goal_error)?;
        self.publication_result(result)
    }

    pub(crate) async fn apply_prepared(
        &self,
        caller: &str,
        reference: uuid::Uuid,
    ) -> Result<GoalUserReceipt, AppError> {
        let _authority = self
            .runtime
            .acquire_local_authority_pass()
            .map_err(|()| AppError::Internal("local authority is closed".into()))?;
        let action = self.prepared_snapshot(caller, reference).await?;
        let db = self.runtime.db();
        let instance = db
            .instance_id()
            .await
            .map_err(crate::api::product_conversations::db_to_app)?;
        let principal = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("coordinator-goal-tools:{instance}").as_bytes(),
        );
        let command = action.command(principal).map_err(goal_error)?;
        let result = self
            .apply_coordinator_command(caller, &action, &command)
            .await;
        db.require_current_coordinator(caller)
            .await
            .map_err(goal_error)?;
        self.publication_result(result)
    }

    async fn apply_coordinator_command(
        &self,
        caller: &str,
        action: &PreparedGoalAction,
        command: &GoalUserCommand,
    ) -> Result<GoalUserReceipt, AppError> {
        let GoalPreparationSource::Local(source) = &action.source else {
            return Err(AppError::BadRequest(
                "remote Goal preparation cannot enter local application".into(),
            ));
        };
        let db = self.runtime.db();
        if let Some(receipt) = db
            .retained_coordinator_goal_command(caller, command)
            .await
            .map_err(goal_error)?
        {
            return Ok(receipt);
        }
        let transcript = db
            .get_ordinary_product_conversation(&action.owner)
            .await
            .map_err(crate::api::product_conversations::db_to_app)?
            .latest_transcript_row_id;
        let clock = || {
            GoalTimestampUs::parse(chrono::Utc::now().timestamp_micros())
                .map_err(phoenix_db::conversation_goals::GoalDbError::from)
        };
        let lost = || {
            self.runtime
                .signal_fatal_local_authority("coordinator_goal_command_write");
        };
        let Some(_product_guard) = self
            .runtime
            .try_lock_product_message_admission(action.owner.as_str())
            .await
        else {
            return self.established_receipt(
                db.apply_coordinator_goal_command(
                    caller,
                    source,
                    command,
                    &transcript,
                    true,
                    clock,
                    lost,
                )
                .await,
            );
        };
        let transcript = db
            .get_ordinary_product_conversation(&action.owner)
            .await
            .map_err(crate::api::product_conversations::db_to_app)?
            .latest_transcript_row_id;
        let Some(_acceptance_guard) = self.runtime.try_lock_message_acceptance(&transcript).await
        else {
            return self.established_receipt(
                db.apply_coordinator_goal_command(
                    caller,
                    source,
                    command,
                    &transcript,
                    true,
                    clock,
                    lost,
                )
                .await,
            );
        };
        self.established_receipt(
            db.apply_coordinator_goal_command(
                caller,
                source,
                command,
                &transcript,
                false,
                clock,
                lost,
            )
            .await,
        )
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
