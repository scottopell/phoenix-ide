use super::*;
use chrono::Utc;
use phoenix_core::domain::{
    conversation_goal::*,
    db_schema::{InputOrigin, MessageContent},
    llm_types::ContentBlock,
    sm_state::ConvState,
};
use phoenix_db::Message;

use super::testing::{fixture, Fixture};

fn request(fixture: &Fixture) -> GoalManagementRequest {
    GoalManagementRequest::Create {
        owner: fixture.target.clone(),
        expected_version: GoalVersion::INITIAL,
        objective: GoalText::parse("Coordinator-owned objective").unwrap(),
    }
}

async fn round(fixture: &Fixture) -> (Message, Vec<ToolRoundResult>) {
    let request = request(fixture);
    let intent = fixture
        .service
        .prepare(
            &fixture.source,
            &fixture.active,
            "management-assistant",
            "manage-call",
            request.clone(),
        )
        .await
        .unwrap();
    let content = MessageContent::agent(vec![ContentBlock::ToolUse {
        id: "manage-call".into(),
        name: "goal_manage".into(),
        input: serde_json::to_value(request).unwrap(),
    }]);
    let assistant = Message {
        origin: InputOrigin::SystemGenerated,
        message_id: "management-assistant".into(),
        conversation_id: fixture.source.clone(),
        sequence_id: 2,
        message_type: content.message_type(),
        content,
        display_data: None,
        usage_data: None,
        created_at: Utc::now(),
    };
    (
        assistant,
        vec![ToolRoundResult::PendingGoalManagement {
            management: intent,
            duration_ms: 3,
        }],
    )
}

fn round_checkpoint<'a>(
    assistant: &'a Message,
    results: &'a [ToolRoundResult],
    state: &'a ConvState,
) -> GoalRoundCheckpoint<'a> {
    let ToolRoundResult::PendingGoalManagement { management, .. } = &results[0] else {
        panic!("intent")
    };
    GoalRoundCheckpoint {
        assistant,
        execution: management.source().execution,
        results,
        state,
        state_updated_at: assistant.created_at,
    }
}

fn established(value: TerminalMutationEstablishment<CommittedGoalRound>) -> CommittedGoalRound {
    let TerminalMutationEstablishment::Established { value, .. } = value else {
        panic!("established checkpoint")
    };
    value
}

#[tokio::test]
async fn conversation_goal_management_coordinator_checkpoint_is_atomic_and_historical_retry_bypasses_gates(
) {
    let fixture = fixture(true).await;
    let (assistant, results) = round(&fixture).await;
    let db = fixture.runtime.db();
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 1);
    let state = ConvState::LlmRequesting { attempt: 1 };
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let committed = established(
        fixture
            .service
            .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
            .await,
    );
    assert_eq!(committed.results.len(), 1);
    let MessageContent::Tool(output) = &committed.results[0].content else {
        panic!("output")
    };
    assert!(!output.is_error);
    let history = db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(matches!(
        history[0].mutation.attribution.requested_by,
        GoalActor::Coordinator(_)
    ));
    assert_eq!(
        history[0].mutation.attribution.requested_by,
        history[0].mutation.attribution.applied_by
    );
    let _product = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let _transcript = fixture
        .runtime
        .lock_message_acceptance("management-target")
        .await;
    let retried = established(
        fixture
            .service
            .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
            .await,
    );
    assert!(retried.replayed);
    assert_eq!(
        serde_json::to_value(&retried.results).unwrap(),
        serde_json::to_value(&committed.results).unwrap()
    );
    assert_eq!(
        db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
            .await
            .unwrap(),
        history
    );
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 3);
    let mut changed = assistant.clone();
    let MessageContent::Agent(blocks) = &mut changed.content else {
        panic!("assistant")
    };
    let ContentBlock::ToolUse { input, .. } = &mut blocks[0] else {
        panic!("call")
    };
    input["objective"] = serde_json::json!("changed");
    assert!(matches!(
        fixture
            .service
            .checkpoint(
                &round_checkpoint(&changed, &results, &state),
                GoalManagementRoundKind::Runtime,
                None
            )
            .await,
        TerminalMutationEstablishment::Unclassifiable(_)
    ));
}

#[tokio::test]
async fn conversation_goal_management_gate_refusal_is_canonical_and_retained_after_gate_release() {
    for product_gate in [true, false] {
        let fixture = fixture(true).await;
        let (assistant, results) = round(&fixture).await;
        let guard = if product_gate {
            fixture
                .runtime
                .lock_product_message_admission(fixture.target.as_str())
                .await
        } else {
            fixture
                .runtime
                .lock_message_acceptance("management-target")
                .await
        };
        let state = ConvState::LlmRequesting { attempt: 1 };
        let checkpoint = round_checkpoint(&assistant, &results, &state);
        let refused = established(
            fixture
                .service
                .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
                .await,
        );
        let MessageContent::Tool(output) = &refused.results[0].content else {
            panic!("output")
        };
        assert!(output.is_error);
        assert!(fixture
            .runtime
            .db()
            .conversation_goal(&fixture.target)
            .await
            .unwrap()
            .is_none());
        drop(guard);
        let retry = established(
            fixture
                .service
                .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
                .await,
        );
        assert_eq!(
            serde_json::to_value(retry.results).unwrap(),
            serde_json::to_value(refused.results).unwrap()
        );
        assert!(fixture
            .runtime
            .db()
            .conversation_goal(&fixture.target)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn conversation_goal_management_source_requires_real_canonical_coordinator_and_generation() {
    let ordinary = fixture(false).await;
    assert!(ordinary
        .service
        .prepare(
            &ordinary.source,
            &ordinary.active,
            "assistant",
            "call",
            request(&ordinary)
        )
        .await
        .is_err());
    let coordinator = fixture(true).await;
    let mut stale = coordinator.active.clone();
    stale.generation += 1;
    assert!(coordinator
        .service
        .prepare(
            &coordinator.source,
            &stale,
            "assistant",
            "call",
            request(&coordinator)
        )
        .await
        .is_err());
    assert!(coordinator
        .runtime
        .db()
        .conversation_goal(&coordinator.target)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn conversation_goal_management_state_failure_rolls_back_goal_history_and_entire_round() {
    let fixture = fixture(true).await;
    let (assistant, results) = round(&fixture).await;
    let db = fixture.runtime.db();
    let before = serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap();
    sqlx::query("CREATE TRIGGER fail_management_state BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'management state failed'); END")
        .execute(db.pool()).await.unwrap();
    let state = ConvState::LlmRequesting { attempt: 1 };
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    assert!(matches!(
        fixture
            .service
            .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
            .await,
        TerminalMutationEstablishment::KnownNotCommitted(_)
    ));
    assert_eq!(
        serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap(),
        before
    );
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DROP TRIGGER fail_management_state")
        .execute(db.pool())
        .await
        .unwrap();
    let accepted = established(
        fixture
            .service
            .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, None)
            .await,
    );
    assert!(!accepted.replayed);
    assert_eq!(
        db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn conversation_goal_management_cancelled_checkpoint_commits_target_pair_state_and_turn_together(
) {
    use crate::runtime::traits::{
        terminal_obligation, ActiveDirectTurnSettlement, ActiveDirectTurnTerminal,
    };
    let fixture = fixture(true).await;
    let (assistant, results) = round(&fixture).await;
    let state = ConvState::Idle;
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let settlement = ActiveDirectTurnSettlement {
        turn: fixture.active.clone(),
        conversation_id: fixture.source.clone(),
        terminal: ActiveDirectTurnTerminal::Cancelled,
        state: state.clone(),
        state_updated_at: checkpoint.state_updated_at,
        execution_occurrence_message_id: None,
    };
    let obligation = terminal_obligation(&settlement, None);
    let db = fixture.runtime.db();
    let before = serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap();
    sqlx::query("CREATE TRIGGER fail_management_terminal BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'management terminal failed'); END")
        .execute(db.pool()).await.unwrap();
    assert!(matches!(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Terminal(&obligation),
                None
            )
            .await,
        TerminalMutationEstablishment::KnownNotCommitted(_)
    ));
    assert_eq!(
        serde_json::to_value(db.get_messages(&fixture.source).await.unwrap()).unwrap(),
        before
    );
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    let ownership: (Option<String>, i64) =
        sqlx::query_as("SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?")
            .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(ownership, (None, 1));
    sqlx::query("DROP TRIGGER fail_management_terminal")
        .execute(db.pool())
        .await
        .unwrap();
    let committed = established(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Terminal(&obligation),
                None,
            )
            .await,
    );
    let ownership: (Option<String>, i64) =
        sqlx::query_as("SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?")
            .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(ownership, (Some("Cancelled".into()), 0));
    let source = db.get_conversation(&fixture.source).await.unwrap();
    assert_eq!(source.state, state);
    assert_eq!(source.state_updated_at, checkpoint.state_updated_at);
    let _guard = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let replayed = established(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Terminal(&obligation),
                None,
            )
            .await,
    );
    assert!(replayed.replayed);
    assert_eq!(
        serde_json::to_value(committed.results).unwrap(),
        serde_json::to_value(replayed.results).unwrap()
    );
    assert_eq!(
        db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn conversation_goal_management_terminal_refusal_replays_without_new_live_admission() {
    use crate::runtime::traits::{
        terminal_obligation, ActiveDirectTurnSettlement, ActiveDirectTurnTerminal,
    };
    let fixture = fixture(true).await;
    let (assistant, results) = round(&fixture).await;
    let state = ConvState::Idle;
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let settlement = ActiveDirectTurnSettlement {
        turn: fixture.active.clone(),
        conversation_id: fixture.source.clone(),
        terminal: ActiveDirectTurnTerminal::Cancelled,
        state: state.clone(),
        state_updated_at: checkpoint.state_updated_at,
        execution_occurrence_message_id: None,
    };
    let obligation = terminal_obligation(&settlement, None);
    let _guard = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let first = established(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Terminal(&obligation),
                None,
            )
            .await,
    );
    let MessageContent::Tool(output) = &first.results[0].content else {
        panic!("refused result")
    };
    assert!(output.is_error);
    let replayed = established(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Terminal(&obligation),
                None,
            )
            .await,
    );
    assert!(replayed.replayed);
    assert_eq!(
        serde_json::to_value(first.results).unwrap(),
        serde_json::to_value(replayed.results).unwrap()
    );
    assert!(fixture
        .runtime
        .db()
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .runtime
        .db()
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn conversation_goal_management_fresh_closed_target_commits_a_refusal_and_replays() {
    for history in [false, true] {
        for terminal in [false, true] {
            closed_target_refusal(history, terminal).await;
        }
    }
}

async fn closed_target_refusal(history: bool, terminal: bool) {
    use crate::runtime::traits::{
        terminal_obligation, ActiveDirectTurnSettlement, ActiveDirectTurnTerminal,
    };
    let fixture = fixture(true).await;
    let (assistant, results) = round(&fixture).await;
    let db = fixture.runtime.db();
    close_target(&fixture, history).await;
    assert_target_gates_are_free(&fixture).await;
    let state = if terminal {
        ConvState::Idle
    } else {
        ConvState::LlmRequesting { attempt: 1 }
    };
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let settlement = ActiveDirectTurnSettlement {
        turn: fixture.active.clone(),
        conversation_id: fixture.source.clone(),
        terminal: ActiveDirectTurnTerminal::Cancelled,
        state: state.clone(),
        state_updated_at: checkpoint.state_updated_at,
        execution_occurrence_message_id: None,
    };
    let obligation = terminal_obligation(&settlement, None);
    let kind = if terminal {
        GoalManagementRoundKind::Terminal(&obligation)
    } else {
        GoalManagementRoundKind::Runtime
    };
    let first = established(fixture.service.checkpoint(&checkpoint, kind, None).await);
    let MessageContent::Tool(output) = &first.results[0].content else {
        panic!("refused result")
    };
    assert!(output.is_error);
    assert!(output.content.contains("Goal management refused"));
    assert!(!first.replayed);
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
    let source = db.get_conversation(&fixture.source).await.unwrap();
    assert_eq!(source.state, state);
    assert_eq!(source.state_updated_at, checkpoint.state_updated_at);
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 3);
    let ownership: (Option<String>, i64) =
        sqlx::query_as("SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?")
            .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        ownership,
        if terminal {
            (Some("Cancelled".into()), 0)
        } else {
            (None, 1)
        }
    );
    let _product = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let _transcript = fixture
        .runtime
        .lock_message_acceptance("management-target")
        .await;
    let replayed = established(fixture.service.checkpoint(&checkpoint, kind, None).await);
    assert!(replayed.replayed);
    assert_eq!(
        serde_json::to_value(first.results).unwrap(),
        serde_json::to_value(replayed.results).unwrap()
    );
    assert_eq!(db.get_messages(&fixture.source).await.unwrap().len(), 3);
    assert!(db
        .conversation_goal(&fixture.target)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap()
        .is_empty());
}

async fn close_target(fixture: &Fixture, history: bool) {
    use phoenix_core::domain::close::TranscriptConversationId;
    let db = fixture.runtime.db();
    if history {
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id=?")
            .bind(fixture.target.as_str())
            .execute(db.pool())
            .await
            .unwrap();
    } else {
        db.begin_direct_close_foundation(
            &fixture.target,
            &TranscriptConversationId::parse("management-target").unwrap(),
            "management-close",
        )
        .await
        .unwrap();
    }
}

async fn assert_target_gates_are_free(fixture: &Fixture) {
    // The gates are free: the refusal must come from transactional lifecycle admission.
    drop(
        fixture
            .runtime
            .try_lock_product_message_admission(fixture.target.as_str())
            .await
            .unwrap(),
    );
    drop(
        fixture
            .runtime
            .try_lock_message_acceptance("management-target")
            .await
            .unwrap(),
    );
}

#[test]
fn conversation_goal_management_schema_exposes_tagged_state_objects_not_string_dispositions() {
    let definition = GoalManagementService::definition();
    let schema = &definition.input_schema["oneOf"][1]["properties"]["disposition"];
    assert_eq!(schema["type"], "object");
    assert_eq!(
        schema["oneOf"][0]["properties"]["state"]["enum"],
        serde_json::json!(["underway"])
    );
    assert_eq!(schema["oneOf"][0]["required"], serde_json::json!(["state"]));
    assert_eq!(
        schema["oneOf"][1]["properties"]["state"]["enum"],
        serde_json::json!(["blocked", "complete", "exhausted"])
    );
    assert_eq!(
        schema["oneOf"][1]["required"],
        serde_json::json!(["state", "reason"])
    );
    for state in ["underway", "blocked", "complete", "exhausted"] {
        let disposition = if state == "underway" {
            serde_json::json!({"state":state})
        } else {
            serde_json::json!({"state":state,"reason":"verified reason"})
        };
        let request = serde_json::json!({"op":"state","owner":"target","expected_version":1,
            "goal_id":"goal","disposition":disposition});
        assert!(serde_json::from_value::<GoalManagementRequest>(request).is_ok());
        let string_request = serde_json::json!({"op":"state","owner":"target","expected_version":1,
            "goal_id":"goal","disposition":state});
        assert!(serde_json::from_value::<GoalManagementRequest>(string_request).is_err());
    }
}
