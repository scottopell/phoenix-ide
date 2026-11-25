use super::*;
use crate::tools::{Tool, ToolContext};
use phoenix_core::domain::db_schema::ToolContent;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::ToolCall;
use phoenix_db::coordinator_tracking::{TrackingDetail, TrackingReceipt};
use phoenix_db::{EndingResponse, WatchEndingEvidence};
use phoenix_llm::{LlmError, LlmRequest, LlmResponse, LlmService};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const COMMISSION: &str =
    "Keep the release concern tracked. It is paused; do not deploy until I explicitly approve.";
const RECOVERY: &str =
    "Inspect the release evidence only; keep execution paused and do not deploy.";
const OLD_INPUT: &str = "Report the earlier baseline, not the release recovery instruction.";
const REJECT_ONLY: &str = "This rejected-only probe must never receive completion credit.";
const TRACK_MESSAGE: &str = "handoff-tracking-assistant";
const TRACK_CALL: &str = "handoff-tracking-call";

struct CapturedRequest {
    ordinal: usize,
    request: LlmRequest,
}

struct HandoffProvider {
    captures: mpsc::UnboundedSender<CapturedRequest>,
    ordinal: AtomicUsize,
    release: Semaphore,
}

#[async_trait::async_trait]
impl LlmService for HandoffProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let ordinal = self.ordinal.fetch_add(1, Ordering::SeqCst);
        self.captures
            .send(CapturedRequest {
                ordinal,
                request: request.clone(),
            })
            .expect("capture receiver remains alive through all turns");
        self.release.acquire().await.unwrap().forget();
        let text = match ordinal {
            0 => "Earlier baseline complete; this is not recovery evidence.",
            1 => "Owner continuation opened; recovery instruction has not settled yet.",
            2 => "Release evidence inspected. Execution remains paused; no deployment performed.",
            3 => "Coordinator resumed with the release concern still tracked and paused.",
            _ => panic!("unexpected provider request {ordinal}"),
        };
        Ok(LlmResponse {
            provider_replay: None,
            content: vec![ContentBlock::Text { text: text.into() }],
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

async fn next_capture(
    captures: &mut mpsc::UnboundedReceiver<CapturedRequest>,
    ordinal: usize,
    conversation_id: &str,
) -> String {
    let observed = tokio::time::timeout(std::time::Duration::from_secs(30), captures.recv())
        .await
        .expect("runtime must reach the gated provider")
        .expect("provider must retain the capture sender");
    assert_eq!(observed.ordinal, ordinal);
    assert_eq!(
        observed.request.telemetry.as_ref().unwrap().conversation_id,
        conversation_id,
        "captured request must belong to the addressed turn: {:?}",
        observed.request
    );
    format!("{:?}", observed.request)
}

async fn native_send(tool: &dyn Tool, source: &str, target: &str, id: Uuid, text: &str) -> Value {
    let output = tool
        .run(
            json!({"target":target, "instance":"local", "message_id":id.to_string(), "message":text}),
            context(source).with_svg_assistant_message_id(TRACK_MESSAGE).with_tool_use_id("handoff-send"),
        )
        .await;
    assert!(output.is_success(), "{}", output.output());
    serde_json::from_str(output.output()).unwrap()
}

async fn settled(events: &mut tokio::sync::broadcast::Receiver<crate::runtime::SseEvent>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match events
                .recv()
                .await
                .expect("settlement event stream remains available")
            {
                crate::runtime::SseEvent::StateChange {
                    state: ConvState::Idle,
                    ..
                } => return,
                crate::runtime::SseEvent::StateChange {
                    state: ConvState::Error { .. },
                    ..
                }
                | crate::runtime::SseEvent::Error { .. } => {
                    panic!("owner turn failed before settlement")
                }
                crate::runtime::SseEvent::Init { .. }
                | crate::runtime::SseEvent::Message { .. }
                | crate::runtime::SseEvent::MessageUpdated { .. }
                | crate::runtime::SseEvent::StateChange { .. }
                | crate::runtime::SseEvent::Token { .. }
                | crate::runtime::SseEvent::LlmFirstByte { .. }
                | crate::runtime::SseEvent::LlmAttempt { .. }
                | crate::runtime::SseEvent::AgentDone { .. }
                | crate::runtime::SseEvent::ConversationBecameTerminal { .. }
                | crate::runtime::SseEvent::ConversationUpdate { .. }
                | crate::runtime::SseEvent::ConversationHardDeleted { .. }
                | crate::runtime::SseEvent::BrowserSessionState { .. }
                | crate::runtime::SseEvent::BashToolProgress { .. }
                | crate::runtime::SseEvent::SteerMessageQueued { .. }
                | crate::runtime::SseEvent::SteerMessageCancelled { .. }
                | crate::runtime::SseEvent::RateLimitSnapshot { .. }
                | crate::runtime::SseEvent::WorkScopeUpdate { .. } => {}
            }
        }
    })
    .await
    .expect("released provider response must settle");
}

async fn evidence_for_input(
    db: &Database,
    transcript: &str,
    input_id: &str,
) -> (String, WatchEndingEvidence) {
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_id FROM coordinator_watch_events WHERE source_transcript_id=?1 AND route_kind='subscription' ORDER BY occurred_at_us",
    ).bind(transcript).fetch_all(db.pool()).await.unwrap();
    for event in events {
        let evidence = db.coordinator_watch_ending_evidence(&event).await.unwrap();
        if evidence
            .settled_input
            .as_ref()
            .is_some_and(|input| input.message_id == input_id)
        {
            return (event, evidence);
        }
    }
    panic!("settled input {input_id} must already have authoritative ending evidence");
}

async fn assert_response_matches_saved_agent(
    db: &Database,
    transcript: &str,
    evidence: &WatchEndingEvidence,
) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let EndingResponse::Captured {
        message_id: assistant_message_id,
        digest: content_digest,
        digest_version,
        excerpt,
        ..
    } = &evidence.response
    else {
        panic!("terminal provider response must have an authoritative response reference")
    };
    assert_eq!(*digest_version, 1);
    let saved = db.get_messages(transcript).await.unwrap();
    let agent = saved
        .iter()
        .find(|message| message.message_id == *assistant_message_id)
        .unwrap();
    assert!(matches!(&agent.content, MessageContent::Agent(_)));
    let mut digest = String::new();
    for byte in Sha256::digest(serde_json::to_vec(&agent.content.to_json()).unwrap()) {
        write!(&mut digest, "{byte:02x}").unwrap();
    }
    assert_eq!(
        *content_digest, digest,
        "reference must digest the saved agent row"
    );
    assert!(agent
        .content
        .to_json()
        .to_string()
        .contains(excerpt.as_ref().expect("fixture returns prose")));
    assistant_message_id.clone()
}

async fn assert_tracking_unchanged(db: &Database, source: &str, original: &TrackingDetail) {
    let current = db
        .read_coordinator_tracking(source, &original.item.id, 0)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&current).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

struct NoLlm;

impl phoenix_core::llm_service::LlmSelector for NoLlm {
    fn get(
        &self,
        _model_id: &str,
    ) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        None
    }

    fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        None
    }
}

pub(super) fn context(conversation_id: &str) -> ToolContext {
    ToolContext::new_without_filesystem(
        CancellationToken::new(),
        conversation_id.to_string(),
        Arc::new(crate::tools::BrowserSessionManager::default()),
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(NoLlm),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(crate::tools::TmuxRegistry::new()),
        crate::work_scope::ResourceScopeKey::Coordinator,
        crate::work_scope::ResourceAuthority::Work,
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "the tracking fixture keeps trusted pending tool context, replay, and persisted evidence assertions together"
)]
async fn record_paused_concern(f: &Fixture, tracking: &dyn Tool) -> TrackingDetail {
    f.db.add_message(
        "handoff-user-commission",
        &f.source,
        &MessageContent::user(COMMISSION),
        None,
        None,
    )
    .await
    .unwrap();
    let instance = f.db.instance_id().await.unwrap().to_string();
    let input = json!({"request":{"op":"record", "record":{
        "kind":"requested_work", "title":"Paused release concern: do not deploy",
        "note":"Execution remains paused. An owner response is evidence, not resolution or permission to deploy.",
        "resumption":"Inspect evidence only; wait for explicit user approval before deployment.",
        "references":[{"kind":"conversation", "instance_id":instance, "product_id":f.product}],
        "evidence":[{"instance_id":instance, "transcript_id":f.source, "message_id":"handoff-user-commission", "quotation":COMMISSION}]
    }}});
    let content = vec![ContentBlock::ToolUse {
        id: TRACK_CALL.into(),
        name: "tracking".into(),
        input: input.clone(),
    }];
    f.db.update_conversation_state(
        &f.source,
        &ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                TRACK_CALL,
                phoenix_core::domain::sm_state::ToolInput::from_name_and_value(
                    "tracking",
                    input.clone(),
                ),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: phoenix_core::domain::sm_state::AssistantMessage {
                message_id: TRACK_MESSAGE.into(),
                content: content.clone(),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    assert!(!f.db.message_exists(TRACK_MESSAGE).await.unwrap());
    let output = tracking
        .run(
            input.clone(),
            context(&f.source)
                .with_svg_assistant_message_id(TRACK_MESSAGE)
                .with_tool_use_id(TRACK_CALL),
        )
        .await;
    assert!(output.is_success(), "{}", output.output());
    let receipt: TrackingReceipt = serde_json::from_str(output.output()).unwrap();
    let replay = tracking
        .run(
            input,
            context(&f.source)
                .with_svg_assistant_message_id(TRACK_MESSAGE)
                .with_tool_use_id(TRACK_CALL),
        )
        .await;
    assert!(replay.is_success(), "{}", replay.output());
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["item_id"], receipt.item_id);
    f.db.add_message(
        TRACK_MESSAGE,
        &f.source,
        &MessageContent::agent(content),
        None,
        None,
    )
    .await
    .unwrap();
    f.db.add_message(
        "handoff-tracking-result",
        &f.source,
        &MessageContent::Tool(ToolContent::new(TRACK_CALL, output.output(), false)),
        None,
        None,
    )
    .await
    .unwrap();
    f.db.update_conversation_state(&f.source, &ConvState::Idle)
        .await
        .unwrap();
    let detail =
        f.db.read_coordinator_tracking(&f.source, &receipt.item_id, 0)
            .await
            .unwrap();
    assert_eq!(detail.evidence.len(), 1);
    assert_eq!(detail.evidence[0].1.quotation, COMMISSION);
    assert_eq!(detail.references.len(), 1);
    assert_eq!(
        detail
            .recorded_by_ember
            .as_ref()
            .unwrap()
            .assistant_message_id,
        TRACK_MESSAGE
    );
    detail
}

async fn assert_no_saved_user_input(db: &Database, transcript: &str, text: &str) {
    let messages = db.get_messages(transcript).await.unwrap();
    assert!(!messages.iter().any(
        |message| matches!(&message.content, MessageContent::User(content) if content.text == text)
    ));
}
async fn assert_no_ending_for_input(db: &Database, input_message_id: &str) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_ending_evidence WHERE input_message_id=?1",
    )
    .bind(input_message_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 0, "an in-flight input cannot have ending evidence");
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one composed owner recovery must retain the same runtime, instruction identity, tracking item, and evidence"
)]
async fn native_owner_rejection_continuation_busy_recovery_and_coordinator_retention() {
    fn phase(name: &str) {
        use std::io::Write as _;

        // Bypass libtest's per-thread capture so aborts retain the last phase.
        let mut stderr = std::io::stderr().lock();
        writeln!(stderr, "[owner-handoff] {name}").expect("write phase marker");
        stderr.flush().expect("flush phase marker");
    }

    phase("fixture construction: begin");
    let (sender, mut captured) = mpsc::unbounded_channel();
    let provider = Arc::new(HandoffProvider {
        captures: sender,
        ordinal: AtomicUsize::new(0),
        release: Semaphore::new(0),
    });
    let f = Fixture::with_registry(Arc::new(phoenix_llm::ModelRegistry::for_test_with_sonnet(
        provider.clone(),
    )))
    .await;
    phase("fixture construction: complete; coordinator tool setup: begin");
    let coordinator_product =
        f.db.get_conversation(&f.source)
            .await
            .unwrap()
            .product_conversation_id;
    let tools = f
        .runtime
        .coordinator_tool_registry(&f.source, None)
        .await
        .unwrap();
    let send = tools.find_tool("send_conversation_message").unwrap();
    let tracking = tools.find_tool("tracking").unwrap();
    let continue_owner = tools.find_tool("continue_conversation").unwrap();
    phase("coordinator tool setup: complete; tracking seed: begin");
    let tracked = record_paused_concern(&f, tracking.as_ref()).await;
    phase("tracking seed: complete; predecessor input seed: begin");
    f.db.watch_product_conversation(&f.product).await.unwrap();
    let target = format!("@conv:{}", f.product);

    // Seed the competing, older ending through the same actual provider seam.
    f.db.update_conversation_state(&f.predecessor, &ConvState::Idle)
        .await
        .unwrap();
    let old_key = Uuid::new_v4();
    let old_receipt = native_send(send.as_ref(), &f.source, &target, old_key, OLD_INPUT).await;
    assert_eq!(old_receipt["outcome"], "delivered");
    phase("predecessor input accepted; standalone dispatch: begin");
    f.runtime
        .dispatch_standalone_turn(&f.predecessor)
        .await
        .unwrap();
    phase("standalone dispatch issued; awaiting predecessor provider observation");
    let old_request = next_capture(&mut captured, 0, &f.predecessor).await;
    phase("predecessor provider observed; predecessor eviction and successor recovery: begin");
    assert!(old_request.contains(OLD_INPUT));
    assert!(!old_request.contains(RECOVERY));
    let mut predecessor_events = f.runtime.subscribe(&f.predecessor).await.unwrap();
    provider.release.add_permits(1);
    settled(&mut predecessor_events).await;
    let old_message = f
        .db
        .get_messages(&f.predecessor)
        .await
        .unwrap()
        .into_iter()
        .find(|message| matches!(&message.content, MessageContent::User(content) if content.text == OLD_INPUT))
        .expect("the delivered input must be materialized in the predecessor")
        .message_id;
    let (old_event, old_evidence) = evidence_for_input(&f.db, &f.predecessor, &old_message).await;
    let old_response =
        assert_response_matches_saved_agent(&f.db, &f.predecessor, &old_evidence).await;
    f.runtime
        .evict_runtime(
            &f.predecessor,
            crate::runtime::EvictionReason::RecoveryReconciliation,
        )
        .await;
    f.db.update_conversation_state(
        &f.predecessor,
        &ConvState::AwaitingContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "handoff-summary-operation".into(),
                rejected_tool_calls: vec![],
                attempt: 1,
            },
        },
    )
    .await
    .unwrap();

    let recovery_key = Uuid::new_v4();
    let rejected_key = Uuid::new_v4();
    for (key, text) in [(recovery_key, RECOVERY), (rejected_key, REJECT_ONLY)] {
        let rejected = native_send(send.as_ref(), &f.source, &target, key, text).await;
        assert_eq!(rejected["outcome"], "rejected");
        assert_eq!(rejected["reason_code"], "invalid_state_for_message");
        assert_eq!(rejected["conversation_id"], f.predecessor);
        assert!(f
            .db
            .product_conversation_client_message_owner(&f.product, &key.to_string())
            .await
            .unwrap()
            .is_none());
        assert_no_ending_for_input(&f.db, &format!("{}:{key}", f.predecessor)).await;
        assert_no_saved_user_input(&f.db, &f.predecessor, text).await;
        assert!(f
            .db
            .get_steering_queue(&f.predecessor)
            .await
            .unwrap()
            .is_empty());
    }
    assert_eq!(provider.ordinal.load(Ordering::SeqCst), 1);
    assert_tracking_unchanged(&f.db, &f.source, &tracked).await;

    // The fixture already persisted SUMMARY_ID before entering summary generation.
    f.db.update_conversation_state(
        &f.predecessor,
        &ConvState::ContextExhausted {
            summary: SUMMARY.into(),
        },
    )
    .await
    .unwrap();
    let reservation = f.reserve().await;
    let successor = reservation.successor_conversation_id.clone();
    let opening_message_id = format!("{successor}:{}", reservation.message_id.as_str());
    let fenced = native_send(send.as_ref(), &f.source, &target, recovery_key, RECOVERY).await;
    assert_eq!(fenced["outcome"], "rejected");
    assert_eq!(fenced["reason_code"], "continuation_opening_pending");
    assert_no_saved_user_input(&f.db, &successor, RECOVERY).await;
    assert!(f.db.get_messages(&successor).await.unwrap().is_empty());
    assert!(f
        .db
        .get_steering_queue(&successor)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(provider.ordinal.load(Ordering::SeqCst), 1);

    let continued = continue_owner
        .run(
            json!({
                "instance":"local", "product_conversation_id":f.product,
                "predecessor_transcript_id":f.predecessor, "summary_message_id":SUMMARY_ID,
            }),
            context(&f.source)
                .with_svg_assistant_message_id(TRACK_MESSAGE)
                .with_tool_use_id("handoff-continue"),
        )
        .await;
    assert!(continued.is_success(), "{}", continued.output());
    let continued: Value = serde_json::from_str(continued.output()).unwrap();
    assert_eq!(continued["conversation_id"], successor);
    assert_eq!(continued["status"], "already_exists");
    f.runtime
        .dispatch_standalone_turn(&successor)
        .await
        .unwrap();
    phase("successor standalone dispatch issued; awaiting opening provider observation");
    let opening_request = next_capture(&mut captured, 1, &successor).await;
    phase("successor opening observed; recovery dispatch/observation: in progress");
    assert!(opening_request.contains(SUMMARY));
    assert!(!opening_request.contains(RECOVERY));
    assert!(!opening_request.contains(REJECT_ONLY));
    let opening_id = opening_message_id;
    let opening_messages = f.db.get_messages(&successor).await.unwrap();
    let opening = opening_messages
        .iter()
        .find(|message| message.message_id == opening_id)
        .unwrap();
    assert_eq!(opening.origin, InputOrigin::SystemGenerated);
    assert!(matches!(&opening.content, MessageContent::Continuation(_)));
    assert!(f
        .db
        .continuation_dispatch_intent(&f.predecessor)
        .await
        .unwrap()
        .is_none());

    let queued = native_send(send.as_ref(), &f.source, &target, recovery_key, RECOVERY).await;
    assert_eq!(queued["outcome"], "queued_as_steering");
    assert_eq!(queued["conversation_id"], successor);
    assert_eq!(queued["message_id"], recovery_key.to_string());
    let replay = native_send(send.as_ref(), &f.source, &target, recovery_key, RECOVERY).await;
    assert_eq!(replay, queued);
    let conflict = native_send(
        send.as_ref(),
        &f.source,
        &target,
        recovery_key,
        "Changed recovery payload must conflict",
    )
    .await;
    assert_eq!(conflict["reason_code"], "idempotency_conflict");
    let queue = f.db.get_steering_queue(&successor).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].text, RECOVERY);
    assert!(
        matches!(&queue[0].origin, InputOrigin::InternalConversation { transcript_id, .. } if transcript_id==&f.source)
    );
    assert_no_saved_user_input(&f.db, &successor, RECOVERY).await;
    assert_no_saved_user_input(&f.db, &f.predecessor, REJECT_ONLY).await;
    assert_tracking_unchanged(&f.db, &f.source, &tracked).await;

    phase("successor recovery steering queued; releasing opening provider");
    provider.release.add_permits(1);
    let recovery_request = next_capture(&mut captured, 2, &successor).await;
    phase("successor recovery provider observed");
    assert!(
        recovery_request.contains(&format!(
            "[Message from conversation {coordinator_product}, transcript {}]",
            f.source
        )),
        "recovery request must retain Coordinator provenance: {recovery_request}"
    );
    assert_eq!(recovery_request.matches(RECOVERY).count(), 1);
    assert!(!recovery_request.contains(REJECT_ONLY));
    assert!(!recovery_request.contains("Changed recovery payload"));
    let in_flight_recovery = f
        .db
        .get_messages(&successor)
        .await
        .unwrap()
        .into_iter()
        .find(|message| matches!(&message.content, MessageContent::User(content) if content.text == RECOVERY))
        .expect("the accepted steering input must be materialized before provider invocation");
    assert_no_ending_for_input(&f.db, &in_flight_recovery.message_id).await;
    assert_tracking_unchanged(&f.db, &f.source, &tracked).await;
    let mut recovery_events = f.runtime.subscribe(&successor).await.unwrap();
    provider.release.add_permits(1);
    settled(&mut recovery_events).await;
    let recovery_messages = f.db.get_messages(&successor).await.unwrap();
    let recovery_input = recovery_messages.iter().find(|message| matches!(&message.content, MessageContent::User(content) if content.text==RECOVERY)).expect("runtime materialized the accepted steering input");
    let recovery_id = recovery_input.message_id.clone();
    assert_eq!(recovery_input.origin, queue[0].origin);
    let (recovery_event, recovery_evidence) =
        evidence_for_input(&f.db, &successor, &recovery_id).await;
    let response = assert_response_matches_saved_agent(&f.db, &successor, &recovery_evidence).await;
    assert_ne!(recovery_event, old_event);
    assert_ne!(response, old_response);
    assert_ne!(recovery_id, old_message);
    assert_ne!(recovery_id, opening_id);
    assert_eq!(
        recovery_evidence.settled_input.as_ref().unwrap().origin,
        Some(recovery_input.origin.clone())
    );
    assert!(
        matches!(&recovery_evidence.response, EndingResponse::Captured { excerpt:Some(text), .. } if text.contains("Execution remains paused"))
    );
    assert_eq!(
        f.db.coordinator_watch_ending_evidence(&old_event)
            .await
            .unwrap(),
        old_evidence
    );
    assert_no_saved_user_input(&f.db, &f.predecessor, REJECT_ONLY).await;
    assert_tracking_unchanged(&f.db, &f.source, &tracked).await;
    assert!(f
        .db
        .get_steering_queue(&successor)
        .await
        .unwrap()
        .is_empty());
    let successor_inputs = f.db.get_messages(&successor).await.unwrap();
    assert_eq!(successor_inputs.iter().filter(|message| matches!(&message.content, MessageContent::User(content) if content.text==RECOVERY)).count(), 1);
    assert_eq!(
        f.db.chain_members_forward(&f.predecessor)
            .await
            .unwrap()
            .len(),
        2
    );

    // The Coordinator's summary deliberately omits the obligation and its pause.
    f.db.add_message(
        "handoff-coordinator-summary",
        &f.source,
        &MessageContent::Continuation(phoenix_core::domain::db_schema::ContinuationContent {
            summary: "A short summary with no release obligations.".into(),
        }),
        None,
        None,
    )
    .await
    .unwrap();
    f.db.update_conversation_state(
        &f.source,
        &ConvState::ContextExhausted {
            summary: "A short summary with no release obligations.".into(),
        },
    )
    .await
    .unwrap();
    let coordinator = f
        .service()
        .continue_manual(
            &f.source,
            crate::api::ContinueConversationRequest {
                handoff: "A short summary with no release obligations.".into(),
                source: crate::api::ContinueHandoffSource::Manual,
                message_id: Uuid::new_v4().to_string(),
                user_agent: None,
                retry_failed_automatic: false,
            },
        )
        .await
        .unwrap();
    f.runtime
        .dispatch_standalone_turn(&coordinator.conversation_id)
        .await
        .unwrap();
    let coordinator_request = next_capture(&mut captured, 3, &coordinator.conversation_id).await;
    assert!(coordinator_request.contains(&tracked.item.id));
    assert!(coordinator_request.contains("Paused release concern: do not deploy"));
    assert!(coordinator_request.contains("historical"));
    assert_tracking_unchanged(&f.db, &coordinator.conversation_id, &tracked).await;
    let detail_output = tracking
        .run(
            json!({"request":{"op":"read", "id":tracked.item.id}}),
            context(&coordinator.conversation_id)
                .with_svg_assistant_message_id("resumed-coordinator")
                .with_tool_use_id("read-paused-concern"),
        )
        .await;
    assert!(detail_output.is_success(), "{}", detail_output.output());
    let retained: Value = serde_json::from_str(detail_output.output()).unwrap();
    assert_eq!(retained["item"]["disposition"]["state"], "tracking");
    assert!(retained["item"]["resumption"]["resumption"]
        .as_str()
        .unwrap()
        .contains("explicit user approval"));
    assert_eq!(retained["evidence"][0][1]["quotation"], COMMISSION);
    let mut coordinator_events = f
        .runtime
        .subscribe(&coordinator.conversation_id)
        .await
        .unwrap();
    provider.release.add_permits(1);
    settled(&mut coordinator_events).await;
    assert_tracking_unchanged(&f.db, &coordinator.conversation_id, &tracked).await;
    assert_eq!(provider.ordinal.load(Ordering::SeqCst), 4);
    assert!(matches!(
        captured.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}
