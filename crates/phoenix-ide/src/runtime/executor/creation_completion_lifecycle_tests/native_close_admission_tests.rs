use super::*;
use crate::runtime::native_close_application::{
    NativeCloseApplicationError, NativeCloseInitialRequest, NativeCloseObserveRequest,
};
use crate::runtime::testing::InMemoryStorage;
use crate::state_machine::event::CancelCause;
use crate::state_machine::Event;
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::{AssistantMessage, ToolCall, ToolInput};
use phoenix_core::llm_language::LlmLanguage;
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseInitialResult, NativeCloseIntent, NativeCloseObserveResult,
    NativeCloseRunRef,
};
use phoenix_db::Database;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};

const TARGET_ID: &str = "native-close-target";

async fn assert_native_resource_admission_fenced(
    backend: &crate::runtime::RuntimeManager,
    scope: &phoenix_core::work_scope::WorkScopeId,
) {
    let key = phoenix_core::work_scope::ResourceScopeKey::Work(scope.clone());
    assert!(matches!(
        backend.bash_handles().reserve_spawn(&key).await,
        Err(phoenix_tools::bash::registry::BashHandleError::SpawnFenced)
    ));
    assert!(matches!(
        backend.terminals.reserve_spawn(&key),
        Err(phoenix_terminal::session::ActiveTerminalInsertError::RetirementFenced)
    ));
    assert!(matches!(
        backend.browser_sessions().get_session(&key).await,
        Err(phoenix_tools::browser::session::BrowserError::RetirementFenced { .. })
    ));
    assert!(matches!(
        backend
            .tmux_registry()
            .ensure_live(&key, std::path::Path::new("/tmp"), None, None)
            .await,
        Err(phoenix_tools::tmux::registry::TmuxError::RetirementFenced { .. })
    ));
}

async fn await_native_close_phase(
    phases: &mut mpsc::UnboundedReceiver<native_close::NativeCloseInitialContinuationPhase>,
    expected: native_close::NativeCloseInitialContinuationPhase,
) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(phase) = phases.recv().await {
            if phase == expected {
                return;
            }
        }
        panic!("native Close phase observer closed before {expected:?}");
    })
    .await
    .unwrap_or_else(|_| panic!("native Close did not reach {expected:?}"));
}

struct CloseFixture {
    db: Database,
    source_transcript_id: String,
    target: ProductConversationId,
    source: SourceToolCall,
    state: ConvState,
    command: NativeCloseCommand,
}

impl CloseFixture {
    async fn new() -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let target = db
            .create_conversation(TARGET_ID, TARGET_ID, "/nonexistent", true, None, None)
            .await
            .unwrap()
            .product_conversation_id;
        let source = SourceToolCall {
            message_id: "executing-close-message".into(),
            tool_use_id: "executing-close-call".into(),
        };
        let state = executing_state(
            &source,
            &NativeCloseIntent::Initial {
                target: target.clone(),
            },
        );
        db.update_conversation_state(&coordinator.id, &state)
            .await
            .unwrap();
        let command = NativeCloseCommand::new(
            coordinator.id.clone(),
            source.clone(),
            NativeCloseIntent::Initial {
                target: target.clone(),
            },
        )
        .unwrap();
        Self {
            db,
            source_transcript_id: command.source_transcript_id().to_owned(),
            target,
            source,
            state,
            command,
        }
    }

    async fn observing() -> (Self, NativeCloseRunRef) {
        use phoenix_core::domain::close::{CloseRunOrdinal, TranscriptConversationId};

        let mut fixture = Self::new().await;
        let obligation = fixture
            .db
            .begin_close_foundation(
                &fixture.target,
                &TranscriptConversationId::parse(TARGET_ID).unwrap(),
                "retained-observe-attempt",
            )
            .await
            .unwrap();
        let referenced = NativeCloseRunRef {
            target: fixture.target.clone(),
            attempt_id: obligation.attempt_id().clone(),
            run_ordinal: CloseRunOrdinal::INITIAL,
        };
        fixture.command = NativeCloseCommand::new(
            fixture.source_transcript_id.clone(),
            fixture.source.clone(),
            NativeCloseIntent::Observe {
                target: referenced.target.clone(),
                attempt_id: referenced.attempt_id.clone(),
                run_ordinal: referenced.run_ordinal,
            },
        )
        .unwrap();
        fixture.state = executing_state(&fixture.source, fixture.command.intent());
        fixture
            .db
            .update_conversation_state(&fixture.source_transcript_id, &fixture.state)
            .await
            .unwrap();
        (fixture, referenced)
    }

    async fn admitted(&self) -> phoenix_db::native_close::NativeCloseReceipt {
        match self.db.apply_native_close_initial(&self.command).await {
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
                NativeCloseInitialResult::NewlyAdmitted(receipt),
            )) => receipt,
            result => panic!("expected newly admitted initial Close, got {result:?}"),
        }
    }

    fn backend(db: Database) -> Arc<crate::runtime::RuntimeManager> {
        Arc::new(crate::runtime::RuntimeManager::new(
            db,
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "native test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ))
    }

    fn runtime(&self, is_coordinator: bool) -> RuntimeHarness {
        let mut harness =
            runtime_with_storage(self.state.clone(), Arc::new(InMemoryStorage::new()));
        harness.runtime.context.conversation_id = self.source_transcript_id.clone();
        harness.runtime.context.is_coordinator = is_coordinator;
        harness.runtime = harness
            .runtime
            .with_native_task_admission_database(self.db.clone());
        harness
    }
}

fn executing_state(source: &SourceToolCall, intent: &NativeCloseIntent) -> ConvState {
    let input = intent.source_input();
    ConvState::ToolExecuting {
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
    }
}

fn setup_actor_queue(harness: &mut RuntimeHarness) -> mpsc::Sender<AcknowledgedRuntimeRequest> {
    let (watch_tx, _watch_rx) = tokio::sync::watch::channel(harness.runtime.state.clone());
    let (sender, receiver) = mpsc::channel(4);
    harness.runtime = std::mem::replace(&mut harness.runtime, runtime(ConvState::Idle).runtime)
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver);
    sender
}

fn spawn_actor(harness: &mut RuntimeHarness) -> tokio::task::JoinHandle<RuntimeExitDisposition> {
    tokio::spawn(std::mem::replace(&mut harness.runtime, runtime(ConvState::Idle).runtime).run())
}

fn start_actor(
    harness: &mut RuntimeHarness,
) -> (
    mpsc::Sender<AcknowledgedRuntimeRequest>,
    tokio::task::JoinHandle<RuntimeExitDisposition>,
) {
    let sender = setup_actor_queue(harness);
    let running = spawn_actor(harness);
    (sender, running)
}

async fn stop_actor(sender: &mpsc::Sender<AcknowledgedRuntimeRequest>) {
    let (acknowledgement, result) = oneshot::channel();
    let (_retirement, retirement) = oneshot::channel();
    sender
        .send(AcknowledgedRuntimeRequest::Event(Box::new(
            AcknowledgedEventRequest {
                event: Event::UserCancel {
                    reason: None,
                    cause: CancelCause::UserRequested,
                },
                acknowledgement,
                retirement,
            },
        )))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), result)
            .await
            .expect("Stop ACK must settle")
            .unwrap()
            .unwrap(),
        AcknowledgedEventOutcome::Settled
    );
}

async fn count(db: &Database, table: &str) -> i64 {
    let sql = match table {
        "native_close_initial_receipts" => "SELECT COUNT(*) FROM native_close_initial_receipts",
        "close_runs" => "SELECT COUNT(*) FROM close_runs",
        "close_completion_retry_allowances" => {
            "SELECT COUNT(*) FROM close_completion_retry_allowances"
        }
        _ => panic!("unexpected Close table"),
    };
    sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap()
}

fn initial_request(
    admitted: crate::runtime::AdmittedOperation,
    fixture: &CloseFixture,
    target_gate: tokio::sync::OwnedMutexGuard<()>,
    response: oneshot::Sender<Result<NativeCloseInitialResult, NativeCloseApplicationError>>,
) -> AcknowledgedRuntimeRequest {
    initial_request_with_backend(
        admitted,
        fixture,
        target_gate,
        response,
        CloseFixture::backend(fixture.db.clone()),
    )
}

fn initial_request_with_backend(
    admitted: crate::runtime::AdmittedOperation,
    fixture: &CloseFixture,
    target_gate: tokio::sync::OwnedMutexGuard<()>,
    response: oneshot::Sender<Result<NativeCloseInitialResult, NativeCloseApplicationError>>,
    backend: Arc<crate::runtime::RuntimeManager>,
) -> AcknowledgedRuntimeRequest {
    AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(Box::new(NativeCloseInitialRequest {
        source: fixture.source.clone(),
        backend,
        target: fixture.target.clone(),
        admitted,
        target_gate,
        response,
    }))
}

#[tokio::test]
async fn native_close_backend_requires_actor_database_authority_not_matching_path() {
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let cloned_backend = fixture.db.clone();
    let unrelated_backend = Database::open_in_memory().await.unwrap();

    assert_eq!(
        harness.runtime.check_native_close_backend(&cloned_backend),
        Ok(())
    );
    assert_eq!(
        harness
            .runtime
            .check_native_close_backend(&unrelated_backend),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    harness.runtime.native_task_admission_db = None;
    assert_eq!(
        harness.runtime.check_native_close_backend(&cloned_backend),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(count(&fixture.db, "close_runs").await, 0);
    assert_eq!(count(&unrelated_backend, "close_runs").await, 0);
}

#[tokio::test]
async fn direct_initial_backend_mismatch_sends_refusal_and_releases_owner() {
    let fixture = CloseFixture::new().await;
    let unrelated_db = Database::open_in_memory().await.unwrap();
    let backend = CloseFixture::backend(unrelated_db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, reply) = oneshot::channel();

    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await;
    assert!(matches!(
        admission,
        Err(NativeCloseApplicationError::SourceRefused)
    ));
    assert_eq!(
        reply.await.unwrap(),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
    assert_eq!(count(&fixture.db, "close_runs").await, 0);
    assert_eq!(
        count(&unrelated_db, "native_close_initial_receipts").await,
        0
    );
    assert_eq!(count(&unrelated_db, "close_runs").await, 0);
}

#[tokio::test]
async fn direct_new_initial_returns_continuation_owning_reply_gate_and_owner() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, mut reply) = oneshot::channel();

    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await;
    let continuation = match admission {
        Ok(native_close::NativeCloseInitialAdmission::Continued {
            result: NativeCloseInitialResult::NewlyAdmitted(receipt),
            continuation,
        }) => {
            assert_eq!(continuation.receipt, receipt);
            continuation
        }
        _ => panic!("fresh admission must return an owned continuation"),
    };
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::Prepare
    );
    assert!(continuation.authority.owner.is_some());
    assert_eq!(fence.owner_count(), 1);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);

    drop(continuation);
    assert!(fence.fatal_authority_loss_is_latched());
    assert_eq!(fence.owner_count(), 0);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

#[tokio::test]
async fn initial_precondition_rollback_does_not_release_running_owner() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, mut reply) = oneshot::channel();

    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await
        .unwrap();
    let mut continuation = match admission {
        native_close::NativeCloseInitialAdmission::Continued { continuation, .. } => continuation,
        _ => panic!("fresh admission must return an owned continuation"),
    };

    assert_eq!(
        continuation.step(&ConvState::Idle).await,
        Err(NativeCloseApplicationError::Storage)
    );
    assert!(!continuation.release_if_exactly_stopped().await);
    assert!(continuation.terminal_observation_retry);
    assert_eq!(fence.owner_count(), 1);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn exact_stopped_initial_run_releases_owner_and_returns_original_receipt() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, reply) = oneshot::channel();
    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await
        .unwrap();
    let mut continuation = match admission {
        native_close::NativeCloseInitialAdmission::Continued { continuation, .. } => continuation,
        _ => panic!("fresh admission must return an owned continuation"),
    };
    let receipt = continuation.receipt.clone();
    let run = phoenix_core::domain::close::CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };

    fixture
        .db
        .classify_interrupted_close_run(&run)
        .await
        .unwrap();
    assert!(continuation.release_if_exactly_stopped().await);
    // The actor consumes the classified continuation before admitting another target owner.
    drop(continuation);
    assert_eq!(
        reply.await.unwrap(),
        Ok(NativeCloseInitialResult::NewlyAdmitted(receipt))
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

#[tokio::test]
async fn unrelated_initial_receipt_cannot_release_stopped_run_owner() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, mut reply) = oneshot::channel();
    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await
        .unwrap();
    let mut continuation = match admission {
        native_close::NativeCloseInitialAdmission::Continued { continuation, .. } => continuation,
        _ => panic!("fresh admission must return an owned continuation"),
    };
    let run = phoenix_core::domain::close::CloseRunRef {
        attempt_id: continuation.receipt.admitted.attempt_id.clone(),
        ordinal: continuation.receipt.admitted.run_ordinal,
    };
    fixture
        .db
        .classify_interrupted_close_run(&run)
        .await
        .unwrap();
    continuation.receipt.request_id = uuid::Uuid::new_v4();

    assert!(!continuation.release_if_exactly_stopped().await);
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn direct_initial_replay_sends_exact_receipt_without_pending_work() {
    let fixture = CloseFixture::new().await;
    let receipt = fixture.admitted().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, reply) = oneshot::channel();

    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await;
    assert!(matches!(
        admission,
        Ok(native_close::NativeCloseInitialAdmission::Finished(
            NativeCloseInitialResult::Replay(ref replay)
        )) if replay == &receipt
    ));
    assert_eq!(
        reply.await.unwrap(),
        Ok(NativeCloseInitialResult::Replay(receipt))
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);
    assert_eq!(
        count(&fixture.db, "close_completion_retry_allowances").await,
        1
    );
}

#[tokio::test]
async fn direct_dropped_pending_initial_fences_before_releasing_owner() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, mut reply) = oneshot::channel();

    let admission = harness
        .runtime
        .admit_native_close_initial(
            &backend,
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            target_gate,
            response,
        )
        .await;
    let pending = match admission {
        Ok(native_close::NativeCloseInitialAdmission::Continued { continuation, .. }) => {
            continuation
        }
        _ => panic!("fresh admission must own pending work"),
    };
    assert_eq!(fence.owner_count(), 1);
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(Arc::clone(&gate).try_lock_owned().is_err());

    drop(pending);
    assert!(fence.fatal_authority_loss_is_latched());
    assert_eq!(fence.owners_at_first_close(), Some(1));
    assert_eq!(fence.owner_count(), 0);
    assert!(fence.try_acquire().is_err());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
}

#[tokio::test]
async fn initial_mismatched_backend_refuses_before_database_work_and_releases_owner() {
    let fixture = CloseFixture::new().await;
    let unrelated_db = Database::open_in_memory().await.unwrap();
    let backend = CloseFixture::backend(unrelated_db.clone());
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();

    assert_eq!(
        harness
            .runtime
            .execute_native_close_initial(
                &backend,
                fixture.source.clone(),
                fixture.target.clone(),
                admitted,
            )
            .await,
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(fence.owner_count(), 0);
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
    assert_eq!(count(&fixture.db, "close_runs").await, 0);
    assert_eq!(
        count(&fixture.db, "close_completion_retry_allowances").await,
        0
    );
    assert_eq!(
        count(&unrelated_db, "native_close_initial_receipts").await,
        0
    );
    assert_eq!(count(&unrelated_db, "close_runs").await, 0);
    assert_eq!(
        count(&unrelated_db, "close_completion_retry_allowances").await,
        0
    );
}

#[tokio::test]
async fn stopped_live_source_refuses_exact_initial_replay_before_database_lookup() {
    let fixture = CloseFixture::new().await;
    let receipt = fixture.admitted().await;
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);

    // Stop ACK persists source revocation in InMemoryStorage, not full cancellation.
    // SQLite remains executing, so live actor matching must prevent this exact replay.
    let mut harness = fixture.runtime(true);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let (sender, running) = start_actor(&mut harness);
    stop_actor(&sender).await;
    let stopped_source_state = harness
        .storage
        .get_current_state(&fixture.source_transcript_id)
        .expect("Stop ACK must persist the live source state");
    assert!(
        !matches!(stopped_source_state, ConvState::ToolExecuting { .. }),
        "acknowledged Stop must revoke the live executing Close source"
    );
    assert!(matches!(
        fixture
            .db
            .get_conversation(&fixture.source_transcript_id)
            .await
            .unwrap()
            .state,
        ConvState::ToolExecuting { .. }
    ));

    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, result) = oneshot::channel();
    sender
        .send(initial_request(admitted, &fixture, target_gate, response))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), result)
            .await
            .expect("replay refusal must settle")
            .unwrap(),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);
    assert_eq!(receipt.request_id, fixture.command.request_id());

    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn dropped_initial_reply_retains_admission_until_exact_stop_releases_gate() {
    let fixture = CloseFixture::new().await;
    // Stop must persist through the same database authority as native admission.
    let (event_tx, event_rx) = mpsc::channel(32);
    let mut actor = ConversationRuntime::new(
        ConvContext::coordinator(&fixture.source_transcript_id, "test-model", 200_000, 16_384),
        fixture.state.clone(),
        Arc::new(crate::runtime::traits::DatabaseStorage::new(
            fixture.db.clone(),
        )),
        Arc::new(MockLlmClient::new("test-model")),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx.clone(),
        crate::runtime::SseBroadcaster::new(128, 0),
    )
    .with_fatal_local_authority_fence(crate::runtime::FatalLocalAuthorityFence::new())
    .with_native_task_admission_database(fixture.db.clone());
    let original_fence = Arc::clone(&actor.fatal_local_authority_fence);
    let admitted = actor.admit_authoritative_effect().unwrap();
    let (sender, receiver) = mpsc::channel(4);
    let (watch_tx, _watch_rx) = tokio::sync::watch::channel(actor.state.clone());
    actor = actor
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver);
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, dropped_reply) = oneshot::channel();
    sender
        .send(initial_request(admitted, &fixture, target_gate, response))
        .await
        .unwrap();
    drop(dropped_reply);

    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert_eq!(original_fence.owner_count(), 1);

    let running = tokio::spawn(actor.run());
    // This ACK follows admission in the FIFO, but deliberately precedes optional
    // terminal observation. Gate acquisition below observes actual guard release.
    stop_actor(&sender).await;
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);
    assert_eq!(
        count(&fixture.db, "close_completion_retry_allowances").await,
        1
    );
    let admitted = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT o.native_managed, a.consumed, a.suppressed
         FROM close_obligations o JOIN close_completion_retry_allowances a
           ON a.attempt_id = o.attempt_id",
    )
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(admitted, (1, 0, 1));
    let _released_gate = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        Arc::clone(&gate).lock_owned(),
    )
    .await
    .expect("exact stopped observation must release the owned continuation");
    let terminal: (String, String) = sqlx::query_as(
        "SELECT r.status, o.phase FROM close_runs r
         JOIN close_obligations o ON o.attempt_id = r.attempt_id",
    )
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(terminal, ("stopped".to_string(), "completed".to_string()));
    assert_eq!(original_fence.owner_count(), 0);

    event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

fn observe_request(
    fixture: &CloseFixture,
    referenced: NativeCloseRunRef,
    admitted: crate::runtime::AdmittedOperation,
    target_gate: tokio::sync::OwnedMutexGuard<()>,
) -> (
    AcknowledgedRuntimeRequest,
    oneshot::Receiver<Result<NativeCloseObserveResult, NativeCloseApplicationError>>,
) {
    let (response, result) = oneshot::channel();
    let request = NativeCloseObserveRequest {
        source: fixture.source.clone(),
        referenced,
        admitted,
        target_gate,
        response,
    };
    (
        AcknowledgedRuntimeRequest::ExecuteNativeCloseObserve(Box::new(request)),
        result,
    )
}

#[tokio::test]
async fn stopped_live_source_refuses_exact_observe_replay_before_database_lookup() {
    let (fixture, referenced) = CloseFixture::observing().await;
    assert!(matches!(
        fixture
            .db
            .apply_native_close_observe(&fixture.command)
            .await,
        phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
            NativeCloseObserveResult::Observed(_)
        ))
    ));
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let (sender, running) = start_actor(&mut harness);
    stop_actor(&sender).await;
    assert!(!matches!(
        harness
            .storage
            .get_current_state(&fixture.source_transcript_id)
            .unwrap(),
        ConvState::ToolExecuting { .. }
    ));
    // InMemoryStorage received Stop; SQLite still permits this historical replay.
    assert!(matches!(
        fixture
            .db
            .apply_native_close_observe(&fixture.command)
            .await,
        phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
            NativeCloseObserveResult::Replay(_)
        ))
    ));
    let gate = Arc::new(Mutex::new(()));
    let (request, result) = observe_request(
        &fixture,
        referenced,
        admitted,
        Arc::clone(&gate).lock_owned().await,
    );
    sender.send(request).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(10), result)
            .await
            .expect("stopped observation must settle")
            .unwrap(),
        Err(NativeCloseApplicationError::SourceRefused)
    ));
    // The awaited request has already settled and released its owner and gate.
    // A second UserCancel is not a FIFO barrier: CancellingTool rejects it.
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn dropped_observe_reply_keeps_queue_ownership_through_adoption_and_classification() {
    let (fixture, referenced) = CloseFixture::observing().await;
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let (request, dropped_reply) = observe_request(
        &fixture,
        referenced.clone(),
        admitted,
        Arc::clone(&gate).lock_owned().await,
    );
    sender.send(request).await.unwrap();
    drop(dropped_reply);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert_eq!(fence.owner_count(), 1);
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
    let running = spawn_actor(&mut harness);
    stop_actor(&sender).await;
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&fixture.db, "close_runs").await, 1);
    let adopted: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT o.native_managed, a.consumed, a.suppressed, r.run_ordinal
         FROM close_obligations o JOIN close_completion_retry_allowances a ON a.attempt_id = o.attempt_id
         JOIN native_close_initial_receipts r ON r.attempt_id = o.attempt_id
         WHERE o.attempt_id = ?1 AND r.intent = 'observe'",
    ).bind(referenced.attempt_id.as_str()).fetch_one(fixture.db.pool()).await.unwrap();
    // Observation/adoption and stopping the distinct source do not consume or
    // suppress the target operation's allowance.
    assert_eq!(adopted, (1, 0, 0, 1));
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn wrong_live_source_role_refuses_without_a_receipt() {
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(false);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let (sender, running) = start_actor(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let target_gate = Arc::clone(&gate).lock_owned().await;
    let (response, result) = oneshot::channel();
    sender
        .send(initial_request(admitted, &fixture, target_gate, response))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), result)
            .await
            .expect("source-role refusal must settle")
            .unwrap(),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
    assert_eq!(count(&fixture.db, "close_runs").await, 0);

    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn aborted_owning_initial_execution_task_latches_authority_before_releasing_owner() {
    let fixture = CloseFixture::new().await;
    let held_connection = fixture.db.pool().acquire().await.unwrap();
    let mut harness = fixture.runtime(true);
    let original_fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let source = fixture.source.clone();
    let target = fixture.target.clone();
    let backend = CloseFixture::backend(fixture.db.clone());
    let (suspended, reached_boundary) = oneshot::channel();
    let owning_task = tokio::spawn(async move {
        let mut future = Box::pin(
            harness
                .runtime
                .execute_native_close_initial(&backend, source, target, admitted),
        );
        // The sole connection is held outside this task. Every earlier check is
        // synchronous, so Pending witnesses the actual DB-await boundary.
        assert!(futures::poll!(future.as_mut()).is_pending());
        suspended.send(()).unwrap();
        future.await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), reached_boundary)
        .await
        .expect("owning task must reach its DB await")
        .unwrap();
    assert_eq!(original_fence.owner_count(), 1);
    assert!(!original_fence.fatal_authority_loss_is_latched());

    owning_task.abort();
    assert!(owning_task.await.unwrap_err().is_cancelled());

    assert!(original_fence.fatal_authority_loss_is_latched());
    assert_eq!(original_fence.owners_at_first_close(), Some(1));
    assert_eq!(original_fence.owner_count(), 0);
    assert!(original_fence.try_acquire().is_err());

    drop(held_connection);
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
}

#[tokio::test]
async fn established_initial_rollback_releases_owner_without_closing_authority() {
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    // Refuse at the DB source check, not the independent live actor matcher.
    fixture
        .db
        .update_conversation_state(&fixture.source_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    let original_fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    assert_eq!(
        harness
            .runtime
            .execute_native_close_initial(
                &CloseFixture::backend(fixture.db.clone()),
                fixture.source.clone(),
                fixture.target.clone(),
                admitted,
            )
            .await,
        Err(NativeCloseApplicationError::Storage)
    );
    assert!(!original_fence.fatal_authority_loss_is_latched());
    assert_eq!(original_fence.owner_count(), 0);
    let next_owner = harness.runtime.admit_authoritative_effect().unwrap();
    drop(next_owner);
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
}

async fn assert_no_initial_inspection_writes(db: &Database, attempt: &str) {
    for table in [
        "close_retirement_inspections",
        "close_retirement_losses",
        "close_retirement_inventories",
        "close_expected_retirement_resources",
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        // The table identifier comes only from the fixed test-local list above;
        // the attempt is separately bound, never interpolated.
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(attempt)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "unexpected {table} write");
    }
}

async fn settled_initial_fixture() -> (CloseFixture, phoenix_db::native_close::NativeCloseReceipt) {
    use phoenix_core::domain::close::ClosePhase;
    use phoenix_db::workflow::LocalAuthorityResult;

    let fixture = CloseFixture::new().await;
    let receipt = fixture.admitted().await;
    let prepared = fixture
        .db
        .prepare_native_close_initial_execution(&fixture.command, &receipt, &fixture.state)
        .await;
    assert!(
        matches!(prepared, LocalAuthorityResult::DurableFactEstablished(Ok(ref execution))
        if execution.phase == ClosePhase::SettlingActiveWork)
    );
    let settled = fixture
        .db
        .try_settle_native_close_initial(&fixture.command, &receipt, &fixture.state)
        .await;
    assert!(
        matches!(settled, LocalAuthorityResult::DurableFactEstablished(Ok(
        phoenix_db::native_close::NativeCloseInitialSettlement::Advanced(ref execution)
    )) if execution.phase == ClosePhase::AwaitingRetirementInspection)
    );
    (fixture, receipt)
}

async fn admit_initial_continuation(
    fixture: &CloseFixture,
    gate: Arc<Mutex<()>>,
    response: oneshot::Sender<Result<NativeCloseInitialResult, NativeCloseApplicationError>>,
) -> (
    native_close::NativeCloseInitialContinuation,
    Arc<crate::runtime::FatalLocalAuthorityFence>,
) {
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let admission = harness
        .runtime
        .admit_native_close_initial(
            &CloseFixture::backend(fixture.db.clone()),
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            gate.lock_owned().await,
            response,
        )
        .await
        .unwrap();
    match admission {
        native_close::NativeCloseInitialAdmission::Continued {
            result: NativeCloseInitialResult::NewlyAdmitted(receipt),
            continuation,
        } => {
            assert_eq!(continuation.receipt, receipt);
            (continuation, fence)
        }
        _ => panic!("new initial Close must retain its source owner"),
    }
}

// All captured identity fields are present: classify an absent resolved path,
// not an unresolved identity.
async fn resolved_but_absent_captured_worktree(
    fixture: &CloseFixture,
) -> phoenix_core::work_scope::WorkScopeId {
    let scope = fixture
        .db
        .get_conversation(TARGET_ID)
        .await
        .unwrap()
        .attached_work_scope_id
        .unwrap();
    sqlx::query(
        "UPDATE work_scopes SET environment_kind='allocated_worktree',
        worktree_path=?1, worktree_id=?2, worktree_fingerprint=?3,
        branch_name='invalid-close-worktree', base_branch='main' WHERE id=?4",
    )
    .bind(format!(
        "/nonexistent/native-close-{}",
        uuid::Uuid::new_v4()
    ))
    .bind("invalid-native-close-worktree")
    .bind("invalid-native-close-fingerprint")
    .bind(scope.as_str())
    .execute(fixture.db.pool())
    .await
    .unwrap();
    scope
}

async fn initial_inspection_continuation(
    fixture: &CloseFixture,
) -> native_close::NativeCloseInitialContinuation {
    let mut harness = fixture.runtime(true);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let gate = Arc::new(Mutex::new(()));
    let (response, _reply) = oneshot::channel();
    match harness
        .runtime
        .admit_native_close_initial(
            &CloseFixture::backend(fixture.db.clone()),
            fixture.source.clone(),
            fixture.target.clone(),
            admitted,
            gate.lock_owned().await,
            response,
        )
        .await
        .unwrap()
    {
        native_close::NativeCloseInitialAdmission::Continued { continuation, .. } => continuation,
        _ => panic!("new initial Close must retain its source owner"),
    }
}

#[tokio::test]
async fn dirty_initial_collector_normalizes_empty_worktrees_without_writing() {
    use phoenix_core::domain::close::ClosePhase;
    let (fixture, receipt) = settled_initial_fixture().await;
    let attempt = receipt.admitted.attempt_id.as_str();
    let scopes = fixture.db.list_close_attempt_scopes(attempt).await.unwrap();
    assert!(!scopes.is_empty(), "fixture must capture a real WorkScope");
    assert!(scopes.iter().all(|scope| scope.captured_worktree.is_none()));
    let before = fixture.db.get_close_obligation(attempt).await.unwrap();
    assert_eq!(before.phase(), ClosePhase::AwaitingRetirementInspection);
    assert!(before.snapshot().is_none());

    let collected = crate::runtime::close_retirement::collect_native_initial_inspection(
        &fixture.db,
        receipt.admitted.attempt_id.clone(),
    )
    .await
    .unwrap();
    assert_eq!(collected.request.attempt_id, receipt.admitted.attempt_id);
    assert!(collected.request.scopes.is_empty());
    assert_eq!(collected.empty_generation, None);
    let after = fixture.db.get_close_obligation(attempt).await.unwrap();
    assert_eq!(after.phase(), before.phase());
    assert_eq!(after.snapshot(), before.snapshot());
    assert_no_initial_inspection_writes(&fixture.db, attempt).await;
}

#[tokio::test]
async fn frozen_initial_continuation_cannot_advance_live_source_before_exact_stop() {
    use phoenix_core::domain::close::{CloseRunRef, CloseRunStatus};

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let run = CloseRunRef {
        attempt_id: continuation.receipt.admitted.attempt_id.clone(),
        ordinal: continuation.receipt.admitted.run_ordinal,
    };
    let admitted_result = continuation.admitted_result.clone();

    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::Prepare
    );
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Running
    );

    continuation.freeze_for_exit_observation();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ExitObservationOnly
    );
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::ExitObservationOnly
    );
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Running
    );
    assert_eq!(fence.owner_count(), 1);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    fixture
        .db
        .classify_interrupted_close_run(&run)
        .await
        .unwrap();
    assert!(continuation.release_if_exactly_stopped().await);
    drop(continuation);
    assert_eq!(reply.await.unwrap(), Ok(admitted_result));
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

#[tokio::test]
async fn exit_interruption_retries_only_classification_before_releasing_exact_owner() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::{CloseRunRef, CloseRunStatus};

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let run = CloseRunRef {
        attempt_id: continuation.receipt.admitted.attempt_id.clone(),
        ordinal: continuation.receipt.admitted.run_ordinal,
    };
    assert_eq!(continuation.phase, Phase::Prepare);
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Running
    );
    sqlx::query(
        "CREATE TRIGGER reject_exit_classification_outbox BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected exit classification outbox failure'); END",
    )
    .execute(fixture.db.pool())
    .await
    .unwrap();

    continuation.begin_exit_interruption();
    assert!(continuation.requires_periodic_step());
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ClassifyingExitInterruption
    );
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Running
    );
    assert!(fixture
        .db
        .list_close_cleanup_failures(run.attempt_id.as_str())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_no_initial_inspection_writes(&fixture.db, run.attempt_id.as_str()).await;

    sqlx::query("DROP TRIGGER reject_exit_classification_outbox")
        .execute(fixture.db.pool())
        .await
        .unwrap();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Finished
    );
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Stopped
    );
    let failures = fixture
        .db
        .list_close_cleanup_failures(run.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(&failures[0].occurrence.failure_occurrence_id)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);
    assert_eq!(
        reply.try_recv().unwrap().unwrap(),
        continuation.admitted_result
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    let source = fixture
        .db
        .get_conversation(&fixture.source_transcript_id)
        .await
        .unwrap();
    assert!(matches!(source.state, ConvState::ToolExecuting { .. }));
    assert!(
        !fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .archived
    );
    assert_no_initial_inspection_writes(&fixture.db, run.attempt_id.as_str()).await;
    drop(continuation);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

#[tokio::test]
async fn process_shutdown_wakes_dormant_close_without_an_actor_event() {
    assert_actor_exit_from_dormant_close(true).await;
}

#[tokio::test]
async fn eviction_shutdown_refuses_ordinary_work_while_close_exit_rolls_back() {
    assert_actor_exit_from_dormant_close(false).await;
}

async fn assert_actor_exit_from_dormant_close(process_shutdown: bool) {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::{CloseRunRef, CloseRunStatus};

    let fixture = CloseFixture::new().await;
    // Admission captures a real busy target, so preparation reaches a dormant
    // confirmation boundary rather than a timer-driven inspection phase.
    fixture
        .db
        .update_conversation_state(TARGET_ID, &fixture.state)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_actor_exit_outbox BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected actor exit outbox failure'); END",
    )
    .execute(fixture.db.pool())
    .await
    .unwrap();
    let mut harness = fixture.runtime(true);
    let (phase_tx, mut phases) = mpsc::unbounded_channel();
    harness.runtime.native_close_phase_observer = Some(phase_tx);
    let (state_tx, state_rx) = tokio::sync::watch::channel(fixture.state.clone());
    harness.runtime = harness.runtime.with_state_watcher(state_tx);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let (response, result) = oneshot::channel();
    sender
        .send(initial_request(
            admitted,
            &fixture,
            Arc::clone(&gate).lock_owned().await,
            response,
        ))
        .await
        .unwrap();
    let running = spawn_actor(&mut harness);
    let prepared = tokio::time::timeout(std::time::Duration::from_secs(10), phases.recv())
        .await
        .expect("actor must report its completed preparation boundary")
        .unwrap();
    assert_eq!(prepared, Phase::AwaitingStopWorkConfirmation);
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());

    if process_shutdown {
        // No event follows this closure: cancellation itself must wake select.
        fence.close_for_process_shutdown();
    } else {
        harness.event_tx.send(Event::Shutdown).await.unwrap();
    }
    let classifying = tokio::time::timeout(std::time::Duration::from_secs(10), phases.recv())
        .await
        .expect("shutdown must wake dormant Close and attempt interruption")
        .unwrap();
    assert_eq!(classifying, Phase::ClassifyingExitInterruption);
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    let failures: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_cleanup_failures")
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
    assert_eq!(
        failures, 0,
        "failed mandatory publication must roll back interruption"
    );

    let (acknowledgement, refused) = oneshot::channel();
    let (_retirement, retirement) = oneshot::channel();
    sender
        .send(AcknowledgedRuntimeRequest::Event(Box::new(
            AcknowledgedEventRequest {
                event: Event::UserCancel {
                    reason: None,
                    cause: CancelCause::UserRequested,
                },
                acknowledgement,
                retirement,
            },
        )))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), refused)
            .await
            .expect("deferred exit must explicitly refuse queued work")
            .unwrap(),
        Err("runtime is exiting".into()),
    );
    // This unacknowledged event must also remain outside the exit-only path.
    harness
        .event_tx
        .send(Event::UserCancel {
            reason: None,
            cause: CancelCause::UserRequested,
        })
        .await
        .unwrap();
    sqlx::query("DROP TRIGGER reject_actor_exit_outbox")
        .execute(fixture.db.pool())
        .await
        .unwrap();
    let receipt = match tokio::time::timeout(std::time::Duration::from_secs(10), result)
        .await
        .expect("interruption reply must follow durable terminal observation")
        .unwrap()
        .unwrap()
    {
        NativeCloseInitialResult::NewlyAdmitted(receipt) => receipt,
        other => panic!("fresh Close must retain its admission result: {other:?}"),
    };
    let exit = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("actor must finish after exact interruption")
        .unwrap();
    assert_eq!(exit, RuntimeExitDisposition::CoordinatedShutdown);
    assert!(
        matches!(*state_rx.borrow(), ConvState::ToolExecuting { .. }),
        "exit-only actor must not publish cancellation from queued ordinary work"
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Stopped
    );
    let failures = fixture
        .db
        .list_close_cleanup_failures(run.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(&failures[0].occurrence.failure_occurrence_id)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(events, 1);
    let source = fixture
        .db
        .get_conversation(&fixture.source_transcript_id)
        .await
        .unwrap();
    assert!(matches!(source.state, ConvState::ToolExecuting { .. }));
    assert!(
        !fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .archived
    );
    assert_no_initial_inspection_writes(&fixture.db, run.attempt_id.as_str()).await;
}

#[tokio::test]
async fn process_shutdown_after_inventory_capture_retains_resource_admission_fences() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::{ClosePhase, CloseRunRef, CloseRunStatus};

    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let (phase_tx, mut phases) = mpsc::unbounded_channel();
    harness.runtime.native_close_phase_observer = Some(phase_tx);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
    let backend = CloseFixture::backend(fixture.db.clone());
    let gate = Arc::new(Mutex::new(()));
    let (response, mut result) = oneshot::channel();
    sender
        .send(initial_request_with_backend(
            admitted,
            &fixture,
            Arc::clone(&gate).lock_owned().await,
            response,
            Arc::clone(&backend),
        ))
        .await
        .unwrap();
    let running = spawn_actor(&mut harness);

    await_native_close_phase(&mut phases, Phase::Captured).await;
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let attempt: String = sqlx::query_scalar(
        "SELECT attempt_id FROM native_close_initial_receipts WHERE request_id = ?1",
    )
    .bind(fixture.command.request_id().to_string())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let sealed_inventory_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_inventories
         WHERE attempt_id = ?1 AND sealed = 1",
    )
    .bind(&attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(sealed_inventory_count, 1);
    let expected_resources: Vec<(String, String)> = sqlx::query_as(
        "SELECT scope, resource_kind FROM close_expected_retirement_resources WHERE attempt_id = ?1",
    )
    .bind(&attempt)
    .fetch_all(fixture.db.pool())
    .await
    .unwrap();
    let scope = fixture
        .db
        .get_conversation(TARGET_ID)
        .await
        .unwrap()
        .attached_work_scope_id
        .unwrap();
    assert_eq!(
        expected_resources,
        vec![(scope.as_str().to_owned(), "work_scope".to_owned())],
        "a no-worktree scope still owns its WorkScope resource"
    );
    assert_native_resource_admission_fenced(&backend, &scope).await;

    fence.close_for_process_shutdown();
    // Phase notifications report completed steps, not entry into classification.
    // Successful interruption can finish in one step; await its original response.
    let receipt = match tokio::time::timeout(std::time::Duration::from_secs(10), result)
        .await
        .expect("exact stopped owner must settle its original response")
        .unwrap()
        .unwrap()
    {
        NativeCloseInitialResult::NewlyAdmitted(receipt) => receipt,
        other => panic!("fresh initial Close must retain its original receipt: {other:?}"),
    };
    assert_eq!(receipt.admitted.attempt_id.as_str(), attempt);
    assert_eq!(receipt.request_id, fixture.command.request_id());
    assert_eq!(receipt.admitted.run_ordinal, 1);
    let exit = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("process shutdown must join the exact source actor")
        .unwrap();
    assert_eq!(exit, RuntimeExitDisposition::CoordinatedShutdown);

    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Stopped
    );
    assert_eq!(
        fixture
            .db
            .get_close_obligation(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap()
            .phase(),
        ClosePhase::Completed
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert_native_resource_admission_fenced(&backend, &scope).await;
    assert!(
        !fixture
            .db
            .get_conversation(TARGET_ID)
            .await
            .unwrap()
            .archived
    );
    for table in [
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "process shutdown must not write {table}");
    }
}

#[tokio::test]
async fn source_owned_initial_inspection_persists_only_normalized_empty_snapshot() {
    use phoenix_core::domain::close::ClosePhase;
    let fixture = CloseFixture::new().await;
    let mut continuation = initial_inspection_continuation(&fixture).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();
    let scopes = fixture
        .db
        .list_close_attempt_scopes(&attempt)
        .await
        .unwrap();
    assert!(!scopes.is_empty());
    assert!(scopes.iter().all(|scope| scope.captured_worktree.is_none()));
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_no_initial_inspection_writes(&fixture.db, &attempt).await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::RetirementRequested
        )
    );
    let persisted = fixture.db.get_close_obligation(&attempt).await.unwrap();
    assert_eq!(persisted.phase(), ClosePhase::RetirementRequested);
    assert_eq!(persisted.snapshot().unwrap().generation(), "no-worktree");
    assert_eq!(
        fixture
            .db
            .list_close_retirement_inspections(&attempt)
            .await
            .unwrap()
            .len(),
        0
    );
    for table in [
        "close_retirement_inventories",
        "close_expected_retirement_resources",
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        // The table identifier comes only from the fixed test-local list above;
        // the attempt is separately bound, never interpolated.
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&attempt)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "inspection must not dispatch or inventory {table}"
        );
    }
}

#[tokio::test]
async fn stale_source_aborts_joined_inventory_worker_before_owner_cleanup() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::ClosePhase;

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::AwaitingRetirementInspection)
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::RetirementRequested)
    );

    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let (exited_tx, exited) = oneshot::channel();
    continuation.set_inventory_preparation_test_barrier(entered_tx, release_rx, exited_tx);
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::PreparingInventory
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .expect("inventory worker must reach its pre-manager barrier")
        .expect("inventory worker must report entry");
    assert_eq!(fence.owner_count(), 2);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());

    let stale_step = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        continuation.step(&ConvState::Idle),
    )
    .await
    .expect("stale source must abort and join inventory preparation");
    assert_eq!(stale_step, Err(NativeCloseApplicationError::SourceRefused));
    tokio::time::timeout(std::time::Duration::from_secs(10), exited)
        .await
        .expect("aborted inventory worker must report exit")
        .expect("inventory worker lifecycle sender must remain live until exit");
    assert!(
        release.send(()).is_err(),
        "joined worker must drop its barrier receiver"
    );
    assert_eq!(continuation.phase, Phase::InspectionRequired);
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .phase(),
        ClosePhase::RetirementRequested
    );
    for table in [
        "close_retirement_inventories",
        "close_expected_retirement_resources",
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&attempt)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "stale worker must not write {table}");
    }

    drop(continuation);
    assert_eq!(fence.owner_count(), 0);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert!(reply.await.is_err());
}

#[tokio::test]
async fn fatal_authority_during_inventory_preparation_joins_worker_before_owner_cleanup() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::ClosePhase;

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::AwaitingRetirementInspection)
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::RetirementRequested)
    );

    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let (exited_tx, exited) = oneshot::channel();
    continuation.set_inventory_preparation_test_barrier(entered_tx, release_rx, exited_tx);
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::PreparingInventory
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .expect("inventory worker must reach its pre-manager barrier")
        .expect("inventory worker must report entry");
    assert_eq!(fence.owner_count(), 2);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    let mut exited = exited;

    fence.close("native_close_fatal_during_inventory_preparation_test");
    let fatal_step = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        continuation.step(&fixture.state),
    )
    .await
    .expect("fatal authority must abort and join inventory preparation");
    assert_eq!(fatal_step, Err(NativeCloseApplicationError::FatalAuthority));
    assert_eq!(
        exited.try_recv(),
        Ok(()),
        "the awaited fatal step must already have joined the worker"
    );
    assert!(
        release.send(()).is_err(),
        "joined worker must drop its barrier receiver"
    );
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    drop(continuation);
    assert_eq!(fence.owner_count(), 0);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert!(reply.await.is_err());
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .phase(),
        ClosePhase::RetirementRequested
    );
    for table in [
        "close_retirement_inventories",
        "close_expected_retirement_resources",
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&attempt)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "fatal preparation must not write {table}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_continuation_during_inventory_preparation_retains_worker_custody_until_exit() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::ClosePhase;

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::AwaitingRetirementInspection)
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::RetirementRequested)
    );

    let (entered_tx, entered) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let (exited_tx, exited) = oneshot::channel();
    continuation.set_inventory_preparation_test_barrier(entered_tx, release_rx, exited_tx);
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::PreparingInventory
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .expect("inventory worker must reach its pre-manager barrier")
        .expect("inventory worker must report entry");
    assert_eq!(fence.owner_count(), 2);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    drop(continuation);
    // This current-thread runtime cannot poll the aborted worker until we yield.
    // The reborrow and exact target gate must outlive the dropped continuation.
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    tokio::time::timeout(std::time::Duration::from_secs(20), exited)
        .await
        .expect("dropped continuation must abort its paused inventory worker")
        .expect("worker lifecycle sender must remain live until its future is dropped");
    assert!(
        release.send(()).is_err(),
        "aborted worker must drop its barrier receiver"
    );
    assert_eq!(fence.owner_count(), 0);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    assert!(reply.await.is_err());
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .phase(),
        ClosePhase::RetirementRequested
    );
    for table in [
        "close_retirement_inventories",
        "close_expected_retirement_resources",
        "close_retirement_resource_dispatches",
        "close_retirement_resources",
        "close_worktree_cleanup_plans",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1");
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&attempt)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "dropped preparation must not write {table}");
    }
}

#[tokio::test]
async fn empty_reinspection_collector_preserves_canonical_empty_request_without_writing() {
    use phoenix_core::domain::close::ClosePhase;
    let fixture = CloseFixture::new().await;
    let mut continuation = initial_inspection_continuation(&fixture).await;
    let attempt = continuation.receipt.admitted.attempt_id.clone();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::RetirementRequested
        )
    );
    fixture
        .db
        .return_close_attempt_to_reinspection(&attempt)
        .await
        .unwrap();
    let before = fixture
        .db
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(before.phase(), ClosePhase::AwaitingRetirementInspection);
    // Ordinary reentry invalidates active inspection authority. Chat-only
    // inspection uses the canonical no-worktree generation, not a fresh nonce.
    assert!(before.snapshot().is_none());
    let collected = crate::runtime::close_retirement::collect_native_initial_inspection(
        &fixture.db,
        attempt.clone(),
    )
    .await
    .unwrap();
    assert!(collected.request.scopes.is_empty());
    assert!(collected.empty_generation.is_none());
    let after = fixture
        .db
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(after.phase(), before.phase());
    assert_eq!(after.snapshot(), before.snapshot());
    assert_no_initial_inspection_writes(&fixture.db, attempt.as_str()).await;
}

#[tokio::test]
async fn stale_source_after_collection_cannot_commit_initial_inspection() {
    use phoenix_core::domain::close::ClosePhase;
    let fixture = CloseFixture::new().await;
    let mut continuation = initial_inspection_continuation(&fixture).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&ConvState::Idle).await,
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::InspectionRequired
    );
    let persisted = fixture.db.get_close_obligation(&attempt).await.unwrap();
    assert_eq!(persisted.phase(), ClosePhase::AwaitingRetirementInspection);
    assert!(persisted.snapshot().is_none());
    assert_no_initial_inspection_writes(&fixture.db, &attempt).await;
}

#[tokio::test]
async fn revoked_source_aborts_owned_collector_without_persisting_result() {
    use phoenix_core::domain::close::ClosePhase;
    let fixture = CloseFixture::new().await;
    let mut continuation = initial_inspection_continuation(&fixture).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    continuation.abort_inspection_if_source_revoked(&ConvState::Idle);
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::InspectionRequired
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::InspectionRequired
    );
    let persisted = fixture.db.get_close_obligation(&attempt).await.unwrap();
    assert_eq!(persisted.phase(), ClosePhase::AwaitingRetirementInspection);
    assert!(persisted.snapshot().is_none());
    assert_no_initial_inspection_writes(&fixture.db, &attempt).await;
}

#[tokio::test]
async fn invalid_captured_worktree_collector_does_not_route_generic_repair() {
    use phoenix_core::domain::close::{ClosePhase, RetirementFailureReason};
    let fixture = CloseFixture::new().await;
    let scope = fixture
        .db
        .get_conversation(TARGET_ID)
        .await
        .unwrap()
        .attached_work_scope_id
        .unwrap();
    sqlx::query(
        "UPDATE work_scopes SET environment_kind='allocated_worktree',
        worktree_path=?1, worktree_id=?2, worktree_fingerprint=?3,
        branch_name='invalid-close-worktree', base_branch='main' WHERE id=?4",
    )
    .bind(format!(
        "/nonexistent/native-close-{}",
        uuid::Uuid::new_v4()
    ))
    .bind("invalid-native-close-worktree")
    .bind("invalid-native-close-fingerprint")
    .bind(scope.as_str())
    .execute(fixture.db.pool())
    .await
    .unwrap();
    let receipt = fixture.admitted().await;
    let attempt = receipt.admitted.attempt_id.as_str();
    let captured = fixture.db.list_close_attempt_scopes(attempt).await.unwrap();
    assert_eq!(captured.len(), 1);
    assert!(captured[0].captured_worktree.is_some());
    assert!(matches!(
        fixture
            .db
            .prepare_native_close_initial_execution(&fixture.command, &receipt, &fixture.state)
            .await,
        phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    assert!(matches!(
        fixture
            .db
            .try_settle_native_close_initial(&fixture.command, &receipt, &fixture.state)
            .await,
        phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
            phoenix_db::native_close::NativeCloseInitialSettlement::Advanced(_)
        ))
    ));
    let failure = match crate::runtime::close_retirement::collect_native_initial_inspection(
        &fixture.db,
        receipt.admitted.attempt_id.clone(),
    )
    .await
    {
        Err(failure) => failure,
        Ok(_) => panic!("invalid captured worktree must fail inspection"),
    };
    assert!(
        matches!(failure, crate::runtime::close_retirement::NativeInitialInspectionFailure::Scope {
        scope: failed_scope, reason: RetirementFailureReason::IdentityNotProven, ..
    } if failed_scope == scope)
    );
    let persisted = fixture.db.get_close_obligation(attempt).await.unwrap();
    assert_eq!(persisted.phase(), ClosePhase::AwaitingRetirementInspection);
    assert!(persisted.snapshot().is_none());
    assert_no_initial_inspection_writes(&fixture.db, attempt).await;
}

#[tokio::test]
async fn invalid_captured_worktree_failure_is_classified_and_releases_exact_admission() {
    use phoenix_core::domain::close::{CloseRunRef, CloseRunStatus, RetirementFailureReason};
    use phoenix_db::CloseCleanupFailureAuthority;

    let fixture = CloseFixture::new().await;
    let scope = resolved_but_absent_captured_worktree(&fixture).await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();
    let original_result = continuation.admitted_result.clone();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ClassifyingInspectionFailure
    );
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Finished
    );
    let failures = fixture
        .db
        .list_close_cleanup_failures(&attempt)
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].occurrence.authority,
        CloseCleanupFailureAuthority::CapturedScope {
            scope: scope.clone(),
            resource: phoenix_core::domain::close::RetiredResourceIdentity::parse(
                phoenix_core::domain::close::RetiredResourceKind::WorkScope,
                phoenix_core::domain::close::LossItemIdentity::Opaque(
                    phoenix_core::domain::close::OpaqueIdentity::parse(scope.as_str()).unwrap(),
                ),
            )
            .unwrap(),
        }
    );
    assert_eq!(
        failures[0].occurrence.reason,
        RetirementFailureReason::IdentityNotProven
    );
    assert_eq!(
        failures[0].occurrence.detail,
        "captured worktree is absent before inspection"
    );
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(&failures[0].occurrence.failure_occurrence_id)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);
    let run = CloseRunRef {
        attempt_id: continuation.receipt.admitted.attempt_id.clone(),
        ordinal: continuation.receipt.admitted.run_ordinal,
    };
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Stopped
    );
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .phase(),
        phoenix_core::domain::close::ClosePhase::Completed
    );
    assert_eq!(reply.try_recv().unwrap().unwrap(), original_result);
    assert_eq!(fence.owner_count(), 0);
    drop(continuation);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

#[tokio::test]
async fn actor_services_initial_close_while_acknowledged_requests_remain_ready() {
    use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};

    let fixture = CloseFixture::new().await;
    resolved_but_absent_captured_worktree(&fixture).await;
    let mut harness = fixture.runtime(true);
    let fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let (response, result) = oneshot::channel();
    sender
        .send(initial_request(
            admitted,
            &fixture,
            Arc::clone(&gate).lock_owned().await,
            response,
        ))
        .await
        .unwrap();

    // These requests cannot match the executing initial Close intent. They are
    // classified refusals, not new Close work, but keep the same actor queue busy.
    let referenced = NativeCloseRunRef {
        target: fixture.target.clone(),
        attempt_id: CloseAttemptId::parse(uuid::Uuid::new_v4().to_string()).unwrap(),
        run_ordinal: CloseRunOrdinal::INITIAL,
    };
    let traffic_source = fixture.source.clone();
    let traffic_reference = referenced.clone();
    let traffic_fence = Arc::clone(&fence);
    let make_traffic = move || {
        let (response, _discarded_reply) = oneshot::channel();
        AcknowledgedRuntimeRequest::ExecuteNativeCloseObserve(Box::new(NativeCloseObserveRequest {
            source: traffic_source.clone(),
            referenced: traffic_reference.clone(),
            admitted: traffic_fence.try_acquire().unwrap(),
            target_gate: Arc::new(Mutex::new(())).try_lock_owned().unwrap(),
            response,
        }))
    };
    // Fill the queue before starting the actor, then replenish it through normal
    // bounded-channel backpressure. No sleep is used to synchronize progress.
    for _ in 0..3 {
        sender.send(make_traffic()).await.unwrap();
    }
    let traffic_sender = sender.clone();
    let (stop_traffic, mut traffic_stopped) = oneshot::channel();
    let traffic = tokio::spawn(async move {
        loop {
            let request = make_traffic();
            tokio::select! {
                biased;
                _ = &mut traffic_stopped => break,
                sent = traffic_sender.send(request) => {
                    if sent.is_err() { break; }
                }
            }
        }
    });
    let running = spawn_actor(&mut harness);
    let completed = tokio::time::timeout(std::time::Duration::from_secs(15), result).await;
    let _ = stop_traffic.send(());
    tokio::time::timeout(std::time::Duration::from_secs(10), traffic)
        .await
        .expect("traffic producer must stop before the FIFO barrier")
        .unwrap();
    let receipt = match completed
        .expect("initial Close must progress under queued traffic")
        .unwrap()
        .unwrap()
    {
        NativeCloseInitialResult::NewlyAdmitted(receipt) => receipt,
        other => panic!("fresh initial Close must retain its original receipt: {other:?}"),
    };

    // An awaited FIFO request observes the actor after it consumed/dropped the
    // finished continuation and all earlier traffic owners, not merely its reply.
    let (barrier, observed) = observe_request(
        &fixture,
        referenced,
        fence.try_acquire().unwrap(),
        Arc::new(Mutex::new(())).lock_owned().await,
    );
    let observed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        sender.send(barrier).await.unwrap();
        observed.await.unwrap()
    })
    .await
    .expect("FIFO barrier must observe the completed continuation");
    assert!(matches!(
        observed,
        Err(NativeCloseApplicationError::SourceRefused)
    ));
    let failures = fixture
        .db
        .list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(fence.owner_count(), 0);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("actor must shut down after exact terminal classification")
        .unwrap();
}

#[tokio::test]
async fn failed_inspection_classification_retains_exact_intent_for_classification_only_retry() {
    use phoenix_core::domain::close::{CloseRunRef, CloseRunStatus, RetirementFailureReason};

    let fixture = CloseFixture::new().await;
    let scope = resolved_but_absent_captured_worktree(&fixture).await;
    let gate = Arc::new(Mutex::new(()));
    let (response, mut reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    let attempt = continuation.receipt.admitted.attempt_id.as_str().to_owned();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
            phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection
        )
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::CollectingInspection
    );
    continuation.await_inspection_completion_for_test().await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ClassifyingInspectionFailure
    );

    sqlx::query(
        "CREATE TRIGGER reject_initial_close_failure_outbox BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected failure outbox error'); END",
    ).execute(fixture.db.pool()).await.unwrap();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::ClassifyingInspectionFailure
    );
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::ClassifyingInspectionFailure
    );
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .phase(),
        phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection
    );
    assert!(fixture
        .db
        .list_close_cleanup_failures(&attempt)
        .await
        .unwrap()
        .is_empty());
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind = 'mandatory_close_failure'",
    ).fetch_one(fixture.db.pool()).await.unwrap();
    assert_eq!(outbox_count, 0);
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(fence.owner_count(), 1);
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
    sqlx::query("DROP TRIGGER reject_initial_close_failure_outbox")
        .execute(fixture.db.pool())
        .await
        .unwrap();

    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        native_close::NativeCloseInitialContinuationPhase::Finished
    );
    let failures = fixture
        .db
        .list_close_cleanup_failures(&attempt)
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].occurrence.reason,
        RetirementFailureReason::IdentityNotProven
    );
    assert_eq!(failures[0].occurrence.authority.scope(), Some(&scope));
    assert_eq!(
        failures[0].occurrence.detail,
        "captured worktree is absent before inspection"
    );
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(&failures[0].occurrence.failure_occurrence_id)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);
    let run = CloseRunRef {
        attempt_id: continuation.receipt.admitted.attempt_id.clone(),
        ordinal: continuation.receipt.admitted.run_ordinal,
    };
    assert_eq!(
        fixture.db.get_close_run(&run).await.unwrap().status,
        CloseRunStatus::Stopped
    );
    assert_eq!(
        reply.try_recv().unwrap().unwrap(),
        continuation.admitted_result
    );
    assert_eq!(fence.owner_count(), 0);
    drop(continuation);
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());
}

fn runtime(state: ConvState) -> RuntimeHarness {
    runtime_with_storage(state, Arc::new(InMemoryStorage::new()))
}
