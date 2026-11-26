//! Finite captured-scope content evidence; not retry admission or retirement authority.

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptScope, CloseRunRef, RetirementFailureReason,
};
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_db::native_close::{
    NativeCloseCapturedLossRetryAssessment, NativeCloseCapturedLossRetryContext,
    NativeCloseCommand, NativeCloseIntent,
};
use phoenix_db::{ReplaceCloseInspectionRequest, ReplaceCloseInspectionScopeRequest};

use super::NativeInitialInspectionFailure;

pub(crate) struct CapturedLossRetryAssessment {
    command: NativeCloseCommand,
    context: NativeCloseCapturedLossRetryContext,
    inspection: ReplaceCloseInspectionRequest,
    observed_at_us: i64,
    safety_evidence: String,
}

impl NativeCloseCapturedLossRetryAssessment for CapturedLossRetryAssessment {
    fn command(&self) -> &NativeCloseCommand {
        &self.command
    }

    fn context(&self) -> &NativeCloseCapturedLossRetryContext {
        &self.context
    }

    fn inspection(&self) -> &ReplaceCloseInspectionRequest {
        &self.inspection
    }

    fn observed_at_us(&self) -> i64 {
        self.observed_at_us
    }

    fn safety_evidence(&self) -> &str {
        &self.safety_evidence
    }
}

impl super::RuntimeManager {
    pub(crate) async fn assess_native_close_captured_loss_retry(
        &self,
        command: &NativeCloseCommand,
    ) -> Result<CapturedLossRetryAssessment, super::RetryAssessmentError> {
        let NativeCloseIntent::ExplicitRetry {
            target,
            attempt_id,
            failed_run_ordinal,
        } = command.intent()
        else {
            return Err("captured-loss assessment requires explicit retry".into());
        };
        let failed_run = CloseRunRef {
            attempt_id: attempt_id.clone(),
            ordinal: *failed_run_ordinal,
        };
        let context = self
            .db()
            .read_native_close_captured_loss_retry_context(target, &failed_run)
            .await
            .map_err(|error| error.to_string())?;
        self.validate_captured_loss_scope_drain(&context).await?;
        for captured in context.scopes() {
            if let Some(CapturedWorktreeIdentity::Resolved(identity)) = &captured.captured_worktree
            {
                validate_captured_registration(identity)?;
                let path = super::worktree_path(identity);
                let quarantine = super::worktree_quarantine_path(identity)?;
                if quarantine.try_exists().map_err(|error| error.to_string())? {
                    return Err("effect-free captured origin has an unexpected quarantine".into());
                }
                super::probe_retry_worktree_writers(
                    &path,
                    &quarantine,
                    None,
                    #[cfg(test)]
                    self.clean_close_writer_fixture,
                )?;
                super::ensure_no_ignored_content(&path)?;
            }
        }
        let inspection = collect_captured_loss_retry_inspection(&context)
            .await
            .map_err(|error| format!("captured-loss inspection failed: {error:?}"))?;
        if inspection.initial_command != *context.initial_command()
            || inspection.failed_run != *context.failed_run()
        {
            return Err("captured-loss inspection origin differs from retained context".into());
        }
        require_clean_inspection(&inspection.request)?;
        self.validate_captured_loss_scope_drain(&context).await?;
        Ok(CapturedLossRetryAssessment {
            command: command.clone(),
            context,
            inspection: inspection.request,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
            safety_evidence: "exact finite captured scopes drained; original worktree registration, writer state and fresh content inspection checked".into(),
        })
    }

    async fn validate_captured_loss_scope_drain(
        &self,
        context: &NativeCloseCapturedLossRetryContext,
    ) -> Result<(), String> {
        for captured in context.scopes() {
            match self
                .native_initial_work_scope_precondition(&ResourceScopeKey::Work(
                    captured.scope.clone(),
                ))
                .await
            {
                super::NativeInitialWorkScopePrecondition::Ready(_) => {}
                super::NativeInitialWorkScopePrecondition::Failed(detail) => return Err(detail),
                super::NativeInitialWorkScopePrecondition::Unclassified => {
                    return Err("captured scope runtime drain could not be established".into());
                }
            }
        }
        Ok(())
    }
}

fn require_clean_inspection(inspection: &ReplaceCloseInspectionRequest) -> Result<(), String> {
    if inspection
        .scopes
        .iter()
        .any(|scope| !scope.losses.is_empty())
    {
        return Err(
            "captured-loss retry still has retained or newly observed content blockers".into(),
        );
    }
    Ok(())
}

fn validate_captured_registration(
    identity: &phoenix_core::domain::close::WorktreeIdentity,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        super::native_initial::worktree::validate_captured_registration(identity)
    }
    #[cfg(not(unix))]
    {
        let _ = identity;
        Err("captured-loss retry registration inspection is unsupported on this platform".into())
    }
}

/// The initial source and failed run must be revalidated together with a new retry
/// source by admission. This request must not replace inspection on the stopped run.
pub(crate) struct CapturedLossRetryInspection {
    pub(crate) initial_command: NativeCloseCommand,
    pub(crate) failed_run: CloseRunRef,
    pub(crate) request: ReplaceCloseInspectionRequest,
}

/// Reads only retained scopes. Missing original paths are refused: this origin has
/// no dispatch authorizing inspection of a quarantine location.
pub(crate) async fn collect_captured_loss_retry_inspection(
    context: &NativeCloseCapturedLossRetryContext,
) -> Result<CapturedLossRetryInspection, NativeInitialInspectionFailure> {
    let generation = format!("server_git_status_v2_retry_{}", uuid::Uuid::new_v4());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut scopes = Vec::with_capacity(context.scopes().len());
    for captured in context.scopes() {
        if let Some(inspection) =
            inspect_captured_scope_until(captured, &generation, deadline).await?
        {
            scopes.push(inspection);
        }
    }
    Ok(CapturedLossRetryInspection {
        initial_command: context.initial_command().clone(),
        failed_run: context.failed_run().clone(),
        request: ReplaceCloseInspectionRequest {
            attempt_id: context.failed_run().attempt_id.clone(),
            scopes,
        },
    })
}

#[cfg(test)]
async fn inspect_captured_scope(
    captured: &CloseAttemptScope,
    generation: &str,
) -> Result<Option<ReplaceCloseInspectionScopeRequest>, NativeInitialInspectionFailure> {
    inspect_captured_scope_until(
        captured,
        generation,
        std::time::Instant::now() + std::time::Duration::from_secs(30),
    )
    .await
}

async fn inspect_captured_scope_until(
    captured: &CloseAttemptScope,
    generation: &str,
    deadline: std::time::Instant,
) -> Result<Option<ReplaceCloseInspectionScopeRequest>, NativeInitialInspectionFailure> {
    let failure = |reason, detail| NativeInitialInspectionFailure::Scope {
        scope: captured.scope.clone(),
        reason,
        detail,
    };
    let identity = match &captured.captured_worktree {
        None => return Ok(None),
        Some(CapturedWorktreeIdentity::Resolved(identity)) => identity,
        Some(CapturedWorktreeIdentity::Unresolved { .. }) => {
            return Err(failure(
                RetirementFailureReason::IdentityNotProven,
                "captured worktree identity is unresolved".into(),
            ));
        }
    };
    let path = super::worktree_path(identity);
    if super::observe_worktree_fingerprint(&path).as_deref()
        != Some(identity.fingerprint().as_str())
    {
        return Err(failure(
            RetirementFailureReason::IdentityNotProven,
            "captured worktree is missing or its administrative incarnation changed".into(),
        ));
    }
    let (snapshot, losses) = super::inspect_worktree_at_until(identity, path, deadline)
        .await
        .map_err(|detail| failure(RetirementFailureReason::ManualRepairRequired, detail))?;
    let snapshot = super::rotate_inspection_generation(snapshot, Some(generation))
        .map_err(|detail| failure(RetirementFailureReason::ManualRepairRequired, detail))?;
    Ok(Some(ReplaceCloseInspectionScopeRequest {
        scope: captured.scope.clone(),
        snapshot,
        losses,
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use phoenix_core::domain::close::{
        CloseAttemptId, CloseLossItem, GitPathIdentity, WorktreeFingerprint, WorktreeId,
        WorktreeIdentity,
    };
    use phoenix_core::work_scope::WorkScopeId;
    use std::os::unix::ffi::OsStrExt as _;

    fn fixture() -> (tempfile::TempDir, CloseAttemptScope) {
        let directory = tempfile::tempdir().unwrap();
        for arguments in [
            vec!["init", "--quiet"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            let output = phoenix_core::git::command()
                .args(arguments)
                .current_dir(directory.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("captured-retry-tree").unwrap(),
            WorktreeFingerprint::parse(
                super::super::observe_worktree_fingerprint(directory.path()).unwrap(),
            )
            .unwrap(),
            GitPathIdentity::from_bytes(directory.path().as_os_str().as_bytes().to_vec()),
        );
        (
            directory,
            CloseAttemptScope {
                attempt_id: CloseAttemptId::parse("captured-retry-attempt").unwrap(),
                scope: WorkScopeId::parse("captured-retry-scope").unwrap(),
                captured_worktree: Some(CapturedWorktreeIdentity::Resolved(identity)),
                captured_at: chrono::Utc::now(),
            },
        )
    }

    #[tokio::test]
    async fn retained_losses_and_changed_content_are_observed_not_filtered() {
        let (directory, captured) = fixture();
        let path = directory.path().join("keep");
        std::fs::write(&path, b"retained loss").unwrap();
        let first = inspect_captured_scope(&captured, "first")
            .await
            .unwrap()
            .unwrap();
        assert!(first.losses.iter().any(|loss| matches!(
            loss, CloseLossItem::UntrackedNonIgnoredPath(path) if path.as_bytes() == b"keep"
        )));
        std::fs::write(&path, b"changed retained loss").unwrap();
        let second = inspect_captured_scope(&captured, "second")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.losses, second.losses);
        assert_ne!(first.snapshot.fingerprint(), second.snapshot.fingerprint());
        std::fs::remove_file(path).unwrap();
        let clean = inspect_captured_scope(&captured, "clean")
            .await
            .unwrap()
            .unwrap();
        assert!(clean.losses.is_empty());
        assert_eq!(clean.snapshot.generation(), "clean");
    }

    #[tokio::test]
    async fn clean_assessment_guard_requires_fresh_inspection_after_owner_cleanup() {
        let (directory, captured) = fixture();
        std::fs::write(directory.path().join("blocker"), b"preserve").unwrap();
        let blocked = inspect_captured_scope(&captured, "blocked")
            .await
            .unwrap()
            .unwrap();
        let mut request = ReplaceCloseInspectionRequest {
            attempt_id: captured.attempt_id.clone(),
            scopes: vec![blocked],
        };
        assert!(require_clean_inspection(&request).is_err());
        std::fs::remove_file(directory.path().join("blocker")).unwrap();
        let clean = inspect_captured_scope(&captured, "fresh-clean")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            request.scopes[0].snapshot.generation(),
            clean.snapshot.generation()
        );
        request.scopes = vec![clean];
        assert!(require_clean_inspection(&request).is_ok());
    }

    #[tokio::test]
    async fn expired_finite_inspection_budget_is_a_blocker() {
        let (_directory, captured) = fixture();
        assert!(matches!(
            inspect_captured_scope_until(&captured, "expired", std::time::Instant::now()).await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::ManualRepairRequired,
                ..
            })
        ));
    }

    #[test]
    fn captured_retry_registration_requires_linked_worktree_and_exact_backlink() {
        let (directory, captured) = fixture();
        let Some(CapturedWorktreeIdentity::Resolved(root)) = &captured.captured_worktree else {
            panic!("fixture has a resolved identity");
        };
        assert!(validate_captured_registration(root).is_err());
        let linked_parent = tempfile::tempdir().unwrap();
        let linked = linked_parent.path().join("linked");
        let output = phoenix_core::git::command()
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&linked)
            .arg("HEAD")
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("exact-linked").unwrap(),
            WorktreeFingerprint::parse(
                super::super::observe_worktree_fingerprint(&linked).unwrap(),
            )
            .unwrap(),
            GitPathIdentity::from_bytes(linked.as_os_str().as_bytes().to_vec()),
        );
        assert!(validate_captured_registration(&identity).is_ok());
        let marker = std::fs::read_to_string(linked.join(".git")).unwrap();
        let admin = std::path::PathBuf::from(marker.trim().strip_prefix("gitdir: ").unwrap());
        std::fs::write(admin.join("gitdir"), b"/different/worktree/.git\n").unwrap();
        assert!(validate_captured_registration(&identity).is_err());
    }

    #[tokio::test]
    async fn unresolved_identity_is_not_resolved_from_its_locator() {
        let (directory, mut captured) = fixture();
        captured.captured_worktree = Some(CapturedWorktreeIdentity::Unresolved {
            locator: GitPathIdentity::from_bytes(directory.path().as_os_str().as_bytes().to_vec()),
        });
        assert!(matches!(
            inspect_captured_scope(&captured, "unresolved").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven,
                ..
            })
        ));
        captured.captured_worktree = None;
        assert!(inspect_captured_scope(&captured, "no-worktree")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn missing_or_replaced_original_identity_is_refused() {
        let (directory, captured) = fixture();
        std::fs::rename(
            directory.path().join(".git"),
            directory.path().join("old-git"),
        )
        .unwrap();
        assert!(matches!(
            inspect_captured_scope(&captured, "missing").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven,
                ..
            })
        ));
        std::fs::create_dir(directory.path().join(".git")).unwrap();
        std::fs::write(
            directory.path().join(".git/HEAD"),
            b"ref: refs/heads/main\n",
        )
        .unwrap();
        assert!(matches!(
            inspect_captured_scope(&captured, "replacement").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven,
                ..
            })
        ));
    }
}
