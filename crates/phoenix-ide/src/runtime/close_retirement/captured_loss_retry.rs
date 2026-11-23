//! Finite captured-scope content evidence; not retry admission or retirement authority.

use phoenix_core::domain::close::{
    CapturedWorktreeIdentity, CloseAttemptScope, CloseRunRef, RetirementFailureReason,
};
use phoenix_db::native_close::{NativeCloseCapturedLossRetryContext, NativeCloseCommand};
use phoenix_db::{ReplaceCloseInspectionRequest, ReplaceCloseInspectionScopeRequest};

use super::NativeInitialInspectionFailure;

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
    let mut scopes = Vec::with_capacity(context.scopes().len());
    for captured in context.scopes() {
        if let Some(inspection) = inspect_captured_scope(captured, &generation).await? {
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

async fn inspect_captured_scope(
    captured: &CloseAttemptScope,
    generation: &str,
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
    let (snapshot, losses) = super::inspect_worktree_at(identity, path)
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
            vec!["-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                 "commit", "--quiet", "--allow-empty", "-m", "base"],
        ] {
            let output = phoenix_core::git::command()
                .args(arguments)
                .current_dir(directory.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output);
        }
        let identity = WorktreeIdentity::from_parts(
            WorktreeId::parse("captured-retry-tree").unwrap(),
            WorktreeFingerprint::parse(
                super::super::observe_worktree_fingerprint(directory.path()).unwrap(),
            ).unwrap(),
            GitPathIdentity::from_bytes(directory.path().as_os_str().as_bytes().to_vec()),
        );
        (directory, CloseAttemptScope {
            attempt_id: CloseAttemptId::parse("captured-retry-attempt").unwrap(),
            scope: WorkScopeId::parse("captured-retry-scope").unwrap(),
            captured_worktree: Some(CapturedWorktreeIdentity::Resolved(identity)),
            captured_at: chrono::Utc::now(),
        })
    }

    #[tokio::test]
    async fn retained_losses_and_changed_content_are_observed_not_filtered() {
        let (directory, captured) = fixture();
        let path = directory.path().join("keep");
        std::fs::write(&path, b"retained loss").unwrap();
        let first = inspect_captured_scope(&captured, "first").await.unwrap().unwrap();
        assert!(first.losses.iter().any(|loss| matches!(
            loss, CloseLossItem::UntrackedNonIgnoredPath(path) if path.as_bytes() == b"keep"
        )));
        std::fs::write(&path, b"changed retained loss").unwrap();
        let second = inspect_captured_scope(&captured, "second").await.unwrap().unwrap();
        assert_eq!(first.losses, second.losses);
        assert_ne!(first.snapshot.fingerprint(), second.snapshot.fingerprint());
        std::fs::remove_file(path).unwrap();
        let clean = inspect_captured_scope(&captured, "clean").await.unwrap().unwrap();
        assert!(clean.losses.is_empty());
        assert_eq!(clean.snapshot.generation(), "clean");
    }

    #[tokio::test]
    async fn unresolved_identity_is_not_resolved_from_its_locator() {
        let (directory, mut captured) = fixture();
        captured.captured_worktree = Some(CapturedWorktreeIdentity::Unresolved {
            locator: GitPathIdentity::from_bytes(
                directory.path().as_os_str().as_bytes().to_vec(),
            ),
        });
        assert!(matches!(inspect_captured_scope(&captured, "unresolved").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven, ..
            })));
        captured.captured_worktree = None;
        assert!(inspect_captured_scope(&captured, "no-worktree")
            .await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_or_replaced_original_identity_is_refused() {
        let (directory, captured) = fixture();
        std::fs::rename(directory.path().join(".git"), directory.path().join("old-git"))
            .unwrap();
        assert!(matches!(inspect_captured_scope(&captured, "missing").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven, ..
            })));
        std::fs::create_dir(directory.path().join(".git")).unwrap();
        std::fs::write(directory.path().join(".git/HEAD"), b"ref: refs/heads/main\n")
            .unwrap();
        assert!(matches!(inspect_captured_scope(&captured, "replacement").await,
            Err(NativeInitialInspectionFailure::Scope {
                reason: RetirementFailureReason::IdentityNotProven, ..
            })));
    }
}
