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
            completed_results: completed_results.into_iter().map(Into::into).collect(),
            assistant_message: assistant.clone(),
            pending_sub_agents: vec![],
        }
    } else {
        ConvState::ToolExecuting {
            current_tool: think("unfinished"),
            remaining_tools: vec![queued],
            completed_results: completed_results.into_iter().map(Into::into).collect(),
            assistant_message: assistant.clone(),
            pending_sub_agents: vec![],
        }
    };
    (state, assistant, image)
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // The fault matrix shares one durable-state and no-message-write oracle.
async fn malformed_restart_rounds_retain_durable_state_and_write_no_messages() {
    for cancelling in [false, true] {
        for fault in [
            "count",
            "duplicate_result",
            "unknown_result",
            "duplicate_call",
            "parse",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("rejected-round.db");
            let path = path.to_str().unwrap();
            let db = Database::open(path).await.unwrap();
            migrations::run_pending_migrations(db.pool()).await.unwrap();
            let conversation = db
                .get_or_create_coordinator(
                    Some("test-model"),
                    phoenix_core::llm_language::LlmLanguage::default(),
                )
                .await
                .unwrap();
            let (mut state, _, _) = trusted_prefix_state(cancelling);
            let (ConvState::ToolExecuting {
                assistant_message: assistant,
                completed_results: results,
                ..
            }
            | ConvState::CancellingTool {
                assistant_message: assistant,
                completed_results: results,
                ..
            }) = &mut state
            else {
                unreachable!()
            };
            match fault {
                "count" => {
                    results.pop();
                }
                "duplicate_result" => {
                    let id = results[0].tool_use_id().to_owned();
                    let phoenix_core::domain::conversation_goal::ToolRoundResult::Finalized(result) =
                        &mut results[1]
                    else {
                        unreachable!()
                    };
                    result.tool_use_id = id;
                }
                "unknown_result" => {
                    let phoenix_core::domain::conversation_goal::ToolRoundResult::Finalized(result) =
                        &mut results[1]
                    else {
                        unreachable!()
                    };
                    result.tool_use_id = "unknown".into();
                }
                "duplicate_call" => {
                    assistant.content[1] = assistant.content[0].clone();
                }
                "parse" => {}
                _ => unreachable!(),
            }
            db.update_conversation_state(&conversation.id, &state)
                .await
                .unwrap();
            if fault == "parse" {
                sqlx::query("UPDATE conversations SET state = ?1 WHERE id = ?2")
                    .bind(
                        serde_json::json!({
                            "type": if cancelling { "cancelling_tool" } else { "tool_executing" },
                            "held": "cannot decode",
                        })
                        .to_string(),
                    )
                    .bind(&conversation.id)
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            let original: (String, String, String) = sqlx::query_as(
                "SELECT state, state_kind, state_updated_at FROM conversations WHERE id = ?1",
            )
            .bind(&conversation.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            db.pool().close().await;
            drop(db);
            let db = Database::open(path).await.unwrap();
            migrations::run_pending_migrations(db.pool()).await.unwrap();
            for _ in 0..2 {
                assert!(
                    db.reset_all_to_idle().await.is_err(),
                    "{fault}, cancelling={cancelling}"
                );
                let retained: (String, String, String) = sqlx::query_as(
                    "SELECT state, state_kind, state_updated_at FROM conversations WHERE id = ?1",
                )
                .bind(&conversation.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
                assert_eq!(retained, original);
                assert!(db.get_messages(&conversation.id).await.unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn malformed_child_round_is_validated_before_parent_recovery_mutates_it() {
    async fn raw(db: &Database) -> Vec<(String, String, String, String)> {
        sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT id, state, state_kind, state_updated_at FROM conversations WHERE id IN ('parent', '11111111-1111-4111-8111-111111111111') ORDER BY id",
        ).fetch_all(db.pool()).await.unwrap()
    }
    for cancelling in [false, true] {
        for duplicate in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("rejected-child.db");
            let path = path.to_str().unwrap();
            let mut db = Database::open(path).await.unwrap();
            migrations::run_pending_migrations(db.pool()).await.unwrap();
            let parent_id = "parent";
            let child_id = "11111111-1111-4111-8111-111111111111";
            db.create_conversation(parent_id, "parent", "/tmp", true, None, None)
                .await
                .unwrap();
            db.create_conversation(child_id, "child", "/tmp", false, Some(parent_id), None)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO sub_agent_batches
                     (batch_id, parent_conversation_id, parallel_work_qualified, admitted_at_unix_micros)
                 VALUES ('retained-batch', ?1, 0, 1)",
            ).bind(parent_id).execute(db.pool()).await.unwrap();
            sqlx::query(
                "INSERT INTO sub_agent_runs
                     (child_conversation_id, batch_id, ordinal, execution_authority, max_turns, timeout_millis)
                 VALUES (?1, 'retained-batch', 0, 'read_only', 10, 1000)",
            ).bind(child_id).execute(db.pool()).await.unwrap();
            let parent_assistant = assistant(&[("spawn", "spawn_agents"), ("wait", "think")]);
            let pending = vec![PendingSubAgent {
                agent_id: child_id.into(),
                task: "retain child evidence".into(),
                mode: SubAgentMode::Explore,
            }];
            let completed = vec![ToolResult::success("spawn".into(), "Spawning child".into())];
            let parent = if cancelling {
                ConvState::CancellingTool {
                    tool_use_id: "wait".into(),
                    cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                    skipped_tools: vec![],
                    completed_results: completed.into_iter().map(Into::into).collect(),
                    assistant_message: parent_assistant,
                    pending_sub_agents: pending,
                }
            } else {
                ConvState::ToolExecuting {
                    current_tool: ToolCall::new(
                        "wait",
                        ToolInput::Think(ThinkInput {
                            thoughts: "pending".into(),
                        }),
                    ),
                    remaining_tools: vec![],
                    completed_results: completed.into_iter().map(Into::into).collect(),
                    assistant_message: parent_assistant,
                    pending_sub_agents: pending,
                }
            };
            let (mut child, _, _) = trusted_prefix_state(cancelling);
            let (ConvState::ToolExecuting {
                assistant_message,
                completed_results,
                ..
            }
            | ConvState::CancellingTool {
                assistant_message,
                completed_results,
                ..
            }) = &mut child
            else {
                unreachable!()
            };
            assistant_message.message_id = "child-round".into();
            let phoenix_core::domain::conversation_goal::ToolRoundResult::Finalized(result) =
                &mut completed_results[1]
            else {
                panic!("fixture result must be finalized");
            };
            result.tool_use_id = if duplicate { "trusted" } else { "unknown" }.into();
            db.update_conversation_state(parent_id, &parent)
                .await
                .unwrap();
            db.update_conversation_state(child_id, &child)
                .await
                .unwrap();
            let original = raw(&db).await;
            db.pool().close().await;
            drop(db);
            db = Database::open(path).await.unwrap();
            migrations::run_pending_migrations(db.pool()).await.unwrap();
            for _ in 0..2 {
                assert!(db.reset_all_to_idle().await.is_err());
                assert_eq!(
                    raw(&db).await,
                    original,
                    "parent recovery overwrote malformed child evidence"
                );
                assert!(db.get_messages(parent_id).await.unwrap().is_empty());
                assert!(db.get_messages(child_id).await.unwrap().is_empty());
                db.pool().close().await;
                drop(db);
                db = Database::open(path).await.unwrap();
                migrations::run_pending_migrations(db.pool()).await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn scoped_materialization_ignores_unselected_malformed_buffers() {
    let db = Database::open_in_memory().await.unwrap();
    for id in ["selected", "unselected"] {
        db.create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let (state, _, _) = trusted_prefix_state(false);
        db.update_conversation_state(id, &state).await.unwrap();
    }
    let malformed = "{\"type\":\"tool_executing\",\"held\":\"outside selected scope\"}";
    sqlx::query("UPDATE conversations SET state = ?1 WHERE id = 'unselected'")
        .bind(malformed)
        .execute(db.pool())
        .await
        .unwrap();
    let selected = std::collections::HashSet::from(["selected".to_owned()]);
    assert_eq!(
        db.materialize_in_flight_tool_rounds(&Utc::now(), Some(&selected))
            .await
            .unwrap(),
        selected
    );
    assert_eq!(db.get_messages("selected").await.unwrap().len(), 5);
    assert!(db.get_messages("unselected").await.unwrap().is_empty());
    let retained: String =
        sqlx::query_scalar("SELECT state FROM conversations WHERE id = 'unselected'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(retained, malformed);
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
