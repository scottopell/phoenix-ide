use super::*;
use phoenix_core::domain::db_schema::{
    FinalizedToolResult, InputOrigin, MessageContent, MessageType, ToolResult, UserContent,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::provider_replay::{
    AnthropicResponseIdentity, AnthropicResponseSet, ProviderReplayUpdate,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ToolCall, ToolInput};
use phoenix_core::domain::task_approval::{ProposalOccurrence, ReviewedTaskKind};
use phoenix_core::task_handoff::TaskApprovalHandoffData;
use phoenix_core::task_source::TaskSource;
use serde_json::{json, Value};

use crate::task_approval_inspection::{
    InspectPendingTaskInput, PendingTaskStatus, INSPECT_PENDING_TASK_TOOL_NAME,
};
use crate::workflow::LocalAuthorityResult;

const BODY: &str = "# Exact reviewed task\r\n\nKeep these bytes.\n";

struct Fixture {
    db: Database,
    command: NativeTaskApprovalCommand,
    approval: TaskApprovalHandoffData,
    message: Message,
    state: ConvState,
    executing: ConvState,
}

impl Fixture {
    async fn new(with_replay: bool) -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let target = db
            .create_conversation(
                "receipt-target",
                "receipt-target",
                "/nonexistent",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        checkpoint_proposal(&db, &target.id, with_replay).await;
        let source = SourceToolCall {
            message_id: "live-approval-assistant".into(),
            tool_use_id: "live-approval-call".into(),
        };
        let input = InspectPendingTaskInput {
            target: target.product_conversation_id.clone(),
        };
        db.update_conversation_state(
            &coordinator.id,
            &executing_source(
                &source,
                INSPECT_PENDING_TASK_TOOL_NAME,
                serde_json::to_value(&input).unwrap(),
            ),
        )
        .await
        .unwrap();
        let inspection = db
            .inspect_pending_task_from_coordinator(&coordinator.id, &source, &input)
            .await
            .unwrap();
        let PendingTaskStatus::Pending(pending) = inspection.pending else {
            panic!("canonical pending proposal");
        };
        let command = NativeTaskApprovalCommand {
            source_transcript_id: coordinator.id,
            source,
            owner: inspection.owner,
            current_transcript_id: inspection.current_transcript_id,
            pending,
            placement: NativeApprovalPlacement::ContinueHere,
            intent: NativeApprovalIntent::ApproveTaskAndRequestContinuation,
        };
        let executing = executing_source(
            &command.source,
            APPROVE_TASK_TOOL_NAME,
            command.input_value().unwrap(),
        );
        db.update_conversation_state(&command.source_transcript_id, &executing)
            .await
            .unwrap();
        let approval = TaskApprovalHandoffData {
            task_id: "reviewed".into(),
            task_title: "reviewed".into(),
            title: command.pending.title.clone(),
            priority: command.pending.priority,
            plan: command.pending.plan.clone(),
            task_file: command.pending.task_file.clone(),
            artifact_body: command.pending.plan.clone(),
        };
        let message = Message {
            message_id: "approval-context".into(),
            conversation_id: target.id,
            sequence_id: 3,
            message_type: MessageType::User,
            content: MessageContent::User(UserContent::meta(
                ApprovedTaskSnapshot::from(&approval).seed_message(),
            )),
            display_data: None,
            usage_data: None,
            origin: InputOrigin::SystemGenerated,
            created_at: Utc::now(),
        };
        let fixture = Self {
            db,
            command,
            approval,
            message,
            state: ConvState::LlmRequesting { attempt: 1 },
            executing,
        };
        fixture.assert_source_not_finalized().await;
        fixture
    }

    async fn settle(&self) -> DbResult<LocalAuthorityResult<()>> {
        self.db
            .persist_approved_task_authority_inner(
                &self.command.current_transcript_id,
                &self.approval,
                Some(ApprovalSettlement::Native(
                    &self.message,
                    &self.state,
                    self.message.created_at,
                    &self.command,
                )),
            )
            .await
            .map(|result| ApprovalFact::user_result(&result))
    }

    async fn install_source(&self) {
        let state = executing_source(
            &self.command.source,
            APPROVE_TASK_TOOL_NAME,
            self.command.input_value().unwrap(),
        );
        self.db
            .update_conversation_state(&self.command.source_transcript_id, &state)
            .await
            .unwrap();
    }

    fn source_message(&self) -> Message {
        let ConvState::ToolExecuting {
            assistant_message, ..
        } = &self.executing
        else {
            unreachable!()
        };
        Message {
            message_id: self.command.source.message_id.clone(),
            conversation_id: self.command.source_transcript_id.clone(),
            sequence_id: 1,
            message_type: MessageType::Agent,
            content: MessageContent::Agent(assistant_message.content.clone()),
            display_data: None,
            usage_data: None,
            origin: InputOrigin::SystemGenerated,
            created_at: Utc::now(),
        }
    }

    async fn checkpoint_source(&self) {
        let assistant = self.source_message();
        let result = FinalizedToolResult::new(
            &assistant.message_id,
            &assistant.conversation_id,
            2,
            assistant.created_at,
            &ToolResult::success(self.command.source.tool_use_id.clone(), "committed".into()),
        );
        self.db
            .persist_tool_round_and_state(
                &assistant.conversation_id,
                &assistant,
                &[result],
                &ConvState::Idle,
                assistant.created_at,
            )
            .await
            .unwrap();
    }

    async fn inspect(&self) -> Result<NativeTaskApprovalInspection, NativeTaskApprovalError> {
        self.db
            .inspect_native_task_approval_command(&self.command)
            .await
    }

    async fn replay(&self) -> NativeTaskApprovalReceipt {
        let NativeTaskApprovalInspection::Replay(receipt) = self.inspect().await.unwrap() else {
            panic!("expected historical receipt")
        };
        receipt
    }

    async fn assert_source_not_finalized(&self) {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id=?1")
            .bind(&self.command.source.message_id)
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}

fn executing_source(source: &SourceToolCall, name: &str, input: Value) -> ConvState {
    ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &source.tool_use_id,
            ToolInput::Unknown {
                name: name.into(),
                input: input.clone(),
            },
        ),
        remaining_tools: vec![],
        completed_results: vec![],
        pending_sub_agents: vec![],
        assistant_message: AssistantMessage::new(
            source.message_id.clone(),
            vec![ContentBlock::ToolUse {
                id: source.tool_use_id.clone(),
                name: name.into(),
                input,
            }],
            None,
            None,
        ),
    }
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

async fn checkpoint_proposal(db: &Database, target: &str, with_replay: bool) {
    let assistant = proposal_message(target);
    let result = FinalizedToolResult::new(
        &assistant.message_id,
        target,
        2,
        assistant.created_at,
        &ToolResult::success("proposal-call".into(), "awaiting approval".into()),
    );
    let state = pending_state(true);
    if with_replay {
        let MessageContent::Agent(blocks) = &assistant.content else {
            unreachable!()
        };
        let replay = ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: "proposal-response".into(),
                    model: "checkpoint-model".into(),
                },
                blocks.clone(),
                vec![],
            )
            .unwrap()
            .with_owner_message_id(assistant.message_id.clone()),
        );
        db.persist_tool_round_state_and_replay(
            target,
            &assistant,
            &[result],
            &state,
            assistant.created_at,
            &replay,
        )
        .await
        .unwrap();
    } else {
        db.persist_tool_round_and_state(
            target,
            &assistant,
            &[result],
            &state,
            assistant.created_at,
        )
        .await
        .unwrap();
    }
    let witnesses: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM canonical_tool_round_receipts r JOIN messages m
         ON m.conversation_id=r.conversation_id AND m.message_id=r.assistant_message_id
         WHERE r.conversation_id=?1 AND r.assistant_message_id='proposal-assistant'",
    )
    .bind(target)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(witnesses, 1);
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

// Quote every cell, including BLOBs/NULLs, so unchanged state covers all persisted tables.
async fn database_rows(db: &Database) -> Vec<(String, Vec<String>)> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(db.pool())
            .await
            .unwrap();
    let mut snapshot = vec![];
    for table in tables {
        let quoted_table = format!("\"{}\"", table.replace('"', "\"\""));
        // Identifiers come from this test database's schema and are double-quote escaped.
        let columns = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA table_info({quoted_table})"
        )))
        .fetch_all(db.pool())
        .await
        .unwrap();
        let projection = columns
            .iter()
            .map(|row| {
                let column: String = row.get("name");
                format!("quote(\"{}\")", column.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(" || ',' || ");
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {projection} AS value FROM {quoted_table} ORDER BY value"
        )))
        .fetch_all(db.pool())
        .await
        .unwrap();
        snapshot.push((table, rows));
    }
    snapshot
}

#[tokio::test]
async fn native_settlement_atomically_establishes_receipt_and_existing_authority() {
    for replay in [false, true] {
        let f = Fixture::new(replay).await;
        assert!(matches!(
            f.settle().await.unwrap(),
            LocalAuthorityResult::DurableFactEstablished(())
        ));
        let objective =
            f.db.get_approved_task_objective(&f.command.current_transcript_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(objective, ApprovedTaskSnapshot::from(&f.approval));
        assert!(
            receipt_established(&f.db, &f.command, &objective, &f.message)
                .await
                .unwrap()
        );
        let receipt: (String, String, String, i64) = sqlx::query_as(
            "SELECT target_product_conversation_id, target_transcript_id, approval_message_id, continuation_requested FROM native_task_approval_receipts",
        ).fetch_one(f.db.pool()).await.unwrap();
        assert_eq!(
            receipt,
            (
                f.command.owner.as_str().into(),
                f.command.current_transcript_id.clone(),
                f.message.message_id.clone(),
                1
            )
        );
        let obligation: String = sqlx::query_scalar(
            "SELECT approval_message_id FROM approval_request_obligations WHERE conversation_id=?1",
        )
        .bind(&f.command.current_transcript_id)
        .fetch_one(f.db.pool())
        .await
        .unwrap();
        assert_eq!(obligation, f.message.message_id);
        assert_eq!(
            f.db.get_conversation(&f.command.current_transcript_id)
                .await
                .unwrap()
                .state,
            f.state
        );
        f.assert_source_not_finalized().await;
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(f.db.pool())
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn live_preflight_is_read_only_transient_and_has_no_finalized_source_row() {
    let f = Fixture::new(false).await;
    let before = database_rows(&f.db).await;
    assert_eq!(
        f.inspect().await.unwrap(),
        NativeTaskApprovalInspection::RequiresOwningActorAdmission
    );
    f.assert_source_not_finalized().await;
    assert_eq!(database_rows(&f.db).await, before);
    f.db.update_conversation_state(&f.command.current_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert!(f.inspect().await.is_err());
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn canonical_source_replay_preserves_exact_receipt_after_progress_and_stop_like_invalidation()
{
    let f = Fixture::new(false).await;
    f.settle().await.unwrap();
    let receipt = f.replay().await;
    assert_eq!(receipt.source_transcript_id, f.command.source_transcript_id);
    assert_eq!(receipt.source, f.command.source);
    assert_eq!(receipt.owner, f.command.owner);
    assert_eq!(
        receipt.target_transcript_id,
        f.command.current_transcript_id
    );
    assert_eq!(
        receipt.proposal,
        f.command
            .pending
            .reviewed_proposal
            .as_ref()
            .unwrap()
            .occurrence
    );
    assert_eq!(receipt.approval_message_id, f.message.message_id);
    assert_eq!(receipt.continuation, NativeApprovalContinuation::Requested);
    assert!(receipt.committed_at_us > 0);
    f.checkpoint_source().await;
    let before = database_rows(&f.db).await;
    assert_eq!(f.replay().await, receipt);
    f.settle().await.unwrap();
    assert_eq!(database_rows(&f.db).await, before);

    // Uses the ordinary state-progress invalidation trigger, not a runtime Stop-latch proof.
    f.db.update_conversation_state(&f.command.current_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_request_obligations")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(owed, 0);
    let before = database_rows(&f.db).await;
    assert_eq!(f.replay().await, receipt);
    f.settle().await.unwrap();
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn finalized_canonical_source_without_same_exact_native_receipt_cannot_start_approval() {
    for other_receipt in [false, true] {
        let mut f = Fixture::new(false).await;
        if other_receipt {
            f.settle().await.unwrap();
            f.command.source.tool_use_id.push_str("-new");
            f.executing = executing_source(
                &f.command.source,
                APPROVE_TASK_TOOL_NAME,
                f.command.input_value().unwrap(),
            );
            f.db.update_conversation_state(&f.command.current_transcript_id, &pending_state(true))
                .await
                .unwrap();
        }
        f.checkpoint_source().await;
        let before = database_rows(&f.db).await;
        assert!(f.inspect().await.is_err());
        assert!(f.settle().await.is_err());
        assert_eq!(database_rows(&f.db).await, before);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn canonical_source_proof_refuses_forged_missing_foreign_and_ambiguous_evidence() {
    for broken in 0..10 {
        let mut f = Fixture::new(false).await;
        f.settle().await.unwrap();
        f.checkpoint_source().await;
        let mut message = f.source_message();
        match broken {
            0 => {
                sqlx::query("DELETE FROM canonical_tool_round_receipts WHERE conversation_id=?1")
                    .bind(&f.command.source_transcript_id)
                    .execute(f.db.pool())
                    .await
                    .unwrap();
            }
            1 => {
                sqlx::query(
                    "UPDATE messages SET conversation_id=?1, sequence_id=99 WHERE message_id=?2",
                )
                .bind(&f.command.current_transcript_id)
                .bind(&f.command.source.message_id)
                .execute(f.db.pool())
                .await
                .unwrap();
            }
            2 => {
                sqlx::query("DELETE FROM messages WHERE message_id=?1")
                    .bind(&f.command.source.message_id)
                    .execute(f.db.pool())
                    .await
                    .unwrap();
            }
            3..=7 => {
                let MessageContent::Agent(blocks) = &mut message.content else {
                    unreachable!()
                };
                match broken {
                    3 => {
                        if let ContentBlock::ToolUse { name, .. } = &mut blocks[0] {
                            *name = "foreign_tool".into();
                        }
                    }
                    4 => {
                        if let ContentBlock::ToolUse { input, .. } = &mut blocks[0] {
                            *input = json!({"forged":true});
                        }
                    }
                    5 => {
                        if let ContentBlock::ToolUse { id, .. } = &mut blocks[0] {
                            id.push('x');
                        }
                    }
                    6 => blocks.push(blocks[0].clone()),
                    7 => {
                        let mut duplicate = blocks[0].clone();
                        if let ContentBlock::ToolUse { name, .. } = &mut duplicate {
                            *name = "different_tool".into();
                        }
                        blocks.push(duplicate);
                    }
                    _ => unreachable!(),
                }
                sqlx::query("UPDATE messages SET content=?1 WHERE message_id=?2")
                    .bind(serde_json::to_string(&message.content.to_stored_json()).unwrap())
                    .bind(&f.command.source.message_id)
                    .execute(f.db.pool())
                    .await
                    .unwrap();
            }
            8 => {
                sqlx::query("UPDATE canonical_tool_round_receipts SET conversation_id=?1 WHERE assistant_message_id=?2")
                    .bind(&f.command.current_transcript_id).bind(&f.command.source.message_id)
                    .execute(f.db.pool()).await.unwrap();
            }
            9 => f.command.source.message_id.push('x'),
            _ => unreachable!(),
        }
        let before = database_rows(&f.db).await;
        assert!(
            f.inspect().await.is_err(),
            "forged canonical evidence {broken}"
        );
        assert!(
            f.settle().await.is_err(),
            "forged canonical settlement {broken}"
        );
        assert_eq!(
            database_rows(&f.db).await,
            before,
            "forged canonical evidence {broken}"
        );
    }
}

#[tokio::test]
async fn canonical_replay_requires_the_stored_source_to_still_be_current_coordinator() {
    let mut f = Fixture::new(false).await;
    f.settle().await.unwrap();
    f.checkpoint_source().await;
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&f.command.source_transcript_id)
        .execute(f.db.pool())
        .await
        .unwrap();
    f.command.pending.title.push_str(" changed");
    let before = database_rows(&f.db).await;
    let error = f.inspect().await.unwrap_err();
    assert!(matches!(
        error,
        NativeTaskApprovalError::Database(DbError::CurrentCoordinatorRequired)
    ));
    assert!(matches!(
        f.settle().await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn canonical_payload_must_prove_the_request_before_typed_fingerprint_conflict() {
    let mut f = Fixture::new(false).await;
    f.settle().await.unwrap();
    f.checkpoint_source().await;
    f.command.pending.title.push_str(" changed");
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.inspect().await,
        Err(NativeTaskApprovalError::Database(DbError::MessageConflict(
            _
        )))
    ));
    assert_eq!(database_rows(&f.db).await, before);
    let mut source = f.source_message();
    let MessageContent::Agent(blocks) = &mut source.content else {
        unreachable!()
    };
    if let ContentBlock::ToolUse { input, .. } = &mut blocks[0] {
        *input = f.command.input_value().unwrap();
    }
    sqlx::query("UPDATE messages SET content=?1 WHERE message_id=?2")
        .bind(serde_json::to_string(&source.content.to_stored_json()).unwrap())
        .bind(&f.command.source.message_id)
        .execute(f.db.pool())
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.inspect().await,
        Err(NativeTaskApprovalError::RequestConflict)
    ));
    assert!(f.settle().await.is_err());
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn failures_at_every_write_roll_back_all_rows_including_native_source() {
    let triggers = [
        "CREATE TRIGGER fail_native BEFORE INSERT ON conversation_approved_task_objectives BEGIN SELECT RAISE(ABORT, 'objective failure'); END",
        "CREATE TRIGGER fail_native BEFORE INSERT ON work_scope_approved_task_authorities BEGIN SELECT RAISE(ABORT, 'authority failure'); END",
        "CREATE TRIGGER fail_native BEFORE UPDATE OF authority_kind ON work_scopes BEGIN SELECT RAISE(ABORT, 'scope failure'); END",
        "CREATE TRIGGER fail_native BEFORE INSERT ON messages WHEN NEW.message_id='approval-context' BEGIN SELECT RAISE(ABORT, 'message failure'); END",
        "CREATE TRIGGER fail_native BEFORE INSERT ON approval_request_obligations BEGIN SELECT RAISE(ABORT, 'obligation failure'); END",
        "CREATE TRIGGER fail_native BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT, 'state failure'); END",
        "CREATE TRIGGER fail_native BEFORE INSERT ON native_task_approval_source_calls BEGIN SELECT RAISE(ABORT, 'source failure'); END",
        "CREATE TRIGGER fail_native BEFORE INSERT ON native_task_approval_receipts BEGIN SELECT RAISE(ABORT, 'receipt failure'); END",
    ];
    for trigger in triggers {
        let f = Fixture::new(false).await;
        sqlx::query(trigger).execute(f.db.pool()).await.unwrap();
        let before = database_rows(&f.db).await;
        assert!(f.settle().await.is_err(), "{trigger}");
        assert_eq!(database_rows(&f.db).await, before, "{trigger}");
    }
}

#[tokio::test]
async fn live_exact_replay_precedes_consumed_pending_progress_and_target_stop_state() {
    let f = Fixture::new(false).await;
    f.settle().await.unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.settle().await.unwrap(),
        LocalAuthorityResult::DurableFactEstablished(())
    ));
    assert_eq!(database_rows(&f.db).await, before);
    f.db.update_conversation_state(&f.command.current_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_request_obligations")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(owed, 0);
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.settle().await.unwrap(),
        LocalAuthorityResult::DurableFactEstablished(())
    ));
    assert_eq!(database_rows(&f.db).await, before);
    // Idle is only a persisted post-Stop state, not proof of the runtime Stop latch.
}

#[tokio::test]
async fn full_identity_hash_is_deterministic_and_covers_every_frozen_field() {
    let f = Fixture::new(false).await;
    let original = f.command.fingerprint().unwrap();
    assert_eq!(original, f.command.clone().fingerprint().unwrap());
    for field in 0..13 {
        let mut changed = f.command.clone();
        match field {
            0 => changed.owner = ProductConversationId::parse("foreign-owner").unwrap(),
            1 => changed.current_transcript_id.push('x'),
            2 => changed.pending.task_file.push('x'),
            3 => changed.pending.title.push('x'),
            4 => changed.pending.priority = Priority::P2,
            5 => changed.pending.plan.push('x'),
            6 => changed
                .pending
                .reviewed_proposal
                .as_mut()
                .unwrap()
                .body_sha256
                .push('x'),
            7 => {
                changed.pending.reviewed_proposal.as_mut().unwrap().kind = ReviewedTaskKind::Taskmd;
            }
            8 => changed
                .pending
                .reviewed_proposal
                .as_mut()
                .unwrap()
                .occurrence
                .assistant_message_id
                .push('x'),
            9 => changed
                .pending
                .reviewed_proposal
                .as_mut()
                .unwrap()
                .occurrence
                .tool_use_id
                .push('x'),
            10 => changed.source.message_id.push('x'),
            11 => changed.source_transcript_id.push('x'),
            12 => changed.source.tool_use_id.push('x'),
            _ => unreachable!(),
        }
        assert_ne!(changed.fingerprint().unwrap(), original, "field {field}");
    }
    let input = f.command.input_value().unwrap();
    assert_eq!(input["placement"], "continue_here");
    assert_eq!(input["intent"], "approve_task_and_request_continuation");
}

#[tokio::test]
async fn changed_payload_under_proven_same_source_conflicts_without_writes() {
    let mut f = Fixture::new(false).await;
    f.settle().await.unwrap();
    f.command.pending.title.push_str(" changed");
    // Match both server-owned current tool and canonical block to the changed typed command.
    f.install_source().await;
    let before = database_rows(&f.db).await;
    assert!(matches!(f.settle().await, Err(DbError::MessageConflict(_))));
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn source_policy_rejects_invalid_sources_before_an_existing_receipt_can_replay() {
    for change in 0..9 {
        let mut f = Fixture::new(false).await;
        f.settle().await.unwrap();
        let mut executing = f.executing.clone();
        match change {
            0 => executing = ConvState::Idle,
            1 => {
                if let ConvState::ToolExecuting { current_tool, .. } = &mut executing {
                    current_tool.id.push('x');
                }
            }
            2 => {
                if let ConvState::ToolExecuting {
                    assistant_message, ..
                } = &mut executing
                {
                    assistant_message.message_id.push('x');
                }
            }
            3 => {
                if let ConvState::ToolExecuting {
                    current_tool,
                    remaining_tools,
                    ..
                } = &mut executing
                {
                    remaining_tools.push(current_tool.clone());
                }
            }
            4 => {
                if let ConvState::ToolExecuting {
                    assistant_message, ..
                } = &mut executing
                {
                    assistant_message
                        .content
                        .push(assistant_message.content[0].clone());
                }
            }
            5 => {
                executing = executing_source(
                    &f.command.source,
                    "not_approve_task",
                    f.command.input_value().unwrap(),
                );
            }
            6 => {
                f.command.source.tool_use_id.push('x');
            }
            7 => {
                if let ConvState::ToolExecuting { current_tool, .. } = &mut executing {
                    current_tool.input = ToolInput::Unknown {
                        name: APPROVE_TASK_TOOL_NAME.into(),
                        input: json!({"forged":true}),
                    };
                }
            }
            8 => {
                if let ConvState::ToolExecuting {
                    assistant_message, ..
                } = &executing
                {
                    executing = ConvState::CancellingTool {
                        tool_use_id: f.command.source.tool_use_id.clone(),
                        cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                        skipped_tools: vec![],
                        completed_results: vec![],
                        assistant_message: assistant_message.clone(),
                        pending_sub_agents: vec![],
                    };
                }
            }
            _ => unreachable!(),
        }
        f.db.update_conversation_state(&f.command.source_transcript_id, &executing)
            .await
            .unwrap();
        let before = database_rows(&f.db).await;
        assert!(f.settle().await.is_err(), "source change {change}");
        assert_eq!(database_rows(&f.db).await, before);
    }
}

#[tokio::test]
async fn historical_and_non_coordinator_sources_cannot_lookup_receipts() {
    let mut f = Fixture::new(false).await;
    f.settle().await.unwrap();
    let original_source = f.command.source_transcript_id.clone();
    f.db.update_conversation_state(
        &original_source,
        &ConvState::ContextExhausted {
            summary: "continue".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(_) =
        f.db.continue_conversation(&original_source).await.unwrap()
    else {
        panic!("Coordinator successor");
    };
    f.db.update_conversation_state(&original_source, &f.executing)
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.settle().await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
    assert_eq!(database_rows(&f.db).await, before);
    f.command.source_transcript_id = f.command.current_transcript_id.clone();
    f.install_source().await;
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.settle().await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn distinct_source_cannot_approve_a_consumed_proposal_even_if_pending_is_restored() {
    let mut f = Fixture::new(false).await;
    f.settle().await.unwrap();
    f.command.source.tool_use_id.push_str("-second");
    f.install_source().await;
    f.db.update_conversation_state(&f.command.current_transcript_id, &pending_state(true))
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(f.settle().await, Err(DbError::MessageConflict(_))));
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn user_approval_uses_unchanged_settlement_without_native_provenance_or_receipts() {
    let f = Fixture::new(false).await;
    f.db.update_conversation_state(&f.command.source_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    assert!(matches!(
        f.db.persist_approved_task_authority_and_state(
            &f.command.current_transcript_id,
            &f.approval,
            &f.message,
            &f.state,
            f.message.created_at,
        )
        .await
        .unwrap(),
        LocalAuthorityResult::DurableFactEstablished(())
    ));
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM native_task_approval_receipts")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    let source_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM native_task_approval_source_calls")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!((receipt_count, source_count), (0, 0));
    assert_eq!(
        f.db.get_approved_task_objective(&f.command.current_transcript_id)
            .await
            .unwrap()
            .unwrap(),
        ApprovedTaskSnapshot::from(&f.approval)
    );
    assert!(!receipt_established(
        &f.db,
        &f.command,
        &ApprovedTaskSnapshot::from(&f.approval),
        &f.message
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn immutable_schema_rejects_updates_without_changing_any_row() {
    let f = Fixture::new(false).await;
    f.settle().await.unwrap();
    for statement in [
        "UPDATE native_task_approval_source_calls SET source_message_id='changed'",
        "UPDATE native_task_approval_source_calls SET source_message_id=source_message_id",
        "UPDATE native_task_approval_receipts SET continuation_requested=1",
        "UPDATE native_task_approval_receipts SET approval_message_id='changed'",
    ] {
        let before = database_rows(&f.db).await;
        assert!(sqlx::query(statement).execute(f.db.pool()).await.is_err());
        assert_eq!(database_rows(&f.db).await, before);
    }
}

#[tokio::test]
async fn receipt_schema_requires_source_exact_owner_proposal_and_approval_message_witnesses() {
    for broken in 0..7 {
        let mut f = Fixture::new(false).await;
        f.settle().await.unwrap();
        sqlx::query("DELETE FROM native_task_approval_receipts")
            .execute(f.db.pool())
            .await
            .unwrap();
        match broken {
            0 => {
                let other =
                    f.db.create_conversation(
                        "foreign-owner",
                        "foreign-owner",
                        "/nonexistent",
                        true,
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                f.command.owner = other.product_conversation_id;
            }
            1 => f.command.current_transcript_id.push_str("-foreign"),
            2 => f
                .command
                .pending
                .reviewed_proposal
                .as_mut()
                .unwrap()
                .occurrence
                .assistant_message_id
                .push_str("-missing"),
            3 => f.message.message_id.push_str("-missing"),
            4 => {
                // A real message without a canonical receipt is not a proposal witness.
                sqlx::query("DELETE FROM canonical_tool_round_receipts WHERE conversation_id=?1")
                    .bind(&f.command.current_transcript_id)
                    .execute(f.db.pool())
                    .await
                    .unwrap();
            }
            5 => {
                // The message exists but is not owned by the target transcript.
                f.message.message_id = "foreign-context".into();
                f.message.conversation_id = f.command.source_transcript_id.clone();
                let mut tx = f.db.pool().begin().await.unwrap();
                crate::insert_message_tx(&mut tx, &f.message).await.unwrap();
                tx.commit().await.unwrap();
            }
            6 => {
                // Receipt insertion below must not fabricate a normalized source witness.
                sqlx::query("DELETE FROM native_task_approval_source_calls")
                    .execute(f.db.pool())
                    .await
                    .unwrap();
                let before = database_rows(&f.db).await;
                let result = sqlx::query(
                    "INSERT INTO native_task_approval_receipts
                     (source_transcript_id, source_message_id, source_tool_use_id, request_sha256,
                      target_product_conversation_id, target_transcript_id, proposal_message_id,
                      proposal_tool_use_id, approval_message_id, continuation_requested, committed_at_us)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'proposal-assistant', 'proposal-call', ?7, 1, 0)",
                ).bind(&f.command.source_transcript_id).bind(&f.command.source.message_id)
                 .bind(&f.command.source.tool_use_id).bind(f.command.fingerprint().unwrap())
                 .bind(f.command.owner.as_str()).bind(&f.command.current_transcript_id)
                 .bind(&f.message.message_id).execute(f.db.pool()).await;
                assert!(result.is_err());
                assert_eq!(database_rows(&f.db).await, before);
                continue;
            }
            _ => unreachable!(),
        }
        let before = database_rows(&f.db).await;
        let mut tx = f.db.pool().begin().await.unwrap();
        assert!(
            insert_receipt_tx(&mut tx, &f.command, &f.message)
                .await
                .is_err(),
            "broken FK {broken}"
        );
        tx.rollback().await.unwrap();
        assert_eq!(database_rows(&f.db).await, before, "broken FK {broken}");
    }
}

#[tokio::test]
async fn invalid_first_settlement_never_writes_source_receipt_or_authority() {
    for broken in 0..7 {
        let mut f = Fixture::new(false).await;
        match broken {
            0 => f.command.pending.plan.push_str("changed body"),
            1 => f
                .command
                .pending
                .reviewed_proposal
                .as_mut()
                .unwrap()
                .body_sha256
                .push('x'),
            2 => f.command.pending.task_file.push('x'),
            3 => f.command.pending.reviewed_proposal = None,
            4 => f.state = ConvState::Idle,
            5 => f.message.conversation_id = f.command.source_transcript_id.clone(),
            6 => f.approval.artifact_body.push('x'),
            _ => unreachable!(),
        }
        if f.command.pending.reviewed_proposal.is_some() {
            f.install_source().await;
        }
        let before = database_rows(&f.db).await;
        assert!(
            f.settle().await.is_err(),
            "invalid first settlement {broken}"
        );
        assert_eq!(database_rows(&f.db).await, before);
    }
}

#[tokio::test]
async fn commit_error_witness_rejects_missing_receipt_even_when_old_user_state_matches() {
    let f = Fixture::new(false).await;
    f.settle().await.unwrap();
    assert!(receipt_established(
        &f.db,
        &f.command,
        &ApprovedTaskSnapshot::from(&f.approval),
        &f.message
    )
    .await
    .unwrap());
    sqlx::query("DELETE FROM native_task_approval_receipts")
        .execute(f.db.pool())
        .await
        .unwrap();
    assert_eq!(
        f.db.get_conversation(&f.command.current_transcript_id)
            .await
            .unwrap()
            .state,
        f.state
    );
    assert!(f
        .db
        .get_approved_task_objective(&f.command.current_transcript_id)
        .await
        .unwrap()
        .is_some());
    assert!(!receipt_established(
        &f.db,
        &f.command,
        &ApprovedTaskSnapshot::from(&f.approval),
        &f.message
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn dispatch_and_later_approval_do_not_invalidate_old_receipt_or_requeue_on_replay() {
    let f = Fixture::new(false).await;
    f.settle().await.unwrap();
    sqlx::query("DELETE FROM approval_request_obligations WHERE conversation_id=?1")
        .bind(&f.command.current_transcript_id)
        .execute(f.db.pool())
        .await
        .unwrap();
    let later_message = Message {
        message_id: "later-user-approval".into(),
        sequence_id: 4,
        ..f.message.clone()
    };
    f.db.persist_approved_task_authority_and_state(
        &f.command.current_transcript_id,
        &f.approval,
        &later_message,
        &f.state,
        later_message.created_at,
    )
    .await
    .unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.settle().await.unwrap(),
        LocalAuthorityResult::DurableFactEstablished(())
    ));
    assert_eq!(database_rows(&f.db).await, before);
    let current_obligation: String = sqlx::query_scalar(
        "SELECT approval_message_id FROM approval_request_obligations WHERE conversation_id=?1",
    )
    .bind(&f.command.current_transcript_id)
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(current_obligation, later_message.message_id);
    let receipt_message: String =
        sqlx::query_scalar("SELECT approval_message_id FROM native_task_approval_receipts")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(receipt_message, f.message.message_id);
}

#[tokio::test]
async fn public_native_settlement_returns_transaction_receipt_and_replays_original_message() {
    let f = Fixture::new(false).await;
    let first =
        f.db.persist_native_approved_task_authority_and_state(
            &f.command,
            &f.approval,
            &f.message,
            &f.state,
            f.message.created_at,
        )
        .await
        .unwrap();
    let LocalAuthorityResult::DurableFactEstablished(first) = first else {
        panic!("native receipt not established");
    };
    assert_eq!(first.approval_message_id, f.message.message_id);
    assert_eq!(first.continuation, NativeApprovalContinuation::Requested);
    f.checkpoint_source().await;
    f.db.update_conversation_state(&f.command.current_transcript_id, &ConvState::Idle)
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    let mut replacement = f.message.clone();
    replacement.message_id = "must-not-be-used-for-replay".into();
    let replay =
        f.db.persist_native_approved_task_authority_and_state(
            &f.command,
            &f.approval,
            &replacement,
            &f.state,
            replacement.created_at,
        )
        .await
        .unwrap();
    assert!(
        matches!(replay, LocalAuthorityResult::DurableFactEstablished(receipt) if receipt == first)
    );
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn public_native_receipt_failure_rolls_back_message_state_authority_and_obligation() {
    let f = Fixture::new(false).await;
    sqlx::query(
        "CREATE TRIGGER reject_native_receipt BEFORE INSERT ON native_task_approval_receipts
        BEGIN SELECT RAISE(ABORT, 'injected native receipt failure'); END",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    let before = database_rows(&f.db).await;
    assert!(f
        .db
        .persist_native_approved_task_authority_and_state(
            &f.command,
            &f.approval,
            &f.message,
            &f.state,
            f.message.created_at,
        )
        .await
        .is_err());
    assert_eq!(database_rows(&f.db).await, before);
}
