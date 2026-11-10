use super::*;
use crate::conversation_goals::round::{CommittedGoalRound, GoalRoundCheckpoint, ToolRoundWrite};
use crate::provider_replay::CanonicalCommitCut;
use chrono::Utc;
use phoenix_core::domain::conversation_goal::{
    PendingSelfGoal, SelfGoalOperation, ToolRoundResult,
};
use phoenix_core::domain::db_schema::{
    InputOrigin, Message, MessageContent, MessageType, ToolResult,
};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::provider_replay::{
    AnthropicResponseIdentity, AnthropicResponseSet, ProviderReplayUpdate,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};

#[derive(Clone)]
struct SelfRound {
    transcript: String,
    owner: ProductConversationId,
    execution: GoalExecution,
    assistant: Message,
    results: Vec<ToolRoundResult>,
    timestamp: chrono::DateTime<Utc>,
}

impl SelfRound {
    async fn new(db: &Database, name: &str, operations: Vec<SelfGoalOperation>) -> Self {
        Self::with_preference(db, name, operations, true).await
    }

    async fn with_preference(
        db: &Database,
        name: &str,
        operations: Vec<SelfGoalOperation>,
        enabled: bool,
    ) -> Self {
        let (transcript, owner) = ordinary(db, name).await;
        if !enabled {
            db.set_auto_continue_on_context_exhaustion(
                &owner,
                crate::AutoContinueOnContextExhaustion::Disabled,
            )
            .await
            .unwrap();
        }
        Self::admit(db, transcript, owner, name, operations).await
    }

    async fn admit(
        db: &Database,
        transcript: String,
        owner: ProductConversationId,
        name: &str,
        operations: Vec<SelfGoalOperation>,
    ) -> Self {
        if db.goal_control_installation() == GoalControlInstallation::Installed {
            release(db, &owner).await;
        }
        let execution =
            accept_execution(&db.workflow_repository(), &runtime_input(&transcript, name)).await;
        let mut round = Self {
            transcript: transcript.clone(),
            owner,
            execution,
            assistant: Message {
                origin: InputOrigin::SystemGenerated,
                message_id: String::new(),
                conversation_id: transcript,
                sequence_id: 1,
                message_type: MessageType::Agent,
                content: MessageContent::agent(vec![]),
                display_data: None,
                usage_data: None,
                created_at: Utc::now(),
            },
            results: vec![],
            timestamp: Utc::now(),
        };
        round.next(db, name, operations).await;
        round
    }

    async fn next(&mut self, db: &Database, name: &str, operations: Vec<SelfGoalOperation>) {
        self.assistant.message_id = format!("assistant-{name}");
        self.assistant.created_at = Utc::now();
        self.assistant.sequence_id = db.next_sequence_id(&self.transcript).await.unwrap();
        self.timestamp = self.assistant.created_at;
        self.results.clear();
        let generation: i64 =
            sqlx::query_scalar("SELECT generation FROM durable_turns WHERE turn_id=?")
                .bind(self.execution.turn_id())
                .fetch_one(db.pool())
                .await
                .unwrap();
        let mut blocks = vec![];
        for (index, operation) in operations.into_iter().enumerate() {
            let id = format!("call-{index}");
            let input = match &operation {
                SelfGoalOperation::Create { objective } => {
                    serde_json::json!({"objective":objective.as_str()})
                }
                SelfGoalOperation::EnableAutoContinuation => serde_json::json!({}),
            };
            blocks.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: operation.tool_name().into(),
                input,
            });
            let pending = db
                .prepare_self_goal_tool(
                    &self.transcript,
                    self.execution.turn_id(),
                    generation,
                    &self.assistant.message_id,
                    &id,
                    operation,
                )
                .await
                .unwrap();
            self.results.push(ToolRoundResult::PendingSelfGoal {
                pending,
                duration_ms: Some(5),
            });
        }
        self.assistant.content = MessageContent::agent(blocks);
        self.install_state(db).await;
    }

    async fn install_state(&self, db: &Database) {
        let MessageContent::Agent(blocks) = &self.assistant.content else {
            unreachable!()
        };
        let mut calls = blocks.iter().filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                id: id.clone(),
                input: ToolInput::from_name_and_value(name, input.clone()),
            }),
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::WebSearchToolResult { .. }
            | ContentBlock::WebFetchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::BashCodeExecutionToolResult { .. }
            | ContentBlock::TextEditorCodeExecutionToolResult { .. }
            | ContentBlock::McpToolUse { .. }
            | ContentBlock::McpToolResult { .. } => None,
        });
        let state = ConvState::ToolExecuting {
            current_tool: calls.next().unwrap(),
            remaining_tools: calls.collect(),
            completed_results: vec![],
            assistant_message: AssistantMessage::new(
                self.assistant.message_id.clone(),
                blocks.clone(),
                None,
                None,
            ),
            pending_sub_agents: vec![],
        };
        db.update_conversation_state_at(&self.transcript, &state, self.timestamp)
            .await
            .unwrap();
    }

    fn checkpoint(&self) -> GoalRoundCheckpoint<'_> {
        GoalRoundCheckpoint {
            assistant: &self.assistant,
            execution: self.execution,
            results: &self.results,
            state: &ConvState::Idle,
            state_updated_at: self.timestamp,
        }
    }

    fn replay(&self) -> ProviderReplayUpdate {
        let MessageContent::Agent(blocks) = &self.assistant.content else {
            unreachable!()
        };
        ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: format!("response-{}", self.assistant.message_id),
                    model: "self-goal-model".into(),
                },
                blocks.clone(),
                vec![],
            )
            .unwrap()
            .with_owner_message_id(self.assistant.message_id.clone()),
        )
    }

    async fn commit(&self, db: &Database, replay: bool) -> crate::DbResult<CommittedGoalRound> {
        if replay {
            db.persist_goal_tool_round_state_and_replay(&self.checkpoint(), &self.replay())
                .await
        } else {
            db.persist_goal_tool_round_and_state(&self.checkpoint())
                .await
        }
    }

    async fn cut(
        &self,
        db: &Database,
        cut: CanonicalCommitCut,
    ) -> crate::DbResult<CommittedGoalRound> {
        db.persist_canonical_state_and_replay_input(
            &self.transcript,
            &self.assistant,
            ToolRoundWrite::Pending {
                execution: self.execution,
                results: &self.results,
            },
            &ConvState::Idle,
            self.timestamp,
            Some(&self.replay()),
            Some(cut),
        )
        .await
    }

    fn pending_mut(&mut self) -> &mut PendingSelfGoal {
        let ToolRoundResult::PendingSelfGoal { pending, .. } = &mut self.results[0] else {
            unreachable!()
        };
        pending
    }
}

fn text(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}

fn create() -> SelfGoalOperation {
    SelfGoalOperation::Create {
        objective: text("finish the approved work"),
    }
}

async fn enabled(db: &Database, owner: &ProductConversationId) -> bool {
    db.auto_continue_on_context_exhaustion(owner)
        .await
        .unwrap()
        .is_enabled()
}

fn status(message: &Message) -> serde_json::Value {
    let MessageContent::Tool(content) = &message.content else {
        panic!("not a canonical tool result")
    };
    serde_json::from_str(&content.content).unwrap()
}

async fn effect_counts(db: &Database) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM conversation_goal_history), (SELECT COUNT(*) FROM durable_turns), (SELECT COUNT(*) FROM workflows), (SELECT COUNT(*) FROM canonical_tool_round_receipts)").fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
async fn no_policy_installers_allow_canonical_enable_then_create() {
    let db = Database::open_in_memory().await.unwrap();
    let mut round = SelfRound::with_preference(
        &db,
        "uninstalled-policies",
        vec![SelfGoalOperation::EnableAutoContinuation],
        false,
    )
    .await;
    assert_eq!(
        status(&round.commit(&db, false).await.unwrap().results[0])["status"],
        "auto_continuation_enabled"
    );

    round.next(&db, "create", vec![create()]).await;
    assert_eq!(
        status(&round.commit(&db, false).await.unwrap().results[0])["status"],
        "goal_created"
    );
    assert!(db.conversation_goal(&round.owner).await.unwrap().is_some());
}

#[tokio::test]
async fn first_control_initialization_invalidates_operations_admitted_without_control() {
    let db = Database::open_in_memory().await.unwrap();
    let round = SelfRound::with_preference(
        &db,
        "first-control-initialization",
        vec![SelfGoalOperation::EnableAutoContinuation, create()],
        false,
    )
    .await;
    let admitted_control: Option<i64> = sqlx::query_scalar(
        "SELECT control_version FROM ordinary_self_goal_admissions WHERE turn_id=?",
    )
    .bind(round.execution.turn_id())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(admitted_control, None);

    let controls = db.clone().install_goal_controls_for_tests().await.unwrap();
    controls
        .apply_goal_control(
            &ControlRequest {
                owner: round.owner.clone(),
                principal_id: Uuid::nil(),
                request_id: Uuid::new_v4(),
                command: ControlCommand::Release,
                expected_control_version: 0,
            },
            at(),
            || panic!("lost authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();

    let before = effect_counts(&db).await;
    let committed = round.commit(&db, false).await.unwrap();
    for result in &committed.results {
        assert_eq!(status(result)["reason"], "conversation_stopped_or_paused");
    }
    assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
    assert!(!enabled(&db, &round.owner).await);
    let after = effect_counts(&db).await;
    assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
}

#[tokio::test]
async fn stop_then_release_control_aba_invalidates_admitted_operation() {
    let db = fixture().await;
    let round = SelfRound::new(&db, "control-aba", vec![create()]).await;
    for command in [ControlCommand::Stop, ControlCommand::Release] {
        let current = db
            .ordinary_goal_control(&round.owner)
            .await
            .unwrap()
            .unwrap();
        db.apply_goal_control(
            &ControlRequest {
                owner: round.owner.clone(),
                principal_id: Uuid::nil(),
                request_id: Uuid::new_v4(),
                command,
                expected_control_version: current.version(),
            },
            at(),
            || panic!("lost authority"),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    }

    let before = effect_counts(&db).await;
    let committed = round.commit(&db, false).await.unwrap();
    assert_eq!(
        status(&committed.results[0])["reason"],
        "conversation_stopped_or_paused"
    );
    assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
    let after = effect_counts(&db).await;
    assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
}

#[tokio::test]
async fn off_create_refuses_then_enable_then_create_in_separate_rounds_without_dispatch_or_rebinding(
) {
    for replay in [false, true] {
        let db = fixture().await;
        let mut round =
            SelfRound::with_preference(&db, "separate-rounds", vec![create()], false).await;
        let binding = db
            .conversation_goal_execution_binding(round.execution)
            .await
            .unwrap();
        let counts = effect_counts(&db).await;
        assert_eq!(
            status(&round.commit(&db, replay).await.unwrap().results[0])["reason"],
            "auto_continuation_disabled"
        );
        assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
        round
            .next(
                &db,
                "enable",
                vec![SelfGoalOperation::EnableAutoContinuation],
            )
            .await;
        assert_eq!(
            status(&round.commit(&db, replay).await.unwrap().results[0])["status"],
            "auto_continuation_enabled"
        );
        assert!(enabled(&db, &round.owner).await);
        assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
        let after_enable = effect_counts(&db).await;
        assert_eq!(
            (after_enable.0, after_enable.1, after_enable.2),
            (counts.0, counts.1, counts.2)
        );
        round.next(&db, "create", vec![create()]).await;
        let first = round.commit(&db, replay).await.unwrap();
        assert_eq!(status(&first.results[0])["status"], "goal_created");
        assert!(round.commit(&db, replay).await.unwrap().replayed);
        let after = effect_counts(&db).await;
        assert_eq!(
            (after.0, after.1, after.2),
            (counts.0 + 1, counts.1, counts.2)
        );
        assert_eq!(
            db.conversation_goal_execution_binding(round.execution)
                .await
                .unwrap(),
            binding
        );
    }
}

#[tokio::test]
async fn mixed_self_goal_rounds_refuse_every_intent_without_mutation() {
    for initially_enabled in [false, true] {
        for create_first in [false, true] {
            let db = fixture().await;
            let operations = if create_first {
                vec![create(), SelfGoalOperation::EnableAutoContinuation]
            } else {
                vec![SelfGoalOperation::EnableAutoContinuation, create()]
            };
            let round =
                SelfRound::with_preference(&db, "mixed-round", operations, initially_enabled).await;
            let before = effect_counts(&db).await;
            let results = round.commit(&db, false).await.unwrap().results;
            assert_eq!(results.len(), 2);
            for result in &results {
                assert_eq!(status(result)["status"], "refused");
                assert_eq!(status(result)["reason"], "mixed_self_goal_operations");
            }
            assert_eq!(enabled(&db, &round.owner).await, initially_enabled);
            assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
            let after = effect_counts(&db).await;
            assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
        }
    }
}

#[tokio::test]
async fn refused_mixed_round_replays_its_receipt_after_preference_changes() {
    let db = fixture().await;
    let round = SelfRound::with_preference(
        &db,
        "mixed-replay",
        vec![SelfGoalOperation::EnableAutoContinuation, create()],
        false,
    )
    .await;
    let first = round.commit(&db, true).await.unwrap();
    assert_eq!(
        status(&first.results[0])["reason"],
        "mixed_self_goal_operations"
    );
    db.set_auto_continue_on_context_exhaustion(
        &round.owner,
        crate::AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &round.owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    let counts = effect_counts(&db).await;
    let replayed = round.commit(&db, true).await.unwrap();
    assert!(replayed.replayed);
    assert_eq!(
        serde_json::to_value(&replayed.results).unwrap(),
        serde_json::to_value(&first.results).unwrap()
    );
    assert!(!enabled(&db, &round.owner).await);
    assert_eq!(effect_counts(&db).await, counts);

    let mut changed_source = round;
    let MessageContent::Agent(blocks) = &mut changed_source.assistant.content else {
        unreachable!()
    };
    let ContentBlock::ToolUse { input, .. } = &mut blocks[0] else {
        unreachable!()
    };
    *input = serde_json::json!({"unexpected":true});
    assert!(changed_source.commit(&db, true).await.is_err());
    assert_eq!(effect_counts(&db).await, counts);
}

#[tokio::test]
async fn mixed_self_goal_checkpoint_cut_preserves_atomic_refusal_and_retry() {
    for cut in [
        CanonicalCommitCut::BeforeCanonicalCommit,
        CanonicalCommitCut::AfterCanonicalCommit,
    ] {
        let db = Database::open(":memory:").await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let round = SelfRound::with_preference(
            &db,
            "mixed-cut",
            vec![SelfGoalOperation::EnableAutoContinuation, create()],
            false,
        )
        .await;
        let before = effect_counts(&db).await;
        let state_before: (String, String) =
            sqlx::query_as("SELECT state,state_updated_at FROM conversations WHERE id=?")
                .bind(&round.transcript)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let cut_error = round.cut(&db, cut).await.err().unwrap().to_string();
        let expected_cut = match cut {
            CanonicalCommitCut::BeforeCanonicalCommit => "injected before canonical commit",
            CanonicalCommitCut::AfterCanonicalCommit => "injected after canonical commit",
        };
        assert!(cut_error.contains(expected_cut), "{cut_error}");
        let after = effect_counts(&db).await;
        let retained = matches!(cut, CanonicalCommitCut::AfterCanonicalCommit);
        assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
        assert_eq!(after.3, before.3 + i64::from(retained));
        assert!(!enabled(&db, &round.owner).await);
        if !retained {
            assert!(db.get_messages(&round.transcript).await.unwrap().is_empty());
            assert!(db
                .load_provider_replay_state(&round.transcript)
                .await
                .unwrap()
                .is_none());
            let state_after: (String, String) =
                sqlx::query_as("SELECT state,state_updated_at FROM conversations WHERE id=?")
                    .bind(&round.transcript)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(state_after, state_before);
        }
        let retry = round.commit(&db, true).await.unwrap();
        assert_eq!(retry.replayed, retained);
        assert!(retry
            .results
            .iter()
            .all(|result| { status(result)["reason"] == "mixed_self_goal_operations" }));
        let committed = effect_counts(&db).await;
        assert_eq!(committed.3, before.3 + 1);
        let selected_state: (String, String) =
            sqlx::query_as("SELECT state,state_updated_at FROM conversations WHERE id=?")
                .bind(&round.transcript)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            selected_state,
            (
                serde_json::to_string(&ConvState::Idle).unwrap(),
                round.timestamp.to_rfc3339()
            )
        );
        let ProviderReplayUpdate::Anthropic(expected_replay) = round.replay() else {
            unreachable!()
        };
        assert_eq!(
            db.load_provider_replay_state(&round.transcript)
                .await
                .unwrap()
                .unwrap()
                .response_sets,
            vec![expected_replay]
        );
        let messages = db.get_messages(&round.transcript).await.unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages
                .iter()
                .map(|message| message.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(!enabled(&db, &round.owner).await);
        assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
        assert!(round.commit(&db, true).await.unwrap().replayed);
        assert_eq!(effect_counts(&db).await, committed);
    }
}

#[tokio::test]
async fn malformed_later_mixed_source_rolls_back_first_application() {
    let db = fixture().await;
    let mut round = SelfRound::with_preference(
        &db,
        "mixed-malformed-later",
        vec![SelfGoalOperation::EnableAutoContinuation, create()],
        false,
    )
    .await;
    let ToolRoundResult::PendingSelfGoal { pending, .. } = &mut round.results[1] else {
        unreachable!()
    };
    pending.operation = SelfGoalOperation::Create {
        objective: text("mismatched later objective"),
    };
    let before = effect_counts(&db).await;
    let message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let error = round.commit(&db, false).await.err().unwrap().to_string();
    assert!(
        error.contains("self-goal source name or arguments"),
        "{error}"
    );
    assert_eq!(effect_counts(&db).await, before);
    let after_message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(after_message_count, message_count);
    assert!(!enabled(&db, &round.owner).await);
    assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
}

#[tokio::test]
async fn repeated_human_false_and_aba_disable_win_and_refused_receipts_remain_permanent() {
    for aba in [false, true] {
        for replay in [false, true] {
            let db = fixture().await;
            let round = SelfRound::with_preference(
                &db,
                "disable-order",
                vec![SelfGoalOperation::EnableAutoContinuation, create()],
                false,
            )
            .await;
            let before_epoch: i64 = sqlx::query_scalar(
                "SELECT auto_continue_disable_epoch FROM product_conversations WHERE id=?",
            )
            .bind(round.owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
            for _ in 0..2 {
                db.set_auto_continue_on_context_exhaustion(
                    &round.owner,
                    crate::AutoContinueOnContextExhaustion::Disabled,
                )
                .await
                .unwrap();
            }
            if aba {
                db.set_auto_continue_on_context_exhaustion(
                    &round.owner,
                    crate::AutoContinueOnContextExhaustion::Enabled,
                )
                .await
                .unwrap();
            }
            let epoch: i64 = sqlx::query_scalar(
                "SELECT auto_continue_disable_epoch FROM product_conversations WHERE id=?",
            )
            .bind(round.owner.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(epoch, before_epoch + 2);
            let first = round.commit(&db, replay).await.unwrap();
            for result in &first.results {
                assert_eq!(status(result)["reason"], "human_disable_since_admission");
            }
            assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
            db.set_auto_continue_on_context_exhaustion(
                &round.owner,
                crate::AutoContinueOnContextExhaustion::Enabled,
            )
            .await
            .unwrap();
            let counts = effect_counts(&db).await;
            let retried = round.commit(&db, replay).await.unwrap();
            assert!(retried.replayed);
            assert_eq!(
                serde_json::to_value(&retried.results).unwrap(),
                serde_json::to_value(&first.results).unwrap()
            );
            assert_eq!(effect_counts(&db).await, counts);
        }
    }
}

#[tokio::test]
async fn human_disable_after_success_survives_replay_without_reenabling() {
    let db = fixture().await;
    let round = SelfRound::with_preference(
        &db,
        "enable-first",
        vec![SelfGoalOperation::EnableAutoContinuation],
        false,
    )
    .await;
    round.commit(&db, true).await.unwrap();
    db.set_auto_continue_on_context_exhaustion(
        &round.owner,
        crate::AutoContinueOnContextExhaustion::Disabled,
    )
    .await
    .unwrap();
    assert!(round.commit(&db, true).await.unwrap().replayed);
    assert!(!enabled(&db, &round.owner).await);
}

#[tokio::test]
async fn forged_pending_source_is_not_an_argument_error_or_a_finalized_success() {
    for field in [
        "owner",
        "execution",
        "transcript",
        "message",
        "call",
        "objective",
        "name",
        "arguments",
    ] {
        let db = fixture().await;
        let mut round = SelfRound::new(&db, field, vec![create()]).await;
        let before = effect_counts(&db).await;
        match field {
            "owner" => {
                round.pending_mut().owner = ProductConversationId::parse("other-owner").unwrap();
            }
            "execution" => {
                let execution = GoalExecution::parse(
                    round.execution.workflow_id(),
                    round.execution.turn_id() + 100,
                )
                .unwrap();
                round.pending_mut().execution = execution;
            }
            "transcript" => round.pending_mut().call.transcript_id = text("other-transcript"),
            "message" => round.pending_mut().call.message_id = text("other-message"),
            "call" => round.pending_mut().call.tool_use_id = text("other-call"),
            "objective" => {
                round.pending_mut().operation = SelfGoalOperation::Create {
                    objective: text("forged objective"),
                }
            }
            "name" | "arguments" => {
                let MessageContent::Agent(blocks) = &mut round.assistant.content else {
                    unreachable!()
                };
                let ContentBlock::ToolUse { name, input, .. } = &mut blocks[0] else {
                    unreachable!()
                };
                if field == "name" {
                    *name = "enable_auto_continuation".into();
                } else {
                    *input = serde_json::json!({"objective":"different"});
                }
            }
            _ => unreachable!(),
        }
        assert!(round.commit(&db, false).await.is_err(), "{field}");
        assert_eq!(effect_counts(&db).await, before, "{field}");
    }
    for name in ["create_goal", "enable_auto_continuation"] {
        for success in [false, true] {
            let db = fixture().await;
            let mut round = SelfRound::new(&db, name, vec![create()]).await;
            let MessageContent::Agent(blocks) = &mut round.assistant.content else {
                unreachable!()
            };
            let ContentBlock::ToolUse {
                name: source_name,
                input,
                ..
            } = &mut blocks[0]
            else {
                unreachable!()
            };
            *source_name = name.into();
            *input = serde_json::json!({"unexpected":true});
            round.results = vec![if success {
                ToolResult::success("call-0".into(), "forged".into())
            } else {
                ToolResult::error("call-0".into(), "invalid arguments".into())
            }
            .into()];
            round.install_state(&db).await;
            assert_eq!(round.commit(&db, false).await.is_ok(), !success);
            assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
        }
    }
}

#[allow(clippy::too_many_lines)] // Exercises all stale and stopped execution refusal paths together.
#[tokio::test]
async fn stale_generation_stop_pause_close_and_continuation_refuse_without_effects() {
    for mutation in [
        "pending-generation",
        "workflow-generation",
        "admission-generation",
        "admission-control",
        "stop",
        "pause",
        "close",
        "continuation",
        "runtime",
    ] {
        let db = fixture().await;
        let mut round = SelfRound::new(&db, mutation, vec![create()]).await;
        match mutation {
            "pending-generation" => round.pending_mut().generation += 1,
            "workflow-generation" => {
                sqlx::query("UPDATE workflows SET generation=generation+1 WHERE workflow_id=?")
                    .bind(round.execution.workflow_id())
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "admission-generation" => {
                sqlx::query("UPDATE ordinary_self_goal_admissions SET generation=generation+1 WHERE turn_id=?").bind(round.execution.turn_id()).execute(db.pool()).await.unwrap();
            }
            "admission-control" | "stop" => {
                let current = db
                    .ordinary_goal_control(&round.owner)
                    .await
                    .unwrap()
                    .unwrap();
                db.apply_goal_control(
                    &ControlRequest {
                        owner: round.owner.clone(),
                        principal_id: Uuid::nil(),
                        request_id: Uuid::new_v4(),
                        command: if mutation == "stop" {
                            ControlCommand::Stop
                        } else {
                            ControlCommand::Release
                        },
                        expected_control_version: current.version(),
                    },
                    at(),
                    || panic!("lost authority"),
                )
                .await
                .established()
                .unwrap()
                .unwrap();
            }
            "pause" => {
                db.update_conversation_state_at(
                    &round.transcript,
                    &ConvState::Idle,
                    round.timestamp,
                )
                .await
                .unwrap();
            }
            "close" => {
                db.begin_close_foundation(
                    &round.owner,
                    &phoenix_core::domain::close::TranscriptConversationId::parse(
                        round.transcript.clone(),
                    )
                    .unwrap(),
                    "self-goal-close",
                )
                .await
                .unwrap();
            }
            "continuation" => {
                db.update_conversation_state_at(
                    &round.transcript,
                    &ConvState::ContextExhausted {
                        summary: "continue approved work".into(),
                    },
                    round.timestamp,
                )
                .await
                .unwrap();
                assert!(matches!(
                    db.continue_conversation(&round.transcript).await.unwrap(),
                    crate::ContinueOutcome::Created(_)
                ));
                round.install_state(&db).await;
            }
            "runtime" => {
                sqlx::query("UPDATE workflows SET status='Completed' WHERE workflow_id=?")
                    .bind(round.execution.workflow_id())
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = effect_counts(&db).await;
        let committed = round.commit(&db, false).await.unwrap();
        assert_eq!(
            status(&committed.results[0])["status"],
            "refused",
            "{mutation}"
        );
        if mutation == "continuation" {
            assert_eq!(
                status(&committed.results[0])["reason"],
                "transcript_changed"
            );
        }
        assert!(db.conversation_goal(&round.owner).await.unwrap().is_none());
        let after = effect_counts(&db).await;
        assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
    }
}

#[tokio::test]
async fn every_existing_goal_disposition_refuses_creation_and_enable_does_not_reactivate() {
    for disposition in [
        GoalDisposition::Underway,
        GoalDisposition::Blocked(text("blocked")),
        GoalDisposition::Suspended(text("suspended")),
        GoalDisposition::Complete(text("complete")),
        GoalDisposition::Exhausted(text("exhausted")),
    ] {
        let db = fixture().await;
        let (transcript, owner) = ordinary(&db, "existing-goal").await;
        let first = db
            .apply_conversation_goal(
                &super::create(&owner, GoalVersion::INITIAL, "existing", "existing-create"),
                &transcript,
                at(),
            )
            .await
            .unwrap();
        if !matches!(disposition, GoalDisposition::Underway) {
            let actor = GoalActor::GeneratedHost {
                action_id: text("existing-state"),
            };
            db.apply_conversation_goal(
                &GoalMutation {
                    owner: owner.clone(),
                    expected_version: first.snapshot().version(),
                    action_id: text("existing-state"),
                    attribution: GoalAttribution {
                        requested_by: actor.clone(),
                        applied_by: actor,
                    },
                    change: GoalChange::SetDisposition {
                        goal_id: first.snapshot().goal_id().clone(),
                        disposition,
                    },
                },
                &transcript,
                at(),
            )
            .await
            .unwrap();
        }
        let round = SelfRound::admit(&db, transcript, owner, "existing-turn", vec![create()]).await;
        let before = db.conversation_goal(&round.owner).await.unwrap();
        let counts = effect_counts(&db).await;
        let committed = round.commit(&db, true).await.unwrap();
        assert_eq!(
            status(&committed.results[0])["reason"],
            "goal_already_exists"
        );
        let mut enable = round.clone();
        enable
            .next(
                &db,
                "existing-goal-enable",
                vec![SelfGoalOperation::EnableAutoContinuation],
            )
            .await;
        assert_eq!(enable.execution, round.execution);
        let enabled_result = enable.commit(&db, true).await.unwrap();
        assert_eq!(
            status(&enabled_result.results[0])["status"],
            "auto_continuation_enabled"
        );
        assert_eq!(db.conversation_goal(&round.owner).await.unwrap(), before);
        assert!(round.commit(&db, true).await.unwrap().replayed);
        assert!(enable.commit(&db, true).await.unwrap().replayed);
        let after = effect_counts(&db).await;
        assert_eq!((after.0, after.1, after.2), (counts.0, counts.1, counts.2));
    }
    let db = fixture().await;
    let round = SelfRound::new(&db, "same-round-race", vec![create(), create()]).await;
    let committed = round.commit(&db, true).await.unwrap();
    assert_eq!(status(&committed.results[0])["status"], "goal_created");
    assert_eq!(
        status(&committed.results[1])["reason"],
        "goal_already_exists"
    );
    assert_eq!(effect_counts(&db).await.0, 1);
}

#[tokio::test]
async fn real_lost_acknowledgements_reopen_and_replay_without_duplicate_effects() {
    for cut in [
        CanonicalCommitCut::BeforeCanonicalCommit,
        CanonicalCommitCut::AfterCanonicalCommit,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("self-goal.sqlite");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let db = db
            .install_goal_controls_for_tests()
            .await
            .unwrap()
            .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
            .await
            .unwrap();
        let round = SelfRound::new(&db, "lost-ack", vec![create()]).await;
        let before = effect_counts(&db).await;
        assert!(round.cut(&db, cut).await.is_err());
        let retained = matches!(cut, CanonicalCommitCut::AfterCanonicalCommit);
        let after = effect_counts(&db).await;
        assert_eq!(after.0, before.0 + i64::from(retained));
        assert_eq!(after.3, before.3 + i64::from(retained));
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap())
            .await
            .unwrap()
            .install_goal_controls_for_tests()
            .await
            .unwrap()
            .install_goal_execution_for_tests(GoalRequestLimit::parse(2).unwrap())
            .await
            .unwrap();
        let committed = round.commit(&reopened, true).await.unwrap();
        assert_eq!(committed.replayed, retained);
        assert_eq!(status(&committed.results[0])["status"], "goal_created");
        let counts = effect_counts(&reopened).await;
        assert_eq!(counts.0, before.0 + 1);
        assert!(round.commit(&reopened, true).await.unwrap().replayed);
        assert_eq!(effect_counts(&reopened).await, counts);
    }
}

#[tokio::test]
async fn retained_self_results_validate_full_canonical_shape_and_pending_evidence() {
    for field in [
        "status",
        "error",
        "duration",
        "display",
        "images",
        "identity",
        "pending-generation",
        "pending-duration",
    ] {
        let db = fixture().await;
        let mut round = SelfRound::new(&db, field, vec![create()]).await;
        let committed = round.commit(&db, true).await.unwrap();
        if field == "pending-generation" {
            round.pending_mut().generation += 1;
        } else if field == "pending-duration" {
            let ToolRoundResult::PendingSelfGoal { duration_ms, .. } = &mut round.results[0] else {
                unreachable!()
            };
            *duration_ms = Some(99);
        } else {
            let mut content = committed.results[0].content.to_json();
            match field {
                "status" => {
                    content["content"] =
                        serde_json::json!("{\"status\":\"auto_continuation_enabled\"}");
                }
                "error" => content["is_error"] = serde_json::json!(true),
                "duration" | "display" => {}

                "images" => {
                    content["images"] =
                        serde_json::json!([{"media_type":"image/png","data":"AA=="}]);
                }
                "identity" => content["tool_use_id"] = serde_json::json!("other-call"),
                _ => unreachable!(),
            }
            if matches!(field, "duration" | "display") {
                let mut display = committed.results[0].display_data.clone().unwrap();
                if field == "duration" {
                    display["duration_ms"] = serde_json::json!(99);
                } else {
                    display["tool_display"] = serde_json::json!({"forged":true});
                }
                sqlx::query(
                    "UPDATE messages SET display_data=? WHERE conversation_id=? AND message_id=?",
                )
                .bind(serde_json::to_string(&display).unwrap())
                .bind(&round.transcript)
                .bind(&committed.results[0].message_id)
                .execute(db.pool())
                .await
                .unwrap();
            } else {
                sqlx::query(
                    "UPDATE messages SET content=? WHERE conversation_id=? AND message_id=?",
                )
                .bind(serde_json::to_string(&content).unwrap())
                .bind(&round.transcript)
                .bind(&committed.results[0].message_id)
                .execute(db.pool())
                .await
                .unwrap();
            }
        }
        let counts = effect_counts(&db).await;
        assert!(round.commit(&db, true).await.is_err(), "{field}");
        assert_eq!(effect_counts(&db).await, counts);
    }
}

#[tokio::test]
async fn deleting_ordinary_conversation_cascades_its_runtime_admission() {
    let db = Database::open_in_memory().await.unwrap();
    let (transcript, _owner) = ordinary(&db, "delete-admitted-conversation").await;
    let execution = accept_execution(
        &db.workflow_repository(),
        &runtime_input(&transcript, "delete-turn"),
    )
    .await;

    let admission_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ordinary_self_goal_admissions WHERE turn_id=?")
            .bind(execution.turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(admission_count, 1, "the accepted ordinary turn is admitted");

    db.delete_conversation(&transcript).await.unwrap();

    let remaining_admissions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ordinary_self_goal_admissions")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(remaining_admissions, 0);
}
