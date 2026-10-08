#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn successful_receipt_work_scope_retry(
    manager: &mut super::RuntimeManager,
    attempt: &super::CloseAttemptId,
    scope: &crate::work_scope::WorkScopeId,
    source: &super::CloseRetirementSnapshot,
    resource: &super::RetiredResourceIdentity,
    identity: &super::WorktreeIdentity,
    race_path_index: Option<usize>,
) {
    use super::{ClosePhase, RetirementOutcome};
    use crate::db::RecordCloseRetirementEvidenceRequest;
    use std::sync::Arc;
    let plan = manager
        .db()
        .close_worktree_cleanup_plan(attempt, scope, source, resource)
        .await
        .unwrap()
        .unwrap();
    let quarantine = worktree_quarantine_path(identity).unwrap();
    std::fs::remove_dir_all(&quarantine).unwrap();
    super::complete_persisted_worktree_administrative_cleanup(
        identity,
        &plan.administrative_dir,
        &plan.administrative_dir_incarnation,
    )
    .unwrap();
    manager
        .db()
        .record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: attempt.clone(),
            scope: scope.clone(),
            snapshot: source.clone(),
            resource: resource.clone(),
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
    // An unresolved wake is a real WorkScope-only blocker after worktree removal.
    sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (900, 'wake', 1, 1, 0, 0, 0, 'Active', 'wake', 1, X'00', 1, 1)")
            .execute(manager.db().pool()).await.unwrap();
    sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (900, 1, 0, 'wake', 'observe', 'wake', 1, X'00', 0, 'Required', 'ReclaimableObservation', 'Eligible')")
            .execute(manager.db().pool()).await.unwrap();
    sqlx::query("INSERT INTO wake_bindings (workflow_id, conversation_id, contract_id, profile_kind, profile_version, work_scope_id, resource_kind, bash_handle_id, registering_tool_use_id, expires_at, prepared_fingerprint, observe_effect_id, created_at) VALUES (900, 'quarantine-retry', 'contract', 'wake', 1, ?1, 'Bash', 'b-900', 'tool', 100, 'fingerprint', 1, 1)")
            .bind(scope.as_str()).execute(manager.db().pool()).await.unwrap();
    assert!(manager
        .retire_close_worktrees_and_scopes(attempt, source)
        .await
        .is_err());
    let before = manager
        .db()
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(before.phase(), ClosePhase::NeedsRepair);
    let evidence = manager
        .db()
        .list_close_retirement_evidence(attempt.as_str())
        .await
        .unwrap();
    assert!(evidence.iter().any(|proof| proof.resource.kind()
        == super::RetiredResourceKind::WorkScope
        && matches!(proof.outcome, RetirementOutcome::Residual { .. })));
    assert!(evidence
        .iter()
        .any(|proof| proof.resource == *resource && proof.outcome == RetirementOutcome::Retired));
    // The same durable DB is inert under both startup resumers before explicit admission.
    let runtime = Arc::new(super::RuntimeManager::new(
        manager.db().clone(),
        Arc::new(phoenix_llm::ModelRegistry::new_empty()),
        phoenix_core::platform::PlatformCapability::None {
            details: "receipt restart".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    for _ in 0..2 {
        assert_eq!(runtime.resume_pending_close_inspections().await.unwrap(), 0);
        assert_eq!(
            runtime
                .resume_pending_close_runtime_retirements()
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(
        runtime
            .db()
            .get_close_obligation(attempt.as_str())
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        runtime
            .db()
            .list_close_retirement_evidence(attempt.as_str())
            .await
            .unwrap(),
        evidence
    );
    sqlx::query("DELETE FROM wake_bindings WHERE workflow_id=900")
        .execute(runtime.db().pool())
        .await
        .unwrap();
    runtime.db().retry_close_retirement(attempt).await.unwrap();
    let admitted = runtime
        .db()
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(admitted.phase(), ClosePhase::AwaitingRetirementInspection);
    if let Some(index) = race_path_index {
        let target = runtime
            .inspect_close_retirement_only(attempt.clone())
            .await
            .unwrap();
        runtime
            .db()
            .capture_close_retirement_inventory(crate::db::CaptureCloseRetirementInventoryRequest {
                attempt_id: attempt.clone(),
                snapshot: target.clone(),
                scopes: vec![crate::db::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: super::CloseOwnedResourceInventory {
                        worktree: Some(identity.clone()),
                        work_scopes: std::collections::BTreeSet::new(),
                        bash_process_groups: std::collections::BTreeSet::new(),
                        tmux_servers: std::collections::BTreeSet::new(),
                        pty_sessions: std::collections::BTreeSet::new(),
                        browser_sessions: std::collections::BTreeSet::new(),
                        equivalent_live_resources: std::collections::BTreeSet::new(),
                    },
                }],
            })
            .await
            .unwrap();
        let request = crate::db::AdoptCloseWorktreeRetirementReceiptRequest {
            attempt_id: attempt.clone(),
            scope: scope.clone(),
            source_snapshot: source.clone(),
            target_snapshot: target.clone(),
            worktree: identity.clone(),
        };
        let paths = [
            super::worktree_path(identity),
            worktree_quarantine_path(identity).unwrap(),
            plan.administrative_dir.clone(),
            super::administrative_dir_quarantine_path(
                &plan.administrative_dir,
                &plan.administrative_dir_incarnation,
            )
            .unwrap(),
        ];
        let reappeared = &paths[index];
        runtime
            .adopt_close_worktree_receipt_with_hook(request.clone(), &plan, || {
                std::fs::create_dir_all(reappeared).unwrap();
                std::fs::write(reappeared.join("keep"), "preserved").unwrap();
            })
            .await
            .unwrap_err();
        assert_eq!(
            runtime
                .db()
                .get_close_obligation(attempt.as_str())
                .await
                .unwrap()
                .phase(),
            ClosePhase::NeedsRepair
        );
        let proofs = runtime
            .db()
            .list_close_retirement_evidence(attempt.as_str())
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].resource, *resource);
        assert!(matches!(
            proofs[0].outcome,
            RetirementOutcome::Residual { .. }
        ));
        let state: String = sqlx::query_scalar(
            "SELECT receipt_state FROM close_worktree_retirement_receipt_adoptions",
        )
        .fetch_one(runtime.db().pool())
        .await
        .unwrap();
        assert_eq!(state, "pending");
        let successes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_retirement_resource_history WHERE attempt_id=?1
             AND inspection_generation=?2 AND inspection_fingerprint=?3 AND proof_kind IN ('retired', 'absence_adopted')",
        ).bind(attempt.as_str()).bind(target.generation()).bind(target.fingerprint())
            .fetch_one(runtime.db().pool()).await.unwrap();
        assert_eq!(successes, 0);
        let before = runtime
            .db()
            .get_close_obligation(attempt.as_str())
            .await
            .unwrap();
        assert!(runtime
            .db()
            .finalize_close_worktree_retirement_receipt(request.clone())
            .await
            .is_err());
        assert!(runtime
            .db()
            .prepare_close_worktree_retirement_receipt(request)
            .await
            .is_err());
        assert_eq!(
            runtime
                .resume_pending_close_runtime_retirements()
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            runtime
                .db()
                .get_close_obligation(attempt.as_str())
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            runtime
                .db()
                .list_close_retirement_evidence(attempt.as_str())
                .await
                .unwrap(),
            proofs
        );
        assert_eq!(
            std::fs::read_to_string(reappeared.join("keep")).unwrap(),
            "preserved"
        );
        return;
    }
    let target = runtime
        .inspect_close_retirement(attempt.clone())
        .await
        .unwrap();
    assert_ne!(&target, source);
    assert_eq!(
        runtime
            .db()
            .get_close_obligation(attempt.as_str())
            .await
            .unwrap()
            .phase(),
        ClosePhase::Completed
    );
    let adopted = runtime
        .db()
        .list_close_retirement_evidence(attempt.as_str())
        .await
        .unwrap();
    assert!(adopted.iter().any(|proof| proof.resource == *resource
        && matches!(proof.outcome, RetirementOutcome::AbsenceAdopted { .. })));
    assert!(adopted.iter().any(|proof| proof.resource.kind()
        == super::RetiredResourceKind::WorkScope
        && proof.outcome == RetirementOutcome::Retired));
    assert!(runtime
        .db()
        .close_worktree_cleanup_plan(attempt, scope, &target, resource)
        .await
        .unwrap()
        .is_none());
    let lineage: (String, String, String, String) = sqlx::query_as(
            "SELECT source_inspection_generation, source_inspection_fingerprint, target_inspection_generation, target_inspection_fingerprint FROM close_worktree_retirement_receipt_adoptions WHERE attempt_id=?1 AND scope=?2",
        ).bind(attempt.as_str()).bind(scope.as_str()).fetch_one(runtime.db().pool()).await.unwrap();
    assert_eq!(
        lineage,
        (
            source.generation().into(),
            source.fingerprint().into(),
            target.generation().into(),
            target.fingerprint().into()
        )
    );
    assert_eq!(
        runtime
            .db()
            .close_worktree_cleanup_plan(attempt, scope, source, resource)
            .await
            .unwrap(),
        Some(plan.clone())
    );
    super::verify_worktree_receipt_paths_absent(identity, &plan).unwrap();
}

#[tokio::test]
async fn successful_worktree_receipt_survives_work_scope_block_and_explicit_retry() {
    retained_quarantine_retry(RetainedAdministrativeObservation::SuccessfulReceipt, false).await;
}

#[tokio::test]
async fn post_prepare_receipt_path_reappearance_never_writes_success() {
    for index in 0..4 {
        retained_quarantine_retry(RetainedAdministrativeObservation::ReceiptRace(index), false)
            .await;
    }
}

#[cfg(unix)]
#[test]
fn worktree_receipt_absence_rejects_each_reappeared_path_and_dangling_symlink() {
    use phoenix_core::domain::close::{
        GitPathIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
    };
    let temp = tempfile::tempdir().unwrap();
    let captured = temp.path().join("captured");
    let admin = temp.path().join("admin");
    use std::fmt::Write;
    let encoded = admin
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(String::new(), |mut encoded, byte| {
            write!(encoded, "{byte:02x}").unwrap();
            encoded
        });
    let identity = WorktreeIdentity::from_parts(
        WorktreeId::parse("receipt-worktree").unwrap(),
        WorktreeFingerprint::parse(format!("git_admin_incarnation_v2:retained:{encoded}")).unwrap(),
        GitPathIdentity::from_bytes(captured.as_os_str().as_encoded_bytes().to_vec()),
    );
    let plan = crate::db::CloseWorktreeCleanupPlan {
        administrative_dir: admin.clone(),
        administrative_dir_incarnation: "receipt-incarnation".into(),
        final_tombstone: None,
    };
    let paths = [
        captured,
        worktree_quarantine_path(&identity).unwrap(),
        admin,
        super::administrative_dir_quarantine_path(
            &plan.administrative_dir,
            &plan.administrative_dir_incarnation,
        )
        .unwrap(),
    ];
    super::verify_worktree_receipt_paths_absent(&identity, &plan).unwrap();
    for path in paths {
        std::fs::create_dir(&path).unwrap();
        assert!(super::verify_worktree_receipt_paths_absent(&identity, &plan).is_err());
        std::fs::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing-target"), &path).unwrap();
        assert!(super::verify_worktree_receipt_paths_absent(&identity, &plan).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
