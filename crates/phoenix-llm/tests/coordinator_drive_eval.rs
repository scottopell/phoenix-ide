//! Isolated, opt-in coordinator behavior evaluation. Never imports application tools.
use phoenix_core::{
    domain::{
        llm_types::{ServiceTier, ToolDefinition},
        provider_replay::{AnthropicReplayPayload, ProviderReplayUpdate},
        tool_availability::ToolAvailability,
    },
    runtime_env::PhoenixRuntimeEnvironment,
};
use phoenix_llm::{
    ContentBlock, LlmConfig, LlmMessage, LlmRequest, LlmResponse, LlmService, MessageRole,
    ModelEffort, ModelRegistry, PromptCacheKey, SystemContent,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

const FIXTURE: &str = include_str!("../../../scripts/qa/coordinator-drive-eval/fixtures.json");
const BUTTON_SOURCE: &str = include_str!("../../../ui/src/pages/coordinatorBriefing.ts");
// Exact system prompt carried in the initial A trial, not the evolving product prompt.
const EVALUATED_BASELINE: &str =
    include_str!("../../../scripts/qa/coordinator-drive-eval/baseline-system-prompt.txt");
const CALL_LIMIT: usize = 10;
const OUTPUT_TOKENS: u32 = 3072;

#[derive(Debug, Deserialize)]
struct Fixture {
    schema_version: u32,
    scenarios: Vec<Scenario>,
    variants: BTreeMap<String, Variant>,
    limits: Limits,
}
#[derive(Debug, Deserialize)]
struct Limits {
    max_provider_calls_per_trial: usize,
    provider_timeout_seconds: u64,
    max_output_tokens: u32,
}
#[derive(Debug, Deserialize)]
struct Scenario {
    id: String,
    context: Vec<History>,
    rows: Vec<Value>,
    conversations: BTreeMap<String, Vec<Value>>,
}
#[derive(Debug, Deserialize)]
struct History {
    origin: String,
    role: String,
    text: String,
}
#[derive(Debug, Deserialize)]
struct Variant {
    button: String,
    principles: Option<String>,
    #[serde(default)]
    replace_receipt_rule: bool,
}

fn fixture() -> Fixture {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid frozen fixture");
    assert_eq!(fixture.schema_version, 2);
    assert_eq!(fixture.limits.max_provider_calls_per_trial, CALL_LIMIT);
    assert_eq!(fixture.limits.max_output_tokens, OUTPUT_TOKENS);
    assert_eq!(fixture.limits.provider_timeout_seconds, 120);
    fixture
}

fn definition(name: &str, description: &str, schema: Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        defer_loading: false,
    }
}

fn tools() -> Vec<ToolDefinition> {
    vec![
        definition(
            "query_database",
            "Read current coordinator-visible relational work rows. Supply a read-only SQL statement as in Phoenix; this isolated fixture returns its fixed rows regardless of SQL and NEVER executes it.",
            json!({"type":"object","properties":{"sql":{"type":"string","minLength":1}},"required":["sql"]}),
        ),
        definition(
            "read_conversation",
            "Read exact historical transcript messages and origin attribution for a known ProductConversation.",
            json!({"type":"object","properties":{"conversation_id":{"oneOf":[{"type":"string","pattern":"^@conv:[^\\s#]+$"},{"type":"string","pattern":"^@transcript:[^\\s#]+(?:#message-[^\\s#]+)?$"}]},"cursor":{"type":"integer","minimum":0}},"required":["conversation_id"],"additionalProperties":false}),
        ),
        definition(
            "send_conversation_message",
            "Send one nonempty text message to an existing non-Coordinator conversation. Accepted/queued is NOT completion or acknowledgement.",
            json!({"type":"object","properties":{"target":{"type":"string","pattern":"^@(?:conv|transcript):[^\\s#]+$"},"message":{"type":"string","minLength":1},"message_id":{"type":"string","format":"uuid"}},"required":["target","message","message_id"],"additionalProperties":false}),
        ),
        definition(
            "ask_user_question",
            "Ask a focused genuine decision question and suspend this trial. This must be the sole tool call in its response; send independent work in a prior response.",
            json!({"type":"object","required":["questions"],"properties":{"questions":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"object","required":["question","header","options"],"properties":{"question":{"type":"string"},"header":{"type":"string","maxLength":12},"options":{"type":"array","minItems":2,"maxItems":4,"items":{"type":"object","required":["label"],"properties":{"label":{"type":"string"},"description":{"type":"string"},"preview":{"type":"string"}}}},"multiSelect":{"type":"boolean"}}}}}}),
        ),
    ]
}

fn system(variant: &Variant) -> String {
    let mut prompt = EVALUATED_BASELINE.trim_end_matches('\n').to_string();
    if variant.replace_receipt_rule {
        prompt = prompt.replace(
            "Message only when intervention is useful, report every target's committed result independently, and never imply recipient understanding, acknowledgement, execution, or completion.",
            "Message only when intervention is useful. Never imply recipient understanding, acknowledgement, execution, or completion from message acceptance; when reporting delivery, distinguish each target's committed result.",
        );
    }
    if let Some(principles) = &variant.principles {
        prompt.push_str("\n\nCoordinator operating principles: ");
        prompt.push_str(principles);
    }
    prompt
}

fn user(text: impl Into<String>) -> LlmMessage {
    LlmMessage {
        source_message_id: None,
        role: MessageRole::User,
        content: vec![ContentBlock::text(text)],
    }
}

fn start_messages(scenario: &Scenario, variant: &Variant) -> Vec<LlmMessage> {
    let mut messages = scenario
        .context
        .iter()
        .map(|entry| {
            assert!(matches!(
                entry.origin.as_str(),
                "user_api" | "system_generated"
            ));
            assert_eq!(
                entry.role, "user",
                "fixture context must be user-role history"
            );
            user(format!("[origin: {}] {}", entry.origin, entry.text))
        })
        .collect::<Vec<_>>();
    if !matches!(
        scenario.id.as_str(),
        "quiet_subscription" | "continuation_pause"
    ) {
        messages.push(user(&variant.button));
    }
    messages
}

const TEST_MESSAGE_ID: &str = "00000000-0000-4000-8000-000000000001";

fn valid_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn nonempty<'a>(input: &'a Value, field: &str) -> Result<&'a str, &'static str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing_or_empty_argument")
}

#[derive(Default)]
struct FakeWorld {
    actions: Vec<Value>,
    suspended: bool,
}

impl FakeWorld {
    fn dispatch(&mut self, scenario: &Scenario, name: &str, input: &Value) -> (Value, bool) {
        let result = match name {
            "query_database" => nonempty(input, "sql").map(|_| {
                json!({"rows": scenario.rows, "source": "synthetic_live_current_rows", "sql_executed": false})
            }),
            "read_conversation" => nonempty(input, "conversation_id").and_then(|id| {
                let (reference, anchor) = id.split_once("#message-")
                    .map_or((id, None), |(reference, anchor)| (reference, Some(anchor)));
                let target = scenario.rows.iter().find_map(|row| {
                    let current = row["current_transcript_id"].as_str()?;
                    let stable = row["product_conversation_id"].as_str()?;
                    (reference == stable || reference == current).then_some(stable)
                }).ok_or("unknown_conversation")?;
                let cursor = usize::try_from(input.get("cursor").and_then(Value::as_u64).unwrap_or(0))
                    .map_err(|_| "invalid_cursor")?;
                scenario.conversations.get(target)
                    .map(|messages| {
                        let page = messages.iter().skip(cursor).filter(|message| {
                            anchor.is_none_or(|anchor| message["message_id"].as_str() == Some(anchor))
                        }).collect::<Vec<_>>();
                        json!({"conversation_id":id,"messages":page,"has_more":false,"cursor":null,"source":"synthetic_exact_tool_read"})
                    })
                    .ok_or("unknown_conversation")
            }),
            "send_conversation_message" => nonempty(input, "target").and_then(|id| {
                let row = scenario.rows.iter().find(|row| {
                    let current = row["current_transcript_id"].as_str();
                    let stable = row["product_conversation_id"].as_str();
                    stable == Some(id) || current == Some(id)
                }).ok_or("unknown_or_coordinator_target")?;
                let target = row["product_conversation_id"].as_str().ok_or("missing_target_id")?;
                if target == "@conv:coordinator" || !scenario.conversations.contains_key(target) {
                    return Err("unknown_or_coordinator_target");
                }
                let outcome = match row["state"].as_str() {
                    Some("idle") => "delivered",
                    Some("working") => "queued_as_steering",
                    _ => return Err("unsupported_fixture_state"),
                };
                let message = nonempty(input, "message")?;
                let message_id = nonempty(input, "message_id")?;
                if !valid_uuid(message_id) { return Err("invalid_message_id"); }
                self.actions.push(json!({"kind":outcome,"target":id,"conversation_id":target,"message":message,"message_id":message_id}));
                Ok(json!({"target":id,"conversation_id":target,"message_id":message_id,"outcome":outcome}))
            }),
            "ask_user_question" => {
                let questions = input.get("questions").and_then(Value::as_array).ok_or("missing_questions");
                questions.and_then(|questions| {
                    if questions.is_empty() || questions.len() > 4 { return Err("invalid_questions"); }
                    for item in questions {
                        nonempty(item, "question")?;
                        let header = nonempty(item, "header")?;
                        if header.chars().count() > 12 { return Err("invalid_header"); }
                        let options = item.get("options").and_then(Value::as_array).ok_or("missing_options")?;
                        if !(2..=4).contains(&options.len()) || options.iter().any(|option| nonempty(option, "label").is_err()) {
                            return Err("invalid_options");
                        }
                    }
                    self.suspended = true;
                    self.actions.push(json!({"kind":"question_suspended","questions":questions}));
                    Ok(json!({"status":"question_pending_user","trial_suspended":true}))
                })
            },
            _ => Err("unknown_tool"),
        };
        match result {
            Ok(value) => (value, false),
            Err(code) => (json!({"error":code}), true),
        }
    }
}

// Reject mixed AUQ batches before dispatch, so tool-call ordering cannot smuggle a
// side effect in the same provider response as a suspending question.
fn dispatch_response(
    world: &mut FakeWorld,
    scenario: &Scenario,
    response: &LlmResponse,
) -> Vec<Value> {
    let calls = response.tool_uses();
    let mixed_question = calls
        .iter()
        .any(|(_, name, _)| *name == "ask_user_question")
        && calls.len() != 1;
    calls
        .into_iter()
        .map(|(id, name, input)| {
            let (result, is_error) = if mixed_question {
                (json!({"error":"question_must_be_sole_call"}), true)
            } else {
                world.dispatch(scenario, name, input)
            };
            json!({"tool_use_id":id,"name":name,"input":input,"result":result,"is_error":is_error})
        })
        .collect()
}

fn append_turn(
    messages: &mut Vec<LlmMessage>,
    response: &LlmResponse,
    results: &[Value],
    owner_id: String,
) {
    messages.push(LlmMessage {
        source_message_id: Some(owner_id),
        role: MessageRole::Assistant,
        content: response.content.clone(),
    });
    if !results.is_empty() {
        messages.push(LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: results
                .iter()
                .map(|result| ContentBlock::ToolResult {
                    tool_use_id: result["tool_use_id"].as_str().unwrap().to_string(),
                    content: result["result"].to_string(),
                    images: vec![],
                    is_error: result["is_error"].as_bool().unwrap(),
                })
                .collect(),
        });
    }
}

fn snapshot(messages: &[LlmMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            json!({
                "role": if message.role == MessageRole::User { "user" } else { "assistant" },
                "source_message_id": message.source_message_id,
                "content": message.content,
            })
        })
        .collect()
}

fn create_fresh_output_directory(output: &Path) -> std::io::Result<()> {
    std::fs::create_dir(output)
}

#[test]
fn output_directory_rejects_existing_evidence_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("evidence");
    create_fresh_output_directory(&output).unwrap();
    let artifact = output.join("manifest.json");
    std::fs::write(&artifact, b"original evidence").unwrap();
    assert_eq!(
        create_fresh_output_directory(&output).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&artifact).unwrap(), b"original evidence");
    assert_eq!(
        create_fresh_output_directory(&temp.path().join("missing-parent/output"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

fn persist(path: &Path, value: &Value) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("serialize trial"),
    )
    .expect("persist trial evidence");
}

fn selected<'a>(available: impl Iterator<Item = &'a str>, env: &str) -> Vec<String> {
    let all = available.map(str::to_string).collect::<Vec<_>>();
    match std::env::var(env) {
        Ok(filter) => {
            let selected = filter.split(',').map(str::trim).collect::<Vec<_>>();
            assert!(
                !selected.is_empty() && selected.iter().all(|item| all.iter().any(|v| v == item)),
                "invalid {env} selection"
            );
            assert_eq!(
                selected.len(),
                selected
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                "duplicate {env} selection"
            );
            selected.iter().map(|item| (*item).to_string()).collect()
        }
        Err(_) => all,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_trial(
    scenario: &Scenario,
    variant_id: &str,
    variant: &Variant,
    model: &str,
    effort: ModelEffort,
    client: &dyn LlmService,
    path: &Path,
    repetition: usize,
) {
    let prompt = system(variant);
    let mut messages = start_messages(scenario, variant);
    let mut world = FakeWorld::default();
    let mut anthropic_sets = Vec::new();
    let mut responses_sets = Vec::new();
    let mut evidence = json!({
        "scenario":scenario.id,"variant":variant_id,"repetition":repetition,
        "model":model,"effort":effort,"service_tier":"standard","max_output_tokens":OUTPUT_TOKENS,
        "max_provider_calls":CALL_LIMIT,"timeout_seconds":120,
        "system_prompt":prompt,"initial_messages":snapshot(&messages),
        "tool_definitions":tools(),"synthetic_rows":scenario.rows,
        "synthetic_tool_readable_messages":scenario.conversations,
        "steps":[],"actions":[],"termination":"prepared"
    });
    persist(path, &evidence);
    for call_index in 0..CALL_LIMIT {
        let request = LlmRequest {
            system: vec![SystemContent::new(&prompt)],
            messages: messages.clone(),
            provider_replay: if anthropic_sets.is_empty() {
                None
            } else {
                Some(
                    AnthropicReplayPayload::new(anthropic_sets.clone())
                        .expect("valid provider replay"),
                )
            },
            responses_replay: responses_sets.clone(),
            tool_availability: ToolAvailability::all(tools()),
            max_tokens: OUTPUT_TOKENS,
            effective_effort: phoenix_llm::EffectiveEffort::explicit(effort),
            service_tier: phoenix_llm::EffectiveServiceTier::from_preference(
                ServiceTier::Standard,
                false,
            ),
            telemetry: None,
            cache_key: PromptCacheKey::stable(format!("coordinator-drive-eval-{variant_id}")),
        };
        let mut step = json!({"call":call_index+1,"request_messages":snapshot(&request.messages),"provider_replay_sets":anthropic_sets.len(),"responses_replay_sets":responses_sets.len()});
        let completed =
            tokio::time::timeout(Duration::from_secs(120), client.complete(&request)).await;
        let response = match completed {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                step["error"] = json!({"kind":format!("{:?}", error.kind),"detail":"provider error payload withheld"});
                evidence["steps"].as_array_mut().unwrap().push(step);
                evidence["termination"] = json!("provider_error");
                persist(path, &evidence);
                return;
            }
            Err(_) => {
                step["error"] = json!({"kind":"timeout","detail":"120 seconds per provider call"});
                evidence["steps"].as_array_mut().unwrap().push(step);
                evidence["termination"] = json!("provider_timeout");
                persist(path, &evidence);
                return;
            }
        };
        let owner_id = format!(
            "eval-{variant_id}-{}-{repetition}-{call_index}",
            scenario.id
        );
        let replay_update = response
            .provider_replay
            .clone()
            .map(|update| update.with_owner_message_id(owner_id.clone()));
        let results = dispatch_response(&mut world, scenario, &response);
        step["response"] = json!({"content":response.content,"end_turn":response.end_turn,"usage":response.usage,"provider_replay_kind":match replay_update {Some(ProviderReplayUpdate::Anthropic(_))=>"anthropic",Some(ProviderReplayUpdate::Responses(_))=>"responses",Some(ProviderReplayUpdate::Clear)=>"clear",None=>"none"}});
        step["tool_results"] = json!(results);
        append_turn(&mut messages, &response, &results, owner_id);
        match replay_update {
            Some(ProviderReplayUpdate::Anthropic(set)) => anthropic_sets.push(set),
            Some(ProviderReplayUpdate::Responses(set)) => responses_sets.push(set),
            Some(ProviderReplayUpdate::Clear) => {
                anthropic_sets.clear();
                responses_sets.clear();
            }
            None => {}
        }
        evidence["steps"].as_array_mut().unwrap().push(step);
        evidence["actions"] = json!(world.actions);
        evidence["termination"] = json!(if world.suspended {
            "question_suspended"
        } else if response.tool_uses().is_empty() {
            "final_answer"
        } else if call_index + 1 == CALL_LIMIT {
            "provider_call_limit"
        } else {
            "continuing"
        });
        persist(path, &evidence);
        if world.suspended || response.tool_uses().is_empty() {
            return;
        }
    }
}

#[test]
fn frozen_baseline_and_hidden_expectations() {
    let fixture = fixture();
    assert!(BUTTON_SOURCE.contains("label: 'Check in & drive'"));
    assert!(BUTTON_SOURCE.contains("including idle owners"));
    assert!(!BUTTON_SOURCE.contains("Do not send messages, change anything"));
    let evaluated: Value = serde_json::from_str(include_str!(
        "../../../scripts/qa/coordinator-drive-eval/results/initial/idle_owner-A-1.json"
    ))
    .unwrap();
    assert_eq!(
        system(&fixture.variants["A"]),
        evaluated["system_prompt"].as_str().unwrap()
    );
    let evaluated_c2: Value = serde_json::from_str(include_str!(
        "../../../scripts/qa/coordinator-drive-eval/results/followup/quiet_subscription-C2-1.json"
    ))
    .unwrap();
    assert_eq!(
        system(&fixture.variants["C2"]),
        evaluated_c2["system_prompt"].as_str().unwrap()
    );
    assert_eq!(fixture.variants["B"].button, fixture.variants["C"].button);
    assert!(fixture.variants["C"].principles.is_some());
    for id in ["quiet_subscription", "continuation_pause"] {
        let case = fixture.scenarios.iter().find(|s| s.id == id).unwrap();
        assert_eq!(
            snapshot(&start_messages(case, &fixture.variants["A"])),
            snapshot(&start_messages(case, &fixture.variants["B"]))
        );
    }
    assert!(!FIXTURE.contains("\"expected\""));
    let expected: Value = serde_json::from_str(include_str!(
        "../../../scripts/qa/coordinator-drive-eval/expected.json"
    ))
    .unwrap();
    assert_eq!(fixture.scenarios.len(), 6);
    for scenario in &fixture.scenarios {
        assert!(expected.get(&scenario.id).is_some());
        assert!(!scenario.rows.is_empty());
    }
}

#[test]
fn fake_tools_reject_unknown_and_wrong_targets() {
    let fixture = fixture();
    let scenario = &fixture.scenarios[0];
    let mut world = FakeWorld::default();
    assert_eq!(
        world.dispatch(scenario, "bash", &json!({})).0["error"],
        "unknown_tool"
    );
    for id in ["@conv:missing", "@conv:coordinator"] {
        assert!(
            world
                .dispatch(
                    scenario,
                    "send_conversation_message",
                    &json!({"target":id,"message":"go","message_id":TEST_MESSAGE_ID})
                )
                .1
        );
    }
    assert!(
        world
            .dispatch(
                scenario,
                "send_conversation_message",
                &json!({"target":"@conv:owner","message":"  ","message_id":TEST_MESSAGE_ID})
            )
            .1
    );
    assert!(
        world
            .dispatch(
                scenario,
                "send_conversation_message",
                &json!({"target":"@conv:owner","message":"Go","message_id":"not-a-uuid"})
            )
            .1
    );
    assert!(world.actions.is_empty());
    assert!(
        !world
            .dispatch(
                scenario,
                "send_conversation_message",
                &json!({"target":"@conv:owner","message":"Please complete the checklist","message_id":TEST_MESSAGE_ID})
            )
            .1
    );
    assert_eq!(world.actions.len(), 1);
    assert_eq!(world.actions[0]["kind"], "delivered");
}

#[test]
fn fake_send_distinguishes_idle_delivery_from_working_steering() {
    let fixture = fixture();
    let mut scenario = fixture.scenarios.into_iter().next().unwrap();
    let mut world = FakeWorld::default();
    let input = json!({"target":"@conv:owner","message":"Continue","message_id":TEST_MESSAGE_ID});

    let (delivered, error) = world.dispatch(&scenario, "send_conversation_message", &input);
    assert!(!error);
    assert_eq!(
        delivered,
        json!({"target":"@conv:owner","conversation_id":"@conv:owner","message_id":TEST_MESSAGE_ID,"outcome":"delivered"})
    );
    scenario.rows[0]["state"] = json!("working");
    let (queued, error) = world.dispatch(&scenario, "send_conversation_message", &input);
    assert!(!error);
    assert_eq!(
        queued,
        json!({"target":"@conv:owner","conversation_id":"@conv:owner","message_id":TEST_MESSAGE_ID,"outcome":"queued_as_steering"})
    );
    assert_eq!(
        world
            .actions
            .iter()
            .map(|action| action["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["delivered", "queued_as_steering"]
    );

    scenario.rows[0]["state"] = json!("unknown");
    let (rejected, error) = world.dispatch(&scenario, "send_conversation_message", &input);
    assert!(error);
    assert_eq!(rejected["error"], "unsupported_fixture_state");
    assert_eq!(world.actions.len(), 2);
}

#[test]
fn question_suspension_and_mixed_batch_have_no_side_effects() {
    let fixture = fixture();
    let scenario = &fixture.scenarios[2];
    let mut world = FakeWorld::default();
    let mixed = LlmResponse::non_streaming(
        vec![
            ContentBlock::ToolUse {
                id: "s".into(),
                name: "send_conversation_message".into(),
                input: json!({"target":"@conv:lane-b","message":"Go","message_id":TEST_MESSAGE_ID}),
            },
            ContentBlock::ToolUse {
                id: "q".into(),
                name: "ask_user_question".into(),
                input: json!({"questions":[{"question":"30 or 90?","header":"Retention","options":[{"label":"30"},{"label":"90"}]}]}),
            },
        ],
        false,
        phoenix_llm::Usage::default(),
    );
    assert!(dispatch_response(&mut world, scenario, &mixed)
        .iter()
        .all(|r| r["is_error"] == true));
    assert!(world.actions.is_empty());
    let separate = LlmResponse::non_streaming(
        vec![ContentBlock::ToolUse {
            id: "q".into(),
            name: "ask_user_question".into(),
            input: json!({"questions":[{"question":"30 or 90?","header":"Retention","options":[{"label":"30"},{"label":"90"}]}]}),
        }],
        false,
        phoenix_llm::Usage::default(),
    );
    assert_eq!(
        dispatch_response(&mut world, scenario, &separate)[0]["is_error"],
        false
    );
    assert!(world.suspended);
}

#[test]
fn revised_candidate_removes_conflicting_receipt_rule() {
    let fixture = fixture();
    let prompt = system(&fixture.variants["C2"]);
    assert!(!prompt.contains("report every target's committed result independently"));
    assert!(prompt.contains("when reporting delivery, distinguish each target's committed result"));
    assert!(prompt.contains("Coordinate quietly by default"));
}

#[test]
fn exact_message_reads_and_end_of_history_are_bounded() {
    let fixture = fixture();
    let scenario = &fixture.scenarios[0];
    let mut world = FakeWorld::default();
    let (page, error) = world.dispatch(
        scenario,
        "read_conversation",
        &json!({
            "conversation_id":"@transcript:owner-current#message-synthetic-message-1"
        }),
    );
    assert!(!error);
    assert_eq!(page["messages"].as_array().unwrap().len(), 1);
    assert_eq!(page["messages"][0]["origin"], "user_api");
    let (page, error) = world.dispatch(
        scenario,
        "read_conversation",
        &json!({
            "conversation_id":"@conv:owner", "cursor":2
        }),
    );
    assert!(!error);
    assert!(page["messages"].as_array().unwrap().is_empty());
    assert_eq!(page["has_more"], false);
    assert!(world.actions.is_empty());
}

#[test]
fn call_budget_is_frozen() {
    assert_eq!(fixture().limits.max_provider_calls_per_trial, CALL_LIMIT);
    assert_eq!(fixture().limits.provider_timeout_seconds, 120);
    assert_eq!(fixture().limits.max_output_tokens, OUTPUT_TOKENS);
}

struct ScriptedService {
    calls: std::sync::atomic::AtomicUsize,
    question: bool,
}

#[async_trait::async_trait]
impl LlmService for ScriptedService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, phoenix_llm::LlmError> {
        let index = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(request.max_tokens, OUTPUT_TOKENS);
        assert_eq!(request.tool_availability.declarations().len(), 4);
        if index > 0 {
            assert!(request.messages.iter().any(|message| message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))));
        }
        let (name, input) = if self.question && index == 1 {
            (
                "ask_user_question",
                json!({"questions":[{"question":"30 or 90 days?","header":"Retention","options":[{"label":"30 days"},{"label":"90 days"}]}]}),
            )
        } else {
            ("query_database", json!({"sql":"SELECT * FROM work_scopes"}))
        };
        Ok(LlmResponse::non_streaming(
            vec![ContentBlock::ToolUse {
                id: format!("tool-{index}"),
                name: name.into(),
                input,
            }],
            false,
            phoenix_llm::Usage {
                input_tokens: 10,
                output_tokens: 4,
                ..Default::default()
            },
        ))
    }

    fn model_id(&self) -> &'static str {
        "scripted"
    }
}

#[tokio::test]
async fn trial_enforces_call_budget_and_records_raw_steps() {
    let fixture = fixture();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("trial.json");
    let service = ScriptedService {
        calls: std::sync::atomic::AtomicUsize::default(),
        question: false,
    };
    run_trial(
        &fixture.scenarios[0],
        "B",
        &fixture.variants["B"],
        "scripted",
        ModelEffort::Medium,
        &service,
        &path,
        1,
    )
    .await;
    let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        service.calls.load(std::sync::atomic::Ordering::SeqCst),
        CALL_LIMIT
    );
    assert_eq!(record["termination"], "provider_call_limit");
    assert_eq!(record["steps"].as_array().unwrap().len(), CALL_LIMIT);
    assert_eq!(record["steps"][0]["response"]["usage"]["input_tokens"], 10);
    assert_eq!(
        record["steps"][0]["tool_results"][0]["result"]["sql_executed"],
        false
    );
}

#[tokio::test]
async fn trial_stops_after_question_without_further_calls() {
    let fixture = fixture();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("trial.json");
    let service = ScriptedService {
        calls: std::sync::atomic::AtomicUsize::default(),
        question: true,
    };
    run_trial(
        &fixture.scenarios[2],
        "C",
        &fixture.variants["C"],
        "scripted",
        ModelEffort::Medium,
        &service,
        &path,
        1,
    )
    .await;
    let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(service.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(record["termination"], "question_suspended");
    assert_eq!(record["actions"][0]["kind"], "question_suspended");
}

#[tokio::test]
#[ignore = "explicit MODE=prepare or MODE=live; live bills provider calls"]
async fn coordinator_drive_eval() {
    let mode = std::env::var("PHOENIX_COORD_EVAL_MODE")
        .expect("explicit PHOENIX_COORD_EVAL_MODE=prepare|live");
    assert!(mode == "prepare" || mode == "live", "invalid eval mode");
    let model =
        std::env::var("PHOENIX_COORD_EVAL_MODEL").expect("explicit PHOENIX_COORD_EVAL_MODEL");
    assert!(!model.trim().is_empty());
    let effort: ModelEffort = std::env::var("PHOENIX_COORD_EVAL_EFFORT")
        .expect("explicit PHOENIX_COORD_EVAL_EFFORT")
        .parse()
        .expect("valid effort");
    let output = std::env::var("PHOENIX_COORD_EVAL_OUTPUT")
        .expect("explicit PHOENIX_COORD_EVAL_OUTPUT directory");
    let output = Path::new(&output);
    let repetitions: usize = std::env::var("PHOENIX_COORD_EVAL_REPETITIONS")
        .expect("explicit PHOENIX_COORD_EVAL_REPETITIONS=1|2")
        .parse()
        .expect("repetitions must be 1 or 2");
    assert!((1..=2).contains(&repetitions), "repetitions must be 1 or 2");
    let fixture = fixture();
    let variants = selected(
        fixture.variants.keys().map(String::as_str),
        "PHOENIX_COORD_EVAL_VARIANTS",
    );
    let scenarios = selected(
        fixture.scenarios.iter().map(|s| s.id.as_str()),
        "PHOENIX_COORD_EVAL_SCENARIOS",
    );
    assert!(
        variants.len() * scenarios.len() * repetitions <= 18,
        "at most 18 trials per invocation; select explicit variants/scenarios"
    );
    create_fresh_output_directory(output)
        .expect("create fresh output directory (existing evidence is never reused)");
    let manifest = json!({"mode":mode,"model":model,"effort":effort,"repetitions":repetitions,"variants":variants,"scenarios":scenarios,"limits":{"max_provider_calls":CALL_LIMIT,"timeout_seconds":120,"max_output_tokens":OUTPUT_TOKENS},"fixture":serde_json::from_str::<Value>(FIXTURE).unwrap(),"expected_outcomes_file":"expected.json (not shown to model)"});
    persist(&output.join("manifest.json"), &manifest);
    if mode == "prepare" {
        for scenario in &fixture.scenarios {
            if !scenarios.contains(&scenario.id) {
                continue;
            }
            for variant_id in &variants {
                let variant = &fixture.variants[variant_id];
                for rep in 1..=repetitions {
                    let record = json!({"scenario":scenario.id,"variant":variant_id,"repetition":rep,"model":model,"effort":effort,"system_prompt":system(variant),"initial_messages":snapshot(&start_messages(scenario,variant)),"tool_definitions":tools(),"synthetic_rows":scenario.rows,"synthetic_tool_readable_messages":scenario.conversations,"termination":"prepared","steps":[],"actions":[]});
                    persist(
                        &output.join(format!("{}-{variant_id}-{rep}.json", scenario.id)),
                        &record,
                    );
                }
            }
        }
        return;
    }
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    let registry = ModelRegistry::new(&config);
    assert!(
        registry.supports_effort(&model, effort),
        "explicit effort unsupported by model"
    );
    let client = registry
        .get(&model)
        .expect("evaluation model not configured");
    for scenario in &fixture.scenarios {
        if !scenarios.contains(&scenario.id) {
            continue;
        }
        for variant_id in &variants {
            for rep in 1..=repetitions {
                run_trial(
                    scenario,
                    variant_id,
                    &fixture.variants[variant_id],
                    &model,
                    effort,
                    client.as_ref(),
                    &output.join(format!("{}-{variant_id}-{rep}.json", scenario.id)),
                    rep,
                )
                .await;
            }
        }
    }
}
