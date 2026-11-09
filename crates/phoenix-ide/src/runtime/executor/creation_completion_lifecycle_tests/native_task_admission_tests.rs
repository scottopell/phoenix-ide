use super::*;
use crate::runtime::task_approval_admission::{FrozenPendingTask, NativeTaskAdmissionReadiness};
use crate::state_machine::event::CancelCause;
use crate::state_machine::state::TaskApprovalOutcome;
use chrono::Utc;
use phoenix_core::domain::db_schema::{
    ConvMode, FinalizedToolResult, InputOrigin, Message, MessageType, NonEmptyString,
    SourceToolCall,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::task_approval::{ProposalOccurrence, ReviewedProposal, ReviewedTaskKind};
use phoenix_core::task_source::{Priority, TaskSource};
use phoenix_db::task_approval_inspection::{
    InspectPendingTaskInput, INSPECT_PENDING_TASK_TOOL_NAME,
};
use phoenix_db::Database;
use serde_json::json;
use std::sync::atomic::Ordering;

mod interleavings;

const BODY: &str = "# Exact reviewed task\r\n\nKeep these bytes.\n";

struct AdmissionFixture {
    db: Database,
    target: String,
    expected: FrozenPendingTask,
}

impl AdmissionFixture {
    async fn new() -> Self {
        Self::with_cwd("/nonexistent").await
    }

    async fn with_cwd(cwd: &str) -> Self {
        Self::with_cwd_and_task_file(cwd, "tasks/reviewed.md").await
    }

    async fn with_cwd_and_task_file(cwd: &str, task_file: &str) -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let target = db
            .create_conversation("actor-target", "actor-target", cwd, true, None, None)
            .await
            .unwrap();
        let pending = pending_state_at_path(true, task_file);
        let assistant = proposal_message(&target.id, task_file);
        let result = FinalizedToolResult::new(
            &assistant.message_id,
            &target.id,
            2,
            assistant.created_at,
            &ToolResult::success("proposal-call".into(), "awaiting approval".into()),
        );
        db.persist_tool_round_and_state(
            &target.id,
            &assistant,
            &[result],
            &pending,
            assistant.created_at,
        )
        .await
        .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let input = InspectPendingTaskInput {
            target: target.product_conversation_id,
        };
        let source = SourceToolCall {
            message_id: "executing-inspector".into(),
            tool_use_id: "inspection-call".into(),
        };
        let payload = serde_json::to_value(&input).unwrap();
        let executing = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: INSPECT_PENDING_TASK_TOOL_NAME.into(),
                    input: payload.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: crate::state_machine::AssistantMessage::new(
                source.message_id.clone(),
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id.clone(),
                    name: INSPECT_PENDING_TASK_TOOL_NAME.into(),
                    input: payload,
                }],
                None,
                None,
            ),
        };
        db.update_conversation_state(&coordinator.id, &executing)
            .await
            .unwrap();
        let inspection = db
            .inspect_pending_task_from_coordinator(&coordinator.id, &source, &input)
            .await
            .unwrap();
        let expected = FrozenPendingTask::from_inspection(inspection).unwrap();
        Self {
            db,
            target: target.id,
            expected,
        }
    }

    fn harness(&self, state: ConvState) -> RuntimeHarness {
        let mut harness = runtime_with_storage(state, Arc::new(InMemoryStorage::new()));
        harness.runtime.context.conversation_id = self.target.clone();
        harness.runtime = harness
            .runtime
            .with_native_task_admission_database(self.db.clone());
        harness
    }
}

async fn checked(
    sender: &mpsc::Sender<AcknowledgedRuntimeRequest>,
    expected: FrozenPendingTask,
) -> NativeTaskAdmissionReadiness {
    let (response, result) = oneshot::channel();
    sender
        .send(
            AcknowledgedRuntimeRequest::InspectNativeTaskApprovalAdmission {
                expected: Box::new(expected),
                response,
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), result)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn event_settled(
    sender: &mpsc::Sender<AcknowledgedRuntimeRequest>,
    event: Event,
) -> AcknowledgedEventOutcome {
    let (acknowledgement, result) = oneshot::channel();
    let (_retirement, retirement) = oneshot::channel();
    sender
        .send(AcknowledgedRuntimeRequest::Event(Box::new(
            AcknowledgedEventRequest {
                event,
                acknowledgement,
                retirement,
            },
        )))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), result)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn objective_count(db: &Database) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM conversation_approved_task_objectives")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn real_stop_consumes_pending_then_bound_check_is_typed_stale_without_approval_effects() {
    let fixture = AdmissionFixture::new().await;
    let mut harness = fixture.harness(pending_state(true));
    let tools = Arc::clone(&harness.runtime.tool_executor);
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(pending_state(true));
    let (sender, receiver) = mpsc::channel(4);
    harness.runtime = harness
        .runtime
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver);
    let running = tokio::spawn(harness.runtime.run());
    assert_eq!(
        event_settled(
            &sender,
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested
            }
        )
        .await,
        AcknowledgedEventOutcome::Settled
    );
    assert_eq!(*watch_rx.borrow(), ConvState::Idle);
    assert_eq!(
        harness.storage.get_current_state(&fixture.target),
        Some(ConvState::Idle)
    );
    let cancellation_messages = harness.storage.recorded_messages();
    assert!(
        !cancellation_messages.is_empty(),
        "real cancellation persists its rejection message"
    );
    let objectives = objective_count(&fixture.db).await;
    assert_eq!(
        checked(&sender, fixture.expected.clone()).await,
        NativeTaskAdmissionReadiness::StalePending
    );
    assert_eq!(
        serde_json::to_value(harness.storage.recorded_messages()).unwrap(),
        serde_json::to_value(&cancellation_messages).unwrap()
    );
    assert_eq!(objective_count(&fixture.db).await, objectives);
    assert!(harness
        .storage
        .approved_task_authority(&fixture.target)
        .is_none());
    assert!(harness.llm.recorded_requests().is_empty());
    assert!(tools.recorded_executions().is_empty());
    // Preserve the existing absorbed stale HTTP-event semantics, not native success.
    assert_eq!(
        event_settled(
            &sender,
            Event::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::Approved {
                    handoff: crate::state_machine::state::TaskApprovalHandoff::ContinueInCurrentConversation
                },
            }
        )
        .await,
        AcknowledgedEventOutcome::Settled
    );
    assert_eq!(*watch_rx.borrow(), ConvState::Idle);
    assert_eq!(
        serde_json::to_value(harness.storage.recorded_messages()).unwrap(),
        serde_json::to_value(&cancellation_messages).unwrap()
    );
    assert!(harness.llm.recorded_requests().is_empty());
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn matching_actor_pending_is_only_readiness_and_later_stop_still_cancels() {
    let fixture = AdmissionFixture::new().await;
    let mut harness = fixture.harness(pending_state(true));
    let (sender, receiver) = mpsc::channel(4);
    let (watch_tx, watch_rx) = tokio::sync::watch::channel(pending_state(true));
    harness.runtime = harness
        .runtime
        .with_state_watcher(watch_tx)
        .with_acknowledged_event_receiver(receiver);
    let running = tokio::spawn(harness.runtime.run());
    let before = objective_count(&fixture.db).await;
    assert_eq!(
        checked(&sender, fixture.expected.clone()).await,
        NativeTaskAdmissionReadiness::PendingMatchesRequiresFullAdmission
    );
    assert_eq!(*watch_rx.borrow(), pending_state(true));
    assert!(harness.storage.recorded_messages().is_empty());
    assert!(harness
        .storage
        .approved_task_authority(&fixture.target)
        .is_none());
    assert!(harness.llm.recorded_requests().is_empty());
    assert_eq!(objective_count(&fixture.db).await, before);
    event_settled(
        &sender,
        Event::UserCancel {
            reason: None,
            cause: CancelCause::UserRequested,
        },
    )
    .await;
    assert_eq!(*watch_rx.borrow(), ConvState::Idle);
    assert_eq!(
        checked(&sender, fixture.expected.clone()).await,
        NativeTaskAdmissionReadiness::StalePending
    );
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn canonical_taskmd_under_tasks_is_ready_at_actor_without_approval_effects() {
    let task_file = "tasks/53019-p1-ready--plan.md";
    let fixture = AdmissionFixture::with_cwd_and_task_file("/nonexistent", task_file).await;
    let state = fixture
        .db
        .get_conversation(&fixture.target)
        .await
        .unwrap()
        .state;
    let ConvState::AwaitingTaskApproval {
        task_file: captured_path,
        title,
        priority,
        plan,
        reviewed_proposal: Some(reviewed),
    } = &state
    else {
        panic!("canonical persisted taskmd proposal");
    };
    assert_eq!(captured_path, task_file);
    assert_eq!(title, "Exact title");
    assert_eq!(*priority, Priority::P1);
    assert_eq!(plan, BODY);
    assert_eq!(reviewed.kind, ReviewedTaskKind::Taskmd);
    let mut harness = fixture.harness(state.clone());
    let (sender, receiver) = mpsc::channel(1);
    harness.runtime = harness.runtime.with_acknowledged_event_receiver(receiver);
    let running = tokio::spawn(harness.runtime.run());
    assert_eq!(
        checked(&sender, fixture.expected.clone()).await,
        NativeTaskAdmissionReadiness::PendingMatchesRequiresFullAdmission
    );
    assert_eq!(
        fixture
            .db
            .get_conversation(&fixture.target)
            .await
            .unwrap()
            .state,
        state
    );
    assert!(harness.storage.recorded_messages().is_empty());
    assert!(harness
        .storage
        .approved_task_authority(&fixture.target)
        .is_none());
    assert!(harness.llm.recorded_requests().is_empty());
    assert_eq!(objective_count(&fixture.db).await, 0);
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}

#[tokio::test]
async fn actor_checks_every_pending_field_and_same_body_new_occurrence() {
    let fixture = AdmissionFixture::new().await;
    for change in 0..10 {
        let mut state = pending_state(true);
        let ConvState::AwaitingTaskApproval {
            task_file,
            title,
            priority,
            plan,
            reviewed_proposal: Some(reviewed),
        } = &mut state
        else {
            unreachable!()
        };
        match change {
            0 => task_file.push_str(".changed.md"),
            1 => title.push_str(" changed"),
            2 => *priority = Priority::P2,
            3 => plan.push('\n'),
            4 => reviewed.occurrence.assistant_message_id = "replacement-assistant".into(),
            5 => reviewed.occurrence.tool_use_id = "replacement-call".into(),
            6 => reviewed.kind = phoenix_core::domain::task_approval::ReviewedTaskKind::Taskmd,
            7 => reviewed.body_sha256 = "changed".into(),
            8 => state = pending_state(false),
            9 => {}
            _ => unreachable!(),
        }
        let mut harness = fixture.harness(state);
        if change == 9 {
            harness.runtime.context.conversation_id = "another-actor".into();
        }
        let (sender, receiver) = mpsc::channel(1);
        harness.runtime = harness.runtime.with_acknowledged_event_receiver(receiver);
        let running = tokio::spawn(harness.runtime.run());
        assert_eq!(
            checked(&sender, fixture.expected.clone()).await,
            if change == 8 {
                NativeTaskAdmissionReadiness::Unavailable
            } else if change == 9 {
                NativeTaskAdmissionReadiness::TargetChanged
            } else {
                NativeTaskAdmissionReadiness::StalePending
            }
        );
        assert!(harness.storage.recorded_messages().is_empty());
        assert!(harness
            .storage
            .approved_task_authority(&fixture.target)
            .is_none());
        assert!(harness.llm.recorded_requests().is_empty());
        harness.event_tx.send(Event::Shutdown).await.unwrap();
        running.await.unwrap();
    }
}

#[tokio::test]
async fn current_owner_advance_refuses_frozen_actor_without_retargeting() {
    let fixture = AdmissionFixture::new().await;
    fixture
        .db
        .update_conversation_state(
            &fixture.target,
            &ConvState::ContextExhausted {
                summary: "continue".into(),
            },
        )
        .await
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = fixture
        .db
        .continue_conversation(&fixture.target)
        .await
        .unwrap()
    else {
        panic!("successor")
    };
    let mut harness = fixture.harness(pending_state(true));
    let (sender, receiver) = mpsc::channel(1);
    harness.runtime = harness.runtime.with_acknowledged_event_receiver(receiver);
    let running = tokio::spawn(harness.runtime.run());
    assert_eq!(
        checked(&sender, fixture.expected.clone()).await,
        NativeTaskAdmissionReadiness::TargetChanged
    );
    assert_ne!(successor.id, fixture.target);
    assert!(harness.storage.recorded_messages().is_empty());
    assert!(harness.llm.recorded_requests().is_empty());
    assert_eq!(objective_count(&fixture.db).await, 0);
    harness.event_tx.send(Event::Shutdown).await.unwrap();
    running.await.unwrap();
}
fn proposal_message(target: &str, task_file: &str) -> Message {
    Message {
        message_id: "proposal-assistant".into(),
        conversation_id: target.into(),
        sequence_id: 1,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "proposal-call".into(),
            name: "propose_task".into(),
            input: json!({"task_file":task_file}),
        }]),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    }
}
fn pending_state(captured: bool) -> ConvState {
    pending_state_at_path(captured, "tasks/reviewed.md")
}

fn pending_state_at_path(captured: bool, task_file: &str) -> ConvState {
    ConvState::AwaitingTaskApproval {
        task_file: task_file.into(),
        title: "Exact title".into(),
        priority: Priority::P1,
        plan: BODY.into(),
        reviewed_proposal: captured.then(|| {
            ReviewedProposal::capture(
                ProposalOccurrence::capture("proposal-assistant", "proposal-call").unwrap(),
                &std::path::Path::new(task_file)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(TaskSource::detect)
                    .unwrap(),
                BODY,
            )
        }),
    }
}

#[tokio::test]
async fn production_manager_cancel_before_native_command_has_no_approval_artifacts() {
    use crate::runtime::task_approval_admission::NativeTaskApprovalOutcome;
    use phoenix_db::native_task_approval::{
        NativeApprovalIntent, NativeApprovalPlacement, NativeTaskApprovalCommand,
    };
    let cwd = tempfile::tempdir().unwrap();
    let f = AdmissionFixture::with_cwd(cwd.path().to_str().unwrap()).await;
    let coordinator =
        f.db.get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
    let source = SourceToolCall {
        message_id: "executing-inspector".into(),
        tool_use_id: "inspection-call".into(),
    };
    let inspection =
        f.db.inspect_pending_task_from_coordinator(
            &coordinator.id,
            &source,
            &InspectPendingTaskInput {
                target: f
                    .db
                    .get_conversation(&f.target)
                    .await
                    .unwrap()
                    .product_conversation_id,
            },
        )
        .await
        .unwrap();
    let phoenix_db::task_approval_inspection::PendingTaskStatus::Pending(pending) =
        inspection.pending
    else {
        panic!("captured pending task");
    };
    let command = NativeTaskApprovalCommand {
        source_transcript_id: coordinator.id.clone(),
        source: source.clone(),
        owner: inspection.owner,
        current_transcript_id: f.target.clone(),
        pending,
        placement: NativeApprovalPlacement::ContinueHere,
        intent: NativeApprovalIntent::ApproveTaskAndRequestContinuation,
    };
    let input = command.input_value().unwrap();
    let source_state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &source.tool_use_id,
            ToolInput::Unknown {
                name: "approve_task".into(),
                input: input.clone(),
            },
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            source.message_id.clone(),
            vec![ContentBlock::ToolUse {
                id: source.tool_use_id.clone(),
                name: "approve_task".into(),
                input,
            }],
            None,
            None,
        ),
    };
    f.db.update_conversation_state(&coordinator.id, &source_state)
        .await
        .unwrap();
    let manager = Arc::new(crate::runtime::RuntimeManager::new(
        f.db.clone(),
        Arc::new(ModelRegistry::new_empty()),
        crate::platform::PlatformCapability::None {
            details: "native test".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    let handle = manager.get_or_create(&f.target).await.unwrap();
    assert_eq!(
        event_settled(
            &handle.acknowledged_event_tx,
            Event::UserCancel {
                reason: None,
                cause: CancelCause::UserRequested,
            }
        )
        .await,
        AcknowledgedEventOutcome::Settled
    );
    let before = f.db.get_messages(&f.target).await.unwrap().len();
    assert!(matches!(
        manager.execute_native_task_approval(command).await,
        NativeTaskApprovalOutcome::NotApplied(_)
    ));
    assert_cancelled_native_approval_unchanged(&f, before).await;
    handle.event_tx.send(Event::Shutdown).await.unwrap();
}

async fn assert_cancelled_native_approval_unchanged(f: &AdmissionFixture, before: usize) {
    assert_eq!(f.db.get_messages(&f.target).await.unwrap().len(), before);
    assert_eq!(
        f.db.get_conversation(&f.target).await.unwrap().state,
        ConvState::Idle
    );
    assert_eq!(objective_count(&f.db).await, 0);
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_task_approval_receipts")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(receipts, 0);
}

const NATIVE_APPROVAL_REPLY: &str = "Native approval continuation completed offline.";

struct NativeApprovalLlm {
    calls: std::sync::atomic::AtomicUsize,
    requests: mpsc::UnboundedSender<phoenix_llm::LlmRequest>,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for NativeApprovalLlm {
    async fn complete(
        &self,
        request: &phoenix_llm::LlmRequest,
    ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.send(request.clone()).unwrap();
        Ok(phoenix_llm::LlmResponse::non_streaming(
            vec![ContentBlock::text(NATIVE_APPROVAL_REPLY)],
            true,
            phoenix_llm::Usage::default(),
        ))
    }

    fn model_id(&self) -> &'static str {
        "claude-sonnet-5"
    }
}

struct NativeApprovalReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);

impl NativeApprovalReleaseOnDrop {
    fn release(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

impl Drop for NativeApprovalReleaseOnDrop {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitialApprovalScenario {
    ContextPressure,
    FreezeFailure,
    ProviderSuccess,
    DroppedCallerProviderSuccess,
    FollowUpProviderSuccess,
    FollowUpCompensatedFailure,
    InitialReceiptFailure,
    FollowUpReceiptFailure,
    InitialProjectionFailure,
    FollowUpProjectionFailure,
}

struct InitialApprovalHarness {
    temp: Option<tempfile::TempDir>,
    manager: Option<Arc<crate::runtime::RuntimeManager>>,
    handle: Option<crate::runtime::ConversationHandle>,
    caller: Option<
        tokio::task::JoinHandle<crate::runtime::task_approval_admission::NativeTaskApprovalOutcome>,
    >,
    release: Option<NativeApprovalReleaseOnDrop>,
    projection_release: Option<tokio::sync::oneshot::Sender<()>>,
}

impl InitialApprovalHarness {
    fn new() -> Self {
        Self {
            temp: None,
            manager: None,
            handle: None,
            caller: None,
            release: None,
            projection_release: None,
        }
    }

    fn release_gate(&mut self) {
        if let Some(release) = self.projection_release.take() {
            let _ = release.send(());
        }
        if let Some(release) = &mut self.release {
            release.release();
        }
    }

    async fn cleanup(&mut self) {
        self.release_gate();
        if let Some(caller) = self.caller.take() {
            caller.abort();
            let _ = caller.await;
        }
        if let Some(manager) = &self.manager {
            manager.begin_process_shutdown();
            manager.drain_process_shutdown().await;
        }
        if let Some(handle) = &self.handle {
            handle.event_tx.closed().await;
        }
        drop(self.manager.take());
        drop(self.handle.take());
        drop(self.temp.take());
    }
}

#[tokio::test]
async fn production_initial_approval_provider_success_preserves_body_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::ProviderSuccess,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_native_follow_up_plain_markdown_approval_succeeds_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FollowUpProviderSuccess,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_native_follow_up_taskmd_approval_succeeds_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FollowUpProviderSuccess,
        ReviewedTaskKind::Taskmd,
    ))
    .await;
}

#[tokio::test]
async fn production_native_follow_up_taskmd_compensates_promotion_failure() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FollowUpCompensatedFailure,
        ReviewedTaskKind::Taskmd,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_receipt_failure_after_git_rolls_back_and_fences() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::InitialReceiptFailure,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_follow_up_receipt_failure_after_git_rolls_back_and_fences() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FollowUpReceiptFailure,
        ReviewedTaskKind::Taskmd,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_postcommit_projection_failure_retains_receipt_and_fences() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::InitialProjectionFailure,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_follow_up_postcommit_projection_failure_retains_receipt_and_fences() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FollowUpProjectionFailure,
        ReviewedTaskKind::Taskmd,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_approval_retains_queued_admission_after_caller_abort() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::DroppedCallerProviderSuccess,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_taskmd_approval_provider_success_preserves_body_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::ProviderSuccess,
        ReviewedTaskKind::Taskmd,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_approval_settles_immediate_context_pressure_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::ContextPressure,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

#[tokio::test]
async fn production_initial_approval_settles_immediate_freeze_failure_and_replays() {
    Box::pin(assert_production_initial_approval(
        InitialApprovalScenario::FreezeFailure,
        ReviewedTaskKind::PlainMarkdown,
    ))
    .await;
}

async fn assert_production_initial_approval(
    scenario: InitialApprovalScenario,
    kind: ReviewedTaskKind,
) {
    use futures::FutureExt;

    let mut harness = InitialApprovalHarness::new();
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Box::pin(assert_production_initial_approval_inner(
            scenario,
            kind,
            &mut harness,
        )),
    ))
    .catch_unwind()
    .await;
    harness.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("initial approval fixture timed out after 30 seconds"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[allow(clippy::too_many_lines)]
async fn assert_production_initial_approval_inner(
    scenario: InitialApprovalScenario,
    kind: ReviewedTaskKind,
    harness: &mut InitialApprovalHarness,
) {
    use super::super::test_git_helpers::{add_explore_worktree, init_repo};
    use crate::runtime::task_approval_admission::NativeTaskApprovalOutcome;
    use crate::runtime::{RuntimeManager, SseEvent};
    use phoenix_db::conversation_goals::control::GoalControlInstallation;
    use phoenix_db::native_task_approval::{
        NativeApprovalContinuation, NativeTaskApprovalInspection,
    };

    let (temp, root) = init_repo();
    harness.temp = Some(temp);
    let worktree = add_explore_worktree(&root, "actor-target", "main");
    std::fs::create_dir_all(worktree.join("tasks")).unwrap();
    let (task_file, committed_task_file) = match kind {
        ReviewedTaskKind::PlainMarkdown => ("tasks/reviewed.md", "tasks/reviewed.md"),
        ReviewedTaskKind::Taskmd => (
            "tasks/53019-p1-ready--plan.md",
            "tasks/53019-p1-in-progress--plan.md",
        ),
    };
    std::fs::write(worktree.join(task_file), BODY).unwrap();
    if kind == ReviewedTaskKind::Taskmd {
        git(&worktree, &["add", "--", task_file]);
        git(&worktree, &["commit", "-m", "Seed existing ready taskmd"]);
    }
    if matches!(
        scenario,
        InitialApprovalScenario::FollowUpProviderSuccess
            | InitialApprovalScenario::FollowUpCompensatedFailure
            | InitialApprovalScenario::FollowUpReceiptFailure
            | InitialApprovalScenario::FollowUpProjectionFailure
    ) {
        std::fs::write(worktree.join("unrelated-staged.txt"), "keep staged").unwrap();
        git(&worktree, &["add", "--", "unrelated-staged.txt"]);
    }
    let f = AdmissionFixture::with_cwd_and_task_file("/nonexistent", task_file).await;
    assert_eq!(
        f.db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    f.db.update_conversation_mode_and_cwd(
        "actor-target",
        &ConvMode::Explore {
            worktree_path: Some(
                NonEmptyString::new(worktree.to_string_lossy().into_owned()).unwrap(),
            ),
            next_taskmd_id_hint: None,
        },
        worktree.to_str().unwrap(),
    )
    .await
    .unwrap();
    let old_approved = if matches!(
        scenario,
        InitialApprovalScenario::FollowUpProviderSuccess
            | InitialApprovalScenario::FollowUpCompensatedFailure
            | InitialApprovalScenario::FollowUpReceiptFailure
            | InitialApprovalScenario::FollowUpProjectionFailure
    ) {
        let old = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "old-task-id".into(),
            task_title: "Old objective".into(),
            title: "Old objective".into(),
            priority: Priority::P2,
            plan: "Old approved plan".into(),
            task_file: "tasks/old-approved.md".into(),
            artifact_body: "Old artifact body".into(),
        };
        std::fs::write(worktree.join(&old.task_file), &old.artifact_body).unwrap();
        git(&worktree, &["add", "--", &old.task_file]);
        git(
            &worktree,
            &[
                "commit",
                "--only",
                "-m",
                "Seed old approved artifact",
                "--",
                &old.task_file,
            ],
        );
        f.db.persist_approved_task_authority("actor-target", &old)
            .await
            .unwrap();
        Some(old)
    } else {
        None
    };
    let before_head = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
    let before_branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
    // HEAD must change for the approval checkpoint; worktree identity must not.
    let worktree_identity = || {
        git(&root, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| !line.starts_with("HEAD "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let before_worktrees = worktree_identity();
    let InitialSource {
        source,
        command,
        source_blocks,
        inspected_owner,
    } = initial_source(&f, task_file, kind).await;
    let before_target = f.db.get_conversation("actor-target").await.unwrap();
    let before_binding = native_authority_binding(&f.db).await;
    let before_provenance_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_conversation_sources")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    let (request_tx, mut requests) = mpsc::unbounded_channel();
    let llm = Arc::new(NativeApprovalLlm {
        calls: std::sync::atomic::AtomicUsize::new(0),
        requests: request_tx,
    });
    let models = match scenario {
        InitialApprovalScenario::ContextPressure => {
            phoenix_llm::ModelRegistry::for_test_with_model_limits(vec![(
                "claude-sonnet-5",
                llm.clone(),
                32,
                1,
            )])
        }
        InitialApprovalScenario::FreezeFailure => phoenix_llm::ModelRegistry::new_empty(),
        InitialApprovalScenario::ProviderSuccess
        | InitialApprovalScenario::DroppedCallerProviderSuccess
        | InitialApprovalScenario::FollowUpProviderSuccess
        | InitialApprovalScenario::FollowUpCompensatedFailure
        | InitialApprovalScenario::InitialReceiptFailure
        | InitialApprovalScenario::FollowUpReceiptFailure
        | InitialApprovalScenario::InitialProjectionFailure
        | InitialApprovalScenario::FollowUpProjectionFailure => {
            phoenix_llm::ModelRegistry::for_test_with_sonnet(llm.clone())
        }
    };
    let manager = Arc::new(RuntimeManager::new(
        f.db.clone(),
        Arc::new(models),
        crate::platform::PlatformCapability::None {
            details: "native initial approval fixture".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    harness.manager = Some(Arc::clone(&manager));
    if matches!(
        scenario,
        InitialApprovalScenario::InitialProjectionFailure
            | InitialApprovalScenario::FollowUpProjectionFailure
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        manager.set_native_projection_gate_for_test("actor-target", entered_tx, release_rx);
        harness.projection_release = Some(release_tx);
        let handle = manager.get_or_create(&f.target).await.unwrap();
        harness.handle = Some(handle.clone());
        let live_before = handle.state_rx.borrow().clone();
        assert!(matches!(
            live_before,
            ConvState::AwaitingTaskApproval { .. }
        ));
        let broadcaster = manager.conversation_broadcaster("actor-target").await;
        let mut events = broadcaster.subscribe();
        let caller_manager = Arc::clone(&manager);
        let caller_command = command.clone();
        harness.caller = Some(tokio::spawn(async move {
            Box::pin(caller_manager.execute_native_task_approval(caller_command)).await
        }));
        entered_rx
            .await
            .expect("classified receipt gate must enter");

        // Real DatabaseStorage settlement has committed, but the actual live
        // install has not run. The gate neither simulates nor replaces its error.
        let NativeTaskApprovalInspection::Replay(receipt) =
            f.db.inspect_native_task_approval_command(&command)
                .await
                .unwrap()
        else {
            panic!("receipt must be durable at the postclassification gate");
        };
        assert_eq!(receipt.source, command.source);
        assert_eq!(receipt.owner, inspected_owner);
        assert_eq!(receipt.target_transcript_id, f.target);
        assert_eq!(receipt.continuation, NativeApprovalContinuation::Requested);
        let committed = native_rollback_snapshot(&f.db, "actor-target").await;
        assert_eq!(&committed.rows[3..], &[1, 1, 1]);
        assert_eq!(committed.state, ConvState::LlmRequesting { attempt: 1 });
        let approved = committed.objective.as_ref().expect("committed objective");
        assert_eq!(approved.plan, BODY);
        assert_eq!(approved.artifact_body, BODY);
        let approved_path = if old_approved.is_some() {
            committed_task_file
        } else {
            task_file
        };
        assert_eq!(approved.task_file, approved_path);
        assert_eq!(committed.authority.len(), 1);
        if old_approved.is_some() {
            assert_eq!(native_authority_binding(&f.db).await, before_binding);
        }
        let owed_message: String = sqlx::query_scalar(
            "SELECT approval_message_id FROM approval_request_obligations WHERE conversation_id = ?1",
        )
        .bind("actor-target")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
        assert_eq!(owed_message, receipt.approval_message_id);
        let approval_message =
            f.db.get_messages("actor-target")
                .await
                .unwrap()
                .into_iter()
                .find(|message| message.message_id == receipt.approval_message_id)
                .expect("committed approval context message");
        let MessageContent::User(content) = &approval_message.content else {
            panic!("approval context is a user meta message");
        };
        assert!(content.text.contains(BODY));
        let committed_head = git(&worktree, &["rev-parse", "HEAD"]);
        assert_ne!(committed_head.trim(), before_head);
        assert_eq!(
            git(&worktree, &["show", &format!("HEAD:{approved_path}")]),
            BODY
        );
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(*handle.state_rx.borrow(), live_before);

        // Close the real authority fence; release into install_live_state's
        // real owner acquisition. It must reject, not publish or dispatch.
        manager.signal_fatal_local_authority("native_projection_fixture");
        harness.release_gate();
        let result = harness.caller.take().unwrap().await.unwrap();
        assert_eq!(
            result,
            NativeTaskApprovalOutcome::Committed(receipt.clone())
        );
        handle.event_tx.closed().await;
        // These observations precede fixture cleanup's shutdown/drain.
        assert!(manager.local_authority_is_closed());
        assert_eq!(*handle.state_rx.borrow(), live_before);
        assert_eq!(
            native_rollback_snapshot(&f.db, "actor-target").await,
            committed
        );
        assert_eq!(
            f.db.inspect_native_task_approval_command(&command)
                .await
                .unwrap(),
            NativeTaskApprovalInspection::Replay(receipt.clone())
        );
        let still_owed: String = sqlx::query_scalar(
            "SELECT approval_message_id FROM approval_request_obligations WHERE conversation_id = ?1",
        )
        .bind("actor-target")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
        assert_eq!(still_owed, owed_message);
        assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), committed_head);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert!(requests.try_recv().is_err());
        loop {
            match events.try_recv() {
                Ok(SseEvent::StateChange {
                    state: ConvState::LlmRequesting { .. },
                    ..
                }) => {
                    panic!("failed live projection must not publish approval state");
                }
                Ok(SseEvent::ConversationUpdate { .. }) => {
                    panic!("failed live projection must not publish approval metadata");
                }
                Ok(SseEvent::Message { message, .. }) => {
                    assert_ne!(message.message_id, receipt.approval_message_id);
                }
                Ok(_) => {}
                Err(
                    tokio::sync::broadcast::error::TryRecvError::Empty
                    | tokio::sync::broadcast::error::TryRecvError::Closed,
                ) => break,
                Err(error) => panic!("must not lose projection evidence: {error}"),
            }
        }
        return;
    }
    let (handle, mut events, mut provider_request, result) = if scenario
        == InitialApprovalScenario::DroppedCallerProviderSuccess
    {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        manager.set_native_pre_git_gate_for_test("actor-target", move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        });
        harness.release = Some(NativeApprovalReleaseOnDrop(Some(release_tx)));
        let handle = manager.get_or_create(&f.target).await.unwrap();
        harness.handle = Some(handle.clone());
        let broadcaster = manager.conversation_broadcaster("actor-target").await;
        let events = broadcaster.subscribe();
        let admission = manager.mutation_admission("actor-target").await;
        let caller_manager = Arc::clone(&manager);
        let caller_command = command.clone();
        harness.caller = Some(tokio::spawn(async move {
            Box::pin(caller_manager.execute_native_task_approval(caller_command)).await
        }));
        let entered = tokio::time::timeout(std::time::Duration::from_secs(10), entered_rx).await;
        // Join cancellation before inspecting the mutex: abort() alone only schedules it.
        let caller = harness.caller.take().unwrap();
        caller.abort();
        let cancelled = matches!(caller.await, Err(error) if error.is_cancelled());
        let retained = admission
            .as_ref()
            .is_ok_and(|admission| admission.try_lock().is_err());
        // Release on every schedule outcome before any assertion or async drain.
        harness.release_gate();
        assert!(
            matches!(entered, Ok(Ok(()))) && cancelled && retained,
            "pre-Git gate must enter, caller must join cancelled, and actor must retain admission"
        );
        let request = tokio::time::timeout(std::time::Duration::from_secs(10), requests.recv())
            .await
            .expect("provider must be entered after gate release")
            .expect("actual provider request");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        let NativeTaskApprovalInspection::Replay(receipt) =
            f.db.inspect_native_task_approval_command(&command)
                .await
                .unwrap()
        else {
            panic!("native receipt is durable before provider entry");
        };
        (
            handle,
            events,
            Some(request),
            NativeTaskApprovalOutcome::Committed(receipt),
        )
    } else {
        let handle = manager.get_or_create(&f.target).await.unwrap();
        harness.handle = Some(handle.clone());
        let broadcaster = manager.conversation_broadcaster("actor-target").await;
        let events = broadcaster.subscribe();
        if matches!(
            scenario,
            InitialApprovalScenario::InitialReceiptFailure
                | InitialApprovalScenario::FollowUpReceiptFailure
        ) {
            let before = native_rollback_snapshot(&f.db, "actor-target").await;
            // This final INSERT follows the real objective, authority, message,
            // obligation, state and source-call writes in one transaction.
            sqlx::query(
                "CREATE TRIGGER reject_runtime_native_receipt BEFORE INSERT ON native_task_approval_receipts
                 BEGIN SELECT RAISE(ABORT, 'injected native receipt failure'); END",
            )
            .execute(f.db.pool())
            .await
            .unwrap();
            assert_eq!(
                manager.execute_native_task_approval(command.clone()).await,
                NativeTaskApprovalOutcome::Unavailable
            );
            handle.event_tx.closed().await;
            // Retirement and authority closure are separate postconditions. Check
            // both before fixture cleanup can close the fence itself.
            assert!(manager.local_authority_is_closed());
            assert_eq!(
                native_rollback_snapshot(&f.db, "actor-target").await,
                before
            );
            let after_target = f.db.get_conversation("actor-target").await.unwrap();
            assert_eq!(
                after_target.product_conversation_id,
                before_target.product_conversation_id
            );
            assert_eq!(
                after_target.attached_work_scope_id,
                before_target.attached_work_scope_id
            );
            assert_eq!(after_target.cwd, before_target.cwd);
            assert_eq!(after_target.conv_mode, before_target.conv_mode);
            assert_ne!(git(&worktree, &["rev-parse", "HEAD"]).trim(), before_head);
            assert_eq!(
                git(
                    &worktree,
                    &["rev-list", "--count", &format!("{before_head}..HEAD")]
                )
                .trim(),
                "1"
            );
            let committed_path = if old_approved.is_some() && kind == ReviewedTaskKind::Taskmd {
                committed_task_file
            } else {
                task_file
            };
            let changed_paths = git(
                &worktree,
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "--no-renames",
                    "-r",
                    "HEAD",
                ],
            );
            let mut changed_paths = changed_paths.lines().collect::<Vec<_>>();
            changed_paths.sort_unstable();
            let mut expected_paths = vec![committed_path];
            if committed_path != task_file {
                expected_paths.push(task_file);
            }
            if old_approved.is_none() {
                // Initial approval prepares the repository's tracked ignore;
                // FollowUp uses local excludes and must not sweep other files.
                expected_paths.push(".gitignore");
                assert_eq!(git(&worktree, &["show", "HEAD:.gitignore"]), ".phoenix/\n");
                assert_eq!(
                    std::fs::read(worktree.join(".gitignore")).unwrap(),
                    b".phoenix/\n"
                );
            }
            expected_paths.sort_unstable();
            assert_eq!(changed_paths, expected_paths);
            assert_eq!(
                git(&worktree, &["show", &format!("HEAD:{committed_path}")]),
                BODY
            );
            assert_eq!(
                std::fs::read(worktree.join(committed_path)).unwrap(),
                BODY.as_bytes()
            );
            if old_approved.is_some() {
                assert_eq!(
                    git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
                    before_branch
                );
                assert_eq!(worktree_identity(), before_worktrees);
                assert_eq!(
                    git(&worktree, &["show", ":unrelated-staged.txt"]),
                    "keep staged"
                );
                assert!(!git(&worktree, &["ls-tree", "-r", "--name-only", "HEAD"])
                    .lines()
                    .any(|path| path == "unrelated-staged.txt"));
                if kind == ReviewedTaskKind::Taskmd {
                    assert!(!worktree.join(task_file).exists());
                }
            }
            assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
            assert!(requests.try_recv().is_err());
            assert_eq!(
                f.db.inspect_native_task_approval_command(&command)
                    .await
                    .unwrap(),
                NativeTaskApprovalInspection::RequiresOwningActorAdmission
            );
            return;
        }
        if scenario == InitialApprovalScenario::FollowUpCompensatedFailure {
            let old = old_approved.as_ref().expect("follow-up old objective");
            let old_authority_rows = native_approval_rows(&f.db).await;
            let old_objective =
                f.db.get_approved_task_objective("actor-target")
                    .await
                    .unwrap()
                    .expect("old approved objective");
            assert_eq!(
                old_objective,
                phoenix_core::task_handoff::ApprovedTaskSnapshot::from(old)
            );
            let old_conversation = f.db.get_conversation("actor-target").await.unwrap();
            let old_scope_id = old_conversation.attached_work_scope_id.clone();
            let old_scope_authority = sqlx::query_as::<_, (String, String, i64)>(
                "SELECT work_scope_id, objective_conversation_id, created_at_us FROM work_scope_approved_task_authorities WHERE objective_conversation_id = ?1",
            )
            .bind("actor-target")
            .fetch_all(f.db.pool())
            .await
            .unwrap();
            assert_eq!(old_scope_authority.len(), 1);
            let old_provenance = sqlx::query_as::<_, (String, String, String, String, i64)>(
                "SELECT target_product_conversation_id, source_product_conversation_id, source_conversation_id, relation_key, created_at_us FROM product_conversation_sources WHERE target_product_conversation_id = ?1",
            )
            .bind(old_conversation.product_conversation_id.as_str())
            .fetch_all(f.db.pool())
            .await
            .unwrap();
            let before_head = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
            let before_branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);
            let git_path = git(&worktree, &["rev-parse", "--git-path", "info/exclude"]);
            let git_path = std::path::PathBuf::from(git_path.trim());
            let exclude = if git_path.is_absolute() {
                git_path
            } else {
                worktree.join(git_path)
            };
            if exclude.exists() {
                std::fs::remove_file(&exclude).unwrap();
            }
            std::fs::create_dir_all(&exclude).unwrap();

            let outcome = manager.execute_native_task_approval(command.clone()).await;
            assert_eq!(outcome, NativeTaskApprovalOutcome::Unavailable);
            handle.event_tx.closed().await;
            assert_eq!(
                f.db.get_approved_task_objective("actor-target")
                    .await
                    .unwrap(),
                Some(old_objective)
            );
            assert_eq!(native_approval_rows(&f.db).await, old_authority_rows);
            let current = f.db.get_conversation("actor-target").await.unwrap();
            assert_eq!(current.conv_mode, old_conversation.conv_mode);
            assert_eq!(current.state, old_conversation.state);
            assert_eq!(current.cwd, old_conversation.cwd);
            assert_eq!(current.attached_work_scope_id, old_scope_id);
            assert_eq!(
                sqlx::query_as::<_, (String, String, i64)>(
                    "SELECT work_scope_id, objective_conversation_id, created_at_us FROM work_scope_approved_task_authorities WHERE objective_conversation_id = ?1",
                )
                .bind("actor-target")
                .fetch_all(f.db.pool())
                .await
                .unwrap(),
                old_scope_authority
            );
            assert_eq!(
                sqlx::query_as::<_, (String, String, String, String, i64)>(
                    "SELECT target_product_conversation_id, source_product_conversation_id, source_conversation_id, relation_key, created_at_us FROM product_conversation_sources WHERE target_product_conversation_id = ?1",
                )
                .bind(current.product_conversation_id.as_str())
                .fetch_all(f.db.pool())
                .await
                .unwrap(),
                old_provenance
            );
            assert_eq!(git(&worktree, &["rev-parse", "HEAD"]).trim(), before_head);
            assert_eq!(
                git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
                before_branch
            );
            assert_eq!(
                std::fs::read(worktree.join(task_file)).unwrap(),
                BODY.as_bytes()
            );
            assert!(!worktree.join(committed_task_file).exists());
            assert_eq!(
                git(&worktree, &["show", ":unrelated-staged.txt"]),
                "keep staged"
            );
            assert!(requests.try_recv().is_err());
            assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
            return;
        }
        let result = manager.execute_native_task_approval(command.clone()).await;
        (handle, events, None, result)
    };
    let NativeTaskApprovalOutcome::Committed(receipt) = result else {
        panic!("Initial approval must commit through the owning production actor: {result:?}");
    };
    assert_eq!(receipt.source, command.source);
    assert_eq!(receipt.owner, inspected_owner);
    assert_eq!(receipt.target_transcript_id, "actor-target");
    assert_eq!(receipt.continuation, NativeApprovalContinuation::Requested);
    assert_eq!(receipt.source_transcript_id, source.id);
    assert_eq!(
        receipt.proposal,
        command
            .pending
            .reviewed_proposal
            .as_ref()
            .unwrap()
            .occurrence
    );
    assert!(receipt.committed_at_us > 0);
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_task_approval_receipts")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(receipts, 1);
    let approved =
        f.db.get_approved_task_objective("actor-target")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(approved.title, "Exact title");
    if let Some(old) = &old_approved {
        assert_ne!(approved.task_id, old.task_id);
        assert_ne!(approved.task_title, old.task_title);
        assert_ne!(approved.title, old.title);
        assert_ne!(approved.priority, old.priority);
        assert_ne!(approved.plan, old.plan);
        assert_ne!(approved.task_file, old.task_file);
        assert_ne!(approved.artifact_body, old.artifact_body);
    }
    let after_target = f.db.get_conversation("actor-target").await.unwrap();
    assert_eq!(after_target.id, before_target.id);
    assert_eq!(
        after_target.product_conversation_id,
        before_target.product_conversation_id
    );
    assert_eq!(
        after_target.attached_work_scope_id,
        before_target.attached_work_scope_id
    );
    assert_eq!(after_target.cwd, before_target.cwd);
    assert_eq!(after_target.conv_mode, before_target.conv_mode);
    if old_approved.is_some() {
        assert_eq!(native_authority_binding(&f.db).await, before_binding);
    }
    let after_provenance_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_conversation_sources")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(after_provenance_count, before_provenance_count);
    assert_eq!(approved.task_title, "Exact title");
    assert_eq!(approved.priority, Priority::P1);
    assert_eq!(
        approved.task_file,
        if old_approved.is_some() && kind == ReviewedTaskKind::Taskmd {
            committed_task_file
        } else {
            task_file
        }
    );
    if kind == ReviewedTaskKind::Taskmd {
        assert_eq!(approved.task_id, "53019");
    }
    assert_eq!(approved.plan, BODY);
    assert_eq!(approved.artifact_body, BODY);
    let messages = f.db.get_messages("actor-target").await.unwrap();
    let approvals: Vec<_> = messages
        .iter()
        .filter(|message| message.message_id == receipt.approval_message_id)
        .collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].message_type, MessageType::User);
    let MessageContent::User(content) = &approvals[0].content else {
        panic!("approval is a persisted user meta message");
    };
    let approval_prefix = if old_approved.is_some() {
        "Follow-up task approved"
    } else {
        "Task approved."
    };
    assert!(content.text.contains(approval_prefix));
    assert!(content.text.contains(BODY));
    let after_head = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(after_head, before_head);
    if old_approved.is_some() {
        assert_eq!(
            git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
            before_branch
        );
        assert_eq!(worktree_identity(), before_worktrees);
    } else {
        // Initial approval deliberately renames the temporary Explore branch.
        assert_ne!(
            git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
            before_branch
        );
    }
    assert_eq!(
        git(
            &worktree,
            &["rev-list", "--count", &format!("{before_head}..HEAD")]
        )
        .trim(),
        "1"
    );
    assert_eq!(
        git(&worktree, &["show", &format!("HEAD:{committed_task_file}")]),
        BODY
    );
    assert_eq!(
        std::fs::read(worktree.join(committed_task_file)).unwrap(),
        BODY.as_bytes()
    );
    if old_approved.is_some() {
        assert!(!git(&worktree, &["ls-tree", "-r", "--name-only", "HEAD"])
            .lines()
            .any(|path| path == "unrelated-staged.txt"));
        assert_eq!(
            git(&worktree, &["show", ":unrelated-staged.txt"]),
            "keep staged"
        );
    }
    if kind == ReviewedTaskKind::Taskmd {
        assert!(!worktree.join(task_file).exists());
        assert_eq!(
            git(&worktree, &["show", "-s", "--format=%s", "HEAD"]).trim(),
            "task 53019: Exact title"
        );
        assert_eq!(
            git(
                &worktree,
                &[
                    "diff",
                    "--name-status",
                    "--find-renames",
                    &before_head,
                    "HEAD",
                    "--",
                    "tasks/"
                ]
            )
            .trim(),
            format!("R100\t{task_file}\t{committed_task_file}")
        );
    }
    assert!(
        matches!(f.db.inspect_native_task_approval_command(&command).await.unwrap(),
        NativeTaskApprovalInspection::Replay(ref stored) if stored == &receipt)
    );
    if matches!(
        scenario,
        InitialApprovalScenario::ProviderSuccess
            | InitialApprovalScenario::FollowUpProviderSuccess
            | InitialApprovalScenario::DroppedCallerProviderSuccess
    ) {
        let request = match provider_request.take() {
            Some(request) => request,
            None => requests
                .recv()
                .await
                .expect("actual LlmService::complete entry"),
        };
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        let submitted_approval = request
            .messages
            .iter()
            .find(|message| {
                message.source_message_id.as_deref() == Some(receipt.approval_message_id.as_str())
            })
            .expect("the actual provider request includes the committed approval message");
        assert_eq!(submitted_approval.role, phoenix_llm::MessageRole::User);
        assert!(submitted_approval.content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text }
                if text.contains(approval_prefix) && text.contains(BODY))
        }));

        let mut saw_approval_state = false;
        let mut reply_message_id = None;
        loop {
            match events.recv().await.expect("terminal production SSE signal") {
                SseEvent::StateChange {
                    state: ConvState::LlmRequesting { attempt: 1 },
                    ..
                } => saw_approval_state = true,
                SseEvent::Message { message, .. }
                    if matches!(&message.content, MessageContent::Agent(blocks)
                        if blocks.iter().any(|block| matches!(block,
                            ContentBlock::Text { text } if text == NATIVE_APPROVAL_REPLY))) =>
                {
                    assert!(
                        saw_approval_state,
                        "approval state precedes the provider reply"
                    );
                    assert_eq!(message.message_type, MessageType::Agent);
                    assert!(reply_message_id.replace(message.message_id).is_none());
                }
                SseEvent::AgentDone { .. } => break,
                SseEvent::Init { .. }
                | SseEvent::Message { .. }
                | SseEvent::MessageUpdated { .. }
                | SseEvent::StateChange { .. }
                | SseEvent::Token { .. }
                | SseEvent::LlmFirstByte { .. }
                | SseEvent::LlmAttempt { .. }
                | SseEvent::ConversationBecameTerminal { .. }
                | SseEvent::ConversationUpdate { .. }
                | SseEvent::Error { .. }
                | SseEvent::ConversationHardDeleted { .. }
                | SseEvent::BrowserSessionState { .. }
                | SseEvent::BashToolProgress { .. }
                | SseEvent::SteerMessageQueued { .. }
                | SseEvent::SteerMessageCancelled { .. }
                | SseEvent::RateLimitSnapshot { .. }
                | SseEvent::WorkScopeUpdate { .. } => {}
            }
        }
        assert!(
            saw_approval_state,
            "Initial approval publishes its committed state"
        );
        let reply_message_id = reply_message_id.expect("provider reply precedes AgentDone");
        assert_eq!(
            f.db.get_conversation("actor-target").await.unwrap().state,
            ConvState::Idle,
            "AgentDone follows durable terminal state settlement"
        );
        let settled_messages = f.db.get_messages("actor-target").await.unwrap();
        let replies: Vec<_> = settled_messages
            .iter()
            .filter(|message| {
                matches!(&message.content, MessageContent::Agent(blocks)
                if blocks.iter().any(|block| matches!(block,
                    ContentBlock::Text { text } if text == NATIVE_APPROVAL_REPLY)))
            })
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].message_id, reply_message_id);
        assert!(matches!(&replies[0].content, MessageContent::Agent(blocks)
            if blocks.len() == 1 && matches!(&blocks[0],
                ContentBlock::Text { text } if text == NATIVE_APPROVAL_REPLY)));
        let pending_approvals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM approval_request_obligations WHERE conversation_id = ?1",
        )
        .bind("actor-target")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
        assert_eq!(
            pending_approvals, 0,
            "terminal response settles its obligation"
        );
        let mut live_state = handle.state_rx.clone();
        live_state
            .wait_for(|state| *state == ConvState::Idle)
            .await
            .unwrap();
    } else {
        let context_pressure = scenario == InitialApprovalScenario::ContextPressure;
        let mut saw_approval_state = false;
        let mut saw_immediate_settlement = false;
        let mut saw_context_terminal = false;
        while let Ok(event) = events.try_recv() {
            if let SseEvent::StateChange { state, .. } = event {
                if matches!(state, ConvState::RecoverableContinuationFailure { .. }) {
                    assert!(
                        saw_immediate_settlement,
                        "terminal failure must follow context pressure"
                    );
                    saw_context_terminal = true;
                }
                saw_approval_state |= matches!(state, ConvState::LlmRequesting { attempt: 1 });
                saw_immediate_settlement |= if context_pressure {
                    matches!(state, ConvState::AwaitingContinuation { .. })
                } else {
                    matches!(state, ConvState::LlmRequesting { attempt: 2 })
                };
            }
        }
        assert!(
            saw_approval_state,
            "Initial approval must publish its committed state"
        );
        assert!(
            saw_immediate_settlement,
            "RequestLlm's immediate event must not be dropped"
        );
        assert!(!matches!(
            f.db.get_conversation("actor-target").await.unwrap().state,
            ConvState::LlmRequesting { attempt: 1 }
        ));

        if context_pressure {
            // This fixed capacity failure settles inline before approval returns.
            // UserCancel is not valid in the resulting recoverable rest state.
            assert!(saw_context_terminal);
            assert!(matches!(
                &*handle.state_rx.borrow(),
                ConvState::RecoverableContinuationFailure { failure }
                    if failure.error_kind == crate::db::ErrorKind::ContextCapacityConflict
            ));
            assert!(matches!(
                f.db.get_conversation("actor-target").await.unwrap().state,
                ConvState::RecoverableContinuationFailure { failure }
                    if failure.error_kind == crate::db::ErrorKind::ContextCapacityConflict
            ));
            assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        } else {
            // Keep the acknowledged real Stop probe in the cancellable branch.
            assert_eq!(
                event_settled(
                    &handle.acknowledged_event_tx,
                    Event::UserCancel {
                        reason: None,
                        cause: CancelCause::UserRequested,
                    }
                )
                .await,
                AcknowledgedEventOutcome::Settled
            );
        }
    }
    let expected_calls = match scenario {
        InitialApprovalScenario::ProviderSuccess
        | InitialApprovalScenario::DroppedCallerProviderSuccess
        | InitialApprovalScenario::FollowUpProviderSuccess => 1,
        InitialApprovalScenario::ContextPressure
        | InitialApprovalScenario::FreezeFailure
        | InitialApprovalScenario::FollowUpCompensatedFailure
        | InitialApprovalScenario::InitialReceiptFailure
        | InitialApprovalScenario::FollowUpReceiptFailure => 0,
    };
    assert_eq!(llm.calls.load(Ordering::SeqCst), expected_calls);
    assert!(requests.try_recv().is_err(), "no additional provider entry");
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
    let source_message = Message {
        message_id: command.source.message_id.clone(),
        conversation_id: source.id.clone(),
        sequence_id: 1,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(source_blocks),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    };
    let source_result = FinalizedToolResult::new(
        &source_message.message_id,
        &source.id,
        2,
        source_message.created_at,
        &ToolResult::success(command.source.tool_use_id.clone(), "committed".into()),
    );
    f.db.persist_tool_round_and_state(
        &source.id,
        &source_message,
        &[source_result],
        &ConvState::Idle,
        source_message.created_at,
    )
    .await
    .unwrap();
    let before_replay = native_approval_rows(&f.db).await;
    let calls = llm.calls.load(Ordering::SeqCst);
    let before_replay_state = f.db.get_conversation("actor-target").await.unwrap().state;
    let before_replay_source_state = f.db.get_conversation(&source.id).await.unwrap().state;
    assert_eq!(before_replay_source_state, ConvState::Idle);
    assert!(f
        .db
        .message_exists(&source_message.message_id)
        .await
        .unwrap());
    let before_replay_git_status = git(&worktree, &["status", "--porcelain=v1"]);
    assert_eq!(
        manager.execute_native_task_approval(command).await,
        NativeTaskApprovalOutcome::Committed(receipt)
    );
    assert_eq!(native_approval_rows(&f.db).await, before_replay);
    assert_eq!(
        f.db.get_approved_task_objective("actor-target")
            .await
            .unwrap(),
        Some(approved)
    );
    assert_eq!(
        f.db.get_conversation("actor-target").await.unwrap().state,
        before_replay_state
    );
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]).trim(), after_head);
    assert_eq!(llm.calls.load(Ordering::SeqCst), calls);
    assert_eq!(calls, expected_calls);
    assert!(
        requests.try_recv().is_err(),
        "historical replay must not redispatch"
    );
    assert_eq!(
        f.db.get_conversation(&source.id).await.unwrap().state,
        before_replay_source_state
    );
    assert_eq!(
        git(&worktree, &["status", "--porcelain=v1"]),
        before_replay_git_status
    );
    if old_approved.is_some() {
        assert!(git(&worktree, &["diff", "--cached", "--name-only"])
            .lines()
            .any(|path| path == "unrelated-staged.txt"));
        assert_eq!(
            std::fs::read(worktree.join("unrelated-staged.txt")).unwrap(),
            b"keep staged"
        );
    }
    assert_eq!(
        git(&worktree, &["show", &format!("HEAD:{committed_task_file}")]),
        BODY
    );
    assert_eq!(
        std::fs::read(worktree.join(committed_task_file)).unwrap(),
        BODY.as_bytes()
    );
}

#[derive(Debug, PartialEq)]
struct NativeRollbackSnapshot {
    rows: Vec<i64>,
    objective: Option<phoenix_core::task_handoff::ApprovedTaskSnapshot>,
    authority: Vec<(String, String, i64)>,
    scope_kinds: Vec<(String, Option<String>)>,
    provenance: Vec<(String, String, String, String, i64)>,
    state: ConvState,
}

async fn native_rollback_snapshot(db: &Database, target: &str) -> NativeRollbackSnapshot {
    NativeRollbackSnapshot {
        rows: native_approval_rows(db).await,
        objective: db.get_approved_task_objective(target).await.unwrap(),
        authority: sqlx::query_as(
            "SELECT work_scope_id, objective_conversation_id, created_at_us FROM work_scope_approved_task_authorities ORDER BY work_scope_id",
        ).fetch_all(db.pool()).await.unwrap(),
        scope_kinds: sqlx::query_as("SELECT id, authority_kind FROM work_scopes ORDER BY id")
            .fetch_all(db.pool()).await.unwrap(),
        provenance: sqlx::query_as(
            "SELECT target_product_conversation_id, source_product_conversation_id, source_conversation_id, relation_key, created_at_us FROM product_conversation_sources ORDER BY target_product_conversation_id, source_product_conversation_id, source_conversation_id, relation_key",
        ).fetch_all(db.pool()).await.unwrap(),
        state: db.get_conversation(target).await.unwrap().state,
    }
}

struct InitialSource {
    source: crate::db::Conversation,
    command: phoenix_db::native_task_approval::NativeTaskApprovalCommand,
    source_blocks: Vec<ContentBlock>,
    inspected_owner: phoenix_core::domain::product_conversation::ProductConversationId,
}

async fn initial_source(
    f: &AdmissionFixture,
    task_file: &str,
    kind: ReviewedTaskKind,
) -> InitialSource {
    use phoenix_db::native_task_approval::{
        NativeApprovalIntent, NativeApprovalPlacement, NativeTaskApprovalCommand,
    };
    let source =
        f.db.get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
    let inspection =
        f.db.inspect_pending_task_from_coordinator(
            &source.id,
            &SourceToolCall {
                message_id: "executing-inspector".into(),
                tool_use_id: "inspection-call".into(),
            },
            &InspectPendingTaskInput {
                target: f
                    .db
                    .get_conversation(&f.target)
                    .await
                    .unwrap()
                    .product_conversation_id,
            },
        )
        .await
        .unwrap();
    let phoenix_db::task_approval_inspection::PendingTaskStatus::Pending(pending) =
        inspection.pending
    else {
        panic!("canonical pending proposal");
    };
    assert_eq!(pending.task_file, task_file);
    assert_eq!(pending.title, "Exact title");
    assert_eq!(pending.priority, Priority::P1);
    assert_eq!(pending.plan, BODY);
    assert_eq!(pending.reviewed_proposal.as_ref().unwrap().kind, kind);
    let command = NativeTaskApprovalCommand {
        source_transcript_id: source.id.clone(),
        source: SourceToolCall {
            message_id: "initial-source-message".into(),
            tool_use_id: "initial-source-call".into(),
        },
        owner: inspection.owner.clone(),
        current_transcript_id: f.target.clone(),
        pending,
        placement: NativeApprovalPlacement::ContinueHere,
        intent: NativeApprovalIntent::ApproveTaskAndRequestContinuation,
    };
    let source_blocks = vec![ContentBlock::ToolUse {
        id: command.source.tool_use_id.clone(),
        name: "approve_task".into(),
        input: command.input_value().unwrap(),
    }];
    let source_state = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &command.source.tool_use_id,
            ToolInput::Unknown {
                name: "approve_task".into(),
                input: command.input_value().unwrap(),
            },
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: crate::state_machine::AssistantMessage::new(
            command.source.message_id.clone(),
            source_blocks.clone(),
            None,
            None,
        ),
    };
    f.db.update_conversation_state(&source.id, &source_state)
        .await
        .unwrap();
    assert!(!f
        .db
        .message_exists(&command.source.message_id)
        .await
        .unwrap());
    InitialSource {
        source,
        command,
        source_blocks,
        inspected_owner: inspection.owner,
    }
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let output = phoenix_core::git::command()
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

async fn native_authority_binding(db: &Database) -> Vec<(String, String)> {
    sqlx::query_as("SELECT work_scope_id, objective_conversation_id FROM work_scope_approved_task_authorities ORDER BY work_scope_id")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

async fn native_approval_rows(db: &Database) -> Vec<i64> {
    let mut counts = Vec::new();
    for query in [
        "SELECT COUNT(*) FROM messages",
        "SELECT COUNT(*) FROM conversation_approved_task_objectives",
        "SELECT COUNT(*) FROM work_scope_approved_task_authorities",
        "SELECT COUNT(*) FROM native_task_approval_receipts",
        "SELECT COUNT(*) FROM native_task_approval_source_calls",
        "SELECT COUNT(*) FROM approval_request_obligations",
    ] {
        counts.push(
            sqlx::query_scalar(query)
                .fetch_one(db.pool())
                .await
                .unwrap(),
        );
    }
    counts
}
