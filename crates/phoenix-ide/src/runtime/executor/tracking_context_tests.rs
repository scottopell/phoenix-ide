use super::*;
use crate::runtime::testing::MockLlmClient;
use crate::runtime::traits::{DatabaseStorage, ToolRegistryExecutor};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_db::coordinator_tracking::{
    TrackingKind, TrackingMutation, TrackingRecord, TrackingResumption,
};
use std::path::PathBuf;

type Runtime = ConversationRuntime<DatabaseStorage, Arc<MockLlmClient>, ToolRegistryExecutor>;
async fn runtime(
    manager: &Arc<crate::runtime::RuntimeManager>,
    db: &crate::db::Database,
    id: &str,
    llm: Arc<MockLlmClient>,
) -> Runtime {
    let mut context = ConvContext::new(id, PathBuf::from("/tmp"), "test-model", 200_000, 16_384);
    context.is_coordinator = true;
    let (tx, rx) = mpsc::channel(16);
    let max = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let registry = manager.coordinator_tool_registry(id, None).await.unwrap();
    ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 1 },
        DatabaseStorage::new(db.clone()),
        llm,
        ToolRegistryExecutor::builtin_only(registry, Arc::from([])),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        SseBroadcaster::new(128, max),
    )
}
async fn save(db: &crate::db::Database, source: &str, kind: TrackingKind, title: &str) -> String {
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: uuid::Uuid::new_v4().to_string(),
    };
    let mutation = TrackingMutation::Record {
        record: TrackingRecord {
            kind,
            title: title.into(),
            note: "Preserved user intent; not fresh worker status".into(),
            resumption: "Return to the exact question without commissioning work".into(),
            references: vec![],
            evidence: vec![],
        },
    };
    db.add_message(
        &call.message_id,
        source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: "tracking".into(),
            input: serde_json::json!({"request":mutation}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    let receipt = db
        .mutate_coordinator_tracking(source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    db.add_message(
        &uuid::Uuid::new_v4().to_string(),
        source,
        &MessageContent::Tool(crate::db::ToolContent::new(
            &call.tool_use_id,
            serde_json::to_string(&receipt).unwrap(),
            false,
        )),
        None,
        None,
    )
    .await
    .unwrap();
    receipt.item_id
}
async fn dispatch(runtime: &mut Runtime, llm: &MockLlmClient) {
    llm.queue_response(phoenix_llm::LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::text("Current question answered")],
        end_turn: true,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    });
    runtime.execute_effect(Effect::RequestLlm).await.unwrap();
    runtime.llm_task_handle.take().unwrap().await.unwrap();
}
fn request_text(request: &phoenix_llm::LlmRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[tokio::test]
async fn tracking_production_registry_prompt_prefix_and_real_continuation_recover_manual_intent() {
    let (manager, db, source) = crate::conversation_action_tools::tests::native_runtime().await;
    let request = save(
        &db,
        &source,
        TrackingKind::RequestedWork,
        "Finish user-requested keyboard fix",
    )
    .await;
    let bookmark = save(
        &db,
        &source,
        TrackingKind::Bookmark,
        "Why did the design choose this boundary?",
    )
    .await;
    let llm = Arc::new(MockLlmClient::new("test-model"));
    let mut current = runtime(&manager, &db, &source, llm.clone()).await;
    dispatch(&mut current, &llm).await;
    let first = llm.recorded_requests().last().unwrap().clone();
    assert!(first
        .tool_availability
        .declarations()
        .iter()
        .any(|tool| tool.name == "tracking"));
    assert!(request_text(&first).contains("Finish user-requested keyboard fix"));
    assert!(request_text(&first).contains("Why did the design choose this boundary?"));
    let original = db.get_messages(&source).await.unwrap();
    dispatch(&mut current, &llm).await;
    let after_loop = db.get_messages(&source).await.unwrap();
    assert_eq!(
        after_loop.len(),
        original.len(),
        "tool/model loops append no unchanged snapshot"
    );
    for (earlier, current) in original.iter().zip(&after_loop) {
        assert_eq!(earlier.message_id, current.message_id);
        assert_eq!(earlier.content, current.content);
    }
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: "append-new-pause".into(),
    };
    let mutation = TrackingMutation::AppendEvidence {
        id: request.clone(),
        expected_revision: 1,
        evidence: vec![phoenix_db::coordinator_tracking::TrackingEvidence {
            instance_id: db.instance_id().await.unwrap().to_string(),
            transcript_id: source.clone(),
            message_id: "later-pause".into(),
            quotation: "Do not proceed until I lift the pause".into(),
        }],
    };
    db.add_message(
        &call.message_id,
        &source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: "tracking".into(),
            input: serde_json::json!({"request":mutation}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    let receipt = db
        .mutate_coordinator_tracking(&source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    db.add_message(
        &uuid::Uuid::new_v4().to_string(),
        &source,
        &MessageContent::Tool(crate::db::ToolContent::new(
            &call.tool_use_id,
            serde_json::to_string(&receipt).unwrap(),
            false,
        )),
        None,
        None,
    )
    .await
    .unwrap();
    let persisted_max = sqlx::query_scalar::<_, i64>(
        "SELECT MAX(sequence_id) FROM messages WHERE conversation_id=?1",
    )
    .bind(&source)
    .fetch_one(db.pool())
    .await
    .unwrap();
    current.broadcast_tx.observe_seq(persisted_max);
    let sequence = current.broadcast_tx.next_seq();
    db.add_message_with_seq_and_origin(
        "new-normal-input",
        &source,
        sequence,
        &MessageContent::User(crate::db::UserContent::new(
            "Let's return to this after our detour",
        )),
        None,
        None,
        &phoenix_core::domain::db_schema::InputOrigin::UserApi,
    )
    .await
    .unwrap();
    current.tracking_observation_pending = true;
    dispatch(&mut current, &llm).await;
    let changed = llm.recorded_requests().last().unwrap().clone();
    assert_eq!(first.system[0].text, changed.system[0].text);
    assert_eq!(first.system[0].cache, changed.system[0].cache);
    for (earlier, current) in first.messages.iter().zip(&changed.messages) {
        assert_eq!(earlier.role, current.role);
        assert_eq!(earlier.source_message_id, current.source_message_id);
        assert_eq!(earlier.content, current.content);
    }
    assert!(changed.messages.len() >= first.messages.len());
    assert!(request_text(&changed).contains("\"review_required\":true"));
    assert!(matches!(
        db.read_coordinator_tracking(&source, &request, 0)
            .await
            .unwrap()
            .item
            .resumption,
        TrackingResumption::ReviewRequired { .. }
    ));
    db.update_conversation_state(
        &source,
        &ConvState::ContextExhausted {
            summary: "Detour context; original requested keyboard work and design bookmark omitted"
                .into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        crate::db::ContinueOutcome::Created(next) => next,
        other => panic!("{other:?}"),
    };
    let mut next = runtime(&manager, &db, &successor.id, llm.clone()).await;
    dispatch(&mut next, &llm).await;
    let recovered = llm.recorded_requests().last().unwrap().clone();
    assert_eq!(first.system[0].text, recovered.system[0].text);
    assert!(request_text(&recovered).contains("Finish user-requested keyboard fix"));
    assert!(request_text(&recovered).contains("Why did the design choose this boundary?"));
    assert_eq!(
        db.read_coordinator_tracking(&successor.id, &bookmark, 0)
            .await
            .unwrap()
            .item
            .kind,
        TrackingKind::Bookmark
    );
    assert_eq!(
        db.list_coordinator_tracking(&successor.id, None, false)
            .await
            .unwrap()
            .review_required_count,
        1
    );
    assert!(recovered.system[0]
        .text
        .contains("Goal/turn/PR completion does not automatically resolve broader intent"));
}

mod live {
    use super::*;
    use crate::runtime::traits::{LlmClient, RegistryLlmClient, ToolExecutor};
    use async_trait::async_trait;
    use phoenix_llm::{LlmError, LlmRequest, LlmResponse, TokenChunk};
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Mutex,
    };

    #[derive(Default)]
    struct Budget {
        calls: AtomicUsize,
        request_bytes: AtomicUsize,
        output_tokens: AtomicU64,
        reported_input_tokens: AtomicU64,
    }
    struct BoundedClient {
        inner: Arc<dyn LlmClient>,
        model: String,
        budget: Arc<Budget>,
    }
    impl BoundedClient {
        fn admit(&self, request: &LlmRequest) -> Result<(), LlmError> {
            let calls = self.budget.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let request_bytes = format!("{request:?}").len();
            let bytes = self
                .budget
                .request_bytes
                .fetch_add(request_bytes, Ordering::SeqCst)
                + request_bytes;
            if calls > 15 || bytes > 2_000_000 || request.max_tokens > 2048 {
                return Err(LlmError::invalid_request(
                    "tracking evaluation budget exhausted",
                ));
            }
            Ok(())
        }
        fn record(&self, response: &LlmResponse) {
            self.budget
                .reported_input_tokens
                .fetch_add(response.usage.input_tokens, Ordering::SeqCst);
            self.budget
                .output_tokens
                .fetch_add(response.usage.output_tokens, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl LlmClient for BoundedClient {
        fn freeze_for_request(self: Arc<Self>) -> Result<Arc<dyn LlmClient>, LlmError> {
            Ok(Arc::new(Self {
                inner: self.inner.clone().freeze_for_request()?,
                model: self.model.clone(),
                budget: self.budget.clone(),
            }))
        }
        fn model_id(&self) -> &str {
            &self.model
        }
        async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            self.admit(request)?;
            let response = self.inner.complete(request).await?;
            self.record(&response);
            Ok(response)
        }
        async fn complete_streaming(
            &self,
            request: &LlmRequest,
            chunks: &mpsc::Sender<TokenChunk>,
        ) -> Result<LlmResponse, LlmError> {
            self.admit(request)?;
            let response = self.inner.complete_streaming(request, chunks).await?;
            self.record(&response);
            Ok(response)
        }
    }
    struct GuardedTools {
        inner: ToolRegistryExecutor,
        calls: Arc<Mutex<Vec<String>>>,
        denied: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl ToolExecutor for GuardedTools {
        async fn execute(
            &self,
            call: crate::runtime::deny_gate::CheckedToolCall,
            ctx: crate::tools::ToolContext,
        ) -> Option<crate::tools::ToolOutput> {
            let name = call.name().to_owned();
            let (_, input) = call.clone().into_parts();
            self.calls.lock().unwrap().push(name.clone());
            let allowed_operation = matches!(
                name.as_str(),
                "tracking"
                    | "query_database"
                    | "read_conversation"
                    | "resolve_reference"
                    | "previous_transcripts"
                    | "read_authority"
                    | "read_goal"
                    | "read_automatic_continuation"
                    | "list_watched_conversations"
                    | "list_instances"
                    | "list_models"
                    | "think"
                    | "ask_user_question"
            ) || (name == "create_conversation"
                && input["request"]["op"] == "options");
            let selected_remote = input
                .get("instance")
                .is_some_and(|instance| instance != "local");
            if !allowed_operation || selected_remote {
                self.denied.lock().unwrap().push(name);
                return Some(crate::tools::ToolOutput::error(
                    "Isolated evaluation: external/effectful tools are disabled",
                ));
            }
            self.inner.execute(call, ctx).await
        }
        async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
            self.inner.definitions().await
        }
        async fn definitions_for_language(
            &self,
            language: crate::llm_language::LlmLanguage,
        ) -> Vec<phoenix_llm::ToolDefinition> {
            self.inner.definitions_for_language(language).await
        }
    }
    struct Fixture {
        manager: Arc<crate::runtime::RuntimeManager>,
        db: crate::db::Database,
        registry: Arc<ModelRegistry>,
        client: Arc<BoundedClient>,
        model: String,
        cwd: PathBuf,
        calls: Arc<Mutex<Vec<String>>>,
        denied: Arc<Mutex<Vec<String>>>,
    }
    impl Fixture {
        async fn turn(&self, source: &str, prompt: &str) -> String {
            let mut context =
                ConvContext::new(source, self.cwd.clone(), &self.model, 200_000, 2048);
            context.is_coordinator = true;
            let state = self.db.get_conversation(source).await.unwrap().state;
            assert!(matches!(state, ConvState::Idle));
            let registry = self
                .manager
                .coordinator_tool_registry(source, None)
                .await
                .unwrap();
            let tools = GuardedTools {
                inner: ToolRegistryExecutor::builtin_only(registry, Arc::from([])),
                calls: self.calls.clone(),
                denied: self.denied.clone(),
            };
            let (tx, rx) = mpsc::channel(16);
            let (states, mut state_rx) = watch::channel(state.clone());
            let sequence = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?1",
            )
            .bind(source)
            .fetch_one(self.db.pool())
            .await
            .unwrap();
            let runtime = ConversationRuntime::new(
                context,
                state,
                DatabaseStorage::new(self.db.clone()),
                self.client.clone(),
                tools,
                Arc::new(BrowserSessionManager::default()),
                Arc::new(crate::tools::BashHandleRegistry::new()),
                Arc::new(crate::tools::TmuxRegistry::new()),
                self.registry.clone(),
                crate::terminal::ActiveTerminals::new(),
                rx,
                tx.clone(),
                SseBroadcaster::new(128, sequence),
            )
            .with_parent_tool_cycle_cap(3)
            .with_state_watcher(states)
            .with_fatal_local_authority_fence(self.manager.fatal_local_authority_fence.clone());
            let task = tokio::spawn(runtime.run());
            let input_id = uuid::Uuid::new_v4().to_string();
            tx.send(Event::UserMessage {
                text: prompt.into(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: input_id.clone(),
                user_agent: Some("isolated-tracking-evaluation".into()),
                skill_invocation: None,
            })
            .await
            .unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
                loop {
                    let state = state_rx.borrow().clone();
                    assert!(
                        !matches!(state, ConvState::Error { .. }),
                        "model turn failed: {state:?}"
                    );
                    if matches!(
                        state,
                        ConvState::Idle | ConvState::AwaitingUserResponse { .. }
                    ) {
                        let messages = self.db.get_messages(source).await.unwrap();
                        if let Some(index) = messages
                            .iter()
                            .position(|message| message.message_id == input_id)
                        {
                            if let Some(text) = messages[index + 1..].iter().rev().find_map(
                                |message| match &message.content {
                                    MessageContent::Agent(blocks) => {
                                        let text = blocks
                                            .iter()
                                            .filter_map(|block| match block {
                                                ContentBlock::Text { text } => Some(text.clone()),
                                                ContentBlock::ToolUse { name, input, .. }
                                                    if name == "ask_user_question" =>
                                                {
                                                    Some(input.to_string())
                                                }
                                                _ => None,
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        if text.is_empty() {
                                            None
                                        } else {
                                            Some(text)
                                        }
                                    }
                                    _ => None,
                                },
                            ) {
                                break text;
                            }
                        }
                    }
                    state_rx
                        .changed()
                        .await
                        .expect("evaluation runtime state channel closed");
                }
            })
            .await;
            task.abort();
            let _ = task.await;
            result.expect("tracking evaluation turn exceeded 120 seconds")
        }
    }
    /// Opt-in, synthetic and isolated. Expected behavior is fixed before model calls:
    /// two records; quiet arithmetic; summary-independent return; then a due request.
    #[tokio::test]
    #[ignore = "uses an explicitly selected configured model; set PHOENIX_TRACKING_EVAL_MODEL"]
    async fn tracking_live_manual_memory_relief_and_quiet_gate() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let model = std::env::var("PHOENIX_TRACKING_EVAL_MODEL")
            .expect("set explicit PHOENIX_TRACKING_EVAL_MODEL; no default fallback");
        let directory = tempfile::tempdir().unwrap();
        let actual = Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect());
        let config = phoenix_llm::LlmConfig::from_env(actual);
        let registry = Arc::new(ModelRegistry::new_with_discovery(&config).await);
        assert!(
            registry.get(&model).is_some(),
            "explicit model unavailable; no fallback"
        );
        let isolated =
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(directory.path());
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.initialize_coordinator_work_scope(directory.path().to_str().unwrap())
            .await
            .unwrap();
        let source = db
            .get_or_create_coordinator(
                Some(&model),
                phoenix_core::llm_language::LlmLanguage::PhoenixNative,
            )
            .await
            .unwrap()
            .id;
        let manager = Arc::new(
            crate::runtime::RuntimeManager::new_with_message_retriever_and_runtime_env(
                db.clone(),
                registry.clone(),
                Arc::new(db.fts_retriever()),
                crate::platform::PlatformCapability::None {
                    details: "isolated tracking acceptance".into(),
                },
                Arc::new(crate::tools::mcp::McpClientManager::new()),
                config.credential_helper.clone(),
                &isolated,
            ),
        );
        let budget = Arc::new(Budget::default());
        let client = Arc::new(BoundedClient {
            inner: Arc::new(RegistryLlmClient::new(registry.clone(), model.clone())),
            model: model.clone(),
            budget: budget.clone(),
        });
        let fixture = Fixture {
            manager,
            db: db.clone(),
            registry,
            client,
            model: model.clone(),
            cwd: directory.path().to_path_buf(),
            calls: Arc::new(Mutex::new(Vec::new())),
            denied: Arc::new(Mutex::new(Vec::new())),
        };
        fixture.turn(&source,"Use tracking to remember exactly two synthetic items for this evaluation: requested work titled 'Check Cobalt export totals', whose next step is asking which period to compare; and a bookmark titled 'Why the Orchard boundary?', whose stopping point is explaining why ownership is separate from execution. Record purpose and resumption; references/evidence may be empty because this is the original request. Ask about the period when this request is worth returning to after our detour. Do not read files, create Goals, start workers, send messages, use Bash or act outside this disposable evaluation; the bookmark remains only a question. Briefly acknowledge.").await;
        let saved = db
            .list_coordinator_tracking(&source, None, false)
            .await
            .unwrap();
        assert_eq!(saved.active_count, 2);
        assert_eq!(
            saved
                .items
                .iter()
                .filter(|item| item.kind == TrackingKind::RequestedWork)
                .count(),
            1
        );
        assert_eq!(
            saved
                .items
                .iter()
                .filter(|item| item.kind == TrackingKind::Bookmark)
                .count(),
            1
        );
        let quiet_before = fixture.calls.lock().unwrap().len();
        let quiet = fixture.turn(&source, "What is 2+2?").await;
        assert!(quiet.contains('4'));
        assert!(!quiet.contains("Cobalt") && !quiet.contains("Orchard"));
        assert_eq!(
            fixture.calls.lock().unwrap().len(),
            quiet_before,
            "nothing due: no redundant review reads"
        );
        db.update_conversation_state(&source,&ConvState::ContextExhausted{summary:"We took an arithmetic detour; reply was 4. Original requests and bookmark intentionally omitted from this test handoff.".into()}).await.unwrap();
        let successor = match db.continue_conversation(&source).await.unwrap() {
            crate::db::ContinueOutcome::Created(next) => next,
            other => panic!("{other:?}"),
        };
        let refresher=fixture.turn(&successor.id,"Where were we before that detour? Give a short refresher of saved work and open questions, preserving their status; do not commission external action.").await;
        assert!(
            refresher.contains("Cobalt") && refresher.contains("Orchard"),
            "saved request and bookmark both return: {refresher}"
        );
        let lower = refresher.to_lowercase();
        assert!(
            (lower.contains("period")
                || lower.contains("timeframe")
                || lower.contains("time frame"))
                && lower.contains("ownership")
                && lower.contains("execution"),
            "refresher must recover saved stopping points, not just titles: {refresher}"
        );
        let before = fixture.calls.lock().unwrap().len();
        let quiet = fixture.turn(&successor.id, "What is 3+4?").await;
        assert!(quiet.contains('7'));
        assert!(!quiet.contains("Cobalt") && !quiet.contains("Orchard"));
        assert_eq!(fixture.calls.lock().unwrap().len(), before);
        sqlx::query(
            "UPDATE coordinator_tracking_items SET reviewed_at_us=?1 WHERE kind='requested_work'",
        )
        .bind(
            Utc::now().timestamp_micros()
                - 2 * phoenix_db::coordinator_tracking::TRACKING_REVIEW_INTERVAL_US,
        )
        .execute(db.pool())
        .await
        .unwrap();
        let recovered = fixture.turn(&successor.id, "Hello.").await;
        assert!(
            recovered.contains("Cobalt"),
            "due forgotten request returns without user naming it: {recovered}"
        );
        let denied = fixture.denied.lock().unwrap().clone();
        assert!(
            denied.is_empty(),
            "model attempted effectful tools: {:?}",
            denied
        );
        assert_eq!(
            db.list_coordinator_tracking(&successor.id, None, false)
                .await
                .unwrap()
                .active_count,
            2
        );
        eprintln!("tracking_live_acceptance model={model} calls={} request_debug_bytes={} reported_input_tokens={} reported_output_tokens={} tool_calls={} external_effects=0",budget.calls.load(Ordering::SeqCst),budget.request_bytes.load(Ordering::SeqCst),budget.reported_input_tokens.load(Ordering::SeqCst),budget.output_tokens.load(Ordering::SeqCst),fixture.calls.lock().unwrap().len());
    }
}

#[tokio::test]
async fn tracking_observation_reservation_blocks_concurrent_input_sequence_allocation_until_commit()
{
    use crate::runtime::testing::{InMemoryStorage, MockToolExecutor};
    use crate::runtime::traits::MessageStore;
    let storage = Arc::new(InMemoryStorage::new());
    let (started, release) = storage.gate_tracking_observation();
    let llm = Arc::new(MockLlmClient::new("test-model"));
    let mut context = ConvContext::new(
        "reservation-test",
        PathBuf::from("/tmp"),
        "test-model",
        200_000,
        16_384,
    );
    context.is_coordinator = true;
    let (tx, rx) = mpsc::channel(8);
    let broadcaster = SseBroadcaster::new(32, 0);
    let mut runtime = ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 1 },
        storage.clone(),
        llm.clone(),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        broadcaster.clone(),
    );
    llm.queue_response(phoenix_llm::LlmResponse {
        provider_replay: None,
        content: vec![ContentBlock::text("done")],
        end_turn: true,
        usage: phoenix_llm::Usage::default(),
        usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
        stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
    });
    let dispatch = tokio::spawn(async move {
        runtime.execute_effect(Effect::RequestLlm).await.unwrap();
        runtime.llm_task_handle.take().unwrap().await.unwrap();
    });
    let observation_sequence = started.await.unwrap();
    let mut writer = Box::pin(async {
        let (reservation, sequences) = broadcaster
            .persisted_message_reservation_authority()
            .await
            .reserve_next_range(1)
            .unwrap();
        let message = storage
            .add_message_with_seq(
                "concurrent-input",
                "reservation-test",
                sequences[0],
                &MessageContent::user("concurrent input"),
                None,
                None,
            )
            .await
            .unwrap();
        broadcaster.send_persisted_message(message).unwrap();
        drop(reservation);
        sequences[0]
    });
    assert!(
        matches!(futures::poll!(writer.as_mut()), std::task::Poll::Pending),
        "a second admission cannot allocate/commit past an outstanding observation reservation"
    );
    release.send(()).unwrap();
    dispatch.await.unwrap();
    let writer_sequence = writer.await;
    let messages = storage.get_all_messages("reservation-test").await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].sequence_id, observation_sequence);
    assert_eq!(messages[1].sequence_id, writer_sequence);
    assert!(writer_sequence > observation_sequence);
}
