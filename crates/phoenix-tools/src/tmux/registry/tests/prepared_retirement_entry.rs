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
