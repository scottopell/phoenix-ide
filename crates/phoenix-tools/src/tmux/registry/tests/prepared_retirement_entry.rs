use super::*;

#[tokio::test]
async fn entry_refusal_before_kill_preserves_live_server_and_registry_entry() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let registry = Arc::new(with_fake(tmp.path(), &fake));
    let work_scope = scope("prepared-entry-refusal-before-kill");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let identity = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();

    let outcome = prepared
        .complete_with_sink(|_sink| Err("source custody refused".to_string()))
        .await
        .unwrap();

    assert_eq!(
        outcome,
        TmuxRetirementOutcome::RemovalFailed {
            reason: "source custody refused".to_string()
        }
    );
    assert_eq!(fake.kill_server_count(&identity.socket_path), 0);
    assert!(fake.server(&identity.socket_path).unwrap().live);
    let retained = registry
        .get_existing(&work_scope)
        .await
        .expect("refused retirement must retain the authoritative registry entry");
    assert!(Arc::ptr_eq(&server, &retained));
    // begin_retirement marks the entry Gone while fencing it; refusal before
    // the kill must preserve that fenced entry, not make it Live again.
    assert_eq!(retained.read().await.status, ServerStatus::Gone);
}

#[tokio::test]
async fn entry_refusal_after_kill_preserves_residual_before_final_gone() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let registry = Arc::new(with_fake(tmp.path(), &fake));
    let work_scope = scope("prepared-entry-refusal-before-final-gone");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let identity = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();
    let mut entry_attempts = 0;

    let outcome = prepared
        .complete_with_sink(|sink| {
            entry_attempts += 1;
            if entry_attempts == 1 {
                sink()
            } else {
                Err("source custody lost before final registry cleanup".to_string())
            }
        })
        .await
        .unwrap();

    assert_eq!(entry_attempts, 2);
    assert_eq!(
        outcome,
        TmuxRetirementOutcome::RemovalFailed {
            reason: "source custody lost before final registry cleanup".to_string()
        }
    );
    assert_eq!(fake.kill_server_count(&identity.socket_path), 1);
    assert!(fake.server(&identity.socket_path).is_none());
    let retained = registry
        .get_existing(&work_scope)
        .await
        .expect("failed final cleanup must retain the authoritative registry entry");
    assert!(Arc::ptr_eq(&server, &retained));
    // Exact server absence is proven, but source-custody refusal leaves the
    // fenced registry entry as a residual for recovery.
    assert_eq!(retained.read().await.status, ServerStatus::Gone);
}

#[tokio::test]
async fn ordinary_prepared_complete_still_retires_without_entry_callback() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let registry = Arc::new(with_fake(tmp.path(), &fake));
    let work_scope = scope("prepared-entry-ordinary-complete");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let identity = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();

    assert_eq!(
        prepared.complete().await.unwrap(),
        TmuxRetirementOutcome::Retired
    );
    assert_eq!(fake.kill_server_count(&identity.socket_path), 1);
    assert_eq!(server.read().await.status, ServerStatus::Gone);
}

#[tokio::test]
async fn strict_replacement_before_approval_never_dispatches_or_finalizes() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let registry = Arc::new(with_fake(tmp.path(), &fake));
    let work_scope = scope("strict-replacement-before-approval");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let original = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();
    fake.kill_server(&original.socket_path);
    fake.spawn_session(&original.socket_path, &registry.config_path(), tmp.path())
        .await
        .unwrap();
    let replacement = fake.server(&original.socket_path).unwrap();

    let outcome = prepared
        .complete_strict_with_typed_sink(|_| panic!("replacement cannot enter a Retry sink"))
        .await
        .unwrap();

    let TmuxRetirementOutcome::ReplacementLive(observation) = outcome else {
        panic!("expected exact original absence with a live replacement, got {outcome:?}");
    };
    let (sealed, replacement_token) = observation.into_parts();
    assert_eq!(sealed, original);
    assert_eq!(replacement.token, Some(replacement_token.to_string()));
    assert_eq!(fake.kill_server_count(&original.socket_path), 0);
    assert!(fake.server(&original.socket_path).unwrap().live);
    assert!(registry.is_retirement_fenced(&work_scope).await);
}

#[tokio::test]
async fn strict_dispatch_confirmation_precedes_refused_finalization() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let registry = Arc::new(with_fake(tmp.path(), &fake));
    let work_scope = scope("strict-dispatch-before-finalization-refusal");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let original = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();
    let mut steps = Vec::new();

    let outcome = prepared
        .complete_strict_with_typed_sink(|event| match event {
            StrictTmuxRetirementEntry::AuthenticatedApproval(sink) => {
                steps.push("approval");
                sink()
            }
            StrictTmuxRetirementEntry::DispatchConfirmed(_) => {
                steps.push("dispatch");
                assert_eq!(fake.kill_server_count(&original.socket_path), 1);
                Ok(())
            }
            StrictTmuxRetirementEntry::FinalizeRegistry(_) => {
                steps.push("finalization");
                Err("source custody lost before finalization".into())
            }
        })
        .await
        .unwrap();

    assert_eq!(steps, ["approval", "dispatch", "finalization"]);
    assert_eq!(
        outcome,
        TmuxRetirementOutcome::RemovalFailed {
            reason: "source custody lost before finalization".into(),
        }
    );
    assert!(fake.server(&original.socket_path).is_none());
    assert!(registry.is_retirement_fenced(&work_scope).await);
}

#[tokio::test]
async fn strict_final_probe_observes_replacement_without_finalizing() {
    let tmp = TempDir::new().unwrap();
    let fake = FakeTmuxBackend::new();
    let final_probe_ready = Arc::new(tokio::sync::Notify::new());
    let release_final_probe = Arc::new(tokio::sync::Notify::new());
    let registry = Arc::new(with_fake(tmp.path(), &fake).with_final_authority_test_hook(
        Arc::clone(&final_probe_ready),
        Arc::clone(&release_final_probe),
    ));
    let work_scope = scope("strict-final-probe-replacement");
    let server = registry
        .ensure_live(&work_scope, tmp.path(), None, None)
        .await
        .unwrap();
    let original = live_identity(&server).await;
    let permit = registry
        .begin_retirement(&work_scope, None, None, close_deadline())
        .await
        .unwrap();
    let prepared = registry.prepare_retirement(&permit).await.unwrap();
    let completing = prepared.complete_strict_with_typed_sink(|event| match event {
        StrictTmuxRetirementEntry::AuthenticatedApproval(sink) => sink(),
        StrictTmuxRetirementEntry::DispatchConfirmed(_) => Ok(()),
        StrictTmuxRetirementEntry::FinalizeRegistry(_) => panic!("replacement cannot finalize"),
    });
    let replace = async {
        tokio::time::timeout_at(permit.expires, final_probe_ready.notified())
            .await
            .expect("completion must reach the final authority gate");
        assert_eq!(fake.kill_server_count(&original.socket_path), 1);
        fake.spawn_session(&original.socket_path, &registry.config_path(), tmp.path())
            .await
            .unwrap();
        release_final_probe.notify_one();
    };
    let (outcome, ()) = tokio::join!(completing, replace);

    let outcome = outcome.unwrap();
    let TmuxRetirementOutcome::ReplacementLive(observation) = outcome else {
        panic!("expected a replacement at the final probe, got {outcome:?}");
    };
    let (sealed, replacement_token) = observation.into_parts();
    let replacement = fake.server(&original.socket_path).unwrap();
    assert_eq!(sealed, original);
    assert_eq!(replacement.token, Some(replacement_token.to_string()));
    assert!(replacement.live);
    assert_eq!(fake.kill_server_count(&original.socket_path), 1);
    assert!(registry.is_retirement_fenced(&work_scope).await);
}
