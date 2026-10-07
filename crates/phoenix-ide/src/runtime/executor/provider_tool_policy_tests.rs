use super::*;
use crate::db::{Database, Message, ToolContent};
use crate::runtime::testing::MockToolExecutor;
use crate::runtime::traits::DatabaseStorage;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::llm_types::{ToolReference, ToolSearchResultContent};
use phoenix_core::domain::provider_replay::{
    AnthropicPrivateBlock, AnthropicResponseIdentity, AnthropicResponseSet, ContentIndex,
    ProviderReplayUpdate,
};
use phoenix_llm::{LlmError, LlmResponse};
use std::sync::Mutex;

const CONVERSATION: &str = "provider-tool-withdrawal";
const MCP_TOOL: &str = "mcp__slack__read_thread";

struct RenderingAnthropicClient {
    requests: Mutex<Vec<LlmRequest>>,
    wires: Mutex<Vec<serde_json::Value>>,
}

impl RenderingAnthropicClient {
    fn new() -> Self {
        Self {
            requests: Mutex::new(vec![]),
            wires: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl LlmClient for RenderingAnthropicClient {
    fn continuation_route_key(&self) -> String {
        "anthropic:https://api.anthropic.com/v1/messages:claude-opus-5-5".into()
    }

    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let spec = phoenix_llm::all_models()
            .into_iter()
            .find(|spec| spec.id == "claude-opus-5-5")
            .expect("built-in Anthropic model");
        let wire = phoenix_llm::render_anthropic_request_for_test(&spec, request)?;
        self.requests.lock().unwrap().push(request.clone());
        self.wires.lock().unwrap().push(wire);
        Err(LlmError::network(
            "Injected transport failure after local request rendering",
        ))
    }

    fn model_id(&self) -> &str {
        "claude-opus-5-5"
    }
}

type TestRuntime =
    ConversationRuntime<DatabaseStorage, Arc<RenderingAnthropicClient>, Arc<MockToolExecutor>>;

fn runtime(
    db: Database,
    directory: &Path,
    client: Arc<RenderingAnthropicClient>,
    tools: Arc<MockToolExecutor>,
) -> TestRuntime {
    let context = ConvContext::new(
        CONVERSATION,
        directory.to_path_buf(),
        "claude-opus-5-5",
        200_000,
    );
    let (event_tx, event_rx) = mpsc::channel(8);
    ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 2 },
        DatabaseStorage::new(db),
        client,
        tools,
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        event_rx,
        event_tx,
        SseBroadcaster::new(32, 4),
    )
}

async fn dispatch(runtime: &mut TestRuntime) {
    runtime
        .execute_effect(Effect::RequestLlm)
        .await
        .expect("request preparation succeeds");
    tokio::time::timeout(
        Duration::from_secs(10),
        runtime.llm_task_handle.take().expect("provider task"),
    )
    .await
    .expect("provider task completes")
    .expect("provider task does not panic");
}

fn saved_message(id: &str, sequence: i64, content: MessageContent) -> Message {
    Message {
        message_id: id.into(),
        origin: InputOrigin::SystemGenerated,
        conversation_id: CONVERSATION.into(),
        sequence_id: sequence,
        message_type: content.message_type(),
        content,
        display_data: None,
        usage_data: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn sqlite_runtime_anthropic_withdrawal_preserves_failed_exchange_after_reopen_and_retry() {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("conversation.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    db.create_conversation(
        CONVERSATION,
        CONVERSATION,
        directory.path().to_str().unwrap(),
        true,
        None,
        Some("claude-opus-5-5"),
    )
    .await
    .unwrap();
    db.add_message(
        "user-before-call",
        CONVERSATION,
        &MessageContent::user("Read the Slack thread"),
        None,
        None,
    )
    .await
    .unwrap();
    let client = Arc::new(RenderingAnthropicClient::new());
    let live_tools =
        Arc::new(MockToolExecutor::new().with_tool(MCP_TOOL, ToolOutput::error("Session expired")));
    let mut first = runtime(
        db.clone(),
        directory.path(),
        client.clone(),
        live_tools.clone(),
    );
    dispatch(&mut first).await;
    assert!(client.requests.lock().unwrap()[0]
        .tool_availability
        .is_callable(MCP_TOOL));
    assert!(live_tools.recorded_executions().is_empty());
    drop(first);

    let public = vec![
        ContentBlock::ServerToolUse {
            id: "srvtoolu_search".into(),
            name: "tool_search_tool_regex".into(),
            input: serde_json::json!({"query":"Slack"}),
        },
        ContentBlock::ToolSearchToolResult {
            tool_use_id: "srvtoolu_search".into(),
            content: ToolSearchResultContent {
                r#type: "tool_search_tool_search_result".into(),
                tool_references: vec![ToolReference {
                    r#type: "tool_reference".into(),
                    tool_name: MCP_TOOL.into(),
                }],
                error_code: None,
            },
        },
        ContentBlock::ToolUse {
            id: "toolu_slack".into(),
            name: MCP_TOOL.into(),
            input: serde_json::json!({"thread":"fixture"}),
        },
    ];
    let owner = saved_message("assistant-owner", 2, MessageContent::Agent(public.clone()));
    let result = saved_message(
        "failed-result",
        3,
        MessageContent::Tool(ToolContent {
            tool_use_id: "toolu_slack".into(),
            content: "Session expired".into(),
            is_error: true,
            images: vec![],
        }),
    );
    let replay = AnthropicResponseSet::with_public_content(
        AnthropicResponseIdentity {
            response_id: "msg_fixture".into(),
            model: "claude-opus-5-5".into(),
        },
        public.clone(),
        vec![AnthropicPrivateBlock::Thinking {
            index: ContentIndex(0),
            thinking: "private fixture".into(),
            signature: "fixture signature".into(),
        }],
    )
    .unwrap()
    .with_owner_message_id("assistant-owner".into());
    db.persist_tool_round_state_and_replay(
        CONVERSATION,
        &owner,
        &[result],
        &ConvState::LlmRequesting { attempt: 2 },
        Utc::now(),
        &ProviderReplayUpdate::Anthropic(replay),
    )
    .await
    .unwrap();
    db.pool().close().await;
    drop(db);

    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    let absent_tools = Arc::new(MockToolExecutor::new());
    let mut resumed = runtime(
        db.clone(),
        directory.path(),
        client.clone(),
        absent_tools.clone(),
    );
    dispatch(&mut resumed).await;
    resumed.state = ConvState::LlmRequesting { attempt: 3 };
    dispatch(&mut resumed).await;
    let persisted = db.get_messages(CONVERSATION).await.unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "both requests must reach adapter rendering after withdrawal"
    );
    for request in &requests[1..] {
        let owner = request
            .messages
            .iter()
            .find(|message| message.source_message_id.as_deref() == Some("assistant-owner"))
            .unwrap();
        assert_eq!(owner.content, public);
        assert_eq!(owner.role, MessageRole::Assistant);
        assert!(!request.tool_availability.is_callable(MCP_TOOL));
        assert!(request
            .tool_availability
            .declarations()
            .iter()
            .any(|definition| definition.name == MCP_TOOL));
        let replay = request.provider_replay.as_ref().unwrap();
        assert_eq!(replay.response_sets[0].owner_message_id, "assistant-owner");
        assert_eq!(replay.response_sets[0].public_content, public);
        assert_eq!(
            replay.response_sets[0].private_blocks[0].index(),
            ContentIndex(0)
        );
    }
    let wires = client.wires.lock().unwrap();
    assert_eq!(
        wires[1], wires[2],
        "retry must preserve the entire provider request"
    );
    let messages = wires[1]["messages"].as_array().unwrap();
    let owner = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    assert_eq!(owner["content"][0]["type"], "thinking");
    assert_eq!(owner["content"][0]["signature"], "fixture signature");
    assert_eq!(
        owner["content"][2]["content"]["tool_references"][0]["tool_name"],
        MCP_TOOL
    );
    assert_eq!(owner["content"][3]["id"], "toolu_slack");
    let result = messages
        .iter()
        .find(|message| message["content"][0]["type"] == "tool_result")
        .unwrap();
    assert_eq!(result["content"][0]["tool_use_id"], "toolu_slack");
    assert_eq!(result["content"][0]["is_error"], true);
    assert_eq!(
        messages.last().unwrap()["content"][0]["type"],
        "tool_removal"
    );
    assert!(absent_tools.recorded_executions().is_empty());
    assert_eq!(
        persisted
            .iter()
            .find(|message| message.message_id == "assistant-owner")
            .unwrap()
            .content
            .to_json(),
        serde_json::to_value(public).unwrap()
    );
}
