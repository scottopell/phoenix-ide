use super::*;
use phoenix_core::domain::sm_state::QuestionRequestId;
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

const MAX_REQUESTS: usize = 10;
const MAX_BYTES: usize = 2_000_000;
const MAX_OUTPUT: usize = 2048;
const MODEL: &str = "gpt-6-astra";

fn digest(bytes: impl AsRef<[u8]>) -> String {
    use sha2::Digest;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes.as_ref()).iter().fold(
        String::with_capacity(64),
        |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        },
    )
}
fn save_json(path: &Path, value: &impl Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[derive(Debug, Serialize)]
struct TurnTrace {
    source: String,
    input_message_id: String,
    authority_turn_id: Option<u64>,
    question_request_id: Option<String>,
    visible_text: String,
    assistant_messages: Vec<Value>,
    tool_results: Vec<Value>,
    state: ConvState,
}

struct Session {
    source: String,
    manager: Arc<crate::runtime::RuntimeManager>,
    db: crate::db::Database,
    handle: crate::runtime::ConversationHandle,
    task: tokio::task::JoinHandle<crate::runtime::executor::RuntimeExitDisposition>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Session {
    async fn start<C: LlmClient + 'static>(
        fixture: &Fixture<C>,
        source: &str,
        catalog: Option<phoenix_skills::AuthenticatedCoordinatorSkillCatalog>,
    ) -> Self {
        let conversation = fixture.db.get_conversation(source).await.unwrap();
        assert!(matches!(conversation.state, ConvState::Idle));
        assert_eq!(conversation.model.as_deref(), Some(fixture.model.as_str()));
        let mut context = ConvContext::new(
            source,
            fixture.cwd.clone(),
            &fixture.model,
            200_000,
            MAX_OUTPUT,
        );
        context.is_coordinator = true;
        context.llm_language = phoenix_core::llm_language::LlmLanguage::PhoenixNative;
        context.effort = conversation.effort;
        let registry = fixture
            .manager
            .coordinator_tool_registry(source, catalog.clone())
            .await
            .unwrap();
        let tools = GuardedTools {
            inner: ToolRegistryExecutor::builtin_only(registry, Arc::from([]))
                .with_coordinator_skill_catalog(catalog),
            db: fixture.db.clone(),
            source_checkpoint_presence: fixture.source_checkpoint_presence.clone(),
            calls: fixture.calls.clone(),
            denied: fixture.denied.clone(),
        };
        let (event_tx, event_rx) = mpsc::channel(16);
        let (ack_tx, ack_rx) = mpsc::channel(1);
        let (state_tx, state_rx) = watch::channel(conversation.state.clone());
        let sequence = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1",
        )
        .bind(source)
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        let broadcast = SseBroadcaster::new(128, sequence);
        let runtime = ConversationRuntime::new(
            context,
            conversation.state,
            DatabaseStorage::new(fixture.db.clone()),
            fixture.client.clone(),
            tools,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            fixture.registry.clone(),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx.clone(),
            broadcast.clone(),
        )
        .with_parent_tool_cycle_cap(8)
        .with_acknowledged_event_receiver(ack_rx)
        .with_state_watcher(state_tx)
        .with_fatal_local_authority_fence(fixture.manager.fatal_local_authority_fence.clone());
        let handle = crate::runtime::ConversationHandle {
            event_tx,
            acknowledged_event_tx: ack_tx,
            handoff_review_tx: None,
            turn_trigger: runtime.turn_trigger_slot(),
            broadcast_tx: broadcast,
            identity: Arc::new(()),
            state_rx,
            startup_llm_recovery_ack: Arc::new(tokio::sync::Mutex::new(None)),
        };
        assert!(fixture
            .manager
            .runtimes
            .write()
            .await
            .insert(source.into(), handle.clone())
            .is_none());
        Self {
            source: source.into(),
            manager: fixture.manager.clone(),
            db: fixture.db.clone(),
            handle,
            task: tokio::spawn(runtime.run()),
        }
    }
    async fn close(mut self) {
        let mut handles = self.manager.runtimes.write().await;
        if handles
            .get(&self.source)
            .is_some_and(|handle| Arc::ptr_eq(&handle.identity, &self.handle.identity))
        {
            handles.remove(&self.source);
        }
        drop(handles);
        self.task.abort();
        let _ = (&mut self.task).await;
    }
    async fn input(&mut self, prompt: &str) -> TurnTrace {
        tokio::time::timeout(Duration::from_secs(120), self.input_inner(prompt))
            .await
            .expect("whole admitted turn exceeded120seconds")
    }
    async fn input_inner(&mut self, prompt: &str) -> TurnTrace {
        assert!(matches!(*self.handle.state_rx.borrow(), ConvState::Idle));
        let input_id = uuid::Uuid::new_v4().to_string();
        let (event, turn_id) = admitted_input(&self.db, &self.source, prompt, &input_id).await;
        let (acknowledgement, acknowledged) = tokio::sync::oneshot::channel();
        let (retired, retirement) = tokio::sync::oneshot::channel();
        self.handle
            .acknowledged_event_tx
            .send(crate::runtime::AcknowledgedEventRequest {
                event,
                acknowledgement,
                retirement,
            })
            .await
            .unwrap();
        assert_eq!(
            acknowledged.await.unwrap().unwrap(),
            crate::runtime::AcknowledgedEventOutcome::Settled
        );
        drop(retired);
        let accepted = self
            .db
            .workflow_repository()
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        let phoenix_workflow::Materialization::Materialized { message_id } =
            accepted.materialization
        else {
            panic!("actual input receipt")
        };
        self.wait_visible(&message_id.0, Some(turn_id.0), None)
            .await
    }
    async fn reply(&mut self, answer: &str) -> TurnTrace {
        tokio::time::timeout(Duration::from_secs(120), self.reply_inner(answer))
            .await
            .expect("whole question-response turn exceeded120seconds")
    }
    async fn reply_inner(&mut self, answer: &str) -> TurnTrace {
        let state = self.db.get_conversation(&self.source).await.unwrap().state;
        let ConvState::AwaitingUserResponse {
            questions,
            request_authority,
            ..
        } = state
        else {
            return self.input(answer).await;
        };
        let request_id = request_authority.request_id().cloned();
        let before = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1",
        )
        .bind(&self.source)
        .fetch_one(self.db.pool())
        .await
        .unwrap();
        crate::send_chat_service::SendChatApplicationService::new(
            self.db.clone(),
            self.manager.clone(),
        )
        .respond_to_question(
            &self.source,
            request_id.clone(),
            questions
                .iter()
                .map(|question| (question.question.clone(), answer.to_owned()))
                .collect(),
            None,
            None,
        )
        .await
        .unwrap();
        let mut observer = self.handle.state_rx.clone();
        let id = tokio::time::timeout(Duration::from_secs(120),async {
            loop {
                if let Some(message) = self.db.get_messages(&self.source).await.unwrap().into_iter().find(|message| message.sequence_id>before && message.origin==phoenix_core::domain::db_schema::InputOrigin::UserApi && matches!(&message.content,MessageContent::User(content) if content.text.contains(answer))) {break message.message_id;}
                observer.changed().await.unwrap();
            }
        }).await.expect("actual question answer was not persisted");
        self.wait_visible(&id, None, request_id).await
    }
    async fn wait_visible(
        &mut self,
        input_id: &str,
        turn_id: Option<u64>,
        question_id: Option<QuestionRequestId>,
    ) -> TurnTrace {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let state = self.handle.state_rx.borrow().clone();
                assert!(
                    !matches!(state, ConvState::Error { .. }),
                    "foreground runtime error: {state:?}"
                );
                if matches!(
                    state,
                    ConvState::Idle | ConvState::AwaitingUserResponse { .. }
                ) && self.db.get_conversation(&self.source).await.unwrap().state == state
                {
                    let messages = self.db.get_messages(&self.source).await.unwrap();
                    if let Some(index) = messages
                        .iter()
                        .position(|message| message.message_id == input_id)
                    {
                        assert_eq!(
                            messages[index].origin,
                            phoenix_core::domain::db_schema::InputOrigin::UserApi
                        );
                        let mut prose = Vec::new();
                        let mut assistant = Vec::new();
                        let mut results = Vec::new();
                        for message in &messages[index + 1..] {
                            match &message.content {
                                MessageContent::Agent(blocks) => {
                                    assistant.push(
                                        json!({"message_id":message.message_id,"content":blocks}),
                                    );
                                    for block in blocks {
                                        match block {
                                            ContentBlock::Text { text } => prose.push(text.clone()),
                                            ContentBlock::ToolUse { name, input, .. }
                                                if name == "ask_user_question" =>
                                            {
                                                prose.push(input.to_string())
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                MessageContent::Tool(result) => results.push(
                                    json!({"message_id":message.message_id,"content":result}),
                                ),
                                MessageContent::System(notice) => assert!(!notice
                                    .text
                                    .contains("Tool-use iteration limit reached")),
                                _ => {}
                            }
                        }
                        if !prose.is_empty() {
                            return TurnTrace {
                                source: self.source.clone(),
                                input_message_id: input_id.into(),
                                authority_turn_id: turn_id,
                                question_request_id: question_id
                                    .as_ref()
                                    .map(|id| id.as_str().into()),
                                visible_text: prose.join("\n"),
                                assistant_messages: assistant,
                                tool_results: results,
                                state,
                            };
                        }
                    }
                }
                self.handle.state_rx.changed().await.unwrap();
            }
        })
        .await
        .expect("foreground turn exceeded120seconds")
    }
}

struct ForegroundClient {
    inner: Arc<dyn LlmClient>,
    model: String,
    expected_system: String,
    expected_tools: String,
    trace: Arc<Mutex<Value>>,
    output: PathBuf,
}
impl ForegroundClient {
    fn request(&self, request: &LlmRequest) -> Result<usize, LlmError> {
        let system = format!("{:?}", request.system);
        let tools = serde_json::to_vec(request.tool_availability.declarations()).unwrap();
        if digest(system.as_bytes()) != self.expected_system
            || digest(&tools) != self.expected_tools
        {
            return Err(LlmError::invalid_request(
                "prepared system/tool definitions changed",
            ));
        }
        let bytes = format!("{request:?}").len();
        let mut trace = self.trace.lock().unwrap();
        let calls = trace["requests"].as_array().unwrap().len();
        let accumulated = usize::try_from(trace["request_debug_bytes"].as_u64().unwrap()).unwrap();
        if calls >= MAX_REQUESTS
            || accumulated + bytes > MAX_BYTES
            || request.max_tokens > u32::try_from(MAX_OUTPUT).unwrap()
        {
            return Err(LlmError::invalid_request(
                "foreground qualification budget exhausted",
            ));
        }
        trace["request_debug_bytes"] = json!(accumulated + bytes);
        trace["requests"].as_array_mut().unwrap().push(json!({
            "request":calls+1,"debug_bytes":bytes,"system_debug_sha256":digest(system),"tool_definitions_sha256":digest(tools),
            "request_debug_sha256":digest(format!("{request:?}")),"effective_effort":format!("{:?}",request.effective_effort),
            "max_output_tokens":request.max_tokens,"messages":request.messages.iter().map(|message|json!({"source_message_id":message.source_message_id,"role":format!("{:?}",message.role),"content":message.content})).collect::<Vec<_>>()
        }));
        save_json(&self.output, &*trace);
        Ok(calls)
    }
    fn response(&self, index: usize, response: &LlmResponse) -> Result<(), LlmError> {
        let encoded = serde_json::to_vec(&response.content).unwrap();
        let valid = response.usage_observation.output
            && response.usage.output_tokens <= u64::try_from(MAX_OUTPUT).unwrap()
            && encoded.len() <= 16_384;
        let mut trace = self.trace.lock().unwrap();
        trace["requests"][index]["end_turn"] = json!(response.end_turn);
        trace["requests"][index]["usage"] = json!(response.usage);
        trace["requests"][index]["usage_observation"] = json!(response.usage_observation);
        trace["requests"][index]["serialized_content_bytes"] = json!(encoded.len());
        if valid {
            trace["requests"][index]["response"] = json!(response.content);
            save_json(&self.output, &*trace);
            Ok(())
        } else {
            trace["requests"][index]["rejected_output_sha256"] = json!(digest(encoded));
            trace["qualification"] = json!("unknown_or_oversized_observed_output_unqualified");
            save_json(&self.output, &*trace);
            Err(LlmError::invalid_request(
                "unknown or oversized observed output; no retry",
            ))
        }
    }
    fn failure(&self, index: usize, kind: &str) -> LlmError {
        let mut trace = self.trace.lock().unwrap();
        trace["requests"][index]["failure"] = json!(kind);
        trace["qualification"] = json!("provider_failure_unqualified");
        save_json(&self.output, &*trace);
        LlmError::invalid_request("foreground provider failure; no automatic retry")
    }
}
#[async_trait]
impl LlmClient for ForegroundClient {
    fn model_id(&self) -> &str {
        &self.model
    }
    fn diagnostic_route(
        &self,
    ) -> (
        phoenix_core::domain::request_capacity::SafeRouteClass,
        Option<String>,
    ) {
        self.inner.diagnostic_route()
    }
    fn continuation_route_key(&self) -> Result<String, LlmError> {
        self.inner.continuation_route_key()
    }
    fn freeze_for_request(self: Arc<Self>) -> Result<Arc<dyn LlmClient>, LlmError> {
        Ok(Arc::new(Self {
            inner: self.inner.clone().freeze_for_request()?,
            model: self.model.clone(),
            expected_system: self.expected_system.clone(),
            expected_tools: self.expected_tools.clone(),
            trace: self.trace.clone(),
            output: self.output.clone(),
        }))
    }
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let index = self.request(request)?;
        match tokio::time::timeout(Duration::from_secs(60), self.inner.complete(request)).await {
            Ok(Ok(response)) => {
                self.response(index, &response)?;
                Ok(response)
            }
            Ok(Err(error)) => Err(self.failure(index, &format!("{:?}", error.kind))),
            Err(_) => Err(self.failure(index, "timeout")),
        }
    }
    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunks: &mpsc::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let index = self.request(request)?;
        match tokio::time::timeout(
            Duration::from_secs(60),
            self.inner.complete_streaming(request, chunks),
        )
        .await
        {
            Ok(Ok(response)) => {
                self.response(index, &response)?;
                Ok(response)
            }
            Ok(Err(error)) => Err(self.failure(index, &format!("{:?}", error.kind))),
            Err(_) => Err(self.failure(index, "timeout")),
        }
    }
}

async fn fixture<C: LlmClient + 'static>(
    db: crate::db::Database,
    registry: Arc<ModelRegistry>,
    client: Arc<C>,
    home: &Path,
    credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
) -> Fixture<C> {
    let isolated = phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(home);
    let retriever = Arc::new(db.fts_retriever());
    retriever.reconcile().await.unwrap();
    let manager = Arc::new(
        crate::runtime::RuntimeManager::new_with_message_retriever_and_runtime_env(
            db.clone(),
            registry.clone(),
            retriever,
            crate::platform::PlatformCapability::None {
                details: "isolated foreground memory qualification".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            credential_helper,
            &isolated,
        ),
    );
    let model = client.model_id().to_owned();
    Fixture {
        manager,
        db,
        registry,
        client,
        model,
        cwd: home.into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        denied: Arc::new(Mutex::new(Vec::new())),
        source_checkpoint_presence: Arc::new(Mutex::new(Vec::new())),
    }
}
fn catalog(home: &Path) -> phoenix_skills::AuthenticatedCoordinatorSkillCatalog {
    phoenix_skills::AuthenticatedCoordinatorSkillCatalog::discover(Some(&home.join("skills")))
        .expect("prepared authenticated catalog")
}
async fn prefix(db: &crate::db::Database, source: &str) -> Vec<Value> {
    db.get_messages(source).await.unwrap().iter().map(|message|json!({"message_id":message.message_id,"sequence_id":message.sequence_id,"content":message.content,"origin":message.origin,"created_at":message.created_at})).collect()
}
async fn assert_prefix(db: &crate::db::Database, source: &str, expected: &[Value]) {
    let actual = prefix(db, source).await;
    assert!(
        actual.starts_with(expected),
        "earlier persisted model-facing prefix changed"
    );
}
async fn snapshot(db: &crate::db::Database, path: &Path) {
    assert!(!path.exists());
    sqlx::query("VACUUM main INTO ?1")
        .bind(path.to_str().unwrap())
        .execute(db.pool())
        .await
        .unwrap();
}
fn seed_response(records: &[Value]) -> LlmResponse {
    LlmResponse {
        provider_replay: None,
        content: records
            .iter()
            .enumerate()
            .map(|(index, record)| ContentBlock::ToolUse {
                id: format!("foreground-seed-{index}"),
                name: "tracking".into(),
                input: json!({"request":{"op":"record","record":record}}),
            })
            .collect(),
        end_turn: false,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    }
}
fn terminal(text: &str) -> LlmResponse {
    LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::text(text)],
        end_turn: true,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    }
}
fn record(title: &str, note: &str, resumption: &str) -> Value {
    json!({"kind":"requested_work","title":title,"note":note,"resumption":resumption,"references":[],"evidence":[]})
}
fn topic_records() -> Vec<Value> {
    vec![
        json!({"record":record("Check Cobalt export totals","User wants the invoice CSV totals checked, after agreeing which periods to compare. No calculations or worker nudge before the answer.","Ask which date range to check and which period to compare against."),"prompt":"We are discussing the numbers in downloaded invoices. What should we settle before touching code?","expected":["period","date"],"domain":"invoice downloads"}),
        json!({"record":record("Settle Harbor keyboard focus","User wants accessible form navigation; agree focus restoration before implementation.","Ask which control should regain focus after the dialog closes."),"prompt":"We are discussing keyboard access for a form dialog. What should we settle before touching code?","expected":["focus","control"],"domain":"keyboard dialog access"}),
        json!({"record":record("Plan Cedar offline delivery","User wants reliable disconnected reading; choose required offline retention before implementation.","Ask how many days of articles must remain available without a connection."),"prompt":"We are discussing reading articles without a network connection. What should we settle before touching code?","expected":["days","articles"],"domain":"disconnected article reading"}),
        json!({"record":record("Clarify Maple warning audience","User wants storage warnings; select warning audience before implementation.","Ask whether the warning is for administrators only or also ordinary members."),"prompt":"We are discussing storage-capacity warnings. What should we settle before touching code?","expected":["administrator","member"],"domain":"storage warning audience"}),
        json!({"record":record("Choose Birch retention boundary","User wants old audit data cleaned up only after agreeing the retention window.","Ask how many months of audit history must be retained."),"prompt":"We are discussing removing old audit data. What should we settle before touching code?","expected":["months","retain"],"domain":"audit retention"}),
        json!({"record":record("Clarify Juniper retry meaning","User wants safe payment retries; agree idempotency identity before implementation.","Ask whether a repeated invoice payment uses the same transaction key or a new one."),"prompt":"We are discussing a repeated invoice payment after a network error. What should we settle before touching code?","expected":["transaction","key"],"domain":"payment retry identity"}),
        json!({"record":record("Set Pine image quality","User wants photo downloads; choose supported export quality before implementation.","Ask whether the downloaded photos must retain original resolution or use a smaller preview."),"prompt":"We are discussing photo downloads for travel. What should we settle before touching code?","expected":["resolution","preview"],"domain":"photo download quality"}),
        json!({"record":record("Clarify Ash search scope","User wants useful document search; determine corpus scope before implementation.","Ask whether archived documents should appear in the results or only active documents."),"prompt":"We are discussing improvements to document search. What should we settle before touching code?","expected":["archiv","active"],"domain":"document search scope"}),
        json!({"record":record("Plan Elm report time zone","User wants a daily activity digest; pick reporting time zone before implementation.","Ask whether report-day boundaries use the viewer's zone or the workspace's zone."),"prompt":"We are discussing daily activity digests. What should we settle before touching code?","expected":["viewer","workspace"],"domain":"digest day boundaries"}),
        json!({"record":record("Choose Oak notification channel","User wants incident notices; agree delivery channel before implementation.","Ask whether incident notices belong in email or only in the in-app inbox."),"prompt":"We are discussing delivery of incident notices. What should we settle before touching code?","expected":["email","inbox"],"domain":"incident delivery channel"}),
    ]
}

async fn observation(
    db: &crate::db::Database,
    source: &str,
) -> phoenix_db::coordinator_tracking::TrackingObservation {
    let message_id: String = sqlx::query_scalar(
        "SELECT message_id FROM coordinator_tracking_observations WHERE transcript_id=?1",
    )
    .bind(source)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let message = db
        .get_messages(source)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.message_id == message_id)
        .unwrap();
    let MessageContent::User(content) = message.content else {
        panic!("actual tracking observation")
    };
    serde_json::from_str(content.text.split_once('\n').unwrap().1).unwrap()
}
async fn all_items(
    db: &crate::db::Database,
    source: &str,
) -> Vec<phoenix_db::coordinator_tracking::TrackingSummary> {
    let first = db
        .list_coordinator_tracking(source, None, false)
        .await
        .unwrap();
    let mut items = first.items;
    if let Some(cursor) = first.next_cursor {
        let next = db
            .list_coordinator_tracking(source, Some(&cursor), false)
            .await
            .unwrap();
        assert!(!next.has_more);
        items.extend(next.items);
    }
    items
}
#[allow(clippy::too_many_lines)]
async fn prepare_case(
    root: &Path,
    id: &str,
    registry: Arc<ModelRegistry>,
    config: &phoenix_llm::LlmConfig,
) -> Value {
    let home = root.join(id);
    std::fs::create_dir(&home).unwrap();
    phoenix_skills::builtin::extract_to(&home.join("skills")).unwrap();
    let db = crate::db::Database::open(home.join("working.sqlite").to_str().unwrap())
        .await
        .unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    db.initialize_coordinator_work_scope(home.to_str().unwrap())
        .await
        .unwrap();
    let source = db
        .get_or_create_coordinator(
            Some(MODEL),
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
        )
        .await
        .unwrap()
        .id;
    db.update_conversation_model_and_effort(
        &source,
        MODEL,
        Some(phoenix_core::domain::llm_types::ModelEffort::High),
        phoenix_core::domain::llm_types::ServiceTier::Standard,
        &registry.connection_for_model(MODEL).unwrap(),
    )
    .await
    .unwrap();
    let mock = Arc::new(MockLlmClient::new(MODEL));
    let fixture = fixture(
        db.clone(),
        registry.clone(),
        mock.clone(),
        &home,
        config.credential_helper.clone(),
    )
    .await;
    let topics = topic_records();
    let records = if id == "post-review-quiet" {
        vec![
        record("Check Cobalt export totals","User wants invoice totals checked; first agree which comparison period. Evaluation-only; no calculations, worker messages or external work before an answer.","Ask which date range to check and which period to compare against."),
        record("Wait for Sage owner","User asked to leave the existing owner alone while it progresses; no check-in or nudge is requested.","Wait quietly for the existing result; do not nudge or inspect a worker because this item exists."),
    ]
    } else {
        topics.iter().map(|topic| topic["record"].clone()).collect()
    };
    mock.queue_response(seed_response(&records));
    mock.queue_response(terminal(
        "Saved the synthetic requested outcomes and their stopping points.",
    ));
    let mut session = Session::start(&fixture, &source, Some(catalog(&home))).await;
    let seeded=session.input(&format!("Use tracking to preserve these synthetic requested outcomes and exact stopping points. They are evaluation-only; do not use files, Bash, Goals, workers, messages, peers or other external actions. Do not ask the saved questions during setup. Records: {}",serde_json::to_string(&records).unwrap())).await;
    assert_eq!(all_items(&db, &source).await.len(), records.len());
    session.close().await;
    let mut sources = vec![source.clone()];
    let mut current = source.clone();
    let target;
    let prompt;
    let expected;
    let recently_reviewed_other;
    if id == "post-review-quiet" {
        let items = all_items(&db, &source).await;
        target = items
            .iter()
            .find(|item| item.title == "Check Cobalt export totals")
            .unwrap()
            .id
            .clone();
        let other = items
            .iter()
            .find(|item| item.title == "Wait for Sage owner")
            .unwrap()
            .clone();
        let aged = Utc::now() - chrono::Duration::days(3);
        sqlx::query("UPDATE coordinator_tracking_items SET created_at_us=?1,updated_at_us=?1,reviewed_at_us=?1").bind(aged.timestamp_micros()).execute(db.pool()).await.unwrap();
        sqlx::query(
            "UPDATE messages SET created_at=?1 WHERE conversation_id=?2 AND message_type!='user'",
        )
        .bind(aged.to_rfc3339())
        .bind(&source)
        .execute(db.pool())
        .await
        .unwrap();
        let original_input = seeded.input_message_id.clone();
        sqlx::query("UPDATE messages SET created_at=?1 WHERE message_id=?2")
            .bind(aged.to_rfc3339())
            .bind(original_input)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET created_at=?1 WHERE id=?2")
            .bind((aged - chrono::Duration::hours(1)).to_rfc3339())
            .bind(&source)
            .execute(db.pool())
            .await
            .unwrap();
        mock.queue_response(LlmResponse{content:vec![ContentBlock::ToolUse{id:"recent-sage-review".into(),name:"tracking".into(),input:json!({"request":{"op":"update","id":other.id,"expected_revision":other.revision,"kind":"requested_work","title":other.title,"note":records[1]["note"],"resumption":records[1]["resumption"],"reviewed_evidence_ordinal":0,"references":[]}})}],end_turn:false,..terminal("")});
        mock.queue_response(terminal("The Sage request is reviewed and remains quiet."));
        let mut session = Session::start(&fixture, &source, Some(catalog(&home))).await;
        let _=session.input("Review the saved Sage waiting instruction without checking or nudging its owner. Preserve its no-action restriction.").await;
        session.close().await;
        recently_reviewed_other = json!(other.id);
        prompt = "Hello.".to_owned();
        expected = json!({"question_terms":["period","date"],"pause":"Pause this until I explicitly say resume. Do not ask the period question again until then.","arithmetic":"What is 8+5?","answer":"13"});
    } else {
        for summary in ["We answered neutral arithmetic. Earlier saved requests intentionally omitted from this handoff.","We finished an unrelated detour. Earlier saved requests and stopping points intentionally omitted from this handoff."] {
            db.update_conversation_state(&current,&ConvState::ContextExhausted{summary:summary.into()}).await.unwrap();
            current=match db.continue_conversation(&current).await.unwrap(){crate::db::ContinueOutcome::Created(next)=>next.id,other=>panic!("{other:?}")};sources.push(current.clone());
        }
        let page = db
            .list_coordinator_tracking(&current, None, false)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 8);
        assert_eq!(page.omitted_count, 2);
        let rest = db
            .list_coordinator_tracking(&current, page.next_cursor.as_deref(), false)
            .await
            .unwrap();
        assert_eq!(rest.items.len(), 2);
        let selected = rest.items.last().unwrap();
        target = selected.id.clone();
        let topic = topics
            .iter()
            .find(|topic| topic["record"]["title"] == selected.title)
            .unwrap();
        prompt = topic["prompt"].as_str().unwrap().into();
        assert!(!prompt.contains(&selected.title) && !prompt.contains(&target));
        expected = json!({"terms":topic["expected"],"domain":topic["domain"],"full_resumption":topic["record"]["resumption"]});
        recently_reviewed_other = Value::Null;
    }
    let detail = db
        .read_coordinator_tracking(&current, &target, 0)
        .await
        .unwrap();
    assert!(detail.recorded_by_ember.as_ref().unwrap().source_available);
    let mut seed_prefix = serde_json::Map::new();
    for source in &sources {
        seed_prefix.insert(source.clone(), json!(prefix(&db, source).await));
    }
    let seed_path = home.join("seed.sqlite");
    snapshot(&db, &seed_path).await;
    let before = mock.recorded_requests().len();
    mock.queue_response(terminal("Preparation only."));
    let mut probe = Session::start(&fixture, &current, Some(catalog(&home))).await;
    let probe_turn = probe.input(&prompt).await;
    let observed = observation(&db, &current).await;
    if id == "post-review-quiet" {
        assert_eq!(observed.requested_item_due_for_review.unwrap().id, target);
    } else {
        assert!(observed.requested_item_due_for_review.is_none());
        assert!(!observed
            .captured_registry
            .items
            .iter()
            .any(|item| item.id == target));
    }
    probe.close().await;
    let requests = mock.recorded_requests();
    let request = &requests[before];
    if id == "contextual-hidden" {
        let text = request_text(request);
        assert!(
            !text.contains(&target) && !text.contains(&detail.item.title),
            "target leaked into initial context"
        );
    }
    json!({"id":id,"home":home,"seed_db":seed_path,"seed_db_sha256":digest(std::fs::read(&seed_path).unwrap()),"source":current,"original_source":source,"all_sources":sources,"seed_prefix":seed_prefix,"seed_turn":seeded,"target_id":target,"target_detail":detail,"other_recent_id":recently_reviewed_other,"prompt":prompt,"expected":expected,"system_debug":format!("{:?}",request.system),"system_debug_sha256":digest(format!("{:?}",request.system)),"tool_definitions":request.tool_availability.declarations(),"tool_definitions_sha256":digest(serde_json::to_vec(request.tool_availability.declarations()).unwrap()),"probe_input_not_in_seed":probe_turn.input_message_id})
}

fn turn_calls(turn: &TurnTrace) -> Vec<&Value> {
    turn.assistant_messages
        .iter()
        .flat_map(|message| message["content"].as_array().unwrap())
        .filter(|block| block["type"] == "tool_use")
        .collect()
}
fn quiet_arithmetic(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty());
    (lower.contains("13") || lower.contains("thirteen"))
        && words.into_iter().all(|word| {
            matches!(
                word,
                "the"
                    | "answer"
                    | "is"
                    | "equals"
                    | "thirteen"
                    | "eight"
                    | "plus"
                    | "five"
                    | "sure"
            )
        })
}
fn append_turn(trace: &Arc<Mutex<Value>>, path: &Path, name: &str, turn: &TurnTrace) {
    let mut value = trace.lock().unwrap();
    value["turns"]
        .as_array_mut()
        .unwrap()
        .push(json!({"stage":name,"turn":turn}));
    save_json(path, &*value);
}
#[allow(clippy::too_many_lines)]
async fn run_case(
    case: &Value,
    output: &Path,
    registry: Arc<ModelRegistry>,
    config: &phoenix_llm::LlmConfig,
) -> Value {
    let id = case["id"].as_str().unwrap();
    let home = Path::new(case["home"].as_str().unwrap());
    let seed = Path::new(case["seed_db"].as_str().unwrap());
    assert_eq!(
        digest(std::fs::read(seed).unwrap()),
        case["seed_db_sha256"].as_str().unwrap()
    );
    let live_db = output.join(format!("{id}.sqlite"));
    assert!(!live_db.exists());
    std::fs::copy(seed, &live_db).unwrap();
    let db = crate::db::Database::open(live_db.to_str().unwrap())
        .await
        .unwrap();
    let source = case["source"].as_str().unwrap();
    let target = case["target_id"].as_str().unwrap();
    assert!(!db
        .message_exists(case["probe_input_not_in_seed"].as_str().unwrap())
        .await
        .unwrap());
    for (member, expected) in case["seed_prefix"].as_object().unwrap() {
        assert_eq!(prefix(&db, member).await, *expected.as_array().unwrap());
    }
    let authority = db.coordinator_authority().await.unwrap();
    let trace_path = output.join(format!("{id}.json"));
    let trace = Arc::new(Mutex::new(
        json!({"case":case,"requests":[],"request_debug_bytes":0,"turns":[],"qualification":"recorded_not_accepted"}),
    ));
    let client = Arc::new(ForegroundClient {
        inner: Arc::new(RegistryLlmClient::new(registry.clone(), MODEL.into())),
        model: MODEL.into(),
        expected_system: case["system_debug_sha256"].as_str().unwrap().into(),
        expected_tools: case["tool_definitions_sha256"].as_str().unwrap().into(),
        trace: trace.clone(),
        output: trace_path.clone(),
    });
    let fixture = fixture(
        db.clone(),
        registry,
        client,
        home,
        config.credential_helper.clone(),
    )
    .await;
    let mut session = Session::start(&fixture, source, Some(catalog(home))).await;
    let before = prefix(&db, source).await;
    let first = session.input(case["prompt"].as_str().unwrap()).await;
    append_turn(&trace, &trace_path, "first", &first);
    assert_prefix(&db, source, &before).await;
    let captured = observation(&db, source).await;
    let first_input = db
        .get_messages(source)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.message_id == first.input_message_id)
        .unwrap();
    assert_eq!(
        captured.admitted_input_sequence_id,
        Some(first_input.sequence_id)
    );
    if id == "post-review-quiet" {
        assert_eq!(
            captured.requested_item_due_for_review.as_ref().unwrap().id,
            target
        );
        let lower = first.visible_text.to_lowercase();
        assert!(
            first.visible_text.contains("Cobalt")
                && (lower.contains("period") || lower.contains("date range"))
                && (first.visible_text.contains('?')
                    || matches!(first.state, ConvState::AwaitingUserResponse { .. })),
            "due question not surfaced: {}",
            first.visible_text
        );
        assert!(
            turn_calls(&first)
                .iter()
                .any(|call| call["name"] == "tracking"
                    && call["input"]["request"]["op"] == "read"
                    && call["input"]["request"]["id"] == target),
            "actual selected item must be inspected"
        );
        let before = prefix(&db, source).await;
        let paused = session
            .reply(case["expected"]["pause"].as_str().unwrap())
            .await;
        append_turn(&trace, &trace_path, "pause_reply", &paused);
        assert_prefix(&db, source, &before).await;
        assert!(
            matches!(paused.state, ConvState::Idle),
            "pause reply must settle rather than ask the period again"
        );
        let current = db
            .read_coordinator_tracking(source, target, 0)
            .await
            .unwrap();
        let saved = format!("{} {:?}", current.item.note, current.item.resumption).to_lowercase();
        assert!(
            current.item.revision > case["target_detail"]["item"]["revision"].as_i64().unwrap()
        );
        assert!(matches!(
            current.item.resumption,
            TrackingResumption::Reviewed { .. }
        ));
        assert!(
            (saved.contains("pause") || saved.contains("hold")) && saved.contains("resume"),
            "actual pause not retained: {saved}"
        );
        assert!(
            turn_calls(&paused)
                .iter()
                .any(|call| call["name"] == "tracking"
                    && call["input"]["request"]["op"] == "update"
                    && call["input"]["request"]["id"] == target),
            "actual pause CAS update required"
        );
        let now = Utc::now().timestamp_micros();
        let items = all_items(&db, source).await;
        assert_eq!(items.len(), 2);
        for item in &items {
            let item = db
                .read_coordinator_tracking(source, &item.id, 0)
                .await
                .unwrap()
                .item;
            assert!(
                item.reviewed_at_us.unwrap()
                    > now - phoenix_db::coordinator_tracking::TRACKING_REVIEW_INTERVAL_US
            );
        }
        let before = prefix(&db, source).await;
        let quiet = session
            .input(case["expected"]["arithmetic"].as_str().unwrap())
            .await;
        append_turn(&trace, &trace_path, "quiet_after_pause", &quiet);
        assert_prefix(&db, source, &before).await;
        assert!(
            quiet_arithmetic(&quiet.visible_text),
            "non-arithmetic visible chatter: {}",
            quiet.visible_text
        );
        assert!(
            turn_calls(&quiet).is_empty(),
            "quiet control must make zero tools, including intercepted questions"
        );
        let captured = observation(&db, source).await;
        let latest = db
            .get_messages(source)
            .await
            .unwrap()
            .into_iter()
            .find(|message| message.message_id == quiet.input_message_id)
            .unwrap();
        assert!(
            captured.admitted_input_sequence_id != Some(latest.sequence_id)
                || captured.requested_item_due_for_review.is_none(),
            "old cue renewed on quiet input"
        );
        trace.lock().unwrap()["pause_record"] = json!(current);
    } else {
        assert!(
            captured.requested_item_due_for_review.is_none(),
            "contextual recovery must not be a due cue"
        );
        assert!(!captured
            .captured_registry
            .items
            .iter()
            .any(|item| item.id == target));
        assert_eq!(captured.captured_registry.items.len(), 8);
        assert_eq!(captured.captured_registry.omitted_count, 2);
        let required = case["expected"]["full_resumption"].as_str().unwrap();
        let recording = case["target_detail"]["recorded_by_ember"]["assistant_message_id"]
            .as_str()
            .unwrap();
        assert!(
            first
                .tool_results
                .iter()
                .any(|result| !result["content"]["is_error"].as_bool().unwrap()
                    && result["content"]["content"]
                        .as_str()
                        .is_some_and(|text| text.contains(target)
                            && text.contains(required)
                            && text.contains(recording))),
            "actual returned target, source checkpoint identity and stopping point required"
        );
        let lower = first.visible_text.to_lowercase();
        assert!(
            case["expected"]["terms"]
                .as_array()
                .unwrap()
                .iter()
                .all(|term| lower.contains(term.as_str().unwrap())),
            "saved stopping point not recovered: {}",
            first.visible_text
        );
        assert!(
            first.visible_text.contains('?')
                || matches!(first.state, ConvState::AwaitingUserResponse { .. }),
            "specific question, not generic advice, required"
        );
        assert_eq!(
            all_items(&db, source).await.len(),
            10,
            "recovery must not make a duplicate request"
        );
        trace.lock().unwrap()["returned_record_source"] =
            case["target_detail"]["recorded_by_ember"].clone();
    }
    let denied = fixture.denied.lock().unwrap().clone();
    assert!(
        denied.is_empty(),
        "out-of-scope action attempted: {denied:?}"
    );
    assert_eq!(db.coordinator_authority().await.unwrap(), authority);
    for (member, expected) in case["seed_prefix"].as_object().unwrap() {
        assert_prefix(&db, member, expected.as_array().unwrap()).await;
    }
    session.close().await;
    trace.lock().unwrap()["qualification"] =
        json!("automatic_oracles_passed_independent_review_required");
    save_json(&trace_path, &*trace.lock().unwrap());
    let final_trace = trace.lock().unwrap();
    let requests = final_trace["requests"].as_array().unwrap().len();
    let bytes = final_trace["request_debug_bytes"].as_u64().unwrap();
    json!({"case":id,"trace":trace_path,"trace_sha256":digest(std::fs::read(&trace_path).unwrap()),"provider_requests":requests,"request_debug_bytes":bytes,"status":"recorded_pending_independent_review"})
}

#[tokio::test]
#[ignore = "explicit prepare/live foreground qualification; real provider only in approved live mode"]
#[allow(clippy::too_many_lines)]
async fn foreground_memory_qualification() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mode = std::env::var("PHOENIX_FOREGROUND_QA_MODE").expect("prepare|live");
    assert!(matches!(mode.as_str(), "prepare" | "live"));
    let output = PathBuf::from(
        std::env::var("PHOENIX_FOREGROUND_QA_OUTPUT").expect("fresh output directory"),
    );
    std::fs::create_dir(&output).expect("fresh output directory with existing parent");
    let config = phoenix_llm::LlmConfig::from_env(Arc::new(
        phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect(),
    ));
    let endpoints=digest(serde_json::to_vec(&json!({"anthropic":config.anthropic_base_url,"responses":config.openai_responses_base_url,"chat":config.openai_chat_completions_base_url,"legacy_openai":config.openai_base_url})).unwrap());
    let registry = Arc::new(ModelRegistry::new_with_discovery(&config).await);
    assert!(registry.supports_effort(MODEL, phoenix_core::domain::llm_types::ModelEffort::High));
    let service = registry
        .get(MODEL)
        .expect("explicit model unavailable; no fallback");
    let outbound = service
        .outbound_model_name()
        .expect("actual outbound model identity required");
    assert_eq!(outbound, MODEL, "no outbound model override");
    let binary = std::env::var("PHOENIX_FOREGROUND_QA_BINARY_SHA256")
        .expect("retained test executable digest");
    let mut manifest = json!({"mode":mode,"model":MODEL,"actual_outbound_model":outbound,"effort":"high","connection":registry.connection_for_model(MODEL),"endpoint_configuration_sha256":endpoints,"language":"phoenix-native","authenticated_catalog":true,"binary_sha256":binary,"limits":{"provider_requests_per_journey":MAX_REQUESTS,"request_debug_bytes_per_journey":MAX_BYTES,"requested_output_tokens":MAX_OUTPUT,"accepted_reported_output_tokens_max":MAX_OUTPUT,"accepted_content_bytes_max":16384,"request_timeout_seconds":60,"turn_timeout_seconds":120,"tool_cycles_per_turn":8,"retries":0,"skips":0},"status":"prepared","cases":[]});
    if mode == "prepare" {
        for id in ["post-review-quiet", "contextual-hidden"] {
            let case = prepare_case(&output, id, registry.clone(), &config).await;
            manifest["cases"].as_array_mut().unwrap().push(case);
            save_json(&output.join("manifest.json"), &manifest);
        }
    } else {
        let approved = PathBuf::from(
            std::env::var("PHOENIX_FOREGROUND_QA_PREPARED").expect("approved manifest"),
        );
        let bytes = std::fs::read(&approved).unwrap();
        assert_eq!(
            digest(&bytes),
            std::env::var("PHOENIX_FOREGROUND_QA_APPROVED_SHA256")
                .expect("explicit approved manifest digest")
        );
        let prepared: Value = serde_json::from_slice(&bytes).unwrap();
        for key in [
            "model",
            "actual_outbound_model",
            "effort",
            "connection",
            "endpoint_configuration_sha256",
            "language",
            "authenticated_catalog",
            "binary_sha256",
            "limits",
        ] {
            assert_eq!(manifest[key], prepared[key], "prepared {key} changed");
        }
        manifest["status"] = json!("recording");
        manifest["prepared_manifest_sha256"] = json!(digest(bytes));
        save_json(&output.join("manifest.json"), &manifest);
        let mut calls = 0usize;
        let mut bytes = 0u64;
        for case in prepared["cases"].as_array().unwrap() {
            let result = run_case(case, &output, registry.clone(), &config).await;
            calls += usize::try_from(result["provider_requests"].as_u64().unwrap()).unwrap();
            bytes += result["request_debug_bytes"].as_u64().unwrap();
            manifest["cases"].as_array_mut().unwrap().push(result);
            save_json(&output.join("manifest.json"), &manifest);
        }
        assert!(calls <= 20 && bytes <= 4_000_000);
        manifest["status"] = json!("recorded_pending_independent_review");
        save_json(&output.join("manifest.json"), &manifest);
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn foreground_question_reply_uses_real_protocol_and_file_reads() {
    let home = tempfile::tempdir().unwrap();
    phoenix_skills::builtin::extract_to(&home.path().join("skills")).unwrap();
    let db = crate::db::Database::open(home.path().join("fixture.sqlite").to_str().unwrap())
        .await
        .unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    db.initialize_coordinator_work_scope(home.path().to_str().unwrap())
        .await
        .unwrap();
    let source = db
        .get_or_create_coordinator(
            Some("test-model"),
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
        )
        .await
        .unwrap()
        .id;
    let mock = Arc::new(MockLlmClient::new("test-model"));
    let fixture = fixture(
        db.clone(),
        Arc::new(ModelRegistry::new_empty()),
        mock.clone(),
        home.path(),
        None,
    )
    .await;
    mock.queue_response(seed_response(&[record(
        "Check Cobalt export totals",
        "Evaluation-only comparison awaits period.",
        "Ask which period to compare.",
    )]));
    mock.queue_response(terminal("Saved."));
    let mut session = Session::start(&fixture, &source, Some(catalog(home.path()))).await;
    let seed = session
        .input("Remember the synthetic Cobalt comparison request; no external work.")
        .await;
    let item = all_items(&db, &source).await.pop().unwrap();
    mock.queue_response(LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "actual-file-query".into(),
            name: "query_database".into(),
            input: json!({"sql":"SELECT COUNT(*) AS n FROM coordinator_tracking_items"}),
        }],
        end_turn: false,
        ..terminal("")
    });
    mock.queue_response(LlmResponse{content:vec![ContentBlock::ToolUse{id:"period-question".into(),name:"ask_user_question".into(),input:json!({"questions":[{"header":"Period","question":"Which period should the Cobalt totals compare?","options":[{"label":"Last month"},{"label":"Choose another"}],"multiSelect":false}]})}],end_turn:false,..terminal("")});
    let asked = session
        .input("Ask the owed comparison question using the structured question tool.")
        .await;
    assert!(matches!(
        asked.state,
        ConvState::AwaitingUserResponse { .. }
    ));
    assert!(asked
        .tool_results
        .iter()
        .any(
            |result| result["content"]["tool_use_id"] == "actual-file-query"
                && result["content"]["is_error"] == false
        ));
    let before = prefix(&db, &source).await;
    let pause = "Pause until I explicitly say resume.";
    mock.queue_response(LlmResponse{content:vec![ContentBlock::ToolUse{id:"actual-pause-cas".into(),name:"tracking".into(),input:json!({"request":{"op":"update","id":item.id,"expected_revision":item.revision,"kind":"requested_work","title":item.title,"note":pause,"resumption":pause,"reviewed_evidence_ordinal":0,"references":[]}})}],end_turn:false,..terminal("")});
    mock.queue_response(terminal("Paused until you explicitly say resume."));
    let answered = session.reply(pause).await;
    assert!(answered.question_request_id.is_some());
    assert!(answered.authority_turn_id.is_none());
    assert_prefix(&db, &source, &before).await;
    assert_eq!(
        db.read_coordinator_tracking(&source, &item.id, 0)
            .await
            .unwrap()
            .item
            .revision,
        item.revision + 1
    );
    assert!(answered
        .tool_results
        .iter()
        .any(
            |result| result["content"]["tool_use_id"] == "actual-pause-cas"
                && !result["content"]["is_error"].as_bool().unwrap()
        ));
    mock.queue_response(terminal("13"));
    let quiet = session.input("What is 8+5?").await;
    assert!(quiet_arithmetic(&quiet.visible_text));
    assert!(turn_calls(&quiet).is_empty());
    session.close().await;
    let frozen = home.path().join("frozen.sqlite");
    snapshot(&db, &frozen).await;
    let reopened = crate::db::Database::open(frozen.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(prefix(&db, &source).await, prefix(&reopened, &source).await);
    assert!(reopened
        .message_exists(&seed.input_message_id)
        .await
        .unwrap());
}
