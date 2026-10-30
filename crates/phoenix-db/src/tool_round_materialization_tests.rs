use super::*;
use phoenix_core::domain::db_schema::{ToolContentImage, ToolOutcome};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::{
    AssistantMessage, PendingSubAgent, SubAgentMode, SubAgentOutcome, ThinkInput, ToolCall,
    ToolInput,
};
use std::collections::HashMap;

fn assistant(calls: &[(&str, &str)]) -> AssistantMessage {
    let mut assistant = AssistantMessage::new(
        "round".into(),
        calls
            .iter()
            .map(|(id, name)| ContentBlock::tool_use(*id, *name, serde_json::json!({})))
            .collect(),
        Some(UsageData::default()),
        Some(serde_json::json!({"held": "assistant metadata"})),
    );
    assistant.created_at = DateTime::from_timestamp(100, 0).unwrap();
    assistant
}

#[test]
fn restart_preserves_completed_metadata_and_interrupted_tail_order() {
    let assistant = assistant(&[
        ("done", "read_image"),
        ("unfinished", "think"),
        ("promoted", "present_svg"),
    ]);
    let now = DateTime::from_timestamp(200, 0).unwrap();
    let image = ToolContentImage {
        media_type: "image/png".into(),
        data: "bytes".into(),
    };
    let completed = [
        ToolResult {
            tool_use_id: "done".into(),
            outcome: ToolOutcome::Error {
                output: "failed with image".into(),
                display_data: Some(serde_json::json!({"duration_ms": 999, "detail": [1, 2]})),
                images: vec![image.clone()],
            },
            duration_ms: Some(17),
        },
        ToolResult::success("promoted".into(), "retained reference".into()),
    ];
    let (agent, rows) = build_materialized_tool_round(
        "conversation",
        10,
        &now,
        &assistant,
        &completed,
        &["unfinished".into()],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(agent.message_id, assistant.message_id);
    assert_eq!(agent.created_at, assistant.created_at);
    assert_eq!(agent.content, MessageContent::agent(assistant.content));
    assert_eq!(agent.display_data, assistant.display_data);
    assert_eq!(agent.usage_data, assistant.usage);
    assert_eq!(agent.sequence_id, 10);
    assert_eq!(agent.conversation_id, "conversation");
    assert_eq!(agent.origin, InputOrigin::SystemGenerated);
    assert_eq!(agent.message_type, MessageType::Agent);

    let expected = [
        (
            "done",
            "failed with image",
            true,
            vec![image],
            Some(serde_json::json!({"duration_ms":17,"detail":[1,2]})),
        ),
        ("promoted", "retained reference", false, vec![], None),
        (
            "unfinished",
            "[Tool execution interrupted by server restart]",
            true,
            vec![],
            None,
        ),
    ];
    assert_eq!(rows.len(), expected.len());
    for (ordinal, (row, (id, output, is_error, images, display))) in
        rows.iter().zip(expected).enumerate()
    {
        assert_eq!(row.message_id, format!("tool-result:5:round:{id}"));
        assert_eq!(row.conversation_id, "conversation");
        assert_eq!(row.sequence_id, 11 + i64::try_from(ordinal).unwrap());
        assert_eq!(row.created_at, now);
        assert_eq!(row.message_type, MessageType::Tool);
        assert_eq!(row.origin, InputOrigin::SystemGenerated);
        assert!(row.usage_data.is_none());
        assert_eq!(
            row.content,
            MessageContent::Tool(ToolContent {
                tool_use_id: id.into(),
                content: output.into(),
                is_error,
                images
            })
        );
        assert_eq!(row.display_data, display);
    }
}

#[test]
fn restart_spawn_fan_in_replaces_display_and_duration_but_preserves_images() {
    let assistant = assistant(&[("spawn", "spawn_agents"), ("unfinished", "think")]);
    let now = DateTime::from_timestamp(200, 0).unwrap();
    let image = ToolContentImage {
        media_type: "image/png".into(),
        data: "retained".into(),
    };
    let spawn = ToolResult {
        tool_use_id: "spawn".into(),
        outcome: ToolOutcome::Success {
            output: "Spawning 1 sub-agent(s): child".into(),
            display_data: Some(serde_json::json!({"old": "placeholder", "duration_ms": 999})),
            images: vec![image.clone()],
        },
        duration_ms: Some(17),
    };
    let completed = [spawn];
    let pending = [PendingSubAgent {
        agent_id: "child".into(),
        task: "investigate".into(),
        mode: SubAgentMode::Explore,
    }];
    let outcomes = HashMap::from([(
        "child".into(),
        SubAgentOutcome::Success {
            result: "answer".into(),
        },
    )]);
    let (_, rows) = build_materialized_tool_round(
        "conversation",
        10,
        &now,
        &assistant,
        &completed,
        &["unfinished".into()],
        &pending,
        &outcomes,
    )
    .unwrap();
    assert_eq!(
        rows[0].content,
        MessageContent::Tool(ToolContent {
            tool_use_id: "spawn".into(),
            content: "Sub-agent results (1 completed):\n\nTask: \"investigate\"\nResult: answer"
                .into(),
            is_error: false,
            images: vec![image],
        })
    );
    assert_eq!(
        rows[0].display_data,
        Some(serde_json::json!({
            "type": "subagent_summary",
            "results": [{"agent_id":"child","task":"investigate","outcome":{"type":"success","result":"answer"}}],
        }))
    );
    assert_eq!(rows[0].created_at, now);
    assert_eq!(rows[0].sequence_id, 11);
    assert_eq!(rows[0].message_id, "tool-result:5:round:spawn");
    assert_eq!(
        rows[1].content,
        MessageContent::Tool(ToolContent {
            tool_use_id: "unfinished".into(),
            content: "[Tool execution interrupted by server restart]".into(),
            is_error: true,
            images: vec![],
        })
    );
    assert!(rows[1].display_data.is_none());
}

fn trusted_prefix_state(cancelling: bool) -> (ConvState, AssistantMessage, ToolContentImage) {
    let assistant = assistant(&[
        ("trusted", "skill"),
        ("ordinary", "read_image"),
        ("unfinished", "think"),
        ("queued", "think"),
    ]);
    let image = ToolContentImage {
        media_type: "image/png".into(),
        data: "retained image".into(),
    };
    let completed_results = vec![
        ToolResult {
            tool_use_id: "trusted".into(),
            outcome: ToolOutcome::TrustedInstructions {
                output: "private authenticated coordinator instructions".into(),
            },
            duration_ms: Some(23),
        },
        ToolResult {
            tool_use_id: "ordinary".into(),
            outcome: ToolOutcome::Error {
                output: "ordinary failure".into(),
                display_data: Some(serde_json::json!({"detail": "preserved"})),
                images: vec![image.clone()],
            },
            duration_ms: Some(17),
        },
    ];
    let think = |id: &str| {
        ToolCall::new(
            id,
            ToolInput::Think(ThinkInput {
                thoughts: "pending".into(),
            }),
        )
    };
    let queued = think("queued");
    let state = if cancelling {
        ConvState::CancellingTool {
            cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
            tool_use_id: "unfinished".into(),
            skipped_tools: vec![queued],
            completed_results,
            assistant_message: assistant.clone(),
            pending_sub_agents: vec![],
        }
    } else {
        ConvState::ToolExecuting {
            current_tool: think("unfinished"),
            remaining_tools: vec![queued],
            completed_results,
            assistant_message: assistant.clone(),
            pending_sub_agents: vec![],
        }
    };
    (state, assistant, image)
}

async fn assert_restart_persists_trusted_receipt(cancelling: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("trusted-recovery.db");
    let path = path.to_str().unwrap();
    let db = Database::open(path).await.unwrap();
    migrations::run_pending_migrations(db.pool()).await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(
            Some("test-model"),
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();
    assert!(db
        .is_coordinator_conversation(&coordinator.id)
        .await
        .unwrap());
    let (state, assistant, image) = trusted_prefix_state(cancelling);
    db.update_conversation_state(&coordinator.id, &state)
        .await
        .unwrap();
    assert!(db.get_messages(&coordinator.id).await.unwrap().is_empty());
    let state_updated_at: String =
        sqlx::query_scalar("SELECT state_updated_at FROM conversations WHERE id = ?1")
            .bind(&coordinator.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let result_time = DateTime::parse_from_rfc3339(&state_updated_at)
        .unwrap()
        .with_timezone(&Utc);
    let start_seq = db.next_sequence_id(&coordinator.id).await.unwrap();
    db.pool().close().await;
    drop(db);
    let db = Database::open(path).await.unwrap();
    migrations::run_pending_migrations(db.pool()).await.unwrap();

    db.reset_all_to_idle().await.unwrap();

    assert!(matches!(
        db.get_conversation(&coordinator.id).await.unwrap().state,
        ConvState::Idle
    ));
    let rows = db.get_messages(&coordinator.id).await.unwrap();
    assert_recovered_trusted_round(
        &rows,
        assistant,
        &coordinator.id,
        start_seq,
        result_time,
        image,
    );
    let stored_content: Vec<String> = sqlx::query_scalar(
        "SELECT content FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id",
    )
    .bind(&coordinator.id)
    .fetch_all(db.pool())
    .await
    .unwrap();
    let stored_content = stored_content.join("\n");
    assert!(!stored_content.contains("private authenticated coordinator instructions"));
    assert!(!stored_content.contains("<trusted_builtin_skill"));
    db.reset_all_to_idle().await.unwrap();
    assert_eq!(
        serde_json::to_value(db.get_messages(&coordinator.id).await.unwrap()).unwrap(),
        serde_json::to_value(rows).unwrap(),
    );
}

fn assert_recovered_trusted_round(
    rows: &[Message],
    assistant: AssistantMessage,
    conversation_id: &str,
    start_seq: i64,
    result_time: DateTime<Utc>,
    image: ToolContentImage,
) {
    assert_eq!(rows.len(), 5);
    let expected_assistant = Message {
        message_id: assistant.message_id.clone(),
        conversation_id: conversation_id.to_owned(),
        sequence_id: start_seq,
        message_type: MessageType::Agent,
        content: MessageContent::agent(assistant.content),
        created_at: assistant.created_at,
        display_data: assistant.display_data,
        usage_data: assistant.usage,
        origin: InputOrigin::SystemGenerated,
    };
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap(),
        serde_json::to_value(expected_assistant).unwrap()
    );
    let expected = [
        (
            "trusted",
            "Authenticated built-in skill instructions were delivered for this live request.",
            false,
            vec![],
            Some(serde_json::json!({"duration_ms":23})),
        ),
        (
            "ordinary",
            "ordinary failure",
            true,
            vec![image],
            Some(serde_json::json!({"detail":"preserved","duration_ms":17})),
        ),
        (
            "unfinished",
            "[Tool execution interrupted by server restart]",
            true,
            vec![],
            None,
        ),
        (
            "queued",
            "[Tool execution interrupted by server restart]",
            true,
            vec![],
            None,
        ),
    ];
    for (ordinal, (row, (id, content, is_error, images, display_data))) in
        rows[1..].iter().zip(expected).enumerate()
    {
        assert_eq!(
            serde_json::to_value(row).unwrap(),
            serde_json::to_value(Message {
                message_id: tool_result_message_id("round", id),
                conversation_id: conversation_id.to_owned(),
                sequence_id: start_seq + 1 + i64::try_from(ordinal).unwrap(),
                message_type: MessageType::Tool,
                content: MessageContent::Tool(ToolContent {
                    tool_use_id: id.into(),
                    content: content.into(),
                    is_error,
                    images
                }),
                created_at: result_time,
                display_data,
                usage_data: None,
                origin: InputOrigin::SystemGenerated,
            })
            .unwrap()
        );
    }
}

#[test]
fn restart_preserves_ordinary_skill_body() {
    let (_, rows) = build_materialized_tool_round(
        "ordinary-conversation",
        1,
        &Utc::now(),
        &assistant(&[("ordinary", "skill")]),
        &[ToolResult::success(
            "ordinary".into(),
            "ordinary skill body".into(),
        )],
        &[],
        &[],
        &HashMap::new(),
    )
    .unwrap();
    let MessageContent::Tool(content) = &rows[0].content else {
        panic!("tool result row");
    };
    assert_eq!(content.content, "ordinary skill body");
    assert!(!content.is_error);
}

#[tokio::test]
async fn restart_executing_round_persists_trusted_receipt() {
    assert_restart_persists_trusted_receipt(false).await;
}

#[tokio::test]
async fn restart_cancelling_round_persists_trusted_receipt() {
    assert_restart_persists_trusted_receipt(true).await;
}
