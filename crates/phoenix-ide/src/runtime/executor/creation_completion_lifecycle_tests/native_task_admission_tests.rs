use super::*;
use crate::runtime::task_approval_admission::{FrozenPendingTask, NativeTaskAdmissionReadiness};
use crate::state_machine::event::CancelCause;
use crate::state_machine::state::TaskApprovalOutcome;
use chrono::Utc;
use phoenix_core::domain::db_schema::{
    FinalizedToolResult, InputOrigin, Message, MessageType, SourceToolCall,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::task_approval::{ProposalOccurrence, ReviewedProposal};
use phoenix_core::task_source::{Priority, TaskSource};
use phoenix_db::task_approval_inspection::{
    InspectPendingTaskInput, INSPECT_PENDING_TASK_TOOL_NAME,
};
use phoenix_db::Database;
use serde_json::json;

const BODY: &str = "# Exact reviewed task\r\n\nKeep these bytes.\n";

struct AdmissionFixture {
    db: Database,
    target: String,
    expected: FrozenPendingTask,
}

impl AdmissionFixture {
    async fn new() -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let target = db
            .create_conversation(
                "actor-target",
                "actor-target",
                "/nonexistent",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let pending = pending_state(true);
        let assistant = proposal_message(&target.id);
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
fn proposal_message(target: &str) -> Message {
    Message {
        message_id: "proposal-assistant".into(),
        conversation_id: target.into(),
        sequence_id: 1,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: "proposal-call".into(),
            name: "propose_task".into(),
            input: json!({"task_file":"tasks/reviewed.md"}),
        }]),
        display_data: None,
        usage_data: None,
        origin: InputOrigin::SystemGenerated,
        created_at: Utc::now(),
    }
}
fn pending_state(captured: bool) -> ConvState {
    ConvState::AwaitingTaskApproval {
        task_file: "tasks/reviewed.md".into(),
        title: "Exact title".into(),
        priority: Priority::P1,
        plan: BODY.into(),
        reviewed_proposal: captured.then(|| {
            ReviewedProposal::capture(
                ProposalOccurrence::capture("proposal-assistant", "proposal-call").unwrap(),
                &TaskSource::detect("tasks/reviewed.md").unwrap(),
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
    let f = AdmissionFixture::new().await;
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
    handle.event_tx.send(Event::Shutdown).await.unwrap();
}

#[derive(Default)]
struct NativeApprovalLlm {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for NativeApprovalLlm {
    async fn complete(
        &self,
        _request: &phoenix_llm::LlmRequest,
    ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(phoenix_llm::LlmError::invalid_request(
            "native approval fixture does not contact a provider",
        ))
    }

    fn model_id(&self) -> &'static str {
        "claude-sonnet-5"
    }
}

#[tokio::test]
async fn production_initial_approval_settles_immediate_context_pressure_and_replays() {
    assert_production_initial_approval(true).await;
}

#[tokio::test]
async fn production_initial_approval_settles_immediate_freeze_failure_and_replays() {
    assert_production_initial_approval(false).await;
}

#[allow(clippy::too_many_lines)]
async fn assert_production_initial_approval(context_pressure: bool) {
    use super::super::test_git_helpers::{add_explore_worktree, init_repo};
    use crate::runtime::task_approval_admission::NativeTaskApprovalOutcome;
    use crate::runtime::{RuntimeManager, SseEvent};
    use phoenix_db::conversation_goals::control::GoalControlInstallation;
    use phoenix_db::native_task_approval::{
        NativeApprovalContinuation, NativeApprovalIntent, NativeApprovalPlacement,
        NativeTaskApprovalCommand, NativeTaskApprovalInspection,
    };

    let (_temp, root) = init_repo();
    let worktree = add_explore_worktree(&root, "actor-target", "main");
    std::fs::create_dir_all(worktree.join("tasks")).unwrap();
    std::fs::write(worktree.join("tasks/reviewed.md"), BODY).unwrap();
    let before_head = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
    let f = AdmissionFixture::new().await;
    assert_eq!(
        f.db.goal_control_installation(),
        GoalControlInstallation::Disabled
    );
    f.db.update_conversation_mode_and_cwd(
        "actor-target",
        &ConvMode::Explore {
            worktree_path: Some(worktree.to_string_lossy().into_owned()),
        },
        worktree.to_str().unwrap(),
    )
    .await
    .unwrap();
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
    let llm = Arc::new(NativeApprovalLlm::default());
    let models = if context_pressure {
        phoenix_llm::ModelRegistry::for_test_with_model_limits(vec![(
            "claude-sonnet-5",
            llm.clone(),
            32,
            1,
        )])
    } else {
        phoenix_llm::ModelRegistry::new_empty()
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
    let handle = manager.get_or_create(&f.target).await.unwrap();
    let broadcaster = manager.conversation_broadcaster("actor-target").await;
    let mut events = broadcaster.subscribe();
    let result = manager.execute_native_task_approval(command.clone()).await;
    let NativeTaskApprovalOutcome::Committed(receipt) = result else {
        panic!("Initial approval must commit through the owning production actor: {result:?}");
    };
    assert_eq!(receipt.source, command.source);
    assert_eq!(receipt.owner, inspection.owner);
    assert_eq!(receipt.target_transcript_id, "actor-target");
    assert_eq!(receipt.continuation, NativeApprovalContinuation::Requested);
    let approved =
        f.db.get_approved_task_objective("actor-target")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(approved.title, "Exact title");
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
    assert!(content.text.contains("Task approved."));
    assert!(content.text.contains(BODY));
    let after_head = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(after_head, before_head);
    assert_eq!(
        git(
            &worktree,
            &["rev-list", "--count", &format!("{before_head}..HEAD")]
        )
        .trim(),
        "1"
    );
    assert_eq!(git(&worktree, &["show", "HEAD:tasks/reviewed.md"]), BODY);
    assert!(
        matches!(f.db.inspect_native_task_approval_command(&command).await.unwrap(),
        NativeTaskApprovalInspection::Replay(ref stored) if stored == &receipt)
    );
    let mut saw_approval_state = false;
    let mut saw_immediate_settlement = false;
    while let Ok(event) = events.try_recv() {
        if let SseEvent::StateChange { state, .. } = event {
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
    assert_eq!(
        manager.execute_native_task_approval(command).await,
        NativeTaskApprovalOutcome::Committed(receipt)
    );
    assert_eq!(native_approval_rows(&f.db).await, before_replay);
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]).trim(), after_head);
    assert_eq!(llm.calls.load(Ordering::SeqCst), calls);
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

async fn native_approval_rows(db: &Database) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "messages",
        "conversation_approved_task_objectives",
        "work_scope_approved_task_authorities",
        "native_task_approval_receipts",
        "approval_request_obligations",
    ] {
        counts.push(
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap(),
        );
    }
    counts
}
