use phoenix_core::domain::db_schema::{FinalizedToolResult, InputOrigin, Message, MessageContent, MessageType, ToolResult};

fn initial_result_round(
    command: &NativeCloseCommand,
    run: &CloseRunRef,
) -> (Message, FinalizedToolResult) {
    let now = chrono::Utc::now();
    let assistant = Message {
        message_id: command.source().message_id.clone(),
        origin: InputOrigin::SystemGenerated,
        conversation_id: command.source_transcript_id().to_owned(),
        sequence_id: 1,
        message_type: MessageType::Agent,
        content: MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: command.source().tool_use_id.clone(),
            name: "close_conversation".into(),
            input: command.source_input(),
        }]),
        display_data: None,
        usage_data: None,
        created_at: now,
    };
    let output = serde_json::json!({
        "action": "initial", "status": "newly_admitted", "request_id": command.request_id(),
        "admitted": {"product_conversation_id": command.intent().target().as_str(),
            "attempt_id": run.attempt_id.as_str(), "run_ordinal": run.ordinal}
    });
    let result = FinalizedToolResult::new(
        &assistant.message_id, &assistant.conversation_id, 2, now,
        &ToolResult::success(command.source().tool_use_id.clone(), output.to_string()),
    );
    (assistant, result)
}

async fn initial_ack_counts(db: &Database, run: &CloseRunRef) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT
        (SELECT COUNT(*) FROM native_close_initial_result_acknowledgements WHERE attempt_id = ?1),
        (SELECT suppressed FROM close_completion_retry_allowances WHERE attempt_id = ?1),
        (SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1),
        (SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1)")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap()
}

async fn assert_initial_completion_origin_revoked(db: &Database, run: &CloseRunRef) {
    let facts: (i64, i64) = sqlx::query_as("SELECT
        (SELECT COUNT(*) FROM native_close_completion_retry_source_revocations WHERE attempt_id = ?1),
        (SELECT COUNT(*) FROM native_close_completion_retry_admissions WHERE attempt_id = ?1)")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(facts, (1, 0));
}

#[tokio::test]
async fn canonical_initial_result_acknowledgement_preserves_loss_retry_origin() {
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, result) = initial_result_round(&command, &run);
    let checkpoint_at = chrono::Utc::now();
    db.persist_tool_round_and_state(
        command.source_transcript_id(),
        &assistant,
        std::slice::from_ref(&result),
        &ConvState::Idle,
        checkpoint_at,
    )
    .await
    .unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
    assert_initial_completion_origin_revoked(&db, &run).await;
    db.read_native_close_captured_loss_retry_context(command.intent().target(), &run)
        .await.unwrap();
    // The canonical result replay cannot mint another acknowledgement or run.
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[result], &ConvState::Idle, checkpoint_at).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
    db.update_conversation_state(command.source_transcript_id(), &ConvState::Idle).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
}

#[tokio::test]
async fn pending_canonical_round_acknowledges_finalized_initial_close_result() {
    use phoenix_core::domain::conversation_goal::{GoalExecution, ToolRoundResult};
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, result) = initial_result_round(&command, &run);
    let MessageContent::Tool(tool) = &result.row().content else { panic!("tool result"); };
    let results = [ToolRoundResult::Finalized(ToolResult::success(tool.tool_use_id.clone(), tool.content.clone()))];
    let checkpoint = crate::conversation_goals::GoalRoundCheckpoint {
        assistant: &assistant,
        execution: GoalExecution::parse(1, 1).unwrap(),
        results: &results,
        state: &ConvState::Idle,
        state_updated_at: chrono::Utc::now(),
    };
    db.persist_canonical_checkpoint(&checkpoint, None).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
    db.read_native_close_captured_loss_retry_context(command.intent().target(), &run).await.unwrap();
    db.persist_canonical_checkpoint(&checkpoint, None).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
}

#[tokio::test]
async fn acknowledged_initial_result_evidence_is_immutable() {
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, result) = initial_result_round(&command, &run);
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[result],
        &ConvState::Idle, chrono::Utc::now()).await.unwrap();
    for statement in [
        "UPDATE native_close_initial_result_acknowledgements SET result_message_id = 'replacement'",
        "DELETE FROM native_close_initial_result_acknowledgements",
        "INSERT OR REPLACE INTO native_close_initial_result_acknowledgements SELECT * FROM native_close_initial_result_acknowledgements",
    ] {
        assert!(sqlx::query(statement).execute(db.pool()).await.is_err(), "{statement}");
    }
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
}

#[tokio::test]
async fn stopped_source_before_initial_result_cannot_be_acknowledged_or_resurrected() {
    let (db, command, run) = retained_loss_origin().await;
    db.update_conversation_state(command.source_transcript_id(), &ConvState::Idle)
        .await.unwrap();
    let (assistant, result) = initial_result_round(&command, &run);
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[result], &ConvState::Idle, chrono::Utc::now()).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (0, 1, 1, 0));
    assert!(db.read_native_close_captured_loss_retry_context(command.intent().target(), &run)
        .await.is_err());
}

#[tokio::test]
async fn wrong_initial_result_identity_cannot_acknowledge_retained_origin() {
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, _) = initial_result_round(&command, &run);
    let wrong = FinalizedToolResult::new(&assistant.message_id, &assistant.conversation_id,
        2, chrono::Utc::now(), &ToolResult::success(command.source().tool_use_id.clone(),
            serde_json::json!({"action":"initial", "status":"newly_admitted",
                "request_id":command.request_id(), "admitted": {
                    "product_conversation_id":"wrong-target", "attempt_id":run.attempt_id.as_str(),
                    "run_ordinal":run.ordinal}}).to_string()));
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[wrong],
        &ConvState::Idle, chrono::Utc::now()).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (0, 1, 1, 0));
}

#[tokio::test]
async fn initial_result_acknowledgement_rolls_back_with_canonical_transaction() {
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, result) = initial_result_round(&command, &run);
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let canonical = crate::conversation_goals::round::write_round_tx(&mut tx, command.source_transcript_id(), &assistant,
        crate::conversation_goals::round::ToolRoundWrite::Finalized(std::slice::from_ref(&result)),
        &ConvState::Idle, chrono::Utc::now(), db.goal_write_policies()).await.unwrap();
    super::super::source_fence::acknowledge_initial_result_tx(&mut tx,
        command.source_transcript_id(), &assistant, &canonical.results).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_close_initial_result_acknowledgements")
        .fetch_one(&mut *tx).await.unwrap();
    assert_eq!(count, 1);
    tx.rollback().await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (0, 0, 1, 0));
    db.read_native_close_captured_loss_retry_context(command.intent().target(), &run)
        .await.unwrap();
}

#[tokio::test]
async fn serial_tool_progress_preserves_initial_origin_until_canonical_acknowledgement() {
    use phoenix_core::domain::conversation_goal::ToolRoundResult;
    let (db, command, run) = retained_loss_origin().await;
    let (mut assistant, result) = initial_result_round(&command, &run);
    let MessageContent::Tool(row) = &result.row().content else { panic!("tool result"); };
    let completed = ToolResult::success(row.tool_use_id.clone(), row.content.clone());
    let next_tool = ToolCall::new("after-close", ToolInput::Unknown {
        name: "next_tool".into(), input: serde_json::json!({}),
    });
    let MessageContent::Agent(blocks) = &mut assistant.content else { panic!("assistant"); };
    blocks.push(ContentBlock::ToolUse { id: "after-close".into(), name: "next_tool".into(), input: serde_json::json!({}) });
    let progressing = ConvState::ToolExecuting {
        current_tool: next_tool, remaining_tools: Vec::new(),
        completed_results: vec![ToolRoundResult::Finalized(completed)], pending_sub_agents: Vec::new(),
        assistant_message: AssistantMessage::new(command.source().message_id.clone(), blocks.clone(), None, None),
    };
    db.update_conversation_state(command.source_transcript_id(), &progressing).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (0, 0, 1, 0));
    assert_initial_completion_origin_revoked(&db, &run).await;
    let next = FinalizedToolResult::new(&assistant.message_id, &assistant.conversation_id, 3,
        chrono::Utc::now(), &ToolResult::success("after-close".into(), "done".into()));
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[result, next],
        &ConvState::Idle, chrono::Utc::now()).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (1, 0, 1, 0));
    db.read_native_close_captured_loss_retry_context(command.intent().target(), &run).await.unwrap();
}

#[tokio::test]
async fn stop_during_serial_tool_progress_still_suppresses_initial_origin() {
    use phoenix_core::domain::conversation_goal::ToolRoundResult;
    let (db, command, run) = retained_loss_origin().await;
    let (assistant, result) = initial_result_round(&command, &run);
    let MessageContent::Tool(row) = &result.row().content else { panic!("tool result"); };
    let MessageContent::Agent(blocks) = &assistant.content else { panic!("assistant"); };
    let progressing = ConvState::ToolExecuting {
        current_tool: ToolCall::new("after-close", ToolInput::Unknown { name: "next_tool".into(), input: serde_json::json!({}) }),
        remaining_tools: Vec::new(),
        completed_results: vec![ToolRoundResult::Finalized(ToolResult::success(row.tool_use_id.clone(), row.content.clone()))],
        pending_sub_agents: Vec::new(), assistant_message: AssistantMessage::new(command.source().message_id.clone(), blocks.clone(), None, None),
    };
    db.update_conversation_state(command.source_transcript_id(), &progressing).await.unwrap();
    db.update_conversation_state(command.source_transcript_id(), &ConvState::Idle).await.unwrap();
    db.persist_tool_round_and_state(command.source_transcript_id(), &assistant, &[result], &ConvState::Idle, chrono::Utc::now()).await.unwrap();
    assert_eq!(initial_ack_counts(&db, &run).await, (0, 1, 1, 0));
}

#[tokio::test]
async fn initial_source_fence_missing_acknowledgement_schema_fails_closed() {
    let (db, command, run) = retained_loss_origin().await;
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    sqlx::query("DROP TABLE native_close_initial_result_acknowledgements")
        .execute(db.pool()).await.unwrap();
    assert!(db.update_conversation_state(command.source_transcript_id(), &ConvState::Idle)
        .await.is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    let facts: (i64, i64) = sqlx::query_as("SELECT
        (SELECT suppressed FROM close_completion_retry_allowances WHERE attempt_id = ?1),
        (SELECT COUNT(*) FROM native_close_completion_retry_source_revocations WHERE attempt_id = ?1)")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(facts, (0, 0));
}
