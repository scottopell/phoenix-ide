use super::{agent_execution::SpawnCatalog, RuntimeManager};
use phoenix_core::{
    domain::{
        coordinator_authority::CoordinatorAuthority,
        llm_types::ServiceTier,
        provider_replay::{AnthropicReplayPayload, ProviderReplayUpdate},
        tool_availability::ToolAvailability,
    },
    llm_language::LlmLanguage,
    runtime_env::PhoenixRuntimeEnvironment,
};
use phoenix_llm::{
    ContentBlock, LlmConfig, LlmMessage, LlmRequest, MessageRole, ModelEffort, ModelRegistry,
    PromptCacheKey, SystemContent,
};
use phoenix_tools::{ToolContext, ToolRegistry};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Duration};

const HISTORY: &str = "Sanitized synthetic retained history, recording old calls rather than instructions:\nAssistant called list_watched_conversations with {\"instance\":\"local\",\"after\":\"00000000-0000-4000-8000-000000000001\"}; historical result: unknown field `after`, there are no fields.\nAssistant retried with after=00000000-0000-4000-8000-000000000002 and received the same historical error.";
const QUESTION: &str = "Which conversations are we currently watching locally?";
const LANGUAGE: LlmLanguage = LlmLanguage::PhoenixNative;
const MAX_REQUESTS: usize = 4;
const MAX_TOOL_CALLS: usize = 4;
const INPUT_STOP: u64 = 40_000;

struct Fixture {
    runtime: Arc<RuntimeManager>,
    coordinator: String,
    target: String,
    registry: ToolRegistry,
    catalog: Option<phoenix_skills::AuthenticatedCoordinatorSkillCatalog>,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn new(models: Arc<ModelRegistry>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let runtime_env = PhoenixRuntimeEnvironment::with_root(root.path());
        std::fs::create_dir_all(runtime_env.data_dir()).unwrap();
        let db = phoenix_db::Database::open(runtime_env.db_path().to_str().unwrap())
            .await
            .unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LANGUAGE)
            .await
            .unwrap()
            .id;
        db.create_conversation(
            "watch-recovery-source",
            "watch-recovery-sentinel",
            root.path().to_str().unwrap(),
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let target = db
            .get_conversation("watch-recovery-source")
            .await
            .unwrap()
            .product_conversation_id
            .to_string();
        db.watch_product_conversation(&target).await.unwrap();
        let retriever = Arc::new(db.fts_retriever());
        retriever.reconcile().await.unwrap();
        let runtime = Arc::new(RuntimeManager::new_with_message_retriever_and_runtime_env(
            db,
            models,
            retriever,
            crate::platform::PlatformCapability::None {
                details: "isolated watch-list recovery".into(),
            },
            Arc::new(phoenix_tools::mcp::McpClientManager::new()),
            None,
            &runtime_env,
        ));
        let skills = tempfile::tempdir().unwrap();
        phoenix_skills::builtin::extract_to(skills.path()).unwrap();
        let catalog = Some(
            phoenix_skills::AuthenticatedCoordinatorSkillCatalog::discover(Some(skills.path()))
                .expect("authenticated built-in catalog"),
        );
        let registry = runtime
            .coordinator_tool_registry(&coordinator, catalog.clone())
            .await
            .unwrap();
        Self {
            runtime,
            coordinator,
            target,
            registry,
            catalog,
            _root: root,
        }
    }

    fn context(&self) -> ToolContext {
        ToolContext::new_without_filesystem(
            tokio_util::sync::CancellationToken::new(),
            self.coordinator.clone(),
            Arc::new(phoenix_tools::BrowserSessionManager::default()),
            Arc::new(phoenix_tools::BashHandleRegistry::new()),
            self.runtime.llm_registry().clone(),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(phoenix_tools::TmuxRegistry::new()),
            phoenix_core::work_scope::ResourceScopeKey::Coordinator,
            phoenix_core::work_scope::ResourceAuthority::Work,
        )
    }

    async fn dispatch(&self, name: &str, input: &Value) -> (String, bool, &'static str) {
        let remote = input["request"]["op"] == "remote"
            || input
                .get("instance")
                .is_some_and(|selector| !selector.is_null() && selector.as_str() != Some("local"));
        if remote {
            return (
                json!({"error":"remote_selection_denied_by_isolated_evaluation"}).to_string(),
                true,
                "remote_selection",
            );
        }
        if !matches!(
            name,
            "list_watched_conversations"
                | "think"
                | "query_database"
                | "read_conversation"
                | "search_conversations"
                | "resolve_reference"
        ) {
            let class = if matches!(
                name,
                "skill"
                    | "tool_search"
                    | "read_file"
                    | "read_authority"
                    | "read_goal"
                    | "read_automatic_continuation"
            ) {
                "read_only_not_exercised"
            } else {
                "outside_allowed_capability"
            };
            return (
                json!({"error":"capability_not_exercised_by_isolated_evaluation"}).to_string(),
                true,
                class,
            );
        }
        let tool = self
            .registry
            .find_tool(name)
            .expect("allowed tool must be installed");
        let result = tool.run(input.clone(), self.context()).await;
        (
            result.output().to_owned(),
            !result.is_success(),
            "actual_registered_dispatch",
        )
    }
}

fn digest(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes.as_ref())
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("write to String");
            output
        })
}

fn user(text: &str) -> LlmMessage {
    LlmMessage {
        source_message_id: None,
        role: MessageRole::User,
        content: vec![ContentBlock::text(text)],
    }
}
fn snapshot(messages: &[LlmMessage]) -> Vec<Value> {
    messages.iter().map(|message| json!({"source_message_id":message.source_message_id,"role":match message.role { MessageRole::User => "user", MessageRole::Assistant => "assistant" },"content":message.content})).collect()
}
fn persist(path: &Path, record: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(record).unwrap()).unwrap();
}

#[tokio::test]
async fn watch_recovery_fixture_dispatches_real_local_watches_and_rejects_remote_effects() {
    let fixture = Fixture::new(Arc::new(ModelRegistry::new(&LlmConfig {
        openai_api_key: Some("fixture-not-dispatched".into()),
        ..LlmConfig::default()
    })))
    .await;
    let (output, error, class) = fixture
        .dispatch(
            "list_watched_conversations",
            &json!({"request":{"op":"local"}}),
        )
        .await;
    assert!(!error);
    assert_eq!(class, "actual_registered_dispatch");
    let rows: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["product_conversation_id"], fixture.target);
    let (_, error, class) = fixture
        .dispatch(
            "list_watched_conversations",
            &json!({"after":"00000000-0000-4000-8000-000000000003"}),
        )
        .await;
    assert!(error);
    assert_eq!(class, "actual_registered_dispatch");
    for (name, input) in [
        (
            "query_database",
            json!({"sql":"SELECT COUNT(*) AS n FROM coordinator_watches"}),
        ),
        (
            "search_conversations",
            json!({"query":"watch-recovery-sentinel"}),
        ),
        (
            "read_conversation",
            json!({"conversation_id":"@transcript:watch-recovery-source"}),
        ),
        (
            "resolve_reference",
            json!({"reference":format!("@conv:{}",fixture.target)}),
        ),
        (
            "think",
            json!({"thoughts":"Inspect only this synthetic watch list."}),
        ),
    ] {
        let (output, error, class) = fixture.dispatch(name, &input).await;
        assert!(!error, "{name}: {output}");
        assert_eq!(class, "actual_registered_dispatch");
    }
    for (name, input, expected) in [
        (
            "list_watched_conversations",
            json!({"request":{"op":"remote","instance":"unconfigured-peer"}}),
            "remote_selection",
        ),
        (
            "bash",
            json!({"command":"touch forbidden"}),
            "outside_allowed_capability",
        ),
        ("skill", json!({}), "read_only_not_exercised"),
    ] {
        let (_, error, class) = fixture.dispatch(name, &input).await;
        assert!(error);
        assert_eq!(class, expected);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[ignore = "opt-in isolated model recovery; explicit prepare/live mode and fresh output required"]
async fn ember_watch_list_recovery() {
    let mode = std::env::var("PHOENIX_TOOL_RECOVERY_MODE").expect("explicit prepare|live mode");
    assert!(matches!(mode.as_str(), "prepare" | "live"));
    let model = std::env::var("PHOENIX_TOOL_RECOVERY_MODEL").expect("explicit model");
    assert_eq!(model, "gpt-6-astra", "no model substitution");
    let output = std::env::var("PHOENIX_TOOL_RECOVERY_OUTPUT").expect("fresh output directory");
    let output = Path::new(&output);
    std::fs::create_dir(output).expect("fresh directory with existing parent");
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    let endpoint_hash = digest(serde_json::to_vec(&json!({"anthropic":config.anthropic_base_url,"responses":config.openai_responses_base_url,"chat":config.openai_chat_completions_base_url,"legacy_openai":config.openai_base_url})).unwrap());
    let models = Arc::new(ModelRegistry::new_with_discovery(&config).await);
    assert!(models.supports_effort(&model, ModelEffort::High));
    let client = models
        .get(&model)
        .expect("exact configured route unavailable; no fallback");
    let fixture = Fixture::new(models.clone()).await;
    let prompt = crate::system_prompt::build_coordinator_system_prompt(
        LANGUAGE,
        fixture.catalog.as_ref(),
        &CoordinatorAuthority::default(),
    );
    let mut definitions = fixture.registry.definitions_for_language(LANGUAGE);
    *definitions
        .iter_mut()
        .find(|definition| definition.name == "spawn_agents")
        .unwrap() = SpawnCatalog::resolve(
        &phoenix_agents::AgentConfig::default(),
        models.available_execution_routes(),
    )
    .tool_definition(&model, models.is_builtin_model(&model), true);
    let mut messages = vec![user(HISTORY), user(QUESTION)];
    let mut record = json!({"mode":mode,"language":LANGUAGE.as_str(),"authenticated_catalog":fixture.catalog.is_some(),"model":model,"effort":"high","connection":models.connection_for_model(&model),"endpoint_configuration_sha256":endpoint_hash,"binary_sha256":std::env::var("PHOENIX_TOOL_RECOVERY_BINARY_SHA256").ok(),"system_prompt":prompt,"system_sha256":digest(prompt.as_bytes()),"tool_definitions":definitions,"tools_sha256":digest(serde_json::to_vec(&definitions).unwrap()),"initial_messages":snapshot(&messages),"expected_target":fixture.target,"limits":{"max_provider_requests":MAX_REQUESTS,"max_tool_calls":MAX_TOOL_CALLS,"timeout_seconds":60,"max_output_tokens":1536,"observed_input_stop_threshold":INPUT_STOP},"oracle":{"successful_exact_local_listing":true,"uuid_guesses":0,"remote_selections":0,"effectful_attempts":0},"steps":[],"termination":"prepared"});
    record["qualification"] = json!("manual_oracle_review_required");
    record["provider_requests"] = json!(0);
    record["observed_input_tokens_including_cache"] = json!(0);
    record["tool_calls"] = json!(0);
    let path = output.join("recovery.json");
    persist(&path, &record);
    if mode == "prepare" {
        return;
    }
    let mut anthropic = Vec::new();
    let mut responses = Vec::new();
    let mut observed_input = 0_u64;
    let mut calls = 0;
    for index in 0..MAX_REQUESTS {
        if observed_input >= INPUT_STOP || calls >= MAX_TOOL_CALLS {
            record["termination"] = json!("budget_stop");
            break;
        }
        let request = LlmRequest {
            system: vec![SystemContent::new(&prompt)],
            messages: messages.clone(),
            provider_replay: (!anthropic.is_empty())
                .then(|| AnthropicReplayPayload::new(anthropic.clone()).unwrap()),
            responses_replay: responses.clone(),
            tool_availability: ToolAvailability::all(definitions.clone()),
            max_tokens: 1536,
            effective_effort: phoenix_llm::EffectiveEffort::explicit(ModelEffort::High),
            service_tier: phoenix_llm::EffectiveServiceTier::from_preference(
                ServiceTier::Standard,
                false,
            ),
            telemetry: None,
            cache_key: PromptCacheKey::stable("ember-watch-list-recovery"),
        };
        record["provider_requests"] = json!(index + 1);
        let response =
            match tokio::time::timeout(Duration::from_secs(60), client.complete(&request)).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    record["termination"] = json!("provider_error");
                    record["provider_error_kind"] = json!(format!("{:?}", error.kind));
                    break;
                }
                Err(_) => {
                    record["termination"] = json!("provider_timeout");
                    break;
                }
            };
        observed_input = observed_input.saturating_add(
            response.usage.input_tokens
                + response.usage.cache_creation_tokens
                + response.usage.cache_read_tokens,
        );
        let owner = format!("watch-recovery-response-{index}");
        let replay = response
            .provider_replay
            .clone()
            .map(|update| update.with_owner_message_id(owner.clone()));
        let mut results = Vec::new();
        let mut result_blocks = Vec::new();
        let mut tool_limit = false;
        for (id, name, input) in response.tool_uses() {
            if calls >= MAX_TOOL_CALLS {
                tool_limit = true;
                break;
            }
            calls += 1;
            let (text, error, class) = fixture.dispatch(name, input).await;
            results.push(
                json!({"name":name,"input":input,"output":text,"is_error":error,"dispatch":class}),
            );
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: text,
                images: vec![],
                is_error: error,
            });
        }
        record["steps"].as_array_mut().unwrap().push(json!({"request":index+1,"response":response.content,"end_turn":response.end_turn,"usage":response.usage,"usage_observation":response.usage_observation,"tool_results":results}));
        let finished = result_blocks.is_empty() && response.end_turn;
        messages.push(LlmMessage {
            source_message_id: Some(owner),
            role: MessageRole::Assistant,
            content: response.content,
        });
        if !result_blocks.is_empty() {
            messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: result_blocks,
            });
        }
        match replay {
            Some(ProviderReplayUpdate::Anthropic(set)) => anthropic.push(set),
            Some(ProviderReplayUpdate::Responses(set)) => responses.push(set),
            Some(ProviderReplayUpdate::Clear) => {
                anthropic.clear();
                responses.clear();
            }
            None => {}
        }
        record["termination"] = json!(if tool_limit {
            "tool_call_limit"
        } else if finished {
            "final_answer"
        } else {
            "provider_call_limit"
        });
        record["observed_input_tokens_including_cache"] = json!(observed_input);
        record["tool_calls"] = json!(calls);
        persist(&path, &record);
        if finished || tool_limit {
            break;
        }
    }
    persist(&path, &record);
}
