use super::*;
use crate::runtime::task_approval_admission::{
    FrozenPendingTask, NativeTaskAdmissionReadiness, NativeTaskApprovalOutcome,
    NativeTaskApprovalRefusal, NativeTaskApprovalUnsupported,
};
use phoenix_db::native_task_approval::{
    NativeTaskApprovalCommand, NativeTaskApprovalInspection, NativeTaskApprovalReceipt,
};
use phoenix_db::workflow::LocalAuthorityResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApprovalGitProgress {
    BeforeEffects,
    EffectsMayHaveOccurred,
}

#[derive(Debug)]
pub(super) enum NativeTaskApprovalExecutionError {
    Refused(NativeTaskApprovalRefusal),
    Database(phoenix_db::DbError),
    Unclassified(String),
}

impl std::fmt::Display for NativeTaskApprovalExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "native approval refused: {reason:?}"),
            Self::Database(error) => write!(formatter, "native approval database error: {error}"),
            Self::Unclassified(error) => formatter.write_str(error),
        }
    }
}

impl From<String> for NativeTaskApprovalExecutionError {
    fn from(error: String) -> Self {
        Self::Unclassified(error)
    }
}

pub(super) type PreGitCheck =
    Box<dyn FnOnce() -> Result<(), NativeTaskApprovalExecutionError> + Send>;

pub(super) enum ApprovalOrigin {
    User,
    Native {
        command: Box<NativeTaskApprovalCommand>,
        progress: ApprovalGitProgress,
        receipt: Option<Box<NativeTaskApprovalReceipt>>,
    },
}

impl ApprovalOrigin {
    pub(super) fn is_native(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub(super) fn effects_may_have_occurred(&mut self) {
        self.set_progress(ApprovalGitProgress::EffectsMayHaveOccurred);
    }

    pub(super) fn set_progress(&mut self, value: ApprovalGitProgress) {
        if let Self::Native { progress, .. } = self {
            *progress = value;
        }
    }
}

impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(super) async fn execute_native_task_approval(
        &mut self,
        command: NativeTaskApprovalCommand,
    ) -> NativeTaskApprovalOutcome {
        use NativeTaskApprovalOutcome::{Committed, NotApplied, Unavailable, Unsupported};
        let Some(db) = self.native_task_admission_db.clone() else {
            return Unavailable;
        };
        // Historical success is checked before target pending state or new-effect controls.
        match db.inspect_native_task_approval_command(&command).await {
            Ok(NativeTaskApprovalInspection::Replay(receipt)) => return Committed(receipt),
            Ok(NativeTaskApprovalInspection::RequiresOwningActorAdmission) => {}
            Err(error) => return NativeTaskApprovalOutcome::preflight_error(&error),
        }
        if self.context.conversation_id != command.current_transcript_id
            || self.context.is_coordinator
            || self.context.is_sub_agent
        {
            return NotApplied(NativeTaskApprovalRefusal::TargetChanged);
        }
        let frozen = FrozenPendingTask::from_command(&command);
        match frozen.check_at_actor(&db, &self.state).await {
            Ok(NativeTaskAdmissionReadiness::PendingMatchesRequiresFullAdmission) => {}
            Ok(_) => return NotApplied(NativeTaskApprovalRefusal::PendingChanged),
            Err(_) => return Unavailable,
        }
        // No new work is allowed with installed controls until the Git worker owns
        // the dispatch-versus-Stop gate. Replay above does not require this gate.
        if db.goal_control_installation()
            == phoenix_db::conversation_goals::control::GoalControlInstallation::Installed
        {
            return Unsupported(NativeTaskApprovalUnsupported::InstalledGoalControls);
        }
        let approval = TaskApprovalInput {
            task_file: command.pending.task_file.clone(),
            title: command.pending.title.clone(),
            priority: command.pending.priority,
            plan: command.pending.plan.clone(),
            reviewed_proposal: command.pending.reviewed_proposal.clone(),
        };
        let Ok(mut admitted) = self.admit_authoritative_effect() else {
            return Unavailable;
        };
        let mut origin = ApprovalOrigin::Native {
            command: Box::new(command),
            progress: ApprovalGitProgress::BeforeEffects,
            receipt: None,
        };
        let execution = match self.resolve_task_approval_execution().await {
            Ok(TaskApprovalExecution::Initial) => TaskApprovalExecution::Initial,
            Ok(TaskApprovalExecution::FollowUp) => TaskApprovalExecution::FollowUp,
            Ok(TaskApprovalExecution::DetachedApprovedTaskCheckpoint) => {
                return Unsupported(NativeTaskApprovalUnsupported::DetachedCheckpoint);
            }
            Err(error) => {
                return self
                    .settle_native_approval_outcome(
                        origin,
                        Err(NativeTaskApprovalExecutionError::Unclassified(error)),
                        &mut admitted,
                    )
                    .await;
            }
        };
        let ApprovalOrigin::Native { command, .. } = &origin else {
            unreachable!();
        };
        #[cfg(test)]
        {
            let gate = self.native_early_target_gate.lock().unwrap().take();
            if let Some(super::super::NativeTaskApprovalEarlyTargetGate { entered, release }) = gate
            {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
        if let Err(error) = check_native_target(
            &db,
            command,
            &self.context.resource_scope,
            self.context.filesystem_root(),
        )
        .await
        {
            return self
                .settle_native_approval_outcome(origin, Err(error), &mut admitted)
                .await;
        }
        let result = self
            .execute_approve_task_with_origin(approval, execution, &mut admitted, &mut origin)
            .await;
        self.settle_native_approval_outcome(origin, result, &mut admitted)
            .await
    }

    async fn settle_native_approval_outcome(
        &mut self,
        origin: ApprovalOrigin,
        result: Result<(), NativeTaskApprovalExecutionError>,
        admitted: &mut crate::runtime::AdmittedOperation,
    ) -> NativeTaskApprovalOutcome {
        use NativeTaskApprovalOutcome::{Committed, NotApplied, Unavailable};
        // Passive, one-shot evidence from the actual execution result. The boolean
        // is derived from typed variants; diagnostic text never selects behavior.
        #[cfg(test)]
        if let Some(observer) = self.native_failure_observer.take() {
            let observation = match &result {
                Err(NativeTaskApprovalExecutionError::Database(error)) => (
                    matches!(error, phoenix_db::DbError::Sqlx(_)),
                    error.to_string(),
                ),
                other => (false, format!("{other:?}")),
            };
            let _ = observer.send(observation);
        }
        let ApprovalOrigin::Native {
            progress, receipt, ..
        } = origin
        else {
            unreachable!()
        };
        if let Some(receipt) = receipt {
            if let Err(error) = result {
                tracing::error!(%error, "native approval committed; live projection unavailable");
                admitted.close("native_approval_postcommit_projection");
            } else {
                let continuation = Box::pin(async {
                    self.execute_effect(Effect::NotifyStateChange).await?;
                    match self.execute_effect(Effect::RequestLlm).await {
                        Ok(Some(event)) => self.process_startup_event(event).await,
                        Ok(None) => Ok(()),
                        Err(error) => {
                            let failure = self.llm_dispatch_failure_event(error);
                            self.process_startup_event(failure).await
                        }
                    }
                })
                .await;
                if let Err(error) = continuation {
                    tracing::error!(%error, "native approval committed; continuation settlement unavailable");
                    admitted.close("native_approval_postcommit_dispatch");
                }
            }
            return Committed(*receipt);
        }
        if progress == ApprovalGitProgress::EffectsMayHaveOccurred {
            tracing::error!(
                ?result,
                "native approval crossed Git boundary without a classified receipt"
            );
            admitted.close("native_approval_postgit_unavailable");
            return Unavailable;
        }
        match result {
            Err(NativeTaskApprovalExecutionError::Refused(reason)) => NotApplied(reason),
            Err(error) => {
                tracing::error!(%error, "native approval unclassified failure before Git");
                admitted.close("native_approval_pregit_unavailable");
                Unavailable
            }
            Ok(()) => {
                tracing::error!("native approval returned success without a durable receipt");
                admitted.close("native_approval_missing_receipt");
                Unavailable
            }
        }
    }

    pub(super) fn native_pre_git_check(
        &self,
        origin: &ApprovalOrigin,
    ) -> Result<Option<PreGitCheck>, NativeTaskApprovalExecutionError> {
        let ApprovalOrigin::Native { command, .. } = origin else {
            return Ok(None);
        };
        let command = command.clone();
        let db = self.native_task_admission_db.clone().ok_or_else(|| {
            NativeTaskApprovalExecutionError::Unclassified(
                "native approval database unavailable".to_owned(),
            )
        })?;
        let scope = self.context.resource_scope.clone();
        let cwd = self.context.filesystem_root().to_path_buf();
        let handle = tokio::runtime::Handle::current();
        #[cfg(test)]
        let gate = self.native_pre_git_gate.lock().unwrap().take();
        Ok(Some(Box::new(move || {
            #[cfg(test)]
            if let Some(gate) = gate {
                gate();
            }
            handle.block_on(async move {
                match db
                    .inspect_native_task_approval_command(&command)
                    .await
                    .map_err(classify_native_preflight_error)?
                {
                    NativeTaskApprovalInspection::RequiresOwningActorAdmission => {}
                    NativeTaskApprovalInspection::Replay(_) => {
                        return Err(NativeTaskApprovalExecutionError::Refused(
                            NativeTaskApprovalRefusal::BeforeGitRefused,
                        ));
                    }
                }
                check_native_target(&db, &command, &scope, &cwd).await
            })
        })))
    }

    pub(super) async fn settle_task_approval(
        &self,
        origin: &mut ApprovalOrigin,
        conversation_id: &str,
        approval: &TaskApprovalHandoffData,
        message: &crate::db::Message,
        state: &ConvState,
        at: DateTime<Utc>,
    ) -> Result<LocalAuthorityResult<()>, String> {
        match origin {
            ApprovalOrigin::User => {
                self.storage
                    .persist_approved_task_authority_and_state(
                        conversation_id,
                        approval,
                        message,
                        state,
                        at,
                    )
                    .await
            }
            ApprovalOrigin::Native {
                command, receipt, ..
            } => {
                match self
                    .storage
                    .persist_native_approved_task_authority_and_state(
                        command, approval, message, state, at,
                    )
                    .await?
                {
                    LocalAuthorityResult::DurableFactEstablished(stored) => {
                        *receipt = Some(Box::new(stored));
                        #[cfg(test)]
                        {
                            // Hold only after classifying/storing the real receipt.
                            // Releasing continues into the actual install_live_state.
                            let gate = self.native_projection_gate.lock().unwrap().take();
                            if let Some(gate) = gate {
                                let _ = gate.entered.send(());
                                let _ = gate.release.await;
                            }
                        }
                        Ok(LocalAuthorityResult::DurableFactEstablished(()))
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Ok(LocalAuthorityResult::DurableFactUnclassified)
                    }
                }
            }
        }
    }
}

fn classify_native_preflight_error(
    error: phoenix_db::native_task_approval::NativeTaskApprovalError,
) -> NativeTaskApprovalExecutionError {
    use phoenix_db::native_task_approval::NativeTaskApprovalError;
    match error {
        NativeTaskApprovalError::RequestConflict => {
            NativeTaskApprovalExecutionError::Refused(NativeTaskApprovalRefusal::RequestConflict)
        }
        NativeTaskApprovalError::Database(
            phoenix_db::DbError::MessageConflict(_)
            | phoenix_db::DbError::ConversationNotFound(_)
            | phoenix_db::DbError::CurrentCoordinatorRequired,
        ) => NativeTaskApprovalExecutionError::Refused(
            NativeTaskApprovalRefusal::SourceOrRequestRefused,
        ),
        NativeTaskApprovalError::Database(error) => {
            NativeTaskApprovalExecutionError::Database(error)
        }
    }
}

async fn check_native_target(
    db: &phoenix_db::Database,
    command: &NativeTaskApprovalCommand,
    scope: &crate::work_scope::ResourceScopeKey,
    cwd: &Path,
) -> Result<(), NativeTaskApprovalExecutionError> {
    let conversation = match db.get_conversation(&command.current_transcript_id).await {
        Ok(conversation) => conversation,
        Err(phoenix_db::DbError::ConversationNotFound(_)) => {
            return Err(NativeTaskApprovalExecutionError::Refused(
                NativeTaskApprovalRefusal::TargetChanged,
            ));
        }
        Err(error) => return Err(NativeTaskApprovalExecutionError::Database(error)),
    };
    let authority =
        match crate::resource_authority::resolve_resource_authority(db, &conversation).await {
            Ok(authority) => authority,
            Err(phoenix_db::DbError::ConversationNotFound(_)) => {
                return Err(NativeTaskApprovalExecutionError::Refused(
                    NativeTaskApprovalRefusal::TargetChanged,
                ));
            }
            Err(error) => return Err(NativeTaskApprovalExecutionError::Database(error)),
        };
    if authority.scope != *scope
        || authority.task_approval_authority()
            != crate::resource_authority::TaskApprovalAuthority::GitBackedActiveWorkScope
        || authority.worktree_path().map(Path::new) != Some(cwd)
    {
        return Err(NativeTaskApprovalExecutionError::Refused(
            NativeTaskApprovalRefusal::TargetChanged,
        ));
    }
    let admission = match db
        .message_target_admission(&command.current_transcript_id)
        .await
    {
        Ok(admission) => admission,
        Err(phoenix_db::DbError::ConversationNotFound(_)) => {
            return Err(NativeTaskApprovalExecutionError::Refused(
                NativeTaskApprovalRefusal::TargetChanged,
            ));
        }
        Err(error) => return Err(NativeTaskApprovalExecutionError::Database(error)),
    };
    if !matches!(
        admission,
        phoenix_db::MessageTargetAdmission::Aggregate(
            phoenix_db::ProductConversationAdmission::Accepted { .. }
        )
    ) {
        return Err(NativeTaskApprovalExecutionError::Refused(
            NativeTaskApprovalRefusal::TargetChanged,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_distinguishes_stale_coordinator_from_storage_failure() {
        use phoenix_db::{native_task_approval::NativeTaskApprovalError, DbError};

        let stale = NativeTaskApprovalError::Database(DbError::CurrentCoordinatorRequired);
        assert_eq!(
            NativeTaskApprovalOutcome::preflight_error(&stale),
            NativeTaskApprovalOutcome::NotApplied(
                NativeTaskApprovalRefusal::SourceOrRequestRefused
            )
        );
        assert!(matches!(
            classify_native_preflight_error(stale),
            NativeTaskApprovalExecutionError::Refused(
                NativeTaskApprovalRefusal::SourceOrRequestRefused
            )
        ));
        for error in [
            DbError::Sqlx(sqlx::Error::RowNotFound),
            DbError::Serialization("invalid persisted source".into()),
        ] {
            let failure = NativeTaskApprovalError::Database(error);
            assert_eq!(
                NativeTaskApprovalOutcome::preflight_error(&failure),
                NativeTaskApprovalOutcome::Unavailable
            );
            assert!(matches!(
                classify_native_preflight_error(failure),
                NativeTaskApprovalExecutionError::Database(_)
            ));
        }
    }

    #[test]
    fn exact_artifact_or_source_refusal_is_before_effects() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        std::fs::create_dir(root.join("tasks")).unwrap();
        let body = "# Exact reviewed bytes\n";
        std::fs::write(root.join("tasks/reviewed.md"), body).unwrap();
        for reviewed in ["changed bytes", body] {
            let mut progress = ApprovalGitProgress::BeforeEffects;
            let result = execute_approve_task_blocking_reviewed_with_progress(
                &root,
                &root,
                "native-proof",
                "tasks",
                "tasks/reviewed.md",
                "Exact title",
                crate::task_source::Priority::P1,
                reviewed,
                Some("main"),
                Some(Box::new(|| {
                    Err(NativeTaskApprovalExecutionError::Refused(
                        NativeTaskApprovalRefusal::BeforeGitRefused,
                    ))
                })),
                &mut progress,
            );
            assert!(result.is_err());
            assert_eq!(progress, ApprovalGitProgress::BeforeEffects);
            assert_eq!(
                std::fs::read_to_string(root.join("tasks/reviewed.md")).unwrap(),
                body
            );
            assert_eq!(std::fs::read_dir(root.join("tasks")).unwrap().count(), 1);
        }
    }

    #[test]
    fn follow_up_requires_the_exact_reviewed_path_after_the_source_gate() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        let reviewed = "tasks/53020-p1-ready--reviewed.md";
        let promoted = "tasks/53020-p1-in-progress--reviewed.md";
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let body = "# Exact reviewed bytes\n";
        std::fs::write(root.join(promoted), body).unwrap();
        let before = git_snapshot(&root);
        let mut progress = ApprovalGitProgress::BeforeEffects;
        let result = persist_fresh_approved_task_artifact_blocking_with_progress(
            &root,
            "tasks",
            reviewed,
            "Exact title",
            crate::task_source::Priority::P1,
            body,
            Some(Box::new(|| Ok(()))),
            true,
            &mut progress,
        );
        assert!(result.is_err());
        assert_eq!(progress, ApprovalGitProgress::BeforeEffects);
        assert_eq!(std::fs::read(root.join(promoted)).unwrap(), body.as_bytes());
        assert!(!root.join(reviewed).exists());
        assert_eq!(git_snapshot(&root), before);
    }

    #[test]
    fn follow_up_rechecks_disk_after_the_source_gate() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let reviewed = "tasks/53022-p1-ready--reviewed.md";
        let body = "# Exact reviewed bytes\n";
        std::fs::write(root.join(reviewed), body).unwrap();
        let before = git_snapshot(&root);
        let gate_root = root.clone();
        let mut progress = ApprovalGitProgress::BeforeEffects;
        let result = persist_fresh_approved_task_artifact_blocking_with_progress(
            &root,
            "tasks",
            reviewed,
            "Exact title",
            crate::task_source::Priority::P1,
            body,
            Some(Box::new(move || {
                std::fs::write(gate_root.join(reviewed), "edited during source gate").map_err(
                    |error| NativeTaskApprovalExecutionError::Unclassified(error.to_string()),
                )
            })),
            true,
            &mut progress,
        );
        assert!(result.is_err());
        assert_eq!(progress, ApprovalGitProgress::BeforeEffects);
        assert_eq!(
            std::fs::read(root.join(reviewed)).unwrap(),
            b"edited during source gate"
        );
        assert_eq!(git_snapshot(&root), before);
    }

    #[test]
    fn follow_up_source_check_refusal_precedes_artifact_mutation() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let reviewed = "tasks/53021-p1-ready--reviewed.md";
        let body = "# Exact reviewed bytes\n";
        std::fs::write(root.join(reviewed), body).unwrap();
        let before = git_snapshot(&root);
        let mut progress = ApprovalGitProgress::BeforeEffects;
        let result = persist_fresh_approved_task_artifact_blocking_with_progress(
            &root,
            "tasks",
            reviewed,
            "Exact title",
            crate::task_source::Priority::P1,
            body,
            Some(Box::new(|| {
                Err(NativeTaskApprovalExecutionError::Refused(
                    NativeTaskApprovalRefusal::BeforeGitRefused,
                ))
            })),
            true,
            &mut progress,
        );
        assert!(result.is_err());
        assert_eq!(progress, ApprovalGitProgress::BeforeEffects);
        assert!(root.join(reviewed).exists());
        assert_eq!(git_snapshot(&root), before);
    }

    #[test]
    fn follow_up_rejects_clean_index_when_head_does_not_contain_reviewed_bytes() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let reviewed = "tasks/reviewed.md";
        std::fs::write(root.join(reviewed), "# Previous bytes\n").unwrap();
        run_git(&root, &["add", "--", reviewed]).unwrap();
        run_git(&root, &["commit", "-m", "seed previous artifact"]).unwrap();
        run_git(
            &root,
            &["update-index", "--assume-unchanged", "--", reviewed],
        )
        .unwrap();
        let body = "# Exact reviewed bytes\r\n";
        std::fs::write(root.join(reviewed), body).unwrap();
        let before = git_snapshot(&root);
        let mut progress = ApprovalGitProgress::BeforeEffects;
        let result = persist_fresh_approved_task_artifact_blocking_with_progress(
            &root,
            "tasks",
            reviewed,
            "Exact title",
            crate::task_source::Priority::P1,
            body,
            None,
            true,
            &mut progress,
        );
        assert!(
            matches!(result, Err(FollowUpArtifactError::AfterGit(ref error))
            if error == "committed task artifact differs from the reviewed bytes")
        );
        assert_eq!(progress, ApprovalGitProgress::EffectsMayHaveOccurred);
        let after = git_snapshot(&root);
        assert_eq!((after.0, after.1), (before.0, before.1));
        assert_eq!(std::fs::read(root.join(reviewed)).unwrap(), body.as_bytes());
        assert_eq!(
            run_git(&root, &["show", "HEAD:tasks/reviewed.md"]).unwrap(),
            "# Previous bytes"
        );
    }

    #[test]
    fn follow_up_reports_effects_after_successful_taskmd_compensation() {
        let (_tmp, root) = super::super::test_git_helpers::init_repo();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        let reviewed = "tasks/53023-p1-ready--reviewed.md";
        let body = "# Exact title\n";
        std::fs::write(root.join(reviewed), body).unwrap();
        let output = phoenix_core::git::command()
            .current_dir(&root)
            .args(["add", "--", reviewed])
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = phoenix_core::git::command()
            .current_dir(&root)
            .args(["commit", "-m", "seed ready task"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let exclude = root.join(".git/info/exclude");
        // Fail staging the promoted untracked path without corrupting metadata
        // that the compensating reset itself needs to read.
        std::fs::write(
            &exclude,
            ".phoenix/\n/tasks/53023-p1-in-progress--reviewed.md\n",
        )
        .unwrap();
        let before = git_snapshot(&root);
        let mut progress = ApprovalGitProgress::BeforeEffects;
        let result = persist_fresh_approved_task_artifact_blocking_with_progress(
            &root,
            "tasks",
            reviewed,
            "Exact title",
            crate::task_source::Priority::P1,
            body,
            None,
            true,
            &mut progress,
        );
        assert!(
            matches!(&result, Err(FollowUpArtifactError::BeforeGit(error)) if error.contains("ignored")),
            "successful compensation retains the legacy error label, not native no-effect truth: {result:?}"
        );
        assert_eq!(progress, ApprovalGitProgress::EffectsMayHaveOccurred);
        assert_eq!(std::fs::read(root.join(reviewed)).unwrap(), body.as_bytes());
        assert!(!root
            .join("tasks/53023-p1-in-progress--reviewed.md")
            .exists());
        let after = git_snapshot(&root);
        assert_eq!(after, before);
        assert!(exclude.is_file());
    }

    fn git_snapshot(root: &std::path::Path) -> (String, String, Option<Vec<u8>>) {
        let output = phoenix_core::git::command()
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let head = String::from_utf8(output.stdout).unwrap();
        let output = phoenix_core::git::command()
            .current_dir(root)
            .args(["status", "--porcelain=v1"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let status = String::from_utf8(output.stdout).unwrap();
        let exclude = std::fs::read(root.join(".git/info/exclude")).ok();
        (head, status, exclude)
    }

    #[test]
    fn git_failure_is_conservatively_effects_may_have_occurred() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("tasks")).unwrap();
        let body = "# Reviewed\n";
        std::fs::write(tmp.path().join("tasks/reviewed.md"), body).unwrap();
        let mut progress = ApprovalGitProgress::BeforeEffects;
        assert!(execute_approve_task_blocking_reviewed_with_progress(
            tmp.path(),
            tmp.path(),
            "native-git-error",
            "tasks",
            "tasks/reviewed.md",
            "Reviewed",
            crate::task_source::Priority::P1,
            body,
            Some("main"),
            Some(Box::new(|| Ok(()))),
            &mut progress,
        )
        .is_err());
        assert_eq!(progress, ApprovalGitProgress::EffectsMayHaveOccurred);
    }
}
