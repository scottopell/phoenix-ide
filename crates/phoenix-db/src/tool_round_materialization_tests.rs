use super::*;
use phoenix_core::domain::db_schema::{ToolContentImage, ToolOutcome};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::{
    AssistantMessage, PendingSubAgent, SubAgentMode, SubAgentOutcome,
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
