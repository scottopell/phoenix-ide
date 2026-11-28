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

mod bash_native_success;
mod bash_stage2_stop_ack;
mod bash_stage2_unknown_release;
mod registered_disposable_path;
#[cfg(target_os = "linux")]
mod worktree_physical;

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
        Self::from_db(Database::open_in_memory().await.unwrap()).await
    }

    async fn from_db(db: Database) -> Self {
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
        harness.runtime = harness.runtime.with_actor_database(self.db.clone());
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

fn install_source_entry_for_test(
    harness: &mut RuntimeHarness,
    fixture: &CloseFixture,
) -> native_close::source_entry::SourceEntryWorker {
    use native_close::source_entry::{ActorEntryResult, SourceEntryController};
    let mut controller = SourceEntryController::new(
        fixture.command.clone(),
        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
    );
    assert_eq!(
        controller.revalidate(&harness.runtime.state, &fixture.command),
        ActorEntryResult::Updated
    );
    let worker = controller.worker();
    harness.runtime.native_close_source_entry = Some(controller);
    worker
}

#[tokio::test]
async fn native_close_source_entry_is_revoked_by_acknowledged_stop_in_real_actor() {
    use native_close::source_entry::WorkerEntryResult;
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let worker = install_source_entry_for_test(&mut harness, &fixture);
    // Prove this was an open cell, not an initially refusing test double.
    assert_eq!(
        worker.try_first_effect(&fixture.command, || ()),
        WorkerEntryResult::Ran(())
    );
    let (sender, running) = start_actor(&mut harness);
    stop_actor(&sender).await;
    let mut effects = 0;
    assert_eq!(
        worker.try_first_effect(&fixture.command, || effects += 1),
        WorkerEntryResult::Refused
    );
    assert_eq!(effects, 0);
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("actor must exit after acknowledged Stop")
        .unwrap();
}

#[tokio::test]
async fn native_close_source_entry_is_revoked_by_shutdown_in_real_actor() {
    use native_close::source_entry::WorkerEntryResult;
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let worker = install_source_entry_for_test(&mut harness, &fixture);
    assert_eq!(
        worker.try_first_effect(&fixture.command, || ()),
        WorkerEntryResult::Ran(())
    );
    let (_sender, running) = start_actor(&mut harness);
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("shutdown must settle the real actor")
        .unwrap();
    let mut effects = 0;
    assert_eq!(
        worker.try_first_effect(&fixture.command, || effects += 1),
        WorkerEntryResult::Refused
    );
    assert_eq!(effects, 0);
}

#[tokio::test]
async fn native_close_source_entry_cannot_reopen_after_live_state_replacement() {
    use native_close::source_entry::WorkerEntryResult;
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let worker = install_source_entry_for_test(&mut harness, &fixture);
    harness
        .runtime
        .install_live_state(ConvState::Idle, chrono::Utc::now(), false)
        .unwrap();
    harness
        .runtime
        .install_live_state(fixture.state.clone(), chrono::Utc::now(), false)
        .unwrap();
    let mut effects = 0;
    assert_eq!(
        worker.try_first_effect(&fixture.command, || effects += 1),
        WorkerEntryResult::Refused
    );
    assert_eq!(effects, 0);
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

async fn wait_for_native_initial_phase(
    db: &Database,
    attempt_id: &phoenix_core::domain::close::CloseAttemptId,
    expected: phoenix_core::domain::close::ClosePhase,
) -> phoenix_core::domain::close::CloseObligation {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let obligation = db.get_close_obligation(attempt_id.as_str()).await.unwrap();
            if obligation.phase() == expected {
                return obligation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("native Initial did not reach {expected:?}"))
}

async fn wait_for_target_mutation_gate_release(backend: &crate::runtime::RuntimeManager) {
    let gate = backend.mutation_admission(TARGET_ID).await.unwrap();
    let _guard = tokio::time::timeout(std::time::Duration::from_secs(10), gate.lock_owned())
        .await
        .expect("native Initial must release the target mutation gate after terminal evidence");
}

struct ParkedNativeInitial {
    fixture: CloseFixture,
    backend: Arc<crate::runtime::RuntimeManager>,
    run: NativeCloseRunRef,
    route: phoenix_db::native_close::NativeCloseInitialControlRoute,
    observer: oneshot::Receiver<Result<NativeCloseInitialResult, NativeCloseApplicationError>>,
    actor: tokio::task::JoinHandle<RuntimeExitDisposition>,
    source_events: mpsc::Sender<Event>,
}

impl ParkedNativeInitial {
    async fn new() -> Self {
        let fixture = CloseFixture::new().await;
        fixture.db.update_conversation_state(TARGET_ID, &fixture.state).await.unwrap();
        let backend = CloseFixture::backend(fixture.db.clone());
        Self::from_busy_target(fixture, backend).await
    }

    async fn from_busy_target(fixture: CloseFixture, backend: Arc<crate::runtime::RuntimeManager>) -> Self {
        use native_close::NativeCloseInitialContinuationPhase as Phase;
        use phoenix_core::domain::close::TranscriptConversationId;

        let mut harness = fixture.runtime(true);
        let (phase_tx, mut phases) = mpsc::unbounded_channel();
        harness.runtime.native_close_phase_observer = Some(phase_tx);
        let sender = setup_actor_queue(&mut harness);
        let (state_tx, state_rx) = tokio::sync::watch::channel(fixture.state.clone());
        harness.runtime = harness.runtime
            .with_state_watcher(state_tx)
            .with_fatal_local_authority_fence(Arc::clone(&backend.fatal_local_authority_fence));
        backend.runtimes.write().await.insert(
            fixture.source_transcript_id.clone(),
            crate::runtime::ConversationHandle {
                event_tx: crate::runtime::ConversationEventSender::for_test_receiver(harness.event_tx.clone()),
                acknowledged_event_tx: crate::runtime::AcknowledgedEventSender::for_test_receiver(sender.clone()),
                handoff_review_tx: None,
                turn_trigger: crate::runtime::TurnTriggerSlot::default(),
                broadcast_tx: crate::runtime::SseBroadcaster::new(128, 0),
                identity: Arc::new(()),
                state_rx,
                startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
            },
        );
        let target_gate = backend.mutation_admission(TARGET_ID).await.unwrap();
        let admitted = harness.runtime.admit_authoritative_effect().unwrap();
        let (response, observer) = oneshot::channel();
        sender.send(initial_request_with_backend(
            admitted,
            &fixture,
            Arc::clone(&target_gate).lock_owned().await,
            response,
            Arc::clone(&backend),
        )).await.unwrap();
        let actor = spawn_actor(&mut harness);
        await_native_close_phase(&mut phases, Phase::AwaitingStopWorkConfirmation).await;
        assert!(target_gate.try_lock().is_err(), "parked source owns the target mutation gate");
        let obligation = fixture.db.get_active_close_obligation_for_product(&fixture.target)
            .await.unwrap().unwrap();
        let run = NativeCloseRunRef {
            target: fixture.target.clone(),
            attempt_id: obligation.attempt_id().clone(),
            run_ordinal: phoenix_core::domain::close::CloseRunOrdinal::INITIAL,
        };
        let latest = TranscriptConversationId::parse(TARGET_ID).unwrap();
        let phoenix_db::native_close::CloseTargetControlResolution::ParkedNativeInitial(route) =
            fixture
                .db
                .resolve_close_target_control(&fixture.target, &latest, &run.attempt_id)
                .await
                .unwrap()
        else {
            panic!("expected parked native Initial source route");
        };
        Self { fixture, backend, run, route, observer, actor, source_events: harness.event_tx }
    }

    async fn control(&self, action: phoenix_db::native_close::NativeCloseInitialControl) {
        tokio::time::timeout(std::time::Duration::from_secs(10),
            self.backend.control_native_close_initial(self.route.clone(), action))
            .await.expect("source actor must not wait on its own target mutation gate")
            .expect("source actor must accept the exact target control");
    }

    async fn finish(self) {
        wait_for_target_mutation_gate_release(&self.backend).await;
        assert!(matches!(self.observer.await.unwrap().unwrap(), NativeCloseInitialResult::NewlyAdmitted(_)));
        self.source_events.send(Event::Shutdown).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), self.actor).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn actor_cancel_of_parked_native_initial_releases_target_gate_without_source_write() {
    use phoenix_core::domain::close::{CloseCompletionOutcome, ClosePhase};

    let parked = ParkedNativeInitial::new().await;
    parked.control(phoenix_db::native_close::NativeCloseInitialControl::Cancel).await;
    let obligation = parked.fixture.db.get_close_obligation(parked.run.attempt_id.as_str()).await.unwrap();
    assert_eq!(obligation.phase(), ClosePhase::Completed);
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::Cancelled));
    let source = parked.fixture.db.get_conversation(&parked.fixture.source_transcript_id).await.unwrap();
    assert_eq!(source.state, parked.fixture.state, "target Cancel must not alter the source occurrence");
    assert_eq!(count(&parked.fixture.db, "native_close_initial_receipts").await, 1);
    assert_eq!(count(&parked.fixture.db, "close_runs").await, 1);
    parked.finish().await;
}

async fn live_close_settlement_target(
    backend: &Arc<crate::runtime::RuntimeManager>,
    db: &Database,
) -> (
    phoenix_workflow::TurnAuthorityId,
    mpsc::Sender<Event>,
    ConversationRuntime<Arc<DatabaseStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
) {
    use phoenix_core::domain::sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    };
    use phoenix_core::domain::db_schema::InputOrigin;
    use phoenix_workflow::{
        AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
        TurnOutcome,
    };

    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: "close-target-input".into(),
            origin: InputOrigin::UserApi,
            text: "target work".into(),
            images: vec![], files: vec![], user_agent: None, skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "target work".into(), llm_text: None,
            images: vec![], files: vec![], user_agent: None, skill_invocation: None,
        },
    );
    let accepted = db.workflow_repository().accept_authoritative_turn(
        &phoenix_db::workflow::AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("close-target-input").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(TARGET_ID.to_string()), payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        },
    ).await.established().unwrap().unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("expected an authoritative target turn");
    };
    let state = ConvState::LlmRequesting { attempt: 1 };
    db.update_conversation_state(TARGET_ID, &state).await.unwrap();
    let (events, event_rx) = mpsc::channel(8);
    let (ack, ack_rx) = mpsc::channel(8);
    let (event_tx, acknowledged_event_tx, cancellation_rx) =
        crate::runtime::cancellation_inbox::channel(events.clone(), ack);
    let broadcast_tx = crate::runtime::SseBroadcaster::new(128, 0);
    let (state_tx, state_rx) = tokio::sync::watch::channel(state.clone());
    let runtime = ConversationRuntime::new(
        ConvContext::new(TARGET_ID, std::path::PathBuf::from("/tmp"), "test-model", 200_000, 16_384),
        state,
        Arc::new(crate::runtime::traits::DatabaseStorage::new(db.clone())),
        Arc::new(MockLlmClient::new("test-model")),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        event_rx, events.clone(), broadcast_tx.clone(),
    )
    .with_actor_database(db.clone())
    .with_cancellation_input_receiver(cancellation_rx)
    .with_acknowledged_event_receiver(ack_rx)
    .with_state_watcher(state_tx)
    .with_fatal_local_authority_fence(Arc::clone(&backend.fatal_local_authority_fence))
    .with_active_direct_turn(Some(crate::runtime::traits::ActiveDirectTurn { turn_id, generation: 0 }));
    backend.runtimes.write().await.insert(TARGET_ID.to_string(), crate::runtime::ConversationHandle {
        event_tx, acknowledged_event_tx, broadcast_tx, state_rx,
        handoff_review_tx: None,
        turn_trigger: crate::runtime::TurnTriggerSlot::default(),
        identity: Arc::new(()),
        startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
    });
    (turn_id, events, runtime)
}

#[tokio::test]
async fn actor_stop_confirmation_of_parked_native_initial_releases_target_gate() {
    use phoenix_core::domain::close::ClosePhase;

    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let (turn, _events, mut target) = live_close_settlement_target(&backend, &fixture.db).await;
    let parked = ParkedNativeInitial::from_busy_target(fixture, backend).await;
    parked.control(phoenix_db::native_close::NativeCloseInitialControl::ConfirmStopWork).await;
    let input = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        target.cancellation_input_rx.as_mut().unwrap().recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(input, crate::runtime::cancellation_inbox::CancellationInput::DirectTurn(_)));
    target.retain_cancellation_input(input);
    assert!(!target.has_pending_effective_cancellation());
    let _ = target.settle_cancellation_input().await;
    wait_for_native_initial_phase(&parked.fixture.db, &parked.run.attempt_id, ClosePhase::Completed).await;
    let row: (i64, String, bool) = sqlx::query_as(
        "SELECT generation, terminal_kind, owns_conversation FROM durable_turns WHERE turn_id = ?1",
    ).bind(i64::try_from(turn.0).unwrap()).fetch_one(parked.fixture.db.pool()).await.unwrap();
    assert_eq!(row, (1, "Cancelled".into(), false));
    let settled: bool = sqlx::query_scalar(
        "SELECT settled_at IS NOT NULL FROM close_attempt_direct_turn_settlements WHERE attempt_id = ?1 AND turn_id = ?2",
    ).bind(parked.run.attempt_id.as_str()).bind(i64::try_from(turn.0).unwrap())
        .fetch_one(parked.fixture.db.pool()).await.unwrap();
    assert!(settled, "mailbox delivery alone must not establish release");
    parked.finish().await;
}

#[tokio::test]
async fn queued_native_close_settlement_stop_refuses_after_source_revocation() {
    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let (turn, _events, mut target) = live_close_settlement_target(&backend, &fixture.db).await;
    let parked = ParkedNativeInitial::from_busy_target(fixture, backend).await;
    parked.control(phoenix_db::native_close::NativeCloseInitialControl::ConfirmStopWork).await;
    let input = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        target.cancellation_input_rx.as_mut().unwrap().recv(),
    ).await.unwrap().unwrap();
    assert!(matches!(input, crate::runtime::cancellation_inbox::CancellationInput::DirectTurn(_)));
    target.retain_cancellation_input(input);
    assert!(!target.has_pending_effective_cancellation());
    parked.source_events.send(Event::UserCancel {
        reason: None,
        cause: CancelCause::UserRequested,
    }).await.unwrap();
    wait_for_target_mutation_gate_release(&parked.backend).await;
    assert!(target.settle_cancellation_input().await.is_none());
    assert!(matches!(target.state, ConvState::LlmRequesting { .. }));
    let retained: (i64, Option<String>, bool) = sqlx::query_as(
        "SELECT generation, terminal_kind, owns_conversation FROM durable_turns WHERE turn_id = ?1",
    ).bind(i64::try_from(turn.0).unwrap()).fetch_one(parked.fixture.db.pool()).await.unwrap();
    assert_eq!(retained, (0, None, true));
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_close_initial_settlement_stop_claims WHERE attempt_id = ?1 AND turn_id = ?2",
    )
    .bind(parked.run.attempt_id.as_str())
    .bind(i64::try_from(turn.0).unwrap())
    .fetch_one(parked.fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(claims, 0);
    parked.finish().await;
}

#[tokio::test]
async fn actor_refuses_stale_native_initial_target_control_route() {
    let parked = ParkedNativeInitial::new().await;
    parked.control(phoenix_db::native_close::NativeCloseInitialControl::Cancel).await;
    assert_eq!(
        parked.backend.control_native_close_initial(parked.route.clone(),
            phoenix_db::native_close::NativeCloseInitialControl::Cancel).await,
        Err(NativeCloseApplicationError::SourceRefused),
    );
    parked.finish().await;
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
    harness.runtime.actor_database = None;
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
    let mut continuation = match admission {
        Ok(native_close::NativeCloseInitialAdmission::Continued {
            _result: NativeCloseInitialResult::NewlyAdmitted(receipt),
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

    // A response classification is not durable authority release. Exercise
    // revocation between phase workers, without fabricating a SQLite Stop.
    continuation.abort_inspection_if_source_revoked(&fixture.state);
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    continuation.abort_inspection_if_source_revoked(&ConvState::Idle);
    assert_eq!(
        reply.try_recv().unwrap(),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    drop(reply);
    continuation.abort_inspection_if_source_revoked(&ConvState::Idle);
    assert!(!continuation.release_if_exactly_terminal().await);
    assert_eq!(
        continuation.phase,
        native_close::NativeCloseInitialContinuationPhase::Prepare
    );
    assert!(continuation.authority.owner.is_some());
    assert_eq!(fence.owner_count(), 1);
    assert!(!fence.fatal_authority_loss_is_latched());
    assert!(Arc::clone(&gate).try_lock_owned().is_err());
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
    let native_close::NativeCloseInitialAdmission::Continued {
        mut continuation, ..
    } = admission
    else {
        panic!("fresh admission must return an owned continuation");
    };

    assert_eq!(
        continuation.step(&ConvState::Idle).await,
        Err(NativeCloseApplicationError::Storage)
    );
    assert!(!continuation.release_if_exactly_terminal().await);
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
    let native_close::NativeCloseInitialAdmission::Continued {
        mut continuation, ..
    } = admission
    else {
        panic!("fresh admission must return an owned continuation");
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
    assert!(continuation.release_if_exactly_terminal().await);
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
    let native_close::NativeCloseInitialAdmission::Continued {
        mut continuation, ..
    } = admission
    else {
        panic!("fresh admission must return an owned continuation");
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

    assert!(!continuation.release_if_exactly_terminal().await);
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
        Ok(native_close::NativeCloseInitialAdmission::Finished {
            _result: NativeCloseInitialResult::Replay(ref replayed_receipt),
        }) if replayed_receipt == &receipt
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
    let Ok(native_close::NativeCloseInitialAdmission::Continued {
        continuation: pending,
        ..
    }) = admission
    else {
        panic!("fresh admission must own pending work");
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
    .with_actor_database(fixture.db.clone());
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
            _result: NativeCloseInitialResult::NewlyAdmitted(receipt),
            continuation,
        } => {
            assert_eq!(continuation.receipt, receipt);
            (*continuation, fence)
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
        native_close::NativeCloseInitialAdmission::Continued { continuation, .. } => *continuation,
        native_close::NativeCloseInitialAdmission::Finished { .. } => {
            panic!("new initial Close must retain its source owner");
        }
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
    assert!(continuation.release_if_exactly_terminal().await);
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
async fn queued_stop_in_cancellation_inbox_wins_over_due_close_settlement() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;

    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let (settlement_tx, mut settlement_entries) = mpsc::unbounded_channel();
    harness.runtime.native_close_settlement_entry_observer = Some(settlement_tx);
    let (phase_tx, mut phases) = mpsc::unbounded_channel();
    harness.runtime.native_close_phase_observer = Some(phase_tx);
    let (ordinary_ack, _) = mpsc::channel(1);
    let (_, stop_sender, cancellation_rx) =
        crate::runtime::cancellation_inbox::channel(harness.event_tx.clone(), ordinary_ack);
    harness.runtime = harness
        .runtime
        .with_cancellation_input_receiver(cancellation_rx);
    let close_sender = setup_actor_queue(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let (response, _close_reply) = oneshot::channel();
    close_sender
        .send(initial_request(
            harness.runtime.admit_authoritative_effect().unwrap(),
            &fixture,
            Arc::clone(&gate).lock_owned().await,
            response,
        ))
        .await
        .unwrap();

    // Drive the actor in this task so no Close step can run between the phase
    // witness and queuing Stop. Advance the actual actor clock past its next tick.
    let actor = harness.runtime.run();
    tokio::pin!(actor);
    tokio::select! {
        biased;
        phase = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            await_native_close_phase(&mut phases, Phase::Settle),
        ) => phase.expect("Close must reach its settlement phase"),
        exit = &mut actor => panic!("actor exited before Close was ready to settle: {exit:?}"),
    }
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    tokio::time::resume();

    let (acknowledgement, stop_ack) = oneshot::channel();
    let (_retirement, retirement) = oneshot::channel();
    stop_sender
        .send(AcknowledgedEventRequest {
            event: Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            },
            acknowledgement,
            retirement,
        })
        .await
        .unwrap();
    let stopped = tokio::select! {
        biased;
        ack = tokio::time::timeout(std::time::Duration::from_secs(10), stop_ack) => ack,
        exit = &mut actor => panic!("actor exited before Stop was acknowledged: {exit:?}"),
    };
    assert_eq!(
        stopped.unwrap().unwrap().unwrap(),
        AcknowledgedEventOutcome::Settled
    );
    assert!(
        matches!(settlement_entries.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "due Close must not enter settlement ahead of the queued inbox Stop, even if settlement would fail"
    );

    harness.event_tx.send(Event::Shutdown).await.unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), actor)
            .await
            .unwrap(),
        RuntimeExitDisposition::CoordinatedShutdown,
    );
    assert!(
        gate.try_lock().is_ok(),
        "actor exit must observe Close owner release"
    );
}

#[tokio::test]
async fn process_shutdown_wakes_dormant_close_without_an_actor_event() {
    assert_actor_exit_from_dormant_close(true).await;
}

#[tokio::test]
async fn eviction_shutdown_refuses_ordinary_work_while_close_exit_rolls_back() {
    assert_actor_exit_from_dormant_close(false).await;
}

// Keep dormant admission, actor exit, rollback, and owner-release witnesses in order.
#[allow(clippy::too_many_lines)]
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

/// Real capture and Terminal entry, with a passive /dev/null handle (no PTY child).
/// The held in-memory connection proves pool unavailability, not `SQLite` BUSY.
#[tokio::test]
async fn acknowledged_stop_revokes_captured_terminal_entry_while_db_pool_is_unavailable() {
    run_acknowledged_stop_revokes_captured_terminal_entry(CloseFixture::new().await, None).await;
}

#[tokio::test]
async fn acknowledged_stop_revokes_captured_terminal_entry_while_independent_sqlite_writer_is_held()
{
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native-close.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    let fixture = CloseFixture::from_db(db).await;
    run_acknowledged_stop_revokes_captured_terminal_entry(fixture, Some(&path)).await;
    // The helper has completed and joined the actor before the disposable DB is removed.
    drop(temp);
}

// Keep capture, writer contention, Stop ACK, and forbidden terminal-entry proofs ordered.
#[allow(clippy::too_many_lines)]
async fn run_acknowledged_stop_revokes_captured_terminal_entry(
    fixture: CloseFixture,
    independent_writer_path: Option<&std::path::Path>,
) {
    use crate::runtime::close_retirement::native_initial::resource::TerminalEntryTestHook;
    use phoenix_terminal::command_tracker::CommandTracker;
    use phoenix_terminal::session::{
        ShellIntegrationStatus, StopReason, TerminalChildKind, TerminalHandle,
        TerminalLaunchIdentity, TerminalRetirementRequest,
    };
    use sqlx::Connection;

    let scopes: Vec<String> = sqlx::query_scalar(
        "SELECT work_scope_id FROM product_conversation_work_scopes WHERE product_conversation_id = ?1",
    )
    .bind(fixture.target.as_str())
    .fetch_all(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(scopes.len(), 1, "the fixture has one original WorkScope");
    let scope = phoenix_core::work_scope::WorkScopeId::parse(scopes[0].clone()).unwrap();
    let backend = CloseFixture::backend(fixture.db.clone());
    let fd: std::os::fd::OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .unwrap()
        .into();
    let (stop_tx, _) = tokio::sync::watch::channel(StopReason::Running);
    let terminal = TerminalHandle {
        master_fd: std::sync::Mutex::new(Some(fd)),
        child_pid: nix::unistd::Pid::from_raw(1),
        child_lifecycle: phoenix_terminal::child_lifecycle::TerminalChildLifecycle::passive(),
        launch_identity: TerminalLaunchIdentity {
            process: phoenix_core::process_identity::ProcessIdentity {
                pid: 1,
                start_time: 1,
            },
            launch_uuid: "native-close-blocked-terminal-test".into(),
        },
        child_kind: TerminalChildKind::Shell,
        tracker: Arc::new(std::sync::Mutex::new(CommandTracker::new(
            "close-blocked-terminal".to_string(),
        ))),
        shell_integration_status: Arc::new(std::sync::Mutex::new(ShellIntegrationStatus::Unknown)),
        stop_tx,
        attach_permit: Arc::new(tokio::sync::Semaphore::new(1)),
        retirement_requested: TerminalRetirementRequest::default(),
    };
    let key = phoenix_core::work_scope::ResourceScopeKey::Work(scope.clone());
    let terminal = backend
        .terminals
        .try_insert_exact(key.clone(), terminal)
        .unwrap();
    let (reached, reached_rx) = oneshot::channel::<bool>();
    let (resume, resume_rx) = oneshot::channel();
    let (pending, pending_rx) = oneshot::channel();
    let (callback, callback_rx) = oneshot::channel();
    let (completed, completed_rx) = oneshot::channel();
    backend
        .set_terminal_entry_test_hook(TerminalEntryTestHook {
            scope,
            reached: Some(reached),
            resume: resume_rx,
            pending: Some(pending),
            callback: Some(callback),
            completed: Some(completed),
        })
        .await;
    let mut harness = fixture.runtime(true);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
    let gate = Arc::new(Mutex::new(()));
    let (response, reply) = oneshot::channel();
    sender
        .send(initial_request_with_backend(
            admitted,
            &fixture,
            gate.lock_owned().await,
            response,
            Arc::clone(&backend),
        ))
        .await
        .unwrap();
    let actor = spawn_actor(&mut harness);
    assert!(
        tokio::time::timeout(Duration::from_secs(30), reached_rx)
            .await
            .expect("captured Terminal never reached DB-entry boundary")
            .unwrap(),
        "source cell was not eligible, unrevoked, and before its original deadline at DB wait"
    );
    // InMemoryStorage lets actor Stop settle independently. Durable ToolExecuting
    // source is NOT changed: DB rejection must not mask a broken live source gate.
    // Keep the independent connection outside its borrowed SQLx transaction so
    // rollback and connection closure have explicit, separate ownership boundaries.
    let mut independent_connection = match independent_writer_path {
        Some(path) => {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(path)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .busy_timeout(Duration::ZERO);
            let connection = sqlx::SqliteConnection::connect_with(&options)
                .await
                .unwrap();
            Some(connection)
        }
        None => None,
    };
    let writer = match independent_connection.as_mut() {
        Some(connection) => {
            let writer = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
            // Explicitly distinguish external SQLite writer contention from an
            // exhausted application pool: acquire and return a pool connection now.
            drop(fixture.db.pool().acquire().await.unwrap());
            writer
        }
        None => fixture
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap(),
    };
    resume.send(()).unwrap();
    // In the independent-writer variant, this proves that the same entry
    // future's first poll returned Pending while the app pool was available.
    // It does not prove SQLx's worker had reached SQLite BEGIN IMMEDIATE then.
    assert!(tokio::time::timeout(Duration::from_secs(30), pending_rx)
        .await
        .expect("entry did not poll its DB future")
        .unwrap());
    stop_actor(&sender).await;
    let untouched = || {
        assert!(!terminal.retirement_requested.is_requested());
        assert_eq!(*terminal.stop_tx.borrow(), StopReason::Running);
        assert!(terminal.master_fd_raw().is_some());
        assert!(Arc::ptr_eq(
            &terminal,
            &backend.terminals.get(&key).unwrap()
        ));
    };
    untouched();
    // Release before DB assertions/teardown; continue the SAME entry future.
    writer.rollback().await.unwrap();
    if let Some(connection) = independent_connection {
        connection.close().await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(30), completed_rx)
        .await
        .expect("retained Terminal entry did not settle after writer release")
        .unwrap();
    untouched();
    // The pre-wait witness proves the source was open and before its original
    // deadline; this callback witness requires it already revoked, still before
    // that same deadline, and Refused. Together with the acknowledged Stop, this
    // supports source revocation on Stop (including controller Drop), but does not
    // claim the flag's unique cause: another concurrent source revoker could race.
    assert_eq!(
        callback_rx
            .await
            .expect("DB validator never invoked the entry callback"),
        (true, true, true)
    );
    let attempt: String = sqlx::query_scalar(
        "SELECT attempt_id FROM native_close_initial_receipts WHERE request_id = ?1",
    )
    .bind(fixture.command.request_id().to_string())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert!(fixture
        .db
        .list_close_retirement_evidence(&attempt)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        backend.terminals.reserve_spawn(&key),
        Err(phoenix_terminal::session::ActiveTerminalInsertError::RetirementFenced)
    ));
    drop(reply);
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), actor)
        .await
        .expect("source actor did not shut down")
        .unwrap();
    // Refused original custody stays fenced until fixture epoch teardown. Never
    // reopen via legacy cancellation or complete the passive child for cleanup.
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn process_shutdown_after_inventory_capture_retains_resource_admission_fences() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::{
        CloseCompletionOutcome, ClosePhase, CloseRunRef, CloseRunStatus, CloseStopCertainty,
        RetirementFailureReason, RetirementOutcome,
    };
    use phoenix_db::{
        CloseCleanupFailureAuthority, CloseCleanupFailureResource, CloseCleanupResourceDisposition,
    };

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
    let captured_resources = fixture
        .db
        .list_close_expected_retirement_resources(&attempt)
        .await
        .unwrap();
    assert_eq!(captured_resources.len(), 1);
    assert!(fixture
        .db
        .list_close_retirement_evidence(&attempt)
        .await
        .unwrap()
        .is_empty());
    let history_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_history WHERE attempt_id = ?1",
    )
    .bind(&attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(history_before, 0);
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
    assert_eq!(receipt.admitted.run_ordinal.get(), 1);
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
    // Interruption observes the sealed unresolved resource; it performs no retirement.
    assert_eq!(
        fixture
            .db
            .list_close_expected_retirement_resources(&attempt)
            .await
            .unwrap(),
        captured_resources
    );
    assert!(fixture
        .db
        .close_retirement_inventory_is_complete(&attempt)
        .await
        .unwrap());
    let evidence = fixture
        .db
        .list_close_retirement_evidence(&attempt)
        .await
        .unwrap();
    assert_eq!(evidence.len(), 1);
    let expected = &captured_resources[0];
    let residual = &evidence[0];
    assert_eq!(residual.attempt_id, expected.attempt_id);
    assert_eq!(residual.scope, expected.scope);
    assert_eq!(residual.snapshot, expected.snapshot);
    assert_eq!(residual.resource, expected.resource);
    assert_eq!(
        residual.outcome,
        RetirementOutcome::Residual {
            residual_reason: RetirementFailureReason::ManualRepairRequired,
        }
    );
    let detail = "process_interrupted: initial native Close admission stopped before effects; exact sealed initial retirement inventory retained";
    assert_eq!(residual.detail.as_deref(), Some(detail));
    assert_eq!(residual.created_at, residual.updated_at);
    let failures = fixture
        .db
        .list_close_cleanup_failures(&attempt)
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].run_ordinal, run.ordinal);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert_eq!(failure.attempt_id, expected.attempt_id);
    assert_eq!(failure.source_product_conversation_id, fixture.target);
    assert_eq!(
        failure.authority,
        CloseCleanupFailureAuthority::ExpectedResource {
            scope: expected.scope.clone(),
            snapshot: expected.snapshot.clone(),
            resource: expected.resource.clone(),
        }
    );
    assert_eq!(
        failure.remaining_resources,
        vec![CloseCleanupFailureResource {
            scope: expected.scope.clone(),
            resource: expected.resource.clone(),
            disposition: CloseCleanupResourceDisposition::Failed,
        }]
    );
    assert_eq!(failure.reason, RetirementFailureReason::Interrupted);
    assert_eq!(failure.detail, detail);
    assert_eq!(
        failure.stop_certainty,
        CloseStopCertainty::ShutdownUncertain
    );
    assert_eq!(
        fixture
            .db
            .get_close_obligation(&attempt)
            .await
            .unwrap()
            .close_outcome(),
        Some(CloseCompletionOutcome::CloseIncomplete)
    );
    // Count every row as well as the exact matching observation: extra success/history
    // rows must not be hidden by the typed current-snapshot reader or a filtered join.
    let observation_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1),
            (SELECT COUNT(*) FROM close_retirement_resource_history WHERE attempt_id = ?1),
            (SELECT COUNT(*) FROM close_retirement_resource_history h
             JOIN close_retirement_resources r
               ON r.attempt_id = h.attempt_id AND r.scope = h.scope
              AND r.inspection_generation = h.inspection_generation
              AND r.inspection_fingerprint = h.inspection_fingerprint
              AND r.resource_kind = h.resource_kind AND r.identity_kind = h.identity_kind
              AND r.identity_codec = h.identity_codec AND r.identity_value = h.identity_value
             WHERE h.attempt_id = ?1 AND h.proof_kind = 'residual'
               AND r.proof_kind = h.proof_kind AND r.absence_basis IS NULL
               AND h.absence_basis IS NULL AND r.residual_reason = h.residual_reason
               AND r.detail = h.detail AND h.recorded_at = r.created_at
               AND r.created_at = r.updated_at)",
    )
    .bind(&attempt)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(observation_counts, (1, 1, 1));
    let event_counts: (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1),
            (SELECT COUNT(*) FROM coordinator_watch_events e
             JOIN close_cleanup_failures f ON f.failure_occurrence_id = e.event_id
             WHERE f.failure_occurrence_id = ?1 AND e.mandatory_failure_occurrence_id = ?1
               AND e.source_occurrence_id = ?1 AND e.route_kind = 'mandatory_close_failure'
               AND e.watch_id IS NULL AND e.mandatory_source_product_id = ?2
               AND e.source_occurrence_kind = 'close_cleanup_failure'
               AND e.source_generation = 0 AND e.source_transcript_id = ?3
               AND e.terminal_kind = 'cleanup_failed' AND e.terminal_reason = 'interrupted'
               AND e.occurred_at_us = f.occurred_at_us)",
    )
    .bind(run.failure_occurrence_id())
    .bind(fixture.target.as_str())
    .bind(TARGET_ID)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(event_counts, (1, 1));
    let active_scope: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_scopes WHERE id = ?1 AND lifecycle = 'active' AND retired_at IS NULL",
    )
    .bind(scope.as_str())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(active_scope, 1);
    for table in [
        "close_retirement_resource_dispatches",
        "close_worktree_cleanup_plans",
        "close_process_step_successes",
        "close_run_retry_effects",
        "close_run_retry_successes",
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
async fn initial_exit_retains_inspection_collector_before_terminal_interruption() {
    use native_close::NativeCloseInitialContinuationPhase as Phase;
    use phoenix_core::domain::close::ClosePhase;

    let fixture = CloseFixture::new().await;
    let gate = Arc::new(Mutex::new(()));
    let (response, _reply) = oneshot::channel();
    let (mut continuation, fence) =
        admit_initial_continuation(&fixture, Arc::clone(&gate), response).await;
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Settle
    );
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ReachedSettlementPhase(ClosePhase::AwaitingRetirementInspection)
    );
    let (entered_tx, entered) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (exited_tx, exited) = oneshot::channel();
    continuation.set_inspection_collection_test_barrier(entered_tx, released, exited_tx);
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::CollectingInspection
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    continuation.begin_exit_interruption();
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::ClassifyingExitInterruption
    );
    assert!(
        gate.try_lock().is_err(),
        "pending collector must retain target gate"
    );
    assert_eq!(fence.owner_count(), 1);
    assert!(!continuation.release_if_exactly_terminal().await);
    let obligation = fixture
        .db
        .get_close_obligation(continuation.receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(obligation.phase(), ClosePhase::AwaitingRetirementInspection);

    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), exited)
        .await
        .unwrap()
        .unwrap();
    // The exit guard sends after collection returns, with no await afterward.
    // This current-thread test cannot resume until that task poll completes.
    assert_eq!(
        continuation.step(&fixture.state).await.unwrap(),
        Phase::Finished
    );
    assert!(continuation.release_if_exactly_terminal().await);
    assert_eq!(fence.owner_count(), 0);
    drop(continuation);
    assert!(gate.try_lock().is_ok());
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
    assert_eq!(
        reply.try_recv(),
        Ok(Err(NativeCloseApplicationError::SourceRefused)),
        "revocation settles the observer without releasing the retained owner or gate"
    );
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
    // The SourceRefused response was consumed by try_recv above. Polling this
    // completed oneshot again would panic rather than prove owner cleanup.
    drop(reply);
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
    tokio::time::timeout(std::time::Duration::from_secs(10), &mut exited)
        .await
        .expect("fatal worker cancellation must reach its completion witness")
        .expect("inventory worker lifecycle sender must remain live until exit");
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
async fn generic_cancel_rejects_native_initial_before_reopening_resource_fences() {
    use phoenix_core::domain::close::CloseRetirementSnapshot;

    let fixture = CloseFixture::new().await;
    let receipt = fixture.admitted().await;
    let attempt = &receipt.admitted.attempt_id;
    let scopes = fixture
        .db
        .list_close_attempt_scopes(attempt.as_str())
        .await
        .unwrap();
    assert_eq!(scopes.len(), 1);
    let scope = &scopes[0].scope;
    let backend = CloseFixture::backend(fixture.db.clone());
    let before = fixture
        .db
        .get_close_obligation(attempt.as_str())
        .await
        .unwrap();
    // Acquire real registry fences without advancing the durable phase.
    let snapshot =
        CloseRetirementSnapshot::parse("cancel-fence-generation", "cancel-fence").unwrap();
    backend
        .prepare_close_retirement_inventory(attempt.clone(), snapshot)
        .await
        .unwrap();
    assert_native_resource_admission_fenced(&backend, scope).await;

    let error = backend
        .cancel_close_before_retirement(attempt)
        .await
        .unwrap_err();
    assert!(error.contains("owned by native initial Close settlement"));
    assert_native_resource_admission_fenced(&backend, scope).await;
    assert_eq!(
        fixture
            .db
            .get_close_obligation(attempt.as_str())
            .await
            .unwrap(),
        before
    );
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
    let Err(failure) = crate::runtime::close_retirement::collect_native_initial_inspection(
        &fixture.db,
        receipt.admitted.attempt_id.clone(),
    )
    .await
    else {
        panic!("invalid captured worktree must fail inspection");
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
// Keep failed inspection, exact-intent retention, and classification-only retry causal.
#[allow(clippy::too_many_lines)]
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

#[tokio::test]
// Keep the real registered tool, held gate, Stop ACK, and refusal witnesses together.
#[allow(clippy::too_many_lines)]
async fn registered_close_waiting_for_target_gate_is_refused_after_source_stop() {
    use crate::coordinator_tools;
    use crate::send_chat_service::SendChatApplicationService;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    let fixture = CloseFixture::new().await;
    let backend = CloseFixture::backend(fixture.db.clone());
    let retriever = Arc::new(fixture.db.fts_retriever());
    let read_service =
        crate::api::global_read::GlobalReadService::new(fixture.db.clone(), retriever);
    let send_chat = Arc::new(SendChatApplicationService::new(
        fixture.db.clone(),
        Arc::clone(&backend),
    ));
    let handles = Arc::new(crate::tools::BashHandleRegistry::new());
    let registered_tools = coordinator_tools::tools(
        read_service,
        send_chat,
        crate::api::work_scope_load::WorkScopeLoadService::new(
            &handles,
            crate::api::resource_monitor::ResourceMonitor::new(),
            phoenix_terminal::ActiveTerminals::new(),
        ),
        &fixture.db,
    );
    let tool = registered_tools
        .into_iter()
        .find(|tool| tool.name() == "close_conversation")
        .expect("the coordinator registration must include close_conversation");

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
    .with_actor_database(fixture.db.clone());
    // Install the real actor's channels, not the fake drain helper. Constructing
    // an actor in its executing state avoids production startup materialization's
    // transient-state recovery and does not launch a second copy of the tool.
    let (sender, receiver) = mpsc::channel(4);
    let (watch_tx, state_rx) = tokio::sync::watch::channel(fixture.state.clone());
    let (startup, started) = oneshot::channel();
    actor = actor
        .with_fatal_local_authority_fence(Arc::clone(&backend.fatal_local_authority_fence))
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver)
        .with_startup_llm_recovery_ack(startup);
    backend.runtimes.write().await.insert(
        fixture.source_transcript_id.clone(),
        crate::runtime::ConversationHandle {
            event_tx: crate::runtime::ConversationEventSender::for_test_receiver(event_tx.clone()),
            acknowledged_event_tx: crate::runtime::AcknowledgedEventSender::for_test_receiver(
                sender.clone(),
            ),
            handoff_review_tx: None,
            turn_trigger: crate::runtime::TurnTriggerSlot::default(),
            broadcast_tx: crate::runtime::SseBroadcaster::new(128, 0),
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(Mutex::new(None)),
        },
    );
    let running = tokio::spawn(actor.run());
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .expect("source actor must finish startup")
        .unwrap()
        .unwrap();
    let target_admission = backend
        .conversation_admission(fixture.target.as_str())
        .await;
    let target_gate = Arc::clone(&target_admission).lock_owned().await;

    let context = crate::tools::ToolContext::new(
        CancellationToken::new(),
        fixture.source_transcript_id.clone(),
        PathBuf::from("/tmp"),
        Arc::new(crate::tools::BrowserSessionManager::default()),
        handles,
        Arc::new(phoenix_llm::ModelRegistry::new_empty()),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(crate::tools::TmuxRegistry::new()),
        None,
        phoenix_core::work_scope::WorkScopeId::parse("close-source-test").unwrap(),
    )
    .with_tool_use_id(fixture.source.tool_use_id.clone())
    .with_svg_assistant_message_id(fixture.source.message_id.clone());
    let input = serde_json::json!({
        "action":"initial",
        "product_conversation_id":fixture.target.as_str()
    });

    let (blocked_tx, blocked_rx) = oneshot::channel();
    let close_tool = tokio::spawn(async move {
        let mut close = Box::pin(tool.run(input, context));
        assert!(futures::poll!(close.as_mut()).is_pending());
        blocked_tx.send(()).unwrap();
        close.await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), blocked_rx)
        .await
        .expect("registered close must reach the held target gate")
        .unwrap();

    stop_actor(&sender).await;
    drop(target_gate);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), close_tool)
        .await
        .expect("registered close must settle after target admission resumes")
        .unwrap();
    assert!(!output.is_success(), "a stopped source must reject Close");
    let error: serde_json::Value = serde_json::from_str(output.output()).unwrap();
    assert_eq!(error["code"], "source_refused");
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
    assert_eq!(count(&fixture.db, "close_runs").await, 0);
    assert!(target_admission.try_lock_owned().is_ok());
    assert_eq!(backend.fatal_local_authority_fence.owner_count(), 0);
    event_tx.send(Event::Shutdown).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("source actor must shut down after rejecting Close")
        .unwrap();
}
