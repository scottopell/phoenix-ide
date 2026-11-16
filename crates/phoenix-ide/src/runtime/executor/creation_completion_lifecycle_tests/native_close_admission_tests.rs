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
    AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(Box::new(NativeCloseInitialRequest {
        source: fixture.source.clone(),
        target: fixture.target.clone(),
        admitted,
        target_gate,
        response,
    }))
}

#[tokio::test]
async fn native_retry_backend_requires_actor_database_authority_not_matching_path() {
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let cloned_backend = fixture.db.clone();
    let unrelated_backend = Database::open_in_memory().await.unwrap();

    assert_eq!(
        harness.runtime.check_native_close_retry_backend(&cloned_backend),
        Ok(())
    );
    assert_eq!(
        harness.runtime.check_native_close_retry_backend(&unrelated_backend),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    harness.runtime.native_task_admission_db = None;
    assert_eq!(
        harness.runtime.check_native_close_retry_backend(&cloned_backend),
        Err(NativeCloseApplicationError::SourceRefused)
    );
    assert_eq!(count(&fixture.db, "close_runs").await, 0);
    assert_eq!(count(&unrelated_backend, "close_runs").await, 0);
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
async fn dropped_initial_reply_settles_queued_admission_and_releases_gate_before_next_ack() {
    let fixture = CloseFixture::new().await;
    let mut harness = fixture.runtime(true);
    let original_fence = Arc::clone(&harness.runtime.fatal_local_authority_fence);
    let admitted = harness.runtime.admit_authoritative_effect().unwrap();
    let sender = setup_actor_queue(&mut harness);
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

    let running = spawn_actor(&mut harness);
    // This ACK follows the owned Close request in the same FIFO. It therefore
    // proves the source actor classified the transaction despite the lost reply.
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
    assert_eq!(admitted, (1, 0, 0));
    assert!(Arc::clone(&gate).try_lock_owned().is_ok());

    assert_eq!(original_fence.owner_count(), 0);

    harness.event_tx.send(Event::Shutdown).await.unwrap();
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
    let (suspended, reached_boundary) = oneshot::channel();
    let owning_task = tokio::spawn(async move {
        let mut future = Box::pin(
            harness
                .runtime
                .execute_native_close_initial(source, target, admitted),
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
            .execute_native_close_initial(fixture.source.clone(), fixture.target.clone(), admitted,)
            .await,
        Err(NativeCloseApplicationError::Storage)
    );
    assert!(!original_fence.fatal_authority_loss_is_latched());
    assert_eq!(original_fence.owner_count(), 0);
    let next_owner = harness.runtime.admit_authoritative_effect().unwrap();
    drop(next_owner);
    assert_eq!(count(&fixture.db, "native_close_initial_receipts").await, 0);
}

fn runtime(state: ConvState) -> RuntimeHarness {
    runtime_with_storage(state, Arc::new(InMemoryStorage::new()))
}
