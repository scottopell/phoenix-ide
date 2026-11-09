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
