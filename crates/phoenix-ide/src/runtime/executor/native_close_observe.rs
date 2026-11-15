//! Exact retained observation application inside the source actor.

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_db::native_close::{NativeCloseObserveResult, NativeCloseRunRef};
use phoenix_db::workflow::LocalAuthorityResult;

use crate::runtime::native_close_application::{
    NativeCloseApplicationError, observe_command_from_executing_source,
};

use super::{
    CancellationToken, ConversationRuntime, LlmClient, RuntimeSettlementAuthority, Storage,
    ToolExecutor,
};

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    /// Invoked only by the SOURCE actor's dedicated acknowledged request branch.
    /// No actor RPC, target lock acquisition or cleanup is permitted here. Exclusive
    /// actor ownership spans live source matching, receipt lookup, actual DB commit
    /// and durable-result classification. Source/target DB authority is rechecked
    /// transactionally, not inferred from the queued request or watch state.
    pub(super) async fn execute_native_close_observe(
        &mut self,
        source: SourceToolCall,
        referenced: NativeCloseRunRef,
        admitted: crate::runtime::AdmittedOperation,
    ) -> Result<NativeCloseObserveResult, NativeCloseApplicationError> {
        if !self.context.is_coordinator
            || self.context.is_sub_agent
            || self
                .fatal_external_effect_cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        // This is supporting correlation matching, not a separately reusable permit.
        let command = observe_command_from_executing_source(
            &self.state,
            &self.context.conversation_id,
            &source,
            referenced,
        )
        .map_err(|_| NativeCloseApplicationError::SourceRefused)?;
        // Production already installs the same database used by native admission.
        // An absent test/dormant dependency fails closed; no synthetic manager.
        let db = self
            .native_task_admission_db
            .as_ref()
            .ok_or(NativeCloseApplicationError::Unavailable)?;
        // Once this boundary starts, losing its future before a typed result must
        // close authority before releasing the admitted owner (REQ-BED-033).
        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native Close exact observation owner disappeared",
        };
        match db.apply_native_close_observe(&command).await {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                // Both committed success and established rollback are classified.
                authority.owner = None;
                result.map_err(|_| NativeCloseApplicationError::Storage)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                // Keep the guard armed: Drop closes the fence before owner release.
                Err(NativeCloseApplicationError::FatalAuthority)
            }
        }
    }
}
