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
    apply_pending_round(db, source, &call, &mutation)
        .await
        .item_id
}
async fn apply_pending_round(
    db: &crate::db::Database,
    source: &str,
    call: &SourceToolCall,
    mutation: &TrackingMutation,
) -> phoenix_db::coordinator_tracking::TrackingReceipt {
    let input = serde_json::json!({"request":mutation});
    let blocks = vec![ContentBlock::ToolUse {
        id: call.tool_use_id.clone(),
        name: "tracking".into(),
        input: input.clone(),
    }];
    db.update_conversation_state(
        source,
        &ConvState::ToolExecuting {
            current_tool: crate::state_machine::state::ToolCall::new(
                &call.tool_use_id,
                crate::state_machine::state::ToolInput::from_name_and_value("tracking", input),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: crate::state_machine::state::AssistantMessage {
                message_id: call.message_id.clone(),
                content: blocks.clone(),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    assert!(!db.message_exists(&call.message_id).await.unwrap());
    let receipt = db
        .mutate_coordinator_tracking(source, call, mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    db.add_message(
        &call.message_id,
        source,
        &MessageContent::Agent(blocks),
        None,
        None,
    )
    .await
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
    db.update_conversation_state(source, &ConvState::Idle)
        .await
        .unwrap();
    receipt
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
#[allow(clippy::too_many_lines)]
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
    apply_pending_round(&db, &source, &call, &mutation).await;
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
        failed_calls: AtomicUsize,
    }
    impl Drop for Budget {
        fn drop(&mut self) {
            eprintln!("tracking_eval_budget_final calls={} failed_calls={} request_debug_bytes={} reported_total_input_tokens={} reported_output_tokens={}",self.calls.load(Ordering::SeqCst),self.failed_calls.load(Ordering::SeqCst),self.request_bytes.load(Ordering::SeqCst),self.reported_input_tokens.load(Ordering::SeqCst),self.output_tokens.load(Ordering::SeqCst));
        }
    }
    struct BoundedClient {
        inner: Arc<dyn LlmClient>,
        model: String,
        budget: Arc<Budget>,
    }
    impl BoundedClient {
        fn admit(&self, request: &LlmRequest) -> Result<(), LlmError> {
            use sha2::Digest;
            let system_hash = format!(
                "{:x}",
                sha2::Sha256::digest(format!("{:?}", request.system).as_bytes())
            );
            let calls = self.budget.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let request_bytes = format!("{request:?}").len();
            let bytes = self
                .budget
                .request_bytes
                .fetch_add(request_bytes, Ordering::SeqCst)
                + request_bytes;
            eprintln!("tracking_eval_request model={} attempt={calls} effort={:?} route={:?} max_output={} system_debug_sha256={system_hash} debug_request_bytes={request_bytes}",self.model,request.effective_effort,self.inner.diagnostic_route().0,request.max_tokens);
            if calls > 15 || bytes > 2_000_000 || request.max_tokens > 2048 {
                return Err(LlmError::invalid_request(
                    "tracking evaluation budget exhausted",
                ));
            }
            Ok(())
        }
        fn record(&self, response: &LlmResponse) {
            let tool_names = response
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            eprintln!("tracking_eval_response end_turn={} blocks={} tools={tool_names:?} input_tokens={} cache_read_tokens={} cache_creation_tokens={} output_tokens={}",response.end_turn,response.content.len(),response.usage.input_tokens,response.usage.cache_read_tokens,response.usage.cache_creation_tokens,response.usage.output_tokens);
            self.budget.reported_input_tokens.fetch_add(
                response
                    .usage
                    .input_tokens
                    .saturating_add(response.usage.cache_read_tokens)
                    .saturating_add(response.usage.cache_creation_tokens),
                Ordering::SeqCst,
            );
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
            let response = self.inner.complete(request).await.inspect_err(|error| {
                self.budget.failed_calls.fetch_add(1, Ordering::SeqCst);
                eprintln!("tracking_eval_provider_failure kind={:?}", error.kind);
            })?;
            self.record(&response);
            Ok(response)
        }
        async fn complete_streaming(
            &self,
            request: &LlmRequest,
            chunks: &mpsc::Sender<TokenChunk>,
        ) -> Result<LlmResponse, LlmError> {
            self.admit(request)?;
            let response = self
                .inner
                .complete_streaming(request, chunks)
                .await
                .inspect_err(|error| {
                    self.budget.failed_calls.fetch_add(1, Ordering::SeqCst);
                    eprintln!("tracking_eval_provider_failure kind={:?}", error.kind);
                })?;
            self.record(&response);
            Ok(response)
        }
    }
    struct GuardedTools {
        inner: ToolRegistryExecutor,
        db: crate::db::Database,
        source_checkpoint_presence: Arc<Mutex<Vec<bool>>>,
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
            if name == "tracking" {
                let source = ctx
                    .source_tool_call()
                    .expect("runtime supplies exact source call");
                let present = self.db.message_exists(&source.message_id).await.unwrap();
                self.source_checkpoint_presence
                    .lock()
                    .unwrap()
                    .push(present);
            }
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
    struct Fixture<C: LlmClient + 'static> {
        manager: Arc<crate::runtime::RuntimeManager>,
        db: crate::db::Database,
        registry: Arc<ModelRegistry>,
        client: Arc<C>,
        model: String,
        cwd: PathBuf,
        calls: Arc<Mutex<Vec<String>>>,
        denied: Arc<Mutex<Vec<String>>>,
        source_checkpoint_presence: Arc<Mutex<Vec<bool>>>,
    }
    async fn admitted_input(
        db: &crate::db::Database,
        source: &str,
        prompt: &str,
        input_id: &str,
    ) -> (Event, phoenix_workflow::TurnAuthorityId) {
        use phoenix_core::domain::sm_event::{
            DirectTurnAttemptAuthority, PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
            SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
        };
        use phoenix_workflow::{
            AcceptedDisposition, ClientTurnKey, ConversationAuthority, LeaseExpiry, PreparedTurn,
            ProcessIncarnation, Timestamp, TurnOutcome,
        };
        let payload = PreparedDirectTurnPayload::from_parts(
            SubmittedDirectTurnIdentity {
                origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
                text: prompt.into(),
                images: vec![],
                files: vec![],
                message_id: input_id.into(),
                user_agent: Some("isolated-tracking-evaluation".into()),
                skill_invocation: None,
                expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
            },
            PreparedDirectTurnDelivery {
                text: prompt.into(),
                llm_text: None,
                images: vec![],
                files: vec![],
                user_agent: Some("isolated-tracking-evaluation".into()),
                skill_invocation: None,
            },
        );
        let now = u64::try_from(Utc::now().timestamp()).unwrap();
        let repo = db.workflow_repository();
        let accepted = repo
            .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(input_id).unwrap(),
                prepared: PreparedTurn::from_exact_payload(
                    &ConversationAuthority(source.into()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(now),
            })
            .await
            .established()
            .expect("input acceptance is classified")
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("new isolated input must be accepted")
        };
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let authority = repo
            .claim_authoritative_turn(&phoenix_db::workflow::ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(now),
                lease_until: LeaseExpiry(now + 300),
            })
            .await
            .unwrap()
            .authority
            .expect("new isolated input owns its workflow attempt");
        assert_eq!(
            repo.preflight_direct_turn_materialization(
                &phoenix_db::workflow::PreflightDirectTurnMaterializationInput {
                    turn_id,
                    authority: authority.clone(),
                    prepared: payload.clone(),
                    now: Timestamp(now),
                }
            )
            .await
            .unwrap(),
            phoenix_db::workflow::DirectTurnMaterializationEligibility::Fresh
        );
        (
            Event::AuthoritativeUserMessage {
                payload,
                authority: DirectTurnAttemptAuthority::new(
                    authority.workflow_id.0,
                    turn_id.0,
                    authority.effect_id.0,
                    authority.attempt_id.0,
                    authority.declared_workflow_version.0,
                    authority.generation.0,
                    authority.process_incarnation.0,
                ),
            },
            turn_id,
        )
    }
    impl<C: LlmClient + 'static> Fixture<C> {
        #[allow(clippy::too_many_lines)]
        async fn turn(&self, source: &str, prompt: &str) -> String {
            eprintln!(
                "tracking_eval_turn_start source={source} prompt_bytes={}",
                prompt.len()
            );
            let mut context =
                ConvContext::new(source, self.cwd.clone(), &self.model, 200_000, 2048);
            context.is_coordinator = true;
            let conversation = self.db.get_conversation(source).await.unwrap();
            context.effort = conversation.effort;
            assert_eq!(
                conversation.model.as_deref(),
                Some(self.model.as_str()),
                "persisted and runtime model identities agree"
            );
            assert_eq!(
                self.client.model_id(),
                self.model,
                "injected provider matches runtime model"
            );
            let state = conversation.state;
            assert!(matches!(state, ConvState::Idle));
            let registry = self
                .manager
                .coordinator_tool_registry(source, None)
                .await
                .unwrap();
            let tools = GuardedTools {
                inner: ToolRegistryExecutor::builtin_only(registry, Arc::from([])),
                db: self.db.clone(),
                source_checkpoint_presence: self.source_checkpoint_presence.clone(),
                calls: self.calls.clone(),
                denied: self.denied.clone(),
            };
            let (tx, rx) = mpsc::channel(16);
            let (acknowledged_tx, acknowledged_rx) = mpsc::channel(1);
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
            .with_parent_tool_cycle_cap(8)
            .with_acknowledged_event_receiver(acknowledged_rx)
            .with_state_watcher(states)
            .with_fatal_local_authority_fence(self.manager.fatal_local_authority_fence.clone());
            let task = tokio::spawn(runtime.run());
            let input_id = uuid::Uuid::new_v4().to_string();
            let (acknowledgement, acknowledged) = tokio::sync::oneshot::channel();
            let (retired, retirement) = tokio::sync::oneshot::channel();
            let (event, turn_id) = admitted_input(&self.db, source, prompt, &input_id).await;
            acknowledged_tx
                .send(crate::runtime::AcknowledgedEventRequest {
                    event,
                    acknowledgement,
                    retirement,
                })
                .await
                .unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
                assert_eq!(acknowledged.await.expect("runtime owns admission response").expect("authoritative input dispatch succeeds"), crate::runtime::AcknowledgedEventOutcome::Settled);
                drop(retired);
                let accepted = self.db.workflow_repository().load_authoritative_turn(turn_id).await.unwrap().unwrap();
                let phoenix_workflow::Materialization::Materialized { message_id } = accepted.materialization else { panic!("settled authoritative input must have a materialization receipt") };
                let canonical_input_id = message_id.0;
                assert!(self.db.message_exists(&canonical_input_id).await.unwrap(), "materialization receipt must name the actual input row before waiting for model text");
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
                        if self.db.get_conversation(source).await.unwrap().state != state {
                            state_rx
                                .changed()
                                .await
                                .expect("stable live state must follow its durable commit");
                            continue;
                        }
                        let messages = self.db.get_messages(source).await.unwrap();
                        if let Some(index) = messages
                            .iter()
                            .position(|message| message.message_id == canonical_input_id)
                        {
                            assert_eq!(messages[index].origin, phoenix_core::domain::db_schema::InputOrigin::UserApi, "fixture exercises actual human-input admission rather than generated legacy messages");
                            assert!(!messages[index + 1..].iter().any(|message| matches!(&message.content, MessageContent::System(notice) if notice.text.contains("Tool-use iteration limit reached"))), "bounded evaluation exhausted its per-turn tool-cycle cap");
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
            if result.is_err() {
                eprintln!("tracking_eval_timeout durable_state={:?} observed_state={:?} runtime_finished={}",self.db.get_conversation(source).await.unwrap().state,*state_rx.borrow(),task.is_finished());
            }
            task.abort();
            let _ = task.await;
            result.expect("tracking evaluation turn exceeded 120 seconds")
        }
    }
    #[tokio::test]
    async fn tracking_real_runtime_loop_applies_native_record_before_assistant_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let isolated =
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::with_root(directory.path());
        let db = crate::db::Database::open_in_memory().await.unwrap();
        db.initialize_coordinator_work_scope(directory.path().to_str().unwrap())
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
        let registry = Arc::new(ModelRegistry::new_empty());
        let manager = Arc::new(
            crate::runtime::RuntimeManager::new_with_message_retriever_and_runtime_env(
                db.clone(),
                registry.clone(),
                Arc::new(db.fts_retriever()),
                crate::platform::PlatformCapability::None {
                    details: "offline complete runtime ordering regression".into(),
                },
                Arc::new(crate::tools::mcp::McpClientManager::new()),
                None,
                &isolated,
            ),
        );
        let client = Arc::new(MockLlmClient::new("test-model"));
        client.queue_response(LlmResponse{provider_replay:None,content:vec![ContentBlock::ToolUse{id:"pending-record-call".into(),name:"tracking".into(),input:serde_json::json!({"request":{"op":"record","record":{"kind":"requested_work","title":"Native pending-round request","note":"Real runtime owns the pending assistant","resumption":"Return to the user's outcome after a detour","references":[],"evidence":[]}}})}],end_turn:false,usage:phoenix_llm::Usage::default(),usage_observation:phoenix_core::domain::llm_types::UsageObservation::default(),stream_telemetry:phoenix_llm::ProviderStreamTelemetry::non_streaming()});
        client.queue_response(LlmResponse {
            provider_replay: None,
            content: vec![ContentBlock::text("Saved the request.")],
            end_turn: true,
            usage: phoenix_llm::Usage::default(),
            usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
            stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
        });
        let fixture = Fixture {
            manager,
            db: db.clone(),
            registry,
            client: client.clone(),
            model: "test-model".into(),
            cwd: directory.path().to_path_buf(),
            calls: Arc::new(Mutex::new(Vec::new())),
            denied: Arc::new(Mutex::new(Vec::new())),
            source_checkpoint_presence: Arc::new(Mutex::new(Vec::new())),
        };
        assert!(fixture
            .turn(&source, "Remember the requested outcome in tracking.")
            .await
            .contains("Saved"));
        assert_eq!(*fixture.source_checkpoint_presence.lock().unwrap(),vec![false],"normal execution occurs before the assistant message checkpoint; no fixture preinserts state or message rows");
        let page = db
            .list_coordinator_tracking(&source, None, false)
            .await
            .unwrap();
        assert_eq!(page.active_count, 1);
        let detail = db
            .read_coordinator_tracking(&source, &page.items[0].id, 0)
            .await
            .unwrap();
        assert!(
            detail.recorded_by_ember.unwrap().source_available,
            "completed tool round checkpoints its owning assistant"
        );
        let messages = db.get_messages(&source).await.unwrap();
        let result = messages
            .iter()
            .find_map(|message| match &message.content {
                MessageContent::Tool(result) if result.tool_use_id == "pending-record-call" => {
                    Some(result)
                }
                _ => None,
            })
            .expect("real checkpoint retains the tool result");
        assert!(!result.is_error, "{}", result.content);
        let receipt: serde_json::Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(receipt["outcome"], "applied");
        assert_eq!(client.recorded_requests().len(), 2);
        assert_eq!(*fixture.calls.lock().unwrap(), vec!["tracking"]);
        assert!(fixture.denied.lock().unwrap().is_empty());
    }

    /// Opt-in, synthetic and isolated. Expected behavior is fixed before model calls:
    /// two records; quiet arithmetic; summary-independent return; then a due request.
    #[tokio::test]
    #[ignore = "uses an explicitly selected configured model; set PHOENIX_TRACKING_EVAL_MODEL"]
    #[allow(clippy::too_many_lines)]
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
        let effort = phoenix_core::domain::llm_types::ModelEffort::High;
        assert!(registry.supports_effort(&model, effort));
        db.update_conversation_model_and_effort(
            &source,
            &model,
            Some(effort),
            phoenix_core::domain::llm_types::ServiceTier::Standard,
            &registry.connection_for_model(&model).unwrap(),
        )
        .await
        .unwrap();
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
            source_checkpoint_presence: Arc::new(Mutex::new(Vec::new())),
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
        let (cue, input_id, observation_id): (Option<String>, String, String) = sqlx::query_as("SELECT cue_item_id,input_message_id,message_id FROM coordinator_tracking_observations WHERE transcript_id=?1").bind(&successor.id).fetch_one(db.pool()).await.unwrap();
        let expected_item = saved
            .items
            .iter()
            .find(|item| item.kind == TrackingKind::RequestedWork)
            .unwrap();
        assert_eq!(
            cue.as_deref(),
            Some(expected_item.id.as_str()),
            "due review must be admitted independently of model behavior"
        );
        let messages = db.get_messages(&successor.id).await.unwrap();
        let current_input = messages
            .iter()
            .find(|message| message.message_id == input_id)
            .unwrap();
        assert_eq!(
            current_input.origin,
            phoenix_core::domain::db_schema::InputOrigin::UserApi
        );
        assert!(
            matches!(&current_input.content, MessageContent::User(content) if content.text == "Hello.")
        );
        let observation = messages
            .iter()
            .find(|message| message.message_id == observation_id)
            .unwrap();
        assert!(observation.sequence_id > current_input.sequence_id);
        let MessageContent::User(meta) = &observation.content else {
            panic!("tracking observation is a durable meta message")
        };
        let captured: phoenix_db::coordinator_tracking::TrackingObservation =
            serde_json::from_str(meta.text.split_once('\n').unwrap().1).unwrap();
        assert_eq!(
            captured.admitted_input_sequence_id,
            Some(current_input.sequence_id)
        );
        assert_eq!(
            captured.requested_item_due_for_review.unwrap().id,
            expected_item.id
        );
        assert!(
            recovered.contains("Cobalt"),
            "due forgotten request returns without user naming it: {recovered}"
        );
        let state = db.get_conversation(&successor.id).await.unwrap().state;
        if matches!(state, ConvState::Idle)
            && budget.calls.load(Ordering::SeqCst) < 14
            && budget.request_bytes.load(Ordering::SeqCst) < 1_750_000
        {
            let calls_before = fixture.calls.lock().unwrap().len();
            let quiet = fixture.turn(&successor.id, "What is 5+6?").await;
            assert!(quiet.contains("11"));
            assert!(!quiet.contains("Cobalt") && !quiet.contains("Orchard"));
            assert_eq!(
                fixture.calls.lock().unwrap().len(),
                calls_before,
                "historical cue must not cause another review"
            );
            eprintln!("tracking_eval_post_due_quiet passed");
        } else {
            eprintln!("tracking_eval_post_due_quiet not_run state={state:?} calls={} request_debug_bytes={}",budget.calls.load(Ordering::SeqCst),budget.request_bytes.load(Ordering::SeqCst));
        }
        let denied = fixture.denied.lock().unwrap().clone();
        assert!(
            denied.is_empty(),
            "model attempted effectful tools: {denied:?}"
        );
        assert_eq!(
            db.list_coordinator_tracking(&successor.id, None, false)
                .await
                .unwrap()
                .active_count,
            2
        );
        eprintln!("tracking_live_acceptance model={model} calls={} failed_calls={} request_debug_bytes={} reported_total_input_tokens={} reported_output_tokens={} tool_calls={} external_effects=0",budget.calls.load(Ordering::SeqCst),budget.failed_calls.load(Ordering::SeqCst),budget.request_bytes.load(Ordering::SeqCst),budget.reported_input_tokens.load(Ordering::SeqCst),budget.output_tokens.load(Ordering::SeqCst),fixture.calls.lock().unwrap().len());
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
    let mut received = broadcaster.subscribe();
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
    let messages = storage.get_all_messages("reservation-test");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].sequence_id, observation_sequence);
    assert_eq!(messages[1].sequence_id, writer_sequence);
    assert!(writer_sequence > observation_sequence);
    let mut published = vec![];
    while let Ok(event) = received.try_recv() {
        if let SseEvent::Message { message, .. } = event {
            published.push(message.sequence_id);
        }
    }
    assert_eq!(published, vec![observation_sequence, writer_sequence]);
}

#[tokio::test]
async fn tracking_observation_cancelled_before_classification_closes_local_authority() {
    use crate::runtime::testing::{InMemoryStorage, MockToolExecutor};
    let storage = Arc::new(InMemoryStorage::new());
    let (started, _release) = storage.gate_tracking_observation();
    let mut context = ConvContext::new(
        "cancel-observation",
        PathBuf::from("/tmp"),
        "test-model",
        200_000,
        16_384,
    );
    context.is_coordinator = true;
    let (tx, rx) = mpsc::channel(8);
    let mut runtime = ConversationRuntime::new(
        context,
        ConvState::LlmRequesting { attempt: 1 },
        storage,
        Arc::new(MockLlmClient::new("test-model")),
        Arc::new(MockToolExecutor::new()),
        Arc::new(BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::tools::TmuxRegistry::new()),
        Arc::new(ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        rx,
        tx,
        SseBroadcaster::new(32, 0),
    );
    let fence = runtime.fatal_local_authority_fence.clone();
    let dispatch = tokio::spawn(async move { runtime.execute_effect(Effect::RequestLlm).await });
    started.await.unwrap();
    dispatch.abort();
    assert!(dispatch.await.unwrap_err().is_cancelled());
    assert!(fence.is_closed());
    assert!(fence.try_acquire().is_err());
}
