use super::*;
use chrono::Utc;
use phoenix_core::domain::db_schema::{
    FinalizedToolResult, InputOrigin, Message, MessageContent, MessageType, ToolResult,
};
use phoenix_core::domain::provider_replay::{
    AnthropicResponseIdentity, AnthropicResponseSet, ProviderReplayUpdate,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ToolCall, ToolInput};
use phoenix_core::domain::task_approval::ProposalOccurrence;
use phoenix_core::task_source::TaskSource;
use serde_json::{json, Value};
use sqlx::Row;

const BODY: &str = "# Exact reviewed task\r\n\nKeep these bytes.\n";

struct Fixture {
    db: Database,
    coordinator: String,
    target: String,
    input: InspectPendingTaskInput,
    source: SourceToolCall,
    executing: ConvState,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_replay(false).await
    }

    async fn with_replay(with_replay: bool) -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let target = db
            .create_conversation(
                "inspection-target",
                "inspection-target",
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
        let input = InspectPendingTaskInput {
            target: target.product_conversation_id,
        };
        let source = SourceToolCall {
            message_id: "live-inspection-assistant".into(),
            tool_use_id: "live-inspection-call".into(),
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
            assistant_message: AssistantMessage::new(
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
        checkpoint_proposal(&db, &target.id, with_replay).await;
        let fixture = Self {
            db,
            coordinator: coordinator.id,
            target: target.id,
            input,
            source,
            executing,
        };
        fixture.assert_no_finalized_source().await;
        fixture
    }

    async fn inspect(&self) -> DbResult<PendingTaskInspection> {
        self.db
            .inspect_pending_task_from_coordinator(&self.coordinator, &self.source, &self.input)
            .await
    }

    async fn assert_no_finalized_source(&self) {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE conversation_id=?1 AND message_id=?2",
        )
        .bind(&self.coordinator)
        .bind(&self.source.message_id)
        .fetch_one(self.db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
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
async fn executing_source_without_finalized_message_inspects_exact_aggregate_read_only() {
    let f = Fixture::new().await;
    let before = database_rows(&f.db).await;
    let inspection = f.inspect().await.unwrap();
    assert_eq!(inspection.owner, f.input.target);
    assert_eq!(inspection.current_transcript_id, f.target);
    let PendingTaskStatus::Pending(pending) = inspection.pending else {
        panic!("pending proposal")
    };
    assert_eq!(pending.task_file, "tasks/reviewed.md");
    assert_eq!(pending.title, "Exact title");
    assert_eq!(pending.priority, Priority::P1);
    assert_eq!(pending.plan.as_bytes(), BODY.as_bytes());
    let ConvState::AwaitingTaskApproval {
        reviewed_proposal, ..
    } = pending_state(true)
    else {
        unreachable!()
    };
    assert_eq!(pending.reviewed_proposal, reviewed_proposal);
    assert_eq!(
        pending.native_approval_availability(),
        NativeTaskApprovalAvailability::CapturedProposalRequiresAdmission
    );
    assert_eq!(database_rows(&f.db).await, before);
    f.assert_no_finalized_source().await;
}

fn assert_exact_snapshot(snapshot: &PendingTaskSnapshot, state: &ConvState) {
    let ConvState::AwaitingTaskApproval {
        task_file,
        title,
        priority,
        plan,
        reviewed_proposal,
    } = state
    else {
        panic!("pending fixture")
    };
    assert_eq!(&snapshot.task_file, task_file);
    assert_eq!(&snapshot.title, title);
    assert_eq!(&snapshot.priority, priority);
    assert_eq!(snapshot.plan.as_bytes(), plan.as_bytes());
    assert_eq!(&snapshot.reviewed_proposal, reviewed_proposal);
}

async fn assert_integrity_read_only(
    f: &Fixture,
    state: &ConvState,
    failure: ProposalIntegrityFailure,
) {
    let before = database_rows(&f.db).await;
    let inspected = f.inspect().await.unwrap();
    assert_eq!(inspected.owner, f.input.target);
    assert_eq!(inspected.current_transcript_id, f.target);
    let PendingTaskStatus::Pending(snapshot) = inspected.pending else {
        panic!("invalid provenance must remain inspectable")
    };
    assert_exact_snapshot(&snapshot, state);
    assert_eq!(
        snapshot.native_approval_availability(),
        NativeTaskApprovalAvailability::UnavailableIntegrity(failure)
    );
    f.assert_no_finalized_source().await;
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn provider_checkpoint_has_target_witness_without_finalized_coordinator_source() {
    let f = Fixture::with_replay(true).await;
    let before = database_rows(&f.db).await;
    let PendingTaskStatus::Pending(snapshot) = f.inspect().await.unwrap().pending else {
        panic!("canonical provider proposal")
    };
    assert_exact_snapshot(&snapshot, &pending_state(true));
    assert_eq!(
        snapshot.native_approval_availability(),
        NativeTaskApprovalAvailability::CapturedProposalRequiresAdmission
    );
    f.assert_no_finalized_source().await;
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn missing_or_foreign_receipt_and_missing_or_foreign_message_remain_inspectable() {
    for (statement, failure) in [
        (
            "DELETE FROM canonical_tool_round_receipts WHERE conversation_id=?1",
            ProposalIntegrityFailure::MissingCanonicalReceipt,
        ),
        (
            "DELETE FROM messages WHERE conversation_id=?1",
            ProposalIntegrityFailure::MissingProposalMessage,
        ),
        (
            "UPDATE canonical_tool_round_receipts SET conversation_id=?2 WHERE conversation_id=?1",
            ProposalIntegrityFailure::MissingCanonicalReceipt,
        ),
        (
            "UPDATE messages SET conversation_id=?2,sequence_id=1009 WHERE conversation_id=?1",
            ProposalIntegrityFailure::ProposalMismatch,
        ),
    ] {
        let f = Fixture::new().await;
        let mut query = sqlx::query(statement).bind(&f.target);
        if statement.contains("?2") {
            query = query.bind(&f.coordinator);
        }
        query.execute(f.db.pool()).await.unwrap();
        assert_integrity_read_only(&f, &pending_state(true), failure).await;
    }
}

#[tokio::test]
async fn forged_occurrence_and_changed_snapshot_do_not_claim_captured_provenance() {
    for change in 0..8 {
        let f = Fixture::new().await;
        let mut state = pending_state(true);
        let ConvState::AwaitingTaskApproval {
            task_file,
            plan,
            reviewed_proposal: Some(reviewed),
            ..
        } = &mut state
        else {
            unreachable!()
        };
        match change {
            0 => reviewed.occurrence.assistant_message_id = "invented-assistant".into(),
            1 => reviewed.occurrence.tool_use_id = "invented-call".into(),
            2 => *task_file = "tasks/stale-input-path.md".into(),
            3 => plan.push_str("changed reviewed bytes"),
            4 => reviewed.body_sha256 = "0".repeat(64),
            5 => reviewed.kind = phoenix_core::domain::task_approval::ReviewedTaskKind::Taskmd,
            6 => reviewed.occurrence.tool_use_id = " ".into(),
            7 => *task_file = "tasks/not-markdown.txt".into(),
            _ => unreachable!(),
        }
        f.db.update_conversation_state(&f.target, &state)
            .await
            .unwrap();
        let failure = if change == 0 {
            ProposalIntegrityFailure::MissingProposalMessage
        } else {
            ProposalIntegrityFailure::ProposalMismatch
        };
        assert_integrity_read_only(&f, &state, failure).await;
    }
}

#[tokio::test]
async fn changed_or_duplicate_proposing_tool_use_fails_witness_validation() {
    for change in 0..6 {
        let f = Fixture::new().await;
        let mut assistant = proposal_message(&f.target);
        let MessageContent::Agent(blocks) = &mut assistant.content else {
            unreachable!()
        };
        match change {
            0 => blocks.clear(),
            1 => blocks.push(blocks[0].clone()),
            2 => blocks.push(ContentBlock::ToolUse {
                id: "proposal-call".into(),
                name: "different_tool".into(),
                input: json!({}),
            }),
            3 => {
                blocks[0] = ContentBlock::ToolUse {
                    id: "proposal-call".into(),
                    name: "propose_task".into(),
                    input: json!({"task_file":"tasks/stale.md"}),
                }
            }
            4 => {
                blocks[0] = ContentBlock::ToolUse {
                    id: "proposal-call".into(),
                    name: "different_tool".into(),
                    input: json!({"task_file":"tasks/reviewed.md"}),
                }
            }
            5 => {
                blocks[0] = ContentBlock::ToolUse {
                    id: "proposal-call".into(),
                    name: "propose_task".into(),
                    input: json!({"task_file":42}),
                }
            }
            _ => unreachable!(),
        }
        sqlx::query("UPDATE messages SET content=?1 WHERE message_id=?2")
            .bind(serde_json::to_string(&assistant.content).unwrap())
            .bind(&assistant.message_id)
            .execute(f.db.pool())
            .await
            .unwrap();
        assert_integrity_read_only(
            &f,
            &pending_state(true),
            ProposalIntegrityFailure::ProposalMismatch,
        )
        .await;
    }
}

#[tokio::test]
async fn legacy_pending_is_inspectable_unavailable_and_distinct_from_no_pending() {
    let f = Fixture::new().await;
    f.db.update_conversation_state(&f.target, &pending_state(false))
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    let PendingTaskStatus::Pending(pending) = f.inspect().await.unwrap().pending else {
        panic!("legacy remains inspectable")
    };
    assert_eq!(pending.plan, BODY);
    assert_eq!(pending.reviewed_proposal, None);
    assert_eq!(
        pending.native_approval_availability(),
        NativeTaskApprovalAvailability::UnavailableRepropose
    );
    assert_eq!(database_rows(&f.db).await, before);
    f.db.update_conversation_state(&f.target, &ConvState::Idle)
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert_eq!(
        f.inspect().await.unwrap().pending,
        PendingTaskStatus::NoPending
    );
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn continued_owner_inspection_resolves_current_successor_not_root() {
    let f = Fixture::new().await;
    f.db.update_conversation_state(
        &f.target,
        &ConvState::ContextExhausted {
            summary: "continue target".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        f.db.continue_conversation(&f.target).await.unwrap()
    else {
        panic!("successor")
    };
    f.db.update_conversation_state(&successor.id, &pending_state(false))
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    let inspected = f.inspect().await.unwrap();
    assert_eq!(inspected.owner, f.input.target);
    assert_eq!(inspected.current_transcript_id, successor.id);
    assert_ne!(inspected.current_transcript_id, f.target);
    let PendingTaskStatus::Pending(pending) = inspected.pending else {
        panic!("current successor pending")
    };
    assert_eq!(pending.plan, BODY);
    assert_eq!(pending.reviewed_proposal, None);
    assert_eq!(database_rows(&f.db).await, before);
}

#[allow(clippy::too_many_lines)] // One matrix shares the same persisted execution fixture.
#[tokio::test]
async fn canonical_source_mismatch_matrix_is_rejected_without_writes() {
    let f = Fixture::new().await;
    for case in [
        "message",
        "current_id",
        "current_name",
        "current_input",
        "block_id",
        "block_name",
        "block_input",
        "duplicate_block",
        "conflicting_duplicate",
        "remaining_source",
        "completed_source",
        "historical",
        "cancelling",
    ] {
        let mut state = f.executing.clone();
        let ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            completed_results,
            assistant_message,
            ..
        } = &mut state
        else {
            unreachable!()
        };
        match case {
            "message" => assistant_message.message_id = "foreign-message".into(),
            "current_id" => current_tool.id = "other-call".into(),
            "current_name" => {
                current_tool.input = ToolInput::Unknown {
                    name: "other_tool".into(),
                    input: serde_json::to_value(&f.input).unwrap(),
                }
            }
            "current_input" => {
                current_tool.input = ToolInput::Unknown {
                    name: INSPECT_PENDING_TASK_TOOL_NAME.into(),
                    input: json!({"target":"different-owner"}),
                }
            }
            "block_id" => {
                if let ContentBlock::ToolUse { id, .. } = &mut assistant_message.content[0] {
                    *id = "different-block".into();
                }
            }
            "block_name" => {
                if let ContentBlock::ToolUse { name, .. } = &mut assistant_message.content[0] {
                    *name = "other_tool".into();
                }
            }
            "block_input" => {
                if let ContentBlock::ToolUse { input, .. } = &mut assistant_message.content[0] {
                    *input = json!({"target":"different-owner"});
                }
            }
            "duplicate_block" => assistant_message
                .content
                .push(assistant_message.content[0].clone()),
            "conflicting_duplicate" => assistant_message.content.push(ContentBlock::ToolUse {
                id: f.source.tool_use_id.clone(),
                name: "other_tool".into(),
                input: json!({}),
            }),
            "remaining_source" => remaining_tools.push(current_tool.clone()),
            "completed_source" => completed_results
                .push(ToolResult::success(f.source.tool_use_id.clone(), "done".into()).into()),
            "historical" => state = ConvState::Idle,
            "cancelling" => {
                state = ConvState::CancellingTool {
                    tool_use_id: f.source.tool_use_id.clone(),
                    cause: Default::default(),
                    skipped_tools: vec![],
                    completed_results: vec![],
                    assistant_message: assistant_message.clone(),
                    pending_sub_agents: vec![],
                }
            }
            _ => unreachable!(),
        }
        f.db.update_conversation_state(&f.coordinator, &state)
            .await
            .unwrap();
        let before = database_rows(&f.db).await;
        assert!(
            matches!(f.inspect().await, Err(DbError::MessageConflict(_))),
            "{case}"
        );
        assert_eq!(database_rows(&f.db).await, before, "{case}");
    }
    f.assert_no_finalized_source().await;
}

#[tokio::test]
async fn continued_coordinator_accepts_live_successor_and_rejects_historical_source() {
    let mut f = Fixture::new().await;
    let stale = f.coordinator.clone();
    f.db.update_conversation_state(
        &stale,
        &ConvState::ContextExhausted {
            summary: "continue Coordinator".into(),
        },
    )
    .await
    .unwrap();
    let crate::ContinueOutcome::Created(successor) =
        f.db.continue_conversation(&stale).await.unwrap()
    else {
        panic!("Coordinator successor")
    };
    f.coordinator = successor.id;
    for transcript in [&stale, &f.coordinator] {
        f.db.update_conversation_state(transcript, &f.executing)
            .await
            .unwrap();
    }
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.db.inspect_pending_task_from_coordinator(&stale, &f.source, &f.input)
            .await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
    assert_eq!(f.inspect().await.unwrap().current_transcript_id, f.target);
    f.assert_no_finalized_source().await;
    assert_eq!(database_rows(&f.db).await, before);
}

#[tokio::test]
async fn caller_identity_and_changed_request_are_rejected_before_target_lookup() {
    let f = Fixture::new().await;
    let missing = InspectPendingTaskInput {
        target: ProductConversationId::parse("missing-owner").unwrap(),
    };
    assert!(matches!(
        f.db.inspect_pending_task_from_coordinator(&f.coordinator, &f.source, &missing)
            .await,
        Err(DbError::MessageConflict(_))
    ));
    for source in [
        SourceToolCall {
            message_id: "other-message".into(),
            tool_use_id: f.source.tool_use_id.clone(),
        },
        SourceToolCall {
            message_id: f.source.message_id.clone(),
            tool_use_id: "other-tool".into(),
        },
        SourceToolCall {
            message_id: String::new(),
            tool_use_id: f.source.tool_use_id.clone(),
        },
    ] {
        assert!(matches!(
            f.db.inspect_pending_task_from_coordinator(&f.coordinator, &source, &f.input)
                .await,
            Err(DbError::MessageConflict(_))
        ));
    }
    f.db.update_conversation_state(&f.target, &f.executing)
        .await
        .unwrap();
    for transcript in [f.target.as_str(), "missing-transcript"] {
        assert!(matches!(
            f.db.inspect_pending_task_from_coordinator(transcript, &f.source, &missing)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&f.coordinator)
        .execute(f.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        f.db.inspect_pending_task_from_coordinator(&f.coordinator, &f.source, &missing)
            .await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
}

#[tokio::test]
async fn finalized_historical_message_never_rescues_nonexecuting_or_changed_source() {
    let f = Fixture::new().await;
    let ConvState::ToolExecuting {
        assistant_message, ..
    } = &f.executing
    else {
        unreachable!()
    };
    f.db.add_message(
        &f.source.message_id,
        &f.coordinator,
        &MessageContent::Agent(assistant_message.content.clone()),
        None,
        None,
    )
    .await
    .unwrap();
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.inspect().await,
        Err(DbError::MessageConflict(_))
    ));
    assert_eq!(database_rows(&f.db).await, before);
    f.db.update_conversation_state(&f.coordinator, &f.executing)
        .await
        .unwrap();
    let changed = InspectPendingTaskInput {
        target: ProductConversationId::parse("changed-request").unwrap(),
    };
    let before = database_rows(&f.db).await;
    assert!(matches!(
        f.db.inspect_pending_task_from_coordinator(&f.coordinator, &f.source, &changed)
            .await,
        Err(DbError::MessageConflict(_))
    ));
    assert_eq!(database_rows(&f.db).await, before);
}

#[test]
fn inspection_input_reuses_validated_owner_and_rejects_extra_authority() {
    for input in [
        json!({"target":""}),
        json!({"target":"   "}),
        json!({"target":"owner", "path":"tasks/fake.md"}),
        Value::Null,
    ] {
        assert!(serde_json::from_value::<InspectPendingTaskInput>(input).is_err());
    }
}
