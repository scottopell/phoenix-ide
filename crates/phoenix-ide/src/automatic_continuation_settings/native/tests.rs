use super::*;
use phoenix_core::llm_language::LlmLanguage;

async fn native_runtime() -> (Arc<RuntimeManager>, crate::db::Database, String) {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap();
    let registry = phoenix_llm::ModelRegistry::new(&phoenix_llm::LlmConfig {
        openai_api_key: Some("fixture-not-dispatched".into()),
        ..phoenix_llm::LlmConfig::default()
    });
    let runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        Arc::new(registry),
        crate::platform::PlatformCapability::None {
            details: "offline tools fixture".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    (runtime, db, coordinator.id)
}

#[tokio::test]
async fn preference_pass_closes_before_unclassified_owner_release() {
    let (runtime, _, _) = native_runtime().await;
    let pass = PreferenceWritePass {
        owner: Some(runtime.acquire_local_authority_pass().unwrap()),
    };
    assert!(pass
        .classify::<()>(LocalAuthorityResult::DurableFactUnclassified)
        .is_err());
    assert!(runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
    assert!(runtime.acquire_local_authority_pass().is_err());
}

#[tokio::test]
async fn preference_pass_drop_is_not_an_acknowledged_rollback() {
    let (runtime, _, _) = native_runtime().await;
    drop(PreferenceWritePass {
        owner: Some(runtime.acquire_local_authority_pass().unwrap()),
    });
    assert!(runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
}

#[tokio::test]
async fn acknowledged_preference_rejection_keeps_authority_open() {
    let (runtime, _, _) = native_runtime().await;
    let pass = PreferenceWritePass {
        owner: Some(runtime.acquire_local_authority_pass().unwrap()),
    };
    let result = pass.classify(LocalAuthorityResult::DurableFactEstablished(Err::<(), _>(
        phoenix_db::DbError::CurrentCoordinatorRequired,
    )));
    assert!(matches!(result, Ok(Err(_))));
    assert!(!runtime.local_authority_is_closed());
    assert!(runtime.acquire_local_authority_pass().is_ok());
}

#[tokio::test]
async fn native_preference_application_reads_and_sets_without_provider_dispatch() {
    let (runtime, db, coordinator) = native_runtime().await;
    let ordinary = db
        .create_conversation("settings-app", "settings-app", "/tmp", true, None, None)
        .await
        .unwrap();
    let target = || Target::Ordinary(ordinary.product_conversation_id.clone());
    let initial = from_coordinator(
        runtime.clone(),
        coordinator.clone(),
        target(),
        Operation::Read,
    )
    .await;
    let Completion::Established(Ok(initial)) = initial else {
        panic!("initial native preference read was not established");
    };
    assert!(!initial.auto_continue_on_context_exhaustion);
    let changed = from_coordinator(
        runtime.clone(),
        coordinator.clone(),
        target(),
        Operation::Set(AutoContinueOnContextExhaustion::Enabled),
    )
    .await;
    let Completion::Established(Ok(changed)) = changed else {
        panic!("native preference set was not established");
    };
    assert!(changed.auto_continue_on_context_exhaustion);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled,
    );
    let Completion::Established(Err(_)) = from_coordinator(
        runtime.clone(),
        ordinary.id,
        target(),
        Operation::Set(AutoContinueOnContextExhaustion::Disabled),
    )
    .await
    else {
        panic!("ordinary source should be refused");
    };
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled,
    );
    assert!(!runtime.local_authority_is_closed());
}

#[tokio::test]
async fn preclosed_native_setting_has_terminal_completion_and_no_preference_write() {
    let (runtime, db, coordinator) = native_runtime().await;
    let owner = runtime.acquire_local_authority_pass().unwrap();
    owner.close("native_setting_preclosed_fixture");
    let completion = from_coordinator(
        runtime,
        coordinator.clone(),
        Target::Coordinator,
        Operation::Set(AutoContinueOnContextExhaustion::Enabled),
    )
    .await;
    assert!(matches!(completion, Completion::LocalAuthorityClosed));
    let coordinator = db.get_conversation(&coordinator).await.unwrap();
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&coordinator.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
}

#[tokio::test]
async fn requester_cancellation_does_not_cancel_owned_preference_classification() {
    let (runtime, db, coordinator) = native_runtime().await;
    let source = db.get_conversation(&coordinator).await.unwrap();
    let gate = runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    let (source_gate_wait, waiting) = tokio::sync::oneshot::channel();
    let (worker_completed, completed) = tokio::sync::oneshot::channel();
    let requester = tokio::spawn(from_coordinator_inner(
        runtime.clone(),
        coordinator,
        Target::Coordinator,
        Operation::Set(AutoContinueOnContextExhaustion::Enabled),
        Some(BoundaryObservation {
            source_gate_wait,
            worker_completed,
        }),
    ));
    waiting.await.unwrap();
    requester.abort();
    assert!(matches!(requester.await, Err(error) if error.is_cancelled()));
    drop(gate);
    completed
        .await
        .expect("owned worker finishes classification after requester cancellation");
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&source.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled,
    );
    assert!(!runtime.local_authority_is_closed());
}

#[tokio::test]
async fn authority_closure_while_waiting_refuses_write_and_ordinary_completion() {
    let (runtime, db, coordinator) = native_runtime().await;
    let source = db.get_conversation(&coordinator).await.unwrap();
    let gate = runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    let (source_gate_wait, waiting) = tokio::sync::oneshot::channel();
    let (worker_completed, completed) = tokio::sync::oneshot::channel();
    let requester = tokio::spawn(from_coordinator_inner(
        runtime.clone(),
        coordinator,
        Target::Coordinator,
        Operation::Set(AutoContinueOnContextExhaustion::Enabled),
        Some(BoundaryObservation {
            source_gate_wait,
            worker_completed,
        }),
    ));
    waiting.await.unwrap();
    runtime
        .acquire_local_authority_pass()
        .unwrap()
        .close("queued_setting_fixture");
    drop(gate);
    assert!(matches!(
        requester.await.unwrap(),
        Completion::LocalAuthorityClosed
    ));
    completed.await.unwrap();
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&source.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
}

#[tokio::test]
async fn rotation_while_waiting_revalidates_read_and_write_source_at_database_boundary() {
    for operation in [
        Operation::Read,
        Operation::Set(AutoContinueOnContextExhaustion::Enabled),
    ] {
        let (runtime, db, coordinator) = native_runtime().await;
        let source = db.get_conversation(&coordinator).await.unwrap();
        let gate = runtime
            .lock_product_message_admission(source.product_conversation_id.as_str())
            .await;
        let (source_gate_wait, waiting) = tokio::sync::oneshot::channel();
        let (worker_completed, completed) = tokio::sync::oneshot::channel();
        let requester = tokio::spawn(from_coordinator_inner(
            runtime.clone(),
            coordinator.clone(),
            Target::Coordinator,
            operation,
            Some(BoundaryObservation {
                source_gate_wait,
                worker_completed,
            }),
        ));
        waiting.await.unwrap();
        db.update_conversation_state(
            &coordinator,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "rotate before settings gate acquisition".into(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            db.continue_conversation(&coordinator).await.unwrap(),
            phoenix_db::ContinueOutcome::Created(_),
        ));
        drop(gate);
        let Completion::Established(Err(error)) = requester.await.unwrap() else {
            panic!("rotated source must be refused without closing local authority");
        };
        assert_eq!(
            error,
            phoenix_db::DbError::CurrentCoordinatorRequired.to_string()
        );
        completed.await.unwrap();
        assert_eq!(
            db.auto_continue_on_context_exhaustion(&source.product_conversation_id)
                .await
                .unwrap(),
            AutoContinueOnContextExhaustion::Disabled,
        );
        assert!(!runtime.local_authority_is_closed());
    }
}
