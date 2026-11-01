use super::*;
use crate::runtime::RuntimeManager;
use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
use phoenix_core::llm_language::LlmLanguage;
use phoenix_db::{ContinuationDispatchIntent, Database};
use std::sync::Arc;

const SUMMARY: &str = "Retained generated context, not new user authority.";
const SUMMARY_ID: &str = "exhausted-summary";
const WINNER: &str = "winning-opening";

struct Fixture {
    db: Database,
    runtime: Arc<RuntimeManager>,
    source: String,
    predecessor: String,
    product: ProductConversationId,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_registry(Arc::new(phoenix_llm::ModelRegistry::new_empty())).await
    }

    async fn with_registry(registry: Arc<phoenix_llm::ModelRegistry>) -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let predecessor = db
            .create_conversation(
                "native-continue-target",
                "native-continue-target",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        db.update_conversation_state(
            &predecessor.id,
            &ConvState::ContextExhausted {
                summary: SUMMARY.to_string(),
            },
        )
        .await
        .unwrap();
        db.add_message_with_seq_and_origin(
            SUMMARY_ID,
            &predecessor.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            registry,
            crate::platform::PlatformCapability::None {
                details: "native continuation tests".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        Self {
            db,
            runtime,
            source: source.id,
            predecessor: predecessor.id,
            product: predecessor.product_conversation_id,
        }
    }

    fn request(&self, key: &str) -> NativeContinueRequest {
        NativeContinueRequest {
            source_transcript_id: self.source.clone(),
            product_conversation_id: self.product.clone(),
            predecessor_transcript_id: self.predecessor.clone(),
            summary_message_id: SUMMARY_ID.into(),
            first_message_id: ClientTurnKey::try_from(key).unwrap(),
            source_gate_wait: None,
        }
    }

    fn service(&self) -> ContinuationApplicationService {
        ContinuationApplicationService::new(self.runtime.clone())
    }

    async fn reserve(&self) -> ContinuationDispatchIntent {
        let result = self
            .db
            .continue_from_current_coordinator(CurrentCoordinatorContinuationRequest {
                source_transcript_id: &self.source,
                product_conversation_id: &self.product,
                predecessor_transcript_id: &self.predecessor,
                summary_message_id: SUMMARY_ID,
                first_message_id: ClientTurnKey::try_from(WINNER).unwrap(),
            })
            .await
            .established()
            .expect("reservation classified")
            .unwrap();
        assert!(matches!(result.0, ContinueOutcome::Created(_)));
        match result.1 {
            CoordinatorContinuationOpening::Pending(intent) => intent,
            CoordinatorContinuationOpening::Accepted { .. } => panic!("new opening was accepted"),
        }
    }

    async fn complete(&self, intent: &ContinuationDispatchIntent) {
        self.db
            .add_message_with_seq_and_origin(
                WINNER,
                &intent.successor_conversation_id,
                1,
                &MessageContent::continuation(SUMMARY),
                None,
                None,
                &InputOrigin::SystemGenerated,
            )
            .await
            .unwrap();
        assert!(self
            .db
            .continuation_dispatch_intent(&self.predecessor)
            .await
            .unwrap()
            .is_none());
    }

    fn pass(&self) -> ReservationPass {
        ReservationPass {
            owner: Some(self.runtime.acquire_local_authority_pass().unwrap()),
        }
    }

    fn assert_closed_with_owner(&self) {
        assert!(self.runtime.fatal_local_authority_is_latched());
        assert!(self.runtime.acquire_local_authority_pass().is_err());
        assert_eq!(
            self.runtime.fatal_authority_owners_at_first_close(),
            Some(1)
        );
    }
}

#[tokio::test]
async fn established_reservation_success_and_rejection_release_without_closing() {
    let f = Fixture::new().await;
    for value in [Ok::<_, &str>(17), Err("acknowledged rejection")] {
        assert_eq!(
            f.pass()
                .classify(LocalAuthorityResult::DurableFactEstablished(value))
                .unwrap(),
            value
        );
        assert!(!f.runtime.fatal_local_authority_is_latched());
    }
    f.runtime.fence_fatal_local_authority().await;
    assert_eq!(f.runtime.fatal_authority_owners_at_first_close(), Some(0));
}

#[tokio::test]
async fn unclassified_reservation_closes_before_owner_release() {
    let f = Fixture::new().await;
    assert!(f
        .pass()
        .classify::<()>(LocalAuthorityResult::DurableFactUnclassified)
        .is_err());
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn abandoned_reservation_closes_before_owner_release() {
    let f = Fixture::new().await;
    drop(f.pass());
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn detached_reservation_worker_still_classifies_after_requester_is_gone() {
    let f = Fixture::new().await;
    let pass = f.pass();
    let (settle, proceed) = tokio::sync::oneshot::channel();
    let (settled, completion) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        proceed.await.unwrap();
        let refused = pass
            .classify::<()>(LocalAuthorityResult::DurableFactUnclassified)
            .is_err();
        settled.send(refused).unwrap();
    });
    drop(worker);
    settle.send(()).unwrap();
    assert!(completion.await.unwrap());
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn reservation_panic_closes_without_an_observing_requester() {
    let f = Fixture::new().await;
    let pass = f.pass();
    let (panic_now, proceed) = tokio::sync::oneshot::channel();
    let mut fatal = f.runtime.fatal_local_authority_receiver();
    let worker = tokio::spawn(async move {
        let _pass = pass;
        proceed.await.unwrap();
        panic!("reservation worker panic");
    });
    drop(worker);
    panic_now.send(()).unwrap();
    fatal.changed().await.unwrap();
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn cancelling_an_armed_reservation_task_closes_before_release() {
    let f = Fixture::new().await;
    let pass = f.pass();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (_hold, waiting) = tokio::sync::oneshot::channel::<()>();
    let worker = tokio::spawn(async move {
        let _pass = pass;
        entered.send(()).unwrap();
        let _ = waiting.await;
    });
    entry.await.unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn closed_authority_returns_terminal_completion_without_reserving_a_successor() {
    let f = Fixture::new().await;
    drop(f.pass());
    let completion = f
        .service()
        .continue_from_coordinator(f.request("closed-retry"))
        .await;
    assert!(matches!(
        completion,
        NativeContinueCompletion::LocalAuthorityClosed
    ));
    assert!(f
        .db
        .get_conversation(&f.predecessor)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
    assert!(f
        .db
        .continuation_dispatch_intent(&f.predecessor)
        .await
        .unwrap()
        .is_none());
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn completed_opening_replay_after_authority_close_has_no_ordinary_reply() {
    let f = Fixture::new().await;
    let intent = f.reserve().await;
    f.complete(&intent).await;
    drop(f.pass());
    let completion = f
        .service()
        .continue_from_coordinator(f.request("closed-replay"))
        .await;
    assert!(matches!(
        completion,
        NativeContinueCompletion::LocalAuthorityClosed
    ));
    let messages =
        f.db.get_messages(&intent.successor_conversation_id)
            .await
            .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, WINNER);
    assert_eq!(
        f.db.get_conversation(&f.predecessor)
            .await
            .unwrap()
            .continued_in_conv_id
            .as_deref(),
        Some(intent.successor_conversation_id.as_str())
    );
    f.assert_closed_with_owner();
}

#[tokio::test]
async fn completed_generated_opening_replays_without_a_provider_or_new_successor() {
    let f = Fixture::new().await;
    let intent = f.reserve().await;
    f.complete(&intent).await;
    let result = f
        .service()
        .continue_from_coordinator(f.request("losing-retry-key"))
        .await
        .established()
        .unwrap();
    assert_eq!(result.conversation_id, intent.successor_conversation_id);
    assert!(matches!(
        result.status,
        ContinueConversationStatus::AlreadyExists
    ));
    assert!(result.error.is_none());
    assert_eq!(
        f.db.get_conversation(&f.predecessor)
            .await
            .unwrap()
            .continued_in_conv_id
            .as_deref(),
        Some(result.conversation_id.as_str())
    );
    let messages = f.db.get_messages(&result.conversation_id).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_id, WINNER);
    assert!(!f.runtime.fatal_local_authority_is_latched());
}

#[tokio::test]
async fn pending_replay_requires_current_successor_capacity_and_retains_winning_intent() {
    let f = Fixture::new().await;
    let intent = f.reserve().await;
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        f.service()
            .continue_from_coordinator(f.request("losing-retry-key")),
    )
    .await
    .expect("pending capacity review completes without a recursive mutation lock")
    .established()
    .expect_err("empty registry has no provider for capacity review");
    assert!(
        error.contains("unavailable through its selected connection"),
        "{error}"
    );
    let pending =
        f.db.continuation_dispatch_intent(&f.predecessor)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        pending.successor_conversation_id,
        intent.successor_conversation_id
    );
    assert_eq!(pending.message_id, intent.message_id);
    assert_eq!(pending.handoff, intent.handoff);
    assert_eq!(
        pending.opening_authority,
        ContinuationOpeningAuthority::GeneratedPredecessorContext
    );
    assert!(pending.user_agent.is_none());
    assert!(f
        .db
        .get_messages(&intent.successor_conversation_id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        f.db.get_conversation(&f.predecessor)
            .await
            .unwrap()
            .continued_in_conv_id
            .as_deref(),
        Some(intent.successor_conversation_id.as_str())
    );
    assert!(!f.runtime.fatal_local_authority_is_latched());
}

struct OpeningProvider {
    entered: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl phoenix_llm::LlmService for OpeningProvider {
    async fn complete(
        &self,
        _request: &phoenix_llm::LlmRequest,
    ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
        self.entered.notify_one();
        Ok(phoenix_llm::LlmResponse {
            provider_replay: None,
            content: Vec::new(),
            end_turn: true,
            usage: phoenix_llm::Usage::default(),
            usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
            stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
        })
    }

    fn model_id(&self) -> &'static str {
        "claude-sonnet-5"
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "preserve the full sequential concurrent-opening integration test and settlement assertions"
)]
async fn fresh_concurrent_requests_dispatch_one_opening_through_exact_turn_settlement() {
    use crate::runtime::SseEvent;
    use phoenix_db::ContinuationOpeningRead;
    use phoenix_workflow::{AcceptedDisposition, Materialization, TurnLifecycle, TurnTerminal};
    use tokio::sync::{broadcast, mpsc, oneshot, Barrier, Semaphore};

    struct CountedProvider {
        entries: mpsc::UnboundedSender<String>,
        release: Semaphore,
    }

    #[async_trait::async_trait]
    impl phoenix_llm::LlmService for CountedProvider {
        async fn complete(
            &self,
            request: &phoenix_llm::LlmRequest,
        ) -> Result<phoenix_llm::LlmResponse, phoenix_llm::LlmError> {
            self.entries
                .send(request.telemetry.as_ref().unwrap().conversation_id.clone())
                .expect("count every provider entry");
            self.release
                .acquire()
                .await
                .expect("provider release gate stays open")
                .forget();
            Ok(phoenix_llm::LlmResponse {
                provider_replay: None,
                content: Vec::new(),
                end_turn: true,
                usage: phoenix_llm::Usage::default(),
                usage_observation: phoenix_core::domain::llm_types::UsageObservation::default(),
                stream_telemetry: phoenix_llm::ProviderStreamTelemetry::non_streaming(),
            })
        }

        fn model_id(&self) -> &'static str {
            "claude-sonnet-5"
        }
    }

    let (entries, mut entered) = mpsc::unbounded_channel();
    let provider = Arc::new(CountedProvider {
        entries,
        release: Semaphore::new(0),
    });
    let registry = Arc::new(phoenix_llm::ModelRegistry::for_test_with_sonnet(
        provider.clone(),
    ));
    let f = Fixture::with_registry(registry).await;
    f.runtime
        .start_direct_turn_worker()
        .await
        .expect("fresh concurrent fixture direct-turn worker startup");
    let predecessor = f.db.get_conversation(&f.predecessor).await.unwrap();
    assert!(matches!(
        predecessor.state,
        ConvState::ContextExhausted { .. }
    ));
    assert!(predecessor.continued_in_conv_id.is_none());
    let scope_id = predecessor.attached_work_scope_id.as_ref().unwrap();
    let scopes_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert!(f
        .db
        .continuation_dispatch_intent(&f.predecessor)
        .await
        .unwrap()
        .is_none());
    let source = f.db.get_conversation(&f.source).await.unwrap();
    let source_gate = f
        .runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    let (left_waiting, left_wait) = oneshot::channel();
    let (right_waiting, right_wait) = oneshot::channel();
    let mut left_request = f.request("fresh-concurrent-left");
    left_request.source_gate_wait = Some(left_waiting);
    let mut right_request = f.request("fresh-concurrent-right");
    right_request.source_gate_wait = Some(right_waiting);
    let start = Barrier::new(2);
    let left_service = f.service();
    let right_service = f.service();
    let (left, right, ()) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(
            async {
                start.wait().await;
                left_service.continue_from_coordinator(left_request).await
            },
            async {
                start.wait().await;
                right_service.continue_from_coordinator(right_request).await
            },
            async {
                left_wait.await.expect("left request reached source gate");
                right_wait.await.expect("right request reached source gate");
                drop(source_gate);
            },
        )
    })
    .await
    .expect("both concurrent requests finish after source admission");
    let left = left.established().unwrap();
    let right = right.established().unwrap();
    let (created, replayed, winning_key) = match (&left.status, &right.status) {
        (ContinueConversationStatus::Accepted, ContinueConversationStatus::AlreadyExists) => {
            (&left, &right, "fresh-concurrent-left")
        }
        (ContinueConversationStatus::AlreadyExists, ContinueConversationStatus::Accepted) => {
            (&right, &left, "fresh-concurrent-right")
        }
        statuses => panic!("one fresh acceptance and one replay required: {statuses:?}"),
    };
    assert_eq!(created.conversation_id, replayed.conversation_id);
    assert_ne!(created.conversation_id, f.predecessor);
    assert!(created.error.is_none());
    assert!(replayed.error.is_none());
    let successor_id = &created.conversation_id;
    let provider_conversation =
        tokio::time::timeout(std::time::Duration::from_secs(30), entered.recv())
            .await
            .expect("fresh opening reaches provider")
            .expect("provider entry channel stays open");
    assert_eq!(provider_conversation, *successor_id);
    let mut events = f.runtime.subscribe(successor_id).await.unwrap();
    let successor = f.db.get_conversation(successor_id).await.unwrap();
    assert!(matches!(successor.state, ConvState::LlmRequesting { .. }));
    assert_eq!(successor.product_conversation_id, f.product);
    assert_eq!(
        successor.attached_work_scope_id,
        predecessor.attached_work_scope_id
    );
    assert_eq!(successor.conv_mode, predecessor.conv_mode);
    assert_eq!(successor.runtime_role, predecessor.runtime_role);
    assert_eq!(
        f.db.get_conversation(&f.predecessor)
            .await
            .unwrap()
            .continued_in_conv_id
            .as_deref(),
        Some(successor_id.as_str())
    );
    let members: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE product_conversation_id = ?1")
            .bind(f.product.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(members, 2);
    let scopes_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(scopes_after, scopes_before);
    let owned_scopes: Vec<String> = sqlx::query_scalar(
        "SELECT work_scope_id FROM product_conversation_work_scopes WHERE product_conversation_id = ?1",
    )
    .bind(f.product.as_str())
    .fetch_all(f.db.pool())
    .await
    .unwrap();
    assert_eq!(owned_scopes, vec![scope_id.as_str().to_string()]);
    let ContinuationOpeningRead::Accepted {
        edge,
        accepted_message_id,
        opening_authority,
        ..
    } = f.db.read_continuation_opening(successor_id).await.unwrap()
    else {
        panic!("fresh opening must be durably accepted before provider entry");
    };
    assert_eq!(edge.product_conversation_id, f.product);
    assert_eq!(edge.predecessor_transcript_row_id, f.predecessor);
    assert_eq!(edge.successor_transcript_row_id, *successor_id);
    assert_eq!(edge.continuation_message_id, SUMMARY_ID);
    assert_eq!(
        opening_authority,
        ContinuationOpeningAuthority::GeneratedPredecessorContext
    );
    assert_eq!(accepted_message_id, format!("{successor_id}:{winning_key}"));
    assert!(f
        .db
        .continuation_dispatch_intent(&f.predecessor)
        .await
        .unwrap()
        .is_none());
    let opening = f.db.get_messages(successor_id).await.unwrap();
    let openings: Vec<_> = opening
        .iter()
        .filter(|m| matches!(m.content, MessageContent::Continuation(_)))
        .collect();
    assert_eq!(openings.len(), 1);
    assert_eq!(openings[0].message_id, accepted_message_id);
    assert!(matches!(openings[0].origin, InputOrigin::SystemGenerated));
    let MessageContent::Continuation(content) = &openings[0].content else {
        unreachable!();
    };
    assert_eq!(content.summary, SUMMARY);
    let workflow = f.db.workflow_repository();
    let turn = workflow
        .load_owning_authoritative_turn(&phoenix_workflow::ConversationAuthority(
            successor_id.clone(),
        ))
        .await
        .unwrap()
        .expect("blocked provider belongs to one admitted turn");
    assert_eq!(turn.conversation.0, *successor_id);
    assert_eq!(turn.client_key.as_str(), winning_key);
    assert_eq!(
        turn.lifecycle,
        TurnLifecycle::Accepted {
            disposition: AcceptedDisposition::Runtime
        }
    );
    assert_eq!(
        turn.materialization,
        Materialization::Materialized {
            message_id: phoenix_workflow::CanonicalMessageId(accepted_message_id.clone())
        }
    );
    let turns: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns WHERE conversation_id = ?1")
            .bind(successor_id)
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(turns, 1);
    loop {
        match events.try_recv() {
            Ok(SseEvent::StateChange {
                state: ConvState::Idle | ConvState::Error { .. },
                ..
            }) => {
                panic!("terminal/error state before provider release");
            }
            Ok(SseEvent::Error { .. }) => panic!("error before provider release"),
            Ok(_) => {}
            Err(broadcast::error::TryRecvError::Empty) => break,
            Err(error) => panic!("pre-release successor event stream failed: {error}"),
        }
    }
    assert!(matches!(
        entered.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    provider.release.add_permits(1);
    let settled_at = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match events
                .recv()
                .await
                .expect("successor event stream must not lag or close")
            {
                SseEvent::StateChange {
                    state: ConvState::Idle,
                    state_updated_at,
                    ..
                } => break state_updated_at,
                SseEvent::StateChange {
                    state: ConvState::Error { .. },
                    ..
                }
                | SseEvent::Error { .. } => panic!("fresh turn failed instead of settling"),
                SseEvent::Init { .. }
                | SseEvent::Message { .. }
                | SseEvent::MessageUpdated { .. }
                | SseEvent::StateChange { .. }
                | SseEvent::Token { .. }
                | SseEvent::LlmFirstByte { .. }
                | SseEvent::LlmAttempt { .. }
                | SseEvent::AgentDone { .. }
                | SseEvent::ConversationBecameTerminal { .. }
                | SseEvent::ConversationUpdate { .. }
                | SseEvent::ConversationHardDeleted { .. }
                | SseEvent::BrowserSessionState { .. }
                | SseEvent::BashToolProgress { .. }
                | SseEvent::SteerMessageQueued { .. }
                | SseEvent::SteerMessageCancelled { .. }
                | SseEvent::RateLimitSnapshot { .. }
                | SseEvent::WorkScopeUpdate { .. } => {}
            }
        }
    })
    .await
    .expect("released fresh turn publishes terminal settlement");
    let settled = workflow
        .load_authoritative_turn(turn.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.id, turn.id);
    assert_eq!(settled.conversation, turn.conversation);
    assert_eq!(settled.client_key, turn.client_key);
    assert_eq!(settled.materialization, turn.materialization);
    assert_eq!(
        settled.lifecycle,
        TurnLifecycle::Terminal {
            terminal: TurnTerminal::Completed,
            disposition: AcceptedDisposition::Runtime
        }
    );
    let final_conversation = f.db.get_conversation(successor_id).await.unwrap();
    assert!(matches!(final_conversation.state, ConvState::Idle));
    assert_eq!(final_conversation.state_updated_at, settled_at);
    assert!(workflow
        .load_owning_authoritative_turn(&phoenix_workflow::ConversationAuthority(
            successor_id.clone()
        ))
        .await
        .unwrap()
        .is_none());
    let final_messages = f.db.get_messages(successor_id).await.unwrap();
    let final_openings: Vec<_> = final_messages
        .iter()
        .filter(|m| matches!(m.content, MessageContent::Continuation(_)))
        .collect();
    assert_eq!(final_openings.len(), 1);
    assert_eq!(final_openings[0].message_id, accepted_message_id);
    assert!(matches!(
        entered.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn pending_replay_uses_winning_generated_opening_and_dispatches_it_once() {
    let provider = Arc::new(OpeningProvider {
        entered: tokio::sync::Notify::new(),
    });
    let registry = Arc::new(phoenix_llm::ModelRegistry::for_test_with_sonnet(
        provider.clone(),
    ));
    let f = Fixture::with_registry(registry).await;
    f.runtime
        .start_direct_turn_worker()
        .await
        .expect("pending replay fixture direct-turn worker startup");
    let intent = f.reserve().await;
    let predecessor_mutation = f.runtime.mutation_admission(&f.predecessor).await.unwrap();
    let successor_mutation = f
        .runtime
        .mutation_admission(&intent.successor_conversation_id)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&predecessor_mutation, &successor_mutation));
    let mutation = predecessor_mutation.lock().await;
    assert!(successor_mutation.try_lock().is_err());
    drop(mutation);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        f.service()
            .continue_from_coordinator(f.request("losing-retry-key")),
    )
    .await
    .expect("shared product mutation gate must not be reacquired")
    .established()
    .unwrap();
    assert_eq!(result.conversation_id, intent.successor_conversation_id);
    assert!(matches!(
        result.status,
        ContinueConversationStatus::AlreadyExists
    ));
    assert!(result.error.is_none());
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.entered.notified(),
    )
    .await
    .expect("provider entry follows durable opening materialization");
    let messages = f.db.get_messages(&result.conversation_id).await.unwrap();
    let openings: Vec<_> = messages
        .iter()
        .filter(|message| matches!(message.content, MessageContent::Continuation(_)))
        .collect();
    assert_eq!(openings.len(), 1);
    assert!(matches!(openings[0].origin, InputOrigin::SystemGenerated));
    assert!(f
        .db
        .continuation_dispatch_intent(&f.predecessor)
        .await
        .unwrap()
        .is_none());
    let second = f
        .service()
        .continue_from_coordinator(f.request("second-losing-retry"))
        .await
        .established()
        .unwrap();
    assert_eq!(second.conversation_id, result.conversation_id);
    assert!(matches!(
        second.status,
        ContinueConversationStatus::AlreadyExists
    ));
    assert!(!f.runtime.fatal_local_authority_is_latched());
}

#[tokio::test]
async fn stale_coordinator_is_refused_even_for_a_completed_opening() {
    let f = Fixture::new().await;
    let intent = f.reserve().await;
    f.complete(&intent).await;
    f.db.update_conversation_state(
        &f.source,
        &ConvState::ContextExhausted {
            summary: "Coordinator rotation".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        f.db.continue_conversation(&f.source).await.unwrap(),
        ContinueOutcome::Created(_)
    ));
    let error = f
        .service()
        .continue_from_coordinator(f.request("stale-retry"))
        .await
        .established()
        .err()
        .unwrap();
    assert_eq!(
        error,
        phoenix_db::DbError::CurrentCoordinatorRequired.to_string()
    );
    assert_eq!(
        f.db.get_messages(&intent.successor_conversation_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(!f.runtime.fatal_local_authority_is_latched());
}

#[tokio::test]
async fn source_rotation_while_waiting_for_source_gate_rejects_completed_replay() {
    let f = Fixture::new().await;
    let intent = f.reserve().await;
    f.complete(&intent).await;
    let source_product =
        f.db.get_conversation(&f.source)
            .await
            .unwrap()
            .product_conversation_id;
    let source_gate = f
        .runtime
        .lock_product_message_admission(source_product.as_str())
        .await;
    let (observed, waiting) = tokio::sync::oneshot::channel();
    let mut request = f.request("rotation-race-retry");
    request.source_gate_wait = Some(observed);
    let service = f.service();
    let worker = tokio::spawn(async move { service.continue_from_coordinator(request).await });
    waiting
        .await
        .expect("initial current-source validation completed before waiting on gate");
    f.db.update_conversation_state(
        &f.source,
        &ConvState::ContextExhausted {
            summary: "rotation before gate acquisition".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        f.db.continue_conversation(&f.source).await.unwrap(),
        ContinueOutcome::Created(_)
    ));
    drop(source_gate);
    let error = worker.await.unwrap().established().err().unwrap();
    assert_eq!(
        error,
        phoenix_db::DbError::CurrentCoordinatorRequired.to_string()
    );
    assert_eq!(
        f.db.get_messages(&intent.successor_conversation_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(!f.runtime.fatal_local_authority_is_latched());
}

#[tokio::test]
async fn coordinator_self_rotation_is_a_nonfatal_preflight_refusal() {
    let f = Fixture::new().await;
    let mut request = f.request("self-rotation");
    request.product_conversation_id =
        f.db.get_conversation(&f.source)
            .await
            .unwrap()
            .product_conversation_id;
    request.predecessor_transcript_id = f.source.clone();
    let error = f
        .service()
        .continue_from_coordinator(request)
        .await
        .established()
        .err()
        .unwrap();
    assert_eq!(
        error,
        "native Continue cannot rotate its Coordinator source"
    );
    assert!(f
        .db
        .get_conversation(&f.source)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
    assert!(f
        .db
        .get_conversation(&f.predecessor)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
    assert!(!f.runtime.fatal_local_authority_is_latched());
}
