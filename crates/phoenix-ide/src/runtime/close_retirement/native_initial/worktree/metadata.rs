//! Original-context metadata writes. SQLite owns the transaction before the
//! source-entry lease is acquired and retained through release; no source lock
//! spans a writer wait, and no admitted physical leaf opens a nested writer.

use std::{path::PathBuf, time::Instant};

use phoenix_db::{
    workflow::LocalAuthorityResult, BindCloseWorktreeFinalTombstoneObjectRequest,
    BindCloseWorktreeFinalTombstoneRequest, CloseWorktreeFinalTombstone, DbError, DbResult,
    RecordCloseWorktreeCleanupPlanRequest,
};
use tokio::runtime::Handle;

use super::{
    check_deadline, NativeCloseCommand, NativeCloseIntent, OriginalWorktreeEffectContext,
    SourceEntryWorker, WorkerEntryResult,
};

impl OriginalWorktreeEffectContext {
    pub(super) fn record_cleanup_plan(
        &mut self,
        runtime: &Handle,
        admin_path: PathBuf,
        admin_incarnation: String,
    ) -> Result<(), String> {
        let request = RecordCloseWorktreeCleanupPlanRequest {
            attempt_id: self.receipt.admitted.attempt_id.clone(),
            scope: self.scope.clone(),
            snapshot: self.snapshot.clone(),
            resource: self.resource.clone(),
            administrative_dir: admin_path,
            administrative_dir_incarnation: admin_incarnation,
        };
        self.begin_metadata();
        // Borrow the local handle, not `self`, so classification can retain its
        // lease while mutating the worker's release marker (or parking forever).
        let source_entry = self.source_entry.clone();
        let entry = || metadata_entry(&source_entry, &self.command, self.deadline);
        let result = match self.command.intent() {
            NativeCloseIntent::Initial { .. } => runtime.block_on(
                self.db
                    .record_native_close_initial_worktree_cleanup_plan_with_entry(
                        &self.command,
                        &self.receipt,
                        &self.live_source,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::ExplicitRetry { .. } => runtime.block_on(
                self.db
                    .record_native_close_retry_worktree_cleanup_plan_with_entry(
                        &self.command,
                        &self.receipt.admitted,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::Observe { .. } => refused_observation(),
        };
        self.classify_metadata(result)
    }

    pub(super) fn bind_final_root(
        &mut self,
        runtime: &Handle,
        root: PathBuf,
        device: u64,
        inode: u64,
    ) -> Result<(), String> {
        let request = BindCloseWorktreeFinalTombstoneRequest {
            attempt_id: self.receipt.admitted.attempt_id.clone(),
            scope: self.scope.clone(),
            snapshot: self.snapshot.clone(),
            resource: self.resource.clone(),
            tombstone: CloseWorktreeFinalTombstone {
                root,
                device,
                inode,
                object_device: None,
                object_inode: None,
            },
        };
        self.begin_metadata();
        // Borrow the local handle, not `self`, so classification can retain its
        // lease while mutating the worker's release marker (or parking forever).
        let source_entry = self.source_entry.clone();
        let entry = || metadata_entry(&source_entry, &self.command, self.deadline);
        let result = match self.command.intent() {
            NativeCloseIntent::Initial { .. } => runtime.block_on(
                self.db
                    .bind_native_close_initial_worktree_final_tombstone_with_entry(
                        &self.command,
                        &self.receipt,
                        &self.live_source,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::ExplicitRetry { .. } => runtime.block_on(
                self.db
                    .bind_native_close_retry_worktree_final_tombstone_with_entry(
                        &self.command,
                        &self.receipt.admitted,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::Observe { .. } => refused_observation(),
        };
        self.classify_metadata(result)
    }

    pub(super) fn bind_final_object(
        &mut self,
        runtime: &Handle,
        device: u64,
        inode: u64,
    ) -> Result<(), String> {
        let request = BindCloseWorktreeFinalTombstoneObjectRequest {
            attempt_id: self.receipt.admitted.attempt_id.clone(),
            scope: self.scope.clone(),
            snapshot: self.snapshot.clone(),
            resource: self.resource.clone(),
            object_device: device,
            object_inode: inode,
        };
        self.begin_metadata();
        // Borrow the local handle, not `self`, so classification can retain its
        // lease while mutating the worker's release marker (or parking forever).
        let source_entry = self.source_entry.clone();
        let entry = || metadata_entry(&source_entry, &self.command, self.deadline);
        let result = match self.command.intent() {
            NativeCloseIntent::Initial { .. } => runtime.block_on(
                self.db
                    .bind_native_close_initial_worktree_final_tombstone_object_with_entry(
                        &self.command,
                        &self.receipt,
                        &self.live_source,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::ExplicitRetry { .. } => runtime.block_on(
                self.db
                    .bind_native_close_retry_worktree_final_tombstone_object_with_entry(
                        &self.command,
                        &self.receipt.admitted,
                        request,
                        entry,
                    ),
            ),
            NativeCloseIntent::Observe { .. } => refused_observation(),
        };
        self.classify_metadata(result)
    }

    fn begin_metadata(&mut self) {
        assert!(
            !self.leaf_release_pending_classification,
            "previous Worktree leaf release must be classified first"
        );
        self.leaf_release_pending_classification = true;
    }

    fn classify_metadata<L>(
        &mut self,
        (result, lease): (LocalAuthorityResult<DbResult<()>>, Option<L>),
    ) -> Result<(), String> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                // Both committed success and confirmed refusal release the
                // writer. The returned Err must stop the physical worker before
                // any subsequent metadata write or compensating mutation.
                self.classify_confirmed_release();
                drop(lease);
                result.map_err(|error| format!("native Worktree metadata write refused: {error}"))
            }
            result @ LocalAuthorityResult::DurableFactUnclassified => {
                self.fatal
                    .close("native_close_initial_worktree_metadata_unknown_writer_release");
                // Keep the pending marker, result and entire outer worker stack
                // (repository lock, descriptors and original authority) alive.
                // Never turn unknown release into an ordinary traversal error.
                let _retained_result = (result, lease);
                loop {
                    std::thread::park();
                }
            }
        }
    }
}

fn refused_observation<L>() -> (LocalAuthorityResult<DbResult<()>>, Option<L>) {
    (
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(
            "observational Close cannot authorize a Worktree metadata write".into(),
        ))),
        None,
    )
}

// Called only after DB validation, while that same writer is held. The
// source worker uses try-lock (never waits), and checks its original command
// and deadline. Check this context's unchanged deadline at actual entry too.
fn metadata_entry<'a>(
    source_entry: &'a SourceEntryWorker,
    command: &NativeCloseCommand,
    deadline: Instant,
) -> DbResult<impl Sized + 'a> {
    match source_entry.try_acquire_first_effect(command) {
        WorkerEntryResult::Ran(lease) => {
            check_deadline(deadline).map_err(DbError::CloseFoundationPrecondition)?;
            Ok(lease)
        }
        WorkerEntryResult::Busy | WorkerEntryResult::Refused => {
            Err(DbError::CloseFoundationPrecondition(
                "original Close source unavailable at Worktree metadata write".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::executor::SourceEntryController;
    use phoenix_core::domain::{
        db_schema::SourceToolCall,
        llm_types::ContentBlock,
        product_conversation::ProductConversationId,
        sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput},
    };
    use phoenix_db::native_close::NativeCloseIntent;
    use std::time::Duration;

    fn original_source() -> (NativeCloseCommand, ConvState) {
        let source = SourceToolCall {
            message_id: "metadata-source-message".into(),
            tool_use_id: "metadata-source-call".into(),
        };
        let intent = NativeCloseIntent::Initial {
            target: ProductConversationId::parse("metadata-target").unwrap(),
        };
        let input = intent.source_input();
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::new(),
            pending_sub_agents: Vec::new(),
            assistant_message: AssistantMessage::new(
                source.message_id.clone(),
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id.clone(),
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        };
        (
            NativeCloseCommand::new("metadata-coordinator".into(), source, intent).unwrap(),
            state,
        )
    }

    #[test]
    fn metadata_gate_requires_current_actor_entry_and_refuses_revocation() {
        let (command, state) = original_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut controller = SourceEntryController::new(command.clone(), deadline.into());
        let worker = controller.worker();
        assert!(metadata_entry(&worker, &command, deadline).is_err());
        let _ = controller.revalidate(&state, &command);
        assert!(worker.is_eligible_unrevoked_before_deadline_for_test());
        let lease = metadata_entry(&worker, &command, deadline).unwrap();
        assert!(matches!(
            worker.try_first_effect(&command, || ()),
            WorkerEntryResult::Busy
        ));
        drop(lease);
        assert!(matches!(
            worker.try_first_effect(&command, || ()),
            WorkerEntryResult::Ran(())
        ));
        controller.revoke();
        assert!(metadata_entry(&worker, &command, deadline).is_err());
    }

    #[test]
    fn metadata_gate_never_substitutes_a_fresh_command_for_the_original() {
        let (command, state) = original_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut controller = SourceEntryController::new(command.clone(), deadline.into());
        let worker = controller.worker();
        let _ = controller.revalidate(&state, &command);
        assert!(worker.is_eligible_unrevoked_before_deadline_for_test());
        let foreign = NativeCloseCommand::new(
            "metadata-coordinator".into(),
            SourceToolCall {
                message_id: "different-message".into(),
                tool_use_id: "different-call".into(),
            },
            NativeCloseIntent::Initial {
                target: ProductConversationId::parse("metadata-target").unwrap(),
            },
        )
        .unwrap();
        assert!(metadata_entry(&worker, &foreign, deadline).is_err());
        assert!(metadata_entry(&worker, &command, deadline).is_err());
    }

    #[test]
    fn metadata_gate_checks_unchanged_deadline_inside_the_source_callback() {
        let (command, state) = original_source();
        let source_deadline = Instant::now() + Duration::from_secs(60);
        let mut controller = SourceEntryController::new(command.clone(), source_deadline.into());
        let worker = controller.worker();
        let _ = controller.revalidate(&state, &command);
        assert!(worker.is_eligible_unrevoked_before_deadline_for_test());
        // Eligibility is still open, but the original context deadline expired
        // while waiting for the writer. No refreshed source deadline can save it.
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(metadata_entry(&worker, &command, expired).is_err());
        assert!(worker.is_eligible_unrevoked_before_deadline_for_test());
    }

    #[test]
    fn metadata_gate_refuses_busy_source_without_waiting_or_nested_entry() {
        let (command, state) = original_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut controller = SourceEntryController::new(command.clone(), deadline.into());
        let worker = controller.worker();
        let _ = controller.revalidate(&state, &command);
        assert!(worker.is_eligible_unrevoked_before_deadline_for_test());
        let result =
            worker.try_first_effect(&command, || metadata_entry(&worker, &command, deadline));
        assert!(matches!(result, WorkerEntryResult::Ran(Err(_))));
    }
}
