use super::*;
use crate::db::{Database, Message};
use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
use crate::runtime::traits::DatabaseStorage;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use phoenix_core::domain::db_schema::{FinalizedToolResult, InputOrigin};
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

struct DeferredMcpCatalog(MockToolExecutor);

impl DeferredMcpCatalog {
    fn new(available: bool) -> Self {
        Self(if available {
            MockToolExecutor::new().with_tool(MCP_TOOL, ToolOutput::error("Session expired"))
        } else {
            MockToolExecutor::new()
        })
    }

    fn recorded_executions(&self) -> Vec<(String, serde_json::Value)> {
        self.0.recorded_executions()
    }
}

#[async_trait]
impl ToolExecutor for DeferredMcpCatalog {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        self.0.execute(call, ctx).await
    }

    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
        self.0
            .definitions()
            .await
            .into_iter()
            .map(|mut definition| {
                definition.defer_loading = true;
                definition
            })
            .collect()
    }
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
    fn freeze_for_request(self: Arc<Self>) -> Result<Arc<dyn LlmClient>, LlmError> {
        Ok(self)
    }

    fn continuation_route_key(&self) -> Result<String, LlmError> {
        Ok("anthropic:https://api.anthropic.com/v1/messages:claude-opus-5-5".into())
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

    fn model_id(&self) -> &'static str {
        "claude-opus-5-5"
    }
}

type TestRuntime<L = RenderingAnthropicClient, T = DeferredMcpCatalog> =
    ConversationRuntime<DatabaseStorage, Arc<L>, Arc<T>>;

fn runtime<L: LlmClient + 'static, T: ToolExecutor + 'static>(
    db: Database,
    directory: &Path,
    client: Arc<L>,
    tools: Arc<T>,
) -> TestRuntime<L, T> {
    let context = ConvContext::new(
        CONVERSATION,
        directory.to_path_buf(),
        client.model_id(),
        200_000,
        16_384,
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

async fn dispatch<L: LlmClient + 'static, T: ToolExecutor + 'static>(
    runtime: &mut TestRuntime<L, T>,
) {
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
#[allow(clippy::too_many_lines)]
async fn dispatch_cannot_expand_or_reinterpret_the_persisted_request_policy() {
    use phoenix_core::domain::sm_state::ToolInput;
    for (callable, same_schema, live, expected_executions) in [
        (false, true, true, 0),
        (true, false, true, 0),
        (true, true, false, 0),
        (true, true, true, 1),
    ] {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("admission.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation(CONVERSATION, CONVERSATION, "/tmp", true, None, None)
            .await
            .unwrap();
        let mut definitions = DeferredMcpCatalog::new(true).definitions().await;
        if !same_schema {
            definitions[0].input_schema =
                serde_json::json!({"type":"object","properties":{"thread":{"type":"integer"}}});
        }
        let callable_names = if callable {
            [MCP_TOOL.to_owned()].into_iter().collect()
        } else {
            std::collections::BTreeSet::default()
        };
        db.prepare_tool_availability(
            CONVERSATION,
            "route",
            None,
            &definitions,
            &callable_names,
            &[],
            &[],
        )
        .await
        .unwrap();
        db.pool().close().await;
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        let tools = Arc::new(DeferredMcpCatalog::new(live));
        let mut rt = runtime(
            db.clone(),
            directory.path(),
            Arc::new(RenderingAnthropicClient::new()),
            tools.clone(),
        );
        let call = ToolCall::new(
            "call",
            ToolInput::Unknown {
                name: MCP_TOOL.into(),
                input: serde_json::json!({}),
            },
        );
        rt.state = ConvState::ToolExecuting {
            current_tool: call.clone(),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: crate::state_machine::AssistantMessage::new(
                "owner".into(),
                vec![ContentBlock::tool_use(
                    "call",
                    MCP_TOOL,
                    serde_json::json!({}),
                )],
                None,
                None,
            ),
        };
        let mut admitted = rt.admit_authoritative_effect().unwrap();
        let event = rt
            .dispatch_tool_execution(call, &mut admitted)
            .await
            .unwrap();
        if callable {
            assert!(event.is_none());
            let (_, outcome) =
                tokio::time::timeout(Duration::from_secs(5), rt.tool_outcome_rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            if expected_executions == 0 {
                assert!(
                    matches!(outcome, ToolExecOutcome::Failed { tool_use_id, error } if tool_use_id == "call" && error.contains("EUNAVAIL"))
                );
            } else {
                assert!(matches!(outcome, ToolExecOutcome::Completed(_)));
            }
        } else {
            let Some(Event::ToolComplete {
                tool_use_id,
                result,
            }) = event
            else {
                panic!("disallowed request call must return a result")
            };
            assert_eq!(tool_use_id, "call");
            assert!(format!("{result:?}").contains("EUNAVAIL"));
        }
        assert_eq!(tools.recorded_executions().len(), expected_executions);
        let policy = db.load_tool_admission_policy(CONVERSATION).await.unwrap();
        assert_eq!(policy.is_callable(MCP_TOOL), callable);
        assert_eq!(
            policy.declarations()[0].input_schema,
            definitions[0].input_schema
        );
        db.pool().close().await;
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
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
    let live_tools = Arc::new(DeferredMcpCatalog::new(true));
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
    let result = FinalizedToolResult::new(
        &owner.message_id,
        CONVERSATION,
        3,
        Utc::now(),
        &ToolResult::error("toolu_slack".into(), "Session expired".into()),
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
    let absent_tools = Arc::new(DeferredMcpCatalog::new(false));
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

struct ReplayCheckingTools {
    db: Database,
    tools: DeferredMcpCatalog,
}

#[async_trait]
impl ToolExecutor for ReplayCheckingTools {
    async fn execute(
        &self,
        call: crate::runtime::deny_gate::CheckedToolCall,
        ctx: ToolContext,
    ) -> Option<ToolOutput> {
        use phoenix_core::domain::responses_replay::ProviderResponseSet;
        let retained = self
            .db
            .load_output_replay_state(CONVERSATION)
            .await
            .unwrap();
        let [ProviderResponseSet::ChatCompletions(replay)] = retained.as_slice() else {
            panic!("signed replay must be durable before tool execution");
        };
        let ConvState::ToolExecuting {
            assistant_message, ..
        } = self.db.get_conversation(CONVERSATION).await.unwrap().state
        else {
            panic!("accepted tool state must already be durable");
        };
        assert_eq!(replay.owner_message_id, assistant_message.message_id);
        assert_eq!(replay.public_content, assistant_message.content);
        assert_eq!(replay.tool_signatures[0].tool_call_id, "signed-call");
        assert_eq!(
            replay.tool_signatures[0].signature,
            "opaque-google-signature"
        );
        self.tools.execute(call, ctx).await
    }

    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
        self.tools.definitions().await
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sqlite_runtime_chat_images_and_signature_survive_reopen() {
    use phoenix_core::domain::responses_replay::{
        ChatResponseSet, ChatToolSignature, ProviderResponseSet,
    };
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("chat-signature.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    db.create_conversation(
        CONVERSATION,
        CONVERSATION,
        directory.path().to_str().unwrap(),
        true,
        None,
        Some("gemini-4-neon"),
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
    let public = vec![ContentBlock::tool_use(
        "signed-call",
        MCP_TOOL,
        serde_json::json!({"thread":"fixture"}),
    )];
    let replay = ChatResponseSet {
        response_id: "google-response".into(),
        model: "google/gemini-4-neon".into(),
        owner_message_id: String::new(),
        public_content: public.clone(),
        tool_signatures: vec![ChatToolSignature {
            tool_call_id: "signed-call".into(),
            signature: "opaque-google-signature".into(),
        }],
    };
    let mut response =
        LlmResponse::non_streaming(public.clone(), false, phoenix_llm::Usage::default());
    response.provider_replay = Some(ProviderReplayUpdate::ChatCompletions(replay));
    let client = Arc::new(MockLlmClient::new("gemini-4-neon"));
    client.queue_response(response);
    let image = crate::tools::ToolImage {
        media_type: "image/png".into(),
        data: "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAdklEQVR4nO3PQQkAMAzAwPo33Yno4xgEIuAyO/t1wwUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWNKAFDWhBA1rQgBY0oAUNaEEDWtCAFjSgBQ1oQQNa0IAWHHtBp+HSthEAJgAAAABJRU5ErkJggg==".into(),
    };
    let tools = Arc::new(ReplayCheckingTools {
        db: db.clone(),
        tools: DeferredMcpCatalog(MockToolExecutor::new().with_tool(
            MCP_TOOL,
            ToolOutput::Error {
                output: "Session expired".into(),
                images: vec![image.clone()],
                display_data: None,
                llm_usage: None,
            },
        )),
    });
    let mut first = runtime(db.clone(), directory.path(), client.clone(), tools.clone());
    dispatch(&mut first).await;
    assert!(client.recorded_requests()[0].output_replay.is_empty());
    let (_, outcome) = first.llm_outcome_rx.try_recv().unwrap();
    Box::pin(first.process_outcome(EffectOutcome::Llm(outcome)))
        .await
        .unwrap();
    assert!(first
        .observe_tool_preparation_for_test()
        .await
        .unwrap()
        .is_none());
    let (_, outcome) = tokio::time::timeout(Duration::from_secs(10), first.tool_outcome_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tools.tools.recorded_executions().len(), 1);
    Box::pin(first.process_outcome(EffectOutcome::Tool(outcome)))
        .await
        .unwrap();
    first
        .llm_task_handle
        .take()
        .expect("tool follow-up request")
        .await
        .unwrap();
    let expected = db.load_output_replay_state(CONVERSATION).await.unwrap();
    let [ProviderResponseSet::ChatCompletions(saved)] = expected.as_slice() else {
        panic!("accepted signed turn must remain retained");
    };
    assert!(!saved.owner_message_id.is_empty());
    assert_eq!(saved.public_content, public);
    assert_eq!(
        saved.tool_signatures[0].signature,
        "opaque-google-signature"
    );
    assert_eq!(client.recorded_requests()[1].output_replay, expected);
    let owner = db
        .get_messages(CONVERSATION)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.message_id == saved.owner_message_id)
        .unwrap();
    assert_eq!(
        owner.content.to_json(),
        serde_json::to_value(&public).unwrap()
    );
    drop(first);
    drop(tools);
    db.pool().close().await;
    drop(db);

    let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
    let mut resumed = runtime(
        reopened.clone(),
        directory.path(),
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(true)),
    );
    dispatch(&mut resumed).await;
    resumed.state = ConvState::LlmRequesting { attempt: 3 };
    dispatch(&mut resumed).await;
    let requests = client.recorded_requests();
    assert_eq!(requests.len(), 4);
    for request in &requests[1..] {
        assert_eq!(request.output_replay, expected);
        let owner = request
            .messages
            .iter()
            .find(|message| {
                message.source_message_id.as_deref() == Some(saved.owner_message_id.as_str())
            })
            .unwrap();
        assert_eq!(owner.content, public);
        let results = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    images,
                } = block
                {
                    Some((tool_use_id, content, is_error, images))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let [(call_id, content, is_error, images)] = results.as_slice() else {
            panic!("one authentic image-bearing tool result survives reopen and retry")
        };
        assert_eq!(call_id.as_str(), "signed-call");
        assert_eq!(content.as_str(), "Session expired");
        assert!(**is_error);
        assert_eq!(
            images.as_slice(),
            &[phoenix_core::domain::llm_types::ImageSource::Base64 {
                media_type: image.media_type.clone(),
                data: image.data.clone(),
            }]
        );
    }
    drop(resumed);
    reopened.pool().close().await;
}
