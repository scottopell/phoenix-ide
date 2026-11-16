use super::{agent_execution::SpawnCatalog, RuntimeManager};
use phoenix_core::{
    domain::{
        db_schema::{InputOrigin, MessageContent},
        llm_types::ServiceTier,
        provider_replay::{AnthropicReplayPayload, ProviderReplaySettlement, ProviderReplayUpdate},
        sm_event::{
            PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
            SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
        },
        tool_availability::ToolAvailability,
    },
    llm_language::LlmLanguage,
    runtime_env::PhoenixRuntimeEnvironment,
};
use phoenix_db::{
    workflow::{
        AcceptAuthoritativeTurn, DirectTurnTerminalObligationInput,
        PersistedConversationProjection, TerminalizeAuthoritativeTurnInput,
    },
    ConvState, Database,
};
use phoenix_llm::{
    ContentBlock, LlmConfig, LlmMessage, LlmRequest, LlmService, MessageRole, ModelEffort,
    ModelRegistry, PromptCacheKey, SystemContent, ToolDefinition,
};
use phoenix_tools::{ToolContext, ToolRegistry};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
    TurnCommand, TurnOutcome, TurnTerminal,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

const CASES: &str = include_str!("../../../../scripts/qa/ember-event-eval/cases.json");
const EXPECTED: &str = include_str!("../../../../scripts/qa/ember-event-eval/expected.json");
const MAX_CALLS: usize = 4;
const MAX_TOTAL_CALLS: usize = 120;
const INPUT_STOP: u64 = 2_000_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    group: String,
    input: String,
    response: Response,
    mutation: Mutation,
    question: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Prose {
        text: String,
        prefix: Option<Prefix>,
    },
    ToolOnly,
    NoCarrier,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prefix {
    text: String,
    repetitions: usize,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Mutation {
    None,
    LaterResponse { text: String },
    EditTail { text: String },
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

struct World {
    runtime: Arc<RuntimeManager>,
    coordinator: String,
    registry: ToolRegistry,
    catalog: Option<phoenix_skills::AuthenticatedCoordinatorSkillCatalog>,
    packets: [String; 3],
    source: Value,
    _database_dir: tempfile::TempDir,
}

impl World {
    async fn new(case: &Case, models: Arc<ModelRegistry>) -> Self {
        let database_dir = tempfile::tempdir().unwrap();
        let db = Database::open(
            database_dir
                .path()
                .join("event-eval.sqlite")
                .to_str()
                .unwrap(),
        )
        .await
        .unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap()
            .id;
        let source = db
            .create_conversation(
                "event-eval-source",
                "Synthetic event source",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        db.watch_product_conversation(&source.product_conversation_id)
            .await
            .unwrap();
        let payload = PreparedDirectTurnPayload::from_parts(
            SubmittedDirectTurnIdentity {
                message_id: "event-eval-input".into(),
                origin: InputOrigin::UserApi,
                text: case.input.clone(),
                images: vec![],
                files: vec![],
                user_agent: None,
                skill_invocation: None,
                expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
            },
            PreparedDirectTurnDelivery {
                text: case.input.clone(),
                llm_text: None,
                images: vec![],
                files: vec![],
                user_agent: None,
                skill_invocation: None,
            },
        );
        let repo = db.workflow_repository();
        let accepted = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("event-eval-client").unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority(source.id.clone()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .established()
            .expect("fixture acceptance established")
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("new turn")
        };
        db.add_message_with_seq(
            "event-eval-input",
            &source.id,
            1,
            &MessageContent::user(&case.input),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE messages SET origin_kind='user_api' WHERE message_id='event-eval-input'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE durable_turns SET canonical_message_id='event-eval-input', materialized=1 WHERE turn_id=?1")
            .bind(i64::try_from(turn_id.0).unwrap())
            .execute(db.pool())
            .await
            .unwrap();
        let prefix = match &case.response {
            Response::Prose { prefix, .. } => prefix
                .as_ref()
                .map_or_else(String::new, |prefix| prefix.text.repeat(prefix.repetitions)),
            Response::ToolOnly | Response::NoCarrier => String::new(),
        };
        let response = match &case.response {
            Response::Prose { text, .. } => Some(MessageContent::agent(vec![ContentBlock::text(
                format!("{prefix}{text}"),
            )])),
            Response::ToolOnly => Some(MessageContent::agent(vec![ContentBlock::tool_use(
                "ending-think",
                "think",
                json!({"thought":"Readiness inspection stopped."}),
            )])),
            Response::NoCarrier => None,
        };
        let projection = PersistedConversationProjection {
            state: ConvState::Idle,
            state_updated_at: chrono::Utc::now(),
        };
        let obligation = DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: TurnTerminal::Completed,
            projection: projection.clone(),
            response_message_id: response.as_ref().map(|_| "event-eval-ending".into()),
        };
        if let Some(response) = &response {
            db.add_message_with_seq_and_terminal_obligation(
                "event-eval-ending",
                &source.id,
                2,
                response,
                None,
                None,
                &obligation,
            )
            .await
            .unwrap();
        } else {
            repo.persist_terminal_obligation(&obligation).await.unwrap();
        }
        repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command: TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
            projection: Some(projection),
            provider_replay_settlement: ProviderReplaySettlement::Preserve,
        })
        .await
        .unwrap();
        let events = db.pending_coordinator_watch_events(16).await.unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        let raw = crate::coordinator_watch_delivery::notification(event);
        db.add_message_with_seq_and_origin(
            &event.event_id,
            &coordinator,
            1,
            &MessageContent::user(&raw),
            None,
            None,
            &InputOrigin::SubscriptionEvent {
                event_id: event.event_id.clone(),
            },
        )
        .await
        .unwrap();
        let evidence = db
            .coordinator_watch_ending_evidence(&event.event_id)
            .await
            .unwrap();
        match &case.mutation {
            Mutation::None => {}
            Mutation::LaterResponse { text } => {
                db.add_message(
                    "event-eval-later",
                    &source.id,
                    &MessageContent::agent(vec![ContentBlock::text(text)]),
                    None,
                    None,
                )
                .await
                .unwrap();
            }
            Mutation::EditTail { text } => {
                let content =
                    MessageContent::agent(vec![ContentBlock::text(format!("{prefix}{text}"))]);
                sqlx::query("UPDATE messages SET content=?1 WHERE message_id='event-eval-ending'")
                    .bind(serde_json::to_string(&content.to_stored_json()).unwrap())
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
        }
        let (_, _, messages) = db
            .load_hydrated_prompt_snapshot(&coordinator)
            .await
            .unwrap()
            .into_parts();
        let MessageContent::User(projected) = &messages[0].content else {
            panic!("event user content")
        };
        let compact = projected.llm_text().to_owned();
        let runtime = Arc::new(RuntimeManager::new(
            db,
            models,
            crate::platform::PlatformCapability::None {
                details: "isolated event comparison".into(),
            },
            Arc::new(phoenix_tools::mcp::McpClientManager::new()),
            None,
        ));
        let skills = tempfile::tempdir().unwrap();
        phoenix_skills::builtin::extract_to(skills.path()).unwrap();
        let catalog =
            phoenix_skills::AuthenticatedCoordinatorSkillCatalog::discover(Some(skills.path()));
        let registry = runtime
            .coordinator_tool_registry(&coordinator, catalog.clone())
            .await
            .unwrap();
        Self {
            runtime,
            coordinator,
            registry,
            catalog,
            packets: [
                raw.clone(),
                format!("{raw}\n{}", evidence.readable(&event.event_id)),
                compact,
            ],
            source: json!({"event_id":event.event_id,"source_transcript_id":source.id,
                "product_conversation_id":source.product_conversation_id,"ending_evidence":evidence}),
            _database_dir: database_dir,
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

    async fn dispatch(&self, name: &str, input: &Value) -> (String, bool, bool) {
        if !read_allowed(name, input) {
            return (
                json!({"error":"effectful_or_remote_tool_denied_by_isolated_evaluation"})
                    .to_string(),
                true,
                true,
            );
        }
        let Some(tool) = self.registry.find_tool(name) else {
            return (json!({"error":"unknown_tool"}).to_string(), true, true);
        };
        let result = tool.run(input.clone(), self.context()).await;
        (result.output().to_owned(), !result.is_success(), false)
    }

    fn definitions(&self, model: &str) -> Vec<ToolDefinition> {
        let mut definitions = self.registry.definitions();
        let spawn = definitions
            .iter_mut()
            .find(|tool| tool.name == "spawn_agents")
            .unwrap();
        *spawn = SpawnCatalog::resolve(
            &phoenix_agents::AgentConfig::default(),
            self.runtime.llm_registry().available_execution_routes(),
        )
        .tool_definition(
            model,
            self.runtime.llm_registry().is_builtin_model(model),
            true,
        );
        definitions
    }
}

fn read_allowed(name: &str, input: &Value) -> bool {
    matches!(
        name,
        "think"
            | "query_database"
            | "read_conversation"
            | "read_event_evidence"
            | "search_conversations"
            | "resolve_reference"
    ) && input
        .get("instance")
        .is_none_or(|value| value.is_null() || value.as_str() == Some("local"))
}

fn persist(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn user(text: impl Into<String>) -> LlmMessage {
    LlmMessage {
        source_message_id: None,
        role: MessageRole::User,
        content: vec![ContentBlock::text(text)],
    }
}

fn message_snapshot(messages: &[LlmMessage]) -> Vec<Value> {
    messages.iter().map(|message| json!({
        "source_message_id":message.source_message_id,
        "role":match message.role {MessageRole::User=>"user", MessageRole::Assistant=>"assistant"},
        "content":message.content,
    })).collect()
}

#[derive(Default)]
struct Budget {
    calls: usize,
    observed_input_tokens: u64,
}

#[allow(clippy::too_many_arguments)]
async fn trial(
    world: &World,
    case: &Case,
    variant: usize,
    repetition: usize,
    model: &str,
    effort: ModelEffort,
    client: Option<&dyn LlmService>,
    path: &Path,
    budget: &mut Budget,
) {
    let prompt = crate::system_prompt::build_coordinator_system_prompt(
        LlmLanguage::default(),
        world.catalog.as_ref(),
        &phoenix_core::domain::coordinator_authority::CoordinatorAuthority::default(),
    );
    let definitions = world.definitions(model);
    let mut messages = vec![
        user(format!("Earlier user request: {}", case.input)),
        user(format!(
            "[origin: subscription_event]\n{}",
            world.packets[variant]
        )),
        user(&case.question),
    ];
    let mut record = json!({"case":case.id,"group":case.group,"variant":(["A","B","C"][variant]),
        "repetition":repetition,"model":model,"effort":effort,"system_prompt":prompt,"system_sha256":digest(prompt.as_bytes()),
        "tool_definitions":definitions,"tools_sha256":digest(serde_json::to_vec(&definitions).unwrap()),
        "initial_messages":message_snapshot(&messages),"source":world.source,"steps":[],"termination":"prepared"});
    persist(path, &record);
    let Some(client) = client else { return };
    let began = Instant::now();
    let mut anthropic = Vec::new();
    let mut responses = Vec::new();
    for index in 0..MAX_CALLS {
        if budget.calls >= MAX_TOTAL_CALLS || budget.observed_input_tokens >= INPUT_STOP {
            record["termination"] = json!("invocation_budget_stop");
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
            effective_effort: phoenix_llm::EffectiveEffort::explicit(effort),
            service_tier: phoenix_llm::EffectiveServiceTier::from_preference(
                ServiceTier::Standard,
                false,
            ),
            telemetry: None,
            cache_key: PromptCacheKey::stable(format!("ember-event-eval-{}-{variant}", case.id)),
        };
        let request_bytes = serde_json::to_vec(
            &json!({"system":prompt,"messages":message_snapshot(&request.messages),"tools":definitions}),
        )
        .unwrap()
        .len();
        budget.calls += 1;
        let call_started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(60), client.complete(&request)).await;
        let response = match result {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                record["termination"] = json!("provider_error");
                record["steps"].as_array_mut().unwrap().push(json!({"call":index+1,"error_kind":format!("{:?}",error.kind),"detail":"provider payload withheld","logical_request_bytes":request_bytes}));
                break;
            }
            Err(_) => {
                record["termination"] = json!("provider_timeout");
                break;
            }
        };
        let input_tokens = response.usage.input_tokens
            + response.usage.cache_creation_tokens
            + response.usage.cache_read_tokens;
        budget.observed_input_tokens = budget.observed_input_tokens.saturating_add(input_tokens);
        let owner = format!("event-eval-{}-{variant}-{index}", case.id);
        let replay = response
            .provider_replay
            .clone()
            .map(|update| update.with_owner_message_id(owner.clone()));
        let mut results = Vec::new();
        let mut result_blocks = Vec::new();
        for (id, name, input) in response.tool_uses() {
            let (output, is_error, denied) = world.dispatch(name, input).await;
            results.push(json!({"id":id,"name":name,"input":input,"output":output,"is_error":is_error,"denied":denied,
                "denial_kind":if denied {Some(if name=="skill" {"read_only_not_exercised"} else {"outside_allowed_capability"})} else {None}}));
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: output,
                images: vec![],
                is_error,
            });
        }
        record["steps"].as_array_mut().unwrap().push(json!({"call":index+1,
            "logical_request_bytes":request_bytes,"latency_ms":call_started.elapsed().as_millis(),
            "provider_replay_sets":anthropic.len(),"responses_replay_sets":responses.len(),
            "response":response.content,"end_turn":response.end_turn,"usage":response.usage,
            "usage_observation":response.usage_observation,"input_tokens_including_cache":input_tokens,
            "tool_results":results}));
        let finished = result_blocks.is_empty() && response.end_turn;
        messages.push(LlmMessage {
            source_message_id: Some(owner),
            role: MessageRole::Assistant,
            content: response.content,
        });
        if !finished {
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
        record["termination"] = json!(if finished {
            "final_answer"
        } else {
            "provider_call_limit"
        });
        persist(path, &record);
        if finished {
            break;
        }
    }
    record["total_latency_ms"] = json!(began.elapsed().as_millis());
    persist(path, &record);
}

#[test]
fn event_eval_effect_boundary_rejects_mutation_and_remote_routing() {
    for name in [
        "bash",
        "spawn_agents",
        "send_conversation_message",
        "watch_conversation",
        "set_authority",
        "apply_goal",
        "reminder",
        "unknown",
    ] {
        assert!(!read_allowed(name, &json!({})), "{name}");
    }
    for name in [
        "query_database",
        "read_conversation",
        "read_event_evidence",
        "search_conversations",
        "resolve_reference",
    ] {
        assert!(read_allowed(name, &json!({})));
        assert!(!read_allowed(name, &json!({"instance":"peer"})));
        assert!(!read_allowed(name, &json!({"instance":1})));
    }
    let corpus: Corpus = serde_json::from_str(CASES).unwrap();
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    assert_eq!(corpus.version, 1);
    assert_eq!(corpus.cases.len(), 6);
    for case in corpus.cases {
        assert!(expected.get(&case.id).is_some());
    }
    assert!(!CASES.contains("must_establish"));
}

#[tokio::test]
async fn event_eval_fixture_uses_captured_evidence_and_real_read_tools() {
    let corpus: Corpus = serde_json::from_str(CASES).unwrap();
    let models = Arc::new(ModelRegistry::new(&LlmConfig {
        openai_api_key: Some("offline-not-dispatched".into()),
        ..LlmConfig::default()
    }));
    for case in &corpus.cases {
        let world = World::new(case, models.clone()).await;
        let event_id = world.source["event_id"].as_str().unwrap();
        for name in [
            "think",
            "query_database",
            "read_conversation",
            "read_event_evidence",
            "search_conversations",
            "resolve_reference",
        ] {
            assert!(
                world.registry.find_tool(name).is_some(),
                "allowed tool {name} must be installed"
            );
        }
        let (query_output, error, denied) = world
            .dispatch(
                "query_database",
                &json!({"sql":"SELECT COUNT(*) AS n FROM coordinator_watch_ending_evidence"}),
            )
            .await;
        assert!(!error && !denied, "{query_output}");
        let (read, error, denied) = world
            .dispatch(
                "read_event_evidence",
                &json!({"event_id":event_id,"cursor":0}),
            )
            .await;
        assert!(!error && !denied, "{read}");
        if case.id == "edited_omitted_tail" {
            assert!(read.contains("changed"), "{read}");
        }
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(world.runtime.db().pool())
            .await
            .unwrap();
        let (_, _, denied) = world
            .dispatch(
                "send_conversation_message",
                &json!({"target":"@conv:any","message":"do work"}),
            )
            .await;
        assert!(denied);
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(world.runtime.db().pool())
            .await
            .unwrap();
        assert_eq!(before, after);
    }
}

#[tokio::test]
#[ignore = "opt-in provider comparison; explicit PHOENIX_EVENT_EVAL_MODE=prepare|live and fresh output directory required"]
async fn ember_event_eval() {
    let mode = std::env::var("PHOENIX_EVENT_EVAL_MODE").expect("explicit mode prepare|live");
    assert!(matches!(mode.as_str(), "prepare" | "live"));
    let model = std::env::var("PHOENIX_EVENT_EVAL_MODEL").expect("explicit configured model");
    let effort: ModelEffort = std::env::var("PHOENIX_EVENT_EVAL_EFFORT")
        .expect("explicit effort")
        .parse()
        .unwrap();
    let output = std::env::var("PHOENIX_EVENT_EVAL_OUTPUT").expect("fresh output directory");
    let output = Path::new(&output);
    std::fs::create_dir(output)
        .expect("new output directory with existing parent; never overwrite evidence");
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    let endpoint_hash = digest(
        serde_json::to_vec(&json!({"anthropic":config.anthropic_base_url,
        "responses":config.openai_responses_base_url,"chat":config.openai_chat_completions_base_url,
        "legacy_openai":config.openai_base_url}))
        .unwrap(),
    );
    let models = Arc::new(ModelRegistry::new(&config));
    assert!(
        models.supports_effort(&model, effort),
        "configured model must support exact effort"
    );
    let configured_client = models
        .get(&model)
        .expect("explicit model unavailable; no fallback");
    let client = (mode == "live").then_some(configured_client);
    let corpus: Corpus = serde_json::from_str(CASES).unwrap();
    let mut manifest = json!({"mode":mode,"model":model,"effort":effort,
        "connection":models.connection_for_model(&model),"endpoint_configuration_sha256":endpoint_hash,
        "cases_sha256":digest(CASES),"expected_sha256":digest(EXPECTED),"expected_shown_to_model":false,
        "test_binary_sha256":std::env::var("PHOENIX_EVENT_EVAL_BINARY_SHA256").ok(),
        "limits":{"max_calls_per_trial":MAX_CALLS,"max_total_calls":MAX_TOTAL_CALLS,"max_output_tokens":1536,
            "timeout_seconds":60,"observed_input_token_stop_threshold":INPUT_STOP},
        "promotion":{"attribution_or_authority_failures":0,"targeted_read_reduction_min_percent":25,
            "control_input_regression_max_percent":10,"control_median_latency_regression_max_percent":20},
        "repetitions":{"targeted":1,"control":3},
        "status":"running","trials":[]});
    persist(&output.join("manifest.json"), &manifest);
    let mut budget = Budget::default();
    for (case_index, case) in corpus.cases.iter().enumerate() {
        let world = World::new(case, models.clone()).await;
        let repetitions = if case.group == "control" { 3 } else { 1 };
        for repetition in 1..=repetitions {
            for offset in 0..3 {
                let variant = (case_index + repetition - 1 + offset) % 3;
                let filename =
                    format!("{}-{}-{repetition}.json", case.id, ["A", "B", "C"][variant]);
                trial(
                    &world,
                    case,
                    variant,
                    repetition,
                    &model,
                    effort,
                    client.as_deref(),
                    &output.join(&filename),
                    &mut budget,
                )
                .await;
                manifest["trials"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(filename));
                manifest["provider_calls"] = json!(budget.calls);
                manifest["observed_input_tokens_including_cache"] =
                    json!(budget.observed_input_tokens);
                persist(&output.join("manifest.json"), &manifest);
            }
        }
    }
    manifest["status"] = json!("recorded_not_qualified");
    persist(&output.join("manifest.json"), &manifest);
}
