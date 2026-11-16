use super::{model, prompt};
use phoenix_core::{
    domain::{
        db_schema::{InputOrigin, SourceToolCall},
        provider_replay::ProviderReplaySettlement,
        sm_event::{
            PreparedDirectTurnDelivery, PreparedDirectTurnPayload,
            SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
        },
        sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput},
    },
    llm_language::LlmLanguage,
    runtime_env::PhoenixRuntimeEnvironment,
};
use phoenix_db::{
    coordinator_scanner::{
        PreparedScanBatch, ScanAction, ScanApplyOutcome, ScanCandidate, ScanCandidateState,
        ScanConnection, ScanContext, ScanFragment, ScanInput, ScanLane, ScanPreparation,
        ScanProfile, ScanReservation, ScanResult,
    },
    coordinator_tracking::{
        TrackingDisposition, TrackingKind, TrackingMutation, TrackingRecord, TrackingRetirement,
    },
    workflow::{
        AcceptAuthoritativeTurn, ClaimAuthoritativeTurnInput, DirectTurnTerminalObligationInput,
        MaterializeAuthoritativeTurnInput, MaterializeAuthoritativeTurnOutcome,
        PersistedConversationProjection, TerminalizeAuthoritativeTurnInput,
    },
    Database, MessageContent, UserContent,
};
use phoenix_llm::{
    ContentBlock, LlmConfig, LlmError, LlmRequest, LlmResponse, LlmService, ModelRegistry,
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, LeaseExpiry, PreparedTurn,
    ProcessIncarnation, Timestamp, TurnCommand, TurnOutcome, TurnTerminal,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const CALL_LIMIT: usize = 37;
const CONTENT_LIMIT: usize = 1024 * 1024;
const CORPUS: &str = "specs/coordinator-scanner/scanner-corpus.json";
const ORACLES: &str = "specs/coordinator-scanner/corpus-oracles.md";

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    input: CaseInput,
    host_only_full_source: Option<Value>,
    host_only_candidate_seed: Option<Vec<Candidate>>,
}
#[derive(Deserialize)]
struct CaseInput {
    messages: Vec<Source>,
    candidates: Vec<Candidate>,
    scan_context: Context,
}
#[derive(Deserialize)]
struct Context {
    captured_at: String,
}
#[derive(Deserialize)]
struct Source {
    message_id: String,
    role: String,
    origin: InputOrigin,
    is_meta: bool,
    created_at: String,
    text: String,
}
#[derive(Deserialize)]
struct Candidate {
    id: String,
    kind: TrackingKind,
    disposition: String,
    title: String,
    context_hint: String,
    retired_at: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct PreparedCase {
    id: String,
    source: String,
    now_us: i64,
    binding_id: String,
    input_sha256: String,
    content_bytes: usize,
    aliases: BTreeMap<String, String>,
    event_aliases: BTreeMap<String, String>,
    message_aliases: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct PredecessorCandidate {
    id: String,
    kind: TrackingKind,
    disposition: TrackingDisposition,
    title: String,
    title_truncated: bool,
    context_hint: String,
    context_hint_truncated: bool,
    retired_at_us: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct PredecessorInput {
    scan_context: ScanContext,
    fragments: Vec<ScanFragment>,
    candidates: Vec<PredecessorCandidate>,
}

fn expected_predecessor_projection(bytes: &[u8], original_input_sha256: &str) -> ScanInput {
    let old: PredecessorInput = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        digest(serde_json::to_vec(&old).unwrap()),
        original_input_sha256
    );
    let candidates = old
        .candidates
        .into_iter()
        .map(|candidate| {
            let state = match candidate.disposition {
                TrackingDisposition::Tracking => {
                    assert!(candidate.retired_at_us.is_none());
                    ScanCandidateState::Tracking
                }
                disposition @ (TrackingDisposition::Resolved | TrackingDisposition::Dismissed) => {
                    let retired = candidate
                        .retired_at_us
                        .expect("closed predecessor has retirement time");
                    let disposition = match disposition {
                        TrackingDisposition::Resolved => TrackingRetirement::Resolved,
                        TrackingDisposition::Dismissed => TrackingRetirement::Dismissed,
                        TrackingDisposition::Tracking => unreachable!(),
                    };
                    ScanCandidateState::Retired {
                        disposition,
                        later_source_fragment_ids: old
                            .fragments
                            .iter()
                            .filter(|fragment| fragment.created_at_us > retired)
                            .map(|fragment| fragment.fragment_id.clone())
                            .collect(),
                    }
                }
            };
            ScanCandidate {
                id: candidate.id,
                kind: candidate.kind,
                state,
                title: candidate.title,
                title_truncated: candidate.title_truncated,
                context_hint: candidate.context_hint,
                context_hint_truncated: candidate.context_hint_truncated,
            }
        })
        .collect();
    ScanInput {
        scan_context: old.scan_context,
        fragments: old.fragments,
        candidates,
    }
}

struct PairedSeeds {
    root: PathBuf,
    manifest: Value,
    manifest_sha256: String,
}

impl PairedSeeds {
    fn load(root: PathBuf, approved_sha256: &str, corpus: &[u8], oracles: &[u8]) -> Self {
        assert!(root.is_absolute());
        let bytes = std::fs::read(root.join("manifest.json")).unwrap();
        assert_eq!(
            digest(&bytes),
            approved_sha256,
            "exact original seeds required"
        );
        let manifest: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(manifest["mode"], "prepared");
        assert_eq!(manifest["provider_calls"], 0);
        assert_eq!(manifest["protocol"]["corpus_sha256"], digest(corpus));
        assert_eq!(manifest["protocol"]["oracles_sha256"], digest(oracles));
        assert_eq!(manifest["cases"].as_array().unwrap().len(), CALL_LIMIT);
        Self {
            root,
            manifest,
            manifest_sha256: approved_sha256.into(),
        }
    }

    async fn copy_case(&self, case: &Case, destination: &Path) -> (Database, PreparedCase) {
        let entry = self.manifest["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == case.id)
            .expect("approved case missing");
        let source = self.root.join(&case.id);
        let record = std::fs::read(source.join("prepared.json")).unwrap();
        assert_eq!(entry["prepared_record_sha256"], digest(&record));
        let prepared: PreparedCase = serde_json::from_slice(&record).unwrap();
        assert_eq!(prepared.id, case.id);
        assert_eq!(entry["binding_id"], prepared.binding_id);
        assert_eq!(entry["input_sha256"], prepared.input_sha256);
        let database = std::fs::read(source.join("source.db")).unwrap();
        assert_eq!(entry["database_sha256"], digest(&database));
        std::fs::write(destination, database).unwrap();
        let db = open(destination).await;
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_scan_attempts")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(attempts, 0, "paired seeds must never contain live attempts");
        (db, prepared)
    }

    fn expected_input(&self, original: &PreparedCase) -> ScanInput {
        assert_eq!(
            self.manifest_sha256,
            "9057badc7ac30f3eab61395569b014d497f2afe24c86e914edf8782daf7e230e",
            "projection conversion is only for the frozen original qualification artifact"
        );
        expected_predecessor_projection(
            &std::fs::read(self.root.join(&original.id).join("input.json")).unwrap(),
            &original.input_sha256,
        )
    }
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    bytes.as_ref().iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").expect("String formatting cannot fail");
        text
    })
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    hex(Sha256::digest(bytes.as_ref()))
}
fn time(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_micros()
}
fn write(path: &Path, value: &impl Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
async fn open(path: &Path) -> Database {
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
    db
}
async fn native(
    db: &Database,
    source: &str,
    mutation: TrackingMutation,
) -> phoenix_db::coordinator_tracking::TrackingReceipt {
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: uuid::Uuid::new_v4().to_string(),
    };
    let input = json!({"request":mutation});
    db.update_conversation_state(
        source,
        &ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &call.tool_use_id,
                ToolInput::from_name_and_value("tracking", input.clone()),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage {
                message_id: call.message_id.clone(),
                content: vec![ContentBlock::ToolUse {
                    id: call.tool_use_id.clone(),
                    name: "tracking".into(),
                    input,
                }],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let receipt = db
        .mutate_coordinator_tracking(source, &call, &mutation)
        .await
        .established()
        .expect("classified native setup")
        .unwrap();
    db.update_conversation_state(source, &ConvState::Idle)
        .await
        .unwrap();
    receipt
}
#[allow(clippy::too_many_lines)]
async fn watched_event(db: &Database, case: &str, event_text: &str) -> String {
    let worker = db
        .create_conversation(
            &format!("{case}-event-source"),
            &format!("synthetic-{case}-event-source"),
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    db.watch_product_conversation(&worker.product_conversation_id)
        .await
        .unwrap();
    let input = "Inspect the synthetic worker checkpoint.";
    let prepared = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: format!("{case}-worker-input"),
            origin: InputOrigin::UserApi,
            text: input.into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: input.into(),
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
            client_key: ClientTurnKey::new(format!("{case}-event-input")).unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(worker.id.clone()),
                prepared.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .established()
        .expect("classified worker acceptance")
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
        panic!("{case}: expected a new worker turn")
    };
    let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
    let authority = repo
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(10),
            lease_until: LeaseExpiry(100),
        })
        .await
        .unwrap()
        .authority
        .expect("real worker claim");
    let materialized = repo
        .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
            turn_id,
            authority,
            prepared,
            sequence_id: 1,
            created_at: Timestamp(10),
            accepted_state: ConvState::LlmRequesting { attempt: 1 },
            state_updated_at: chrono::Utc::now(),
            now: Timestamp(10),
        })
        .await
        .established()
        .expect("classified worker materialization");
    let MaterializeAuthoritativeTurnOutcome::Materialized(materialized) = materialized else {
        panic!("{case}: actual materialization failed: {materialized:?}")
    };
    let projection = PersistedConversationProjection {
        state: ConvState::Idle,
        state_updated_at: chrono::Utc::now(),
    };
    let obligation = DirectTurnTerminalObligationInput {
        turn_id,
        expected_generation: materialized.generation,
        terminal: TurnTerminal::Completed,
        projection: projection.clone(),
        response_message_id: Some(format!("{case}-worker-response")),
    };
    db.add_message_with_seq_and_terminal_obligation(
        &format!("{case}-worker-response"),
        &worker.id,
        2,
        &MessageContent::agent(vec![ContentBlock::text(event_text)]),
        None,
        None,
        &obligation,
    )
    .await
    .unwrap();
    repo.terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
        command: TurnCommand::Complete {
            turn_id,
            expected_generation: materialized.generation,
        },
        projection: Some(projection),
        provider_replay_settlement: ProviderReplaySettlement::Preserve,
    })
    .await
    .unwrap();
    let events = db.pending_coordinator_watch_events(16).await.unwrap();
    let matching: Vec<_> = events
        .into_iter()
        .filter(|event| {
            event.source_transcript_id == worker.id
                && event.source_occurrence_kind == "direct_turn"
                && event.source_occurrence_id == turn_id.0.to_string()
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "{case}: genuine watched completion event must exist"
    );
    matching[0].event_id.clone()
}

#[allow(clippy::too_many_lines)]
async fn fixture(
    db: &Database,
    case: &Case,
) -> (
    String,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
) {
    let source = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    let mut aliases = BTreeMap::new();
    let mut event_aliases = BTreeMap::new();
    let mut message_aliases = BTreeMap::new();
    let seeds = case
        .host_only_candidate_seed
        .as_ref()
        .unwrap_or(&case.input.candidates);
    for (ordinal, candidate) in seeds.iter().enumerate() {
        let receipt = native(
            db,
            &source,
            TrackingMutation::Record {
                record: TrackingRecord {
                    kind: candidate.kind,
                    title: candidate.title.clone(),
                    note: candidate.context_hint.clone(),
                    resumption: candidate.context_hint.clone(),
                    references: vec![],
                    evidence: vec![],
                },
            },
        )
        .await;
        let id = receipt.item_id.clone();
        if candidate.disposition != "tracking" {
            let disposition = match candidate.disposition.as_str() {
                "resolved" => TrackingRetirement::Resolved,
                "dismissed" => TrackingRetirement::Dismissed,
                other => panic!("unsupported disposition {other}"),
            };
            native(
                db,
                &source,
                TrackingMutation::Retire {
                    id: id.clone(),
                    expected_revision: receipt.item_revision,
                    disposition,
                    reason: candidate.context_hint.clone(),
                },
            )
            .await;
            let retired = time(
                candidate
                    .retired_at
                    .as_ref()
                    .expect("explicit synthetic retirement clock"),
            );
            sqlx::query(
                "UPDATE coordinator_tracking_retirements SET recorded_at_us=? WHERE item_id=?",
            )
            .bind(retired)
            .bind(&id)
            .execute(db.pool())
            .await
            .unwrap();
        }
        let created = time("2026-10-01T00:00:00Z") + i64::try_from(ordinal).unwrap();
        let updated = candidate.retired_at.as_deref().map(time).unwrap_or(created);
        sqlx::query("UPDATE coordinator_tracking_items SET created_at_us=?,updated_at_us=?,reviewed_at_us=? WHERE id=?")
            .bind(created).bind(updated).bind(updated).bind(&id).execute(db.pool()).await.unwrap();
        assert!(aliases.insert(candidate.id.clone(), id).is_none());
    }
    for (ordinal, message) in case.input.messages.iter().enumerate() {
        let actual_message_id = uuid::Uuid::new_v4().to_string();
        assert!(message_aliases
            .insert(message.message_id.clone(), actual_message_id.clone())
            .is_none());
        let text = case
            .host_only_full_source
            .as_ref()
            .filter(|full| full["message_id"] == message.message_id)
            .map_or(message.text.as_str(), |full| full["text"].as_str().unwrap());
        let content = match message.role.as_str() {
            "user" if message.is_meta => MessageContent::User(UserContent::meta(text)),
            "user" => MessageContent::User(UserContent::new(text)),
            "assistant" => MessageContent::Agent(vec![ContentBlock::text(text)]),
            other => panic!("unsupported source role {other}"),
        };
        let origin = match &message.origin {
            InputOrigin::SubscriptionEvent { event_id: logical } => {
                let actual = watched_event(db, &case.id, text).await;
                assert!(event_aliases
                    .insert(logical.clone(), actual.clone())
                    .is_none());
                InputOrigin::SubscriptionEvent { event_id: actual }
            }
            original @ (InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::SystemGenerated
            | InputOrigin::SlackGateway { .. }
            | InputOrigin::RemoteApi { .. }
            | InputOrigin::RemoteConversation { .. }
            | InputOrigin::RemoteCoordinator { .. }
            | InputOrigin::ScheduledReminder { .. }) => original.clone(),
        };
        db.add_message_with_seq_and_origin(
            &actual_message_id,
            &source,
            i64::try_from(ordinal + 1).unwrap(),
            &content,
            None,
            None,
            &origin,
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{} source {} ({origin:?}): {error}",
                case.id, message.message_id
            )
        });
        sqlx::query("UPDATE messages SET created_at=? WHERE message_id=?")
            .bind(&message.created_at)
            .bind(&actual_message_id)
            .execute(db.pool())
            .await
            .unwrap();
    }
    (source, aliases, event_aliases, message_aliases)
}
async fn prepare(db: &Database, profile: &ScanProfile, now: i64) -> Box<PreparedScanBatch> {
    for _ in 0..4 {
        match db
            .prepare_coordinator_scan(Some(profile), now)
            .await
            .established()
            .expect("classified prepare")
            .unwrap()
        {
            ScanPreparation::Ready(batch) => return batch,
            ScanPreparation::Progress => {}
            other @ (ScanPreparation::Wait { .. } | ScanPreparation::Idle) => {
                panic!("fixture did not produce actual inference batch: {other:?}")
            }
        }
    }
    panic!("fixture exceeded host-only enrollment progress bound")
}
async fn snapshot(db: &Database, source: &str) -> Value {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM coordinator_tracking_items ORDER BY id")
            .fetch_all(db.pool())
            .await
            .unwrap();
    let mut items = Vec::new();
    for id in ids {
        items.push(db.read_coordinator_tracking(source, &id, 0).await.unwrap());
    }
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let observations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_tracking_observations")
            .fetch_one(db.pool())
            .await
            .unwrap();
    json!({"items":items,"coverage":db.read_coordinator_scan_coverage(source,None).await.unwrap(),
        "messages":messages,"foreground_observations":observations})
}
fn proposal_audit(
    result: &ScanResult,
    input: &phoenix_db::coordinator_scanner::ScanInput,
) -> Value {
    let ScanResult::Complete { actions } = result;
    let mut quotes = Vec::new();
    for (action_index, action) in actions.iter().enumerate() {
        let evidence = match action {
            ScanAction::New { quotes, .. } | ScanAction::Append { quotes, .. } => quotes,
        };
        for quote in evidence {
            let fragments: Vec<_> = input
                .fragments
                .iter()
                .filter(|f| f.fragment_id == quote.fragment_id)
                .collect();
            let occurrences = fragments.first().map_or(0, |f| {
                if quote.quotation.is_empty() {
                    0
                } else {
                    f.text
                        .char_indices()
                        .filter(|(offset, _)| {
                            f.text
                                .get(*offset..)
                                .is_some_and(|tail| tail.starts_with(&quote.quotation))
                        })
                        .count()
                }
            });
            quotes.push(json!({"action_index":action_index,"quote":quote,"fragment_matches":fragments.len(),
                "quotation_occurrences":occurrences,"valid_unique_verbatim":fragments.len()==1 && occurrences==1}));
        }
    }
    json!({"proposed_actions":actions.len(),"quotes":quotes,
        "includes_host_rejected_proposals":true,"semantic_adjudication":"independent_review_required"})
}

fn raw_proposals(records: &[Value], input: &phoenix_db::coordinator_scanner::ScanInput) -> Value {
    let Some(response) = records.last() else {
        return json!({"response_received":false});
    };
    let text: String = response["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            (block["type"] == "text")
                .then(|| block["text"].as_str())
                .flatten()
        })
        .collect();
    let raw_json = serde_json::from_str::<Value>(&text).ok();
    let result = serde_json::from_str::<ScanResult>(&text).ok();
    json!({"raw_json":raw_json,"closed_result":result,
        "source_audit":result.as_ref().map(|result|proposal_audit(result,input)),
        "scored_independently_of_adapter_or_host_acceptance":true})
}

#[allow(clippy::too_many_lines)]
fn validate_prepared(
    case: &Case,
    batch: &PreparedScanBatch,
    aliases: &BTreeMap<String, String>,
    event_aliases: &BTreeMap<String, String>,
    message_aliases: &BTreeMap<String, String>,
) {
    assert_eq!(batch.input().scan_context.lane, ScanLane::Historical);
    assert_eq!(
        batch.input().fragments.len(),
        case.input.messages.len(),
        "every corpus row must be captured: {}",
        case.id
    );
    assert_eq!(message_aliases.len(), case.input.messages.len());
    let encoded = serde_json::to_string(batch.input()).unwrap();
    for (logical, actual) in message_aliases {
        assert!(
            uuid::Uuid::parse_str(actual).is_ok(),
            "source identifiers must be opaque UUIDs"
        );
        assert!(
            !encoded.contains(logical),
            "logical source alias must remain outside inference"
        );
    }
    for source in &case.input.messages {
        let actual = batch
            .input()
            .fragments
            .iter()
            .find(|fragment| fragment.message_id == message_aliases[&source.message_id])
            .unwrap();
        assert_eq!(
            actual.text, source.text,
            "actual default bounded projection differs: {}",
            case.id
        );
        let expected_origin = match &source.origin {
            InputOrigin::SubscriptionEvent { event_id: logical } => {
                InputOrigin::SubscriptionEvent {
                    event_id: event_aliases[logical].clone(),
                }
            }
            original @ (InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::SystemGenerated
            | InputOrigin::SlackGateway { .. }
            | InputOrigin::RemoteApi { .. }
            | InputOrigin::RemoteConversation { .. }
            | InputOrigin::RemoteCoordinator { .. }
            | InputOrigin::ScheduledReminder { .. }) => original.clone(),
        };
        assert_eq!(
            actual.origin, expected_origin,
            "{} retains actual origin class and normalized event identity",
            case.id
        );
        assert_eq!(actual.created_at_us, time(&source.created_at));
    }
    let actual_ids: std::collections::BTreeSet<_> = batch
        .input()
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    let expected_ids: std::collections::BTreeSet<_> = case
        .input
        .candidates
        .iter()
        .map(|candidate| aliases[&candidate.id].as_str())
        .collect();
    assert_eq!(
        actual_ids, expected_ids,
        "genuine visible selection must match frozen fixture: {}",
        case.id
    );
    for expected in &case.input.candidates {
        let actual = batch
            .input()
            .candidates
            .iter()
            .find(|candidate| candidate.id == aliases[&expected.id])
            .unwrap();
        let expected_state = if expected.disposition == "tracking" {
            json!({"status":"tracking"})
        } else {
            let retired = time(expected.retired_at.as_ref().unwrap());
            let later: Vec<_> = batch
                .input()
                .fragments
                .iter()
                .filter(|fragment| fragment.created_at_us > retired)
                .map(|fragment| &fragment.fragment_id)
                .collect();
            json!({"status":"retired","disposition":expected.disposition,"later_source_fragment_ids":later})
        };
        assert_eq!(
            serde_json::to_value(&actual.state).unwrap(),
            expected_state,
            "{} host-computed retirement relation",
            case.id
        );
    }
    if case.id == "U01" {
        let fragment = &batch.input().fragments[0];
        let full = case.host_only_full_source.as_ref().unwrap()["text"]
            .as_str()
            .unwrap();
        assert!(!fragment.complete_message);
        assert_eq!(fragment.utf8_start, 0);
        assert_eq!(fragment.prose_digest, digest(full));
        assert!(fragment.utf8_end < full.len());
        assert!(fragment
            .text
            .contains("Draft the Café résumé exporter for東京; do not send any exports."));
    }
    if case.id == "I01" {
        assert_eq!(aliases.len(), 17);
        assert_eq!(batch.input().candidates.len(), 16);
        assert_eq!(batch.input().scan_context.omitted_active_candidates, 1);
        assert!(!actual_ids.contains(aliases["azure-hidden"].as_str()));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scanner_quality_all_real_host_fixtures() {
    let corpus_bytes = std::fs::read(root().join(CORPUS)).unwrap();
    let oracle_bytes = std::fs::read(root().join(ORACLES)).unwrap();
    let corpus: Corpus = serde_json::from_slice(&corpus_bytes).unwrap();
    let registry = ModelRegistry::new(&LlmConfig {
        openai_api_key: Some("offline-never-dispatched".into()),
        ..LlmConfig::default()
    });
    let service = model::eligible_service(&registry).unwrap();
    let profile = super::profile(service.as_ref(), ScanConnection::OpenAiResponses);
    let root = tempfile::tempdir().unwrap();
    let mut aggregate = 0;
    let mut entries = Vec::new();
    for case in &corpus.cases {
        let dir = root.path().join(&case.id);
        std::fs::create_dir(&dir).unwrap();
        let db = open(&dir.join("source.db")).await;
        let (source, aliases, event_aliases, message_aliases) = fixture(&db, case).await;
        let now_us = time(&case.input.scan_context.captured_at);
        let batch = prepare(&db, &profile, now_us).await;
        validate_prepared(case, &batch, &aliases, &event_aliases, &message_aliases);
        let predecessor = PredecessorInput {
            scan_context: batch.input().scan_context.clone(),
            fragments: batch.input().fragments.clone(),
            candidates: batch
                .input()
                .candidates
                .iter()
                .map(|actual| {
                    let expected = case
                        .input
                        .candidates
                        .iter()
                        .find(|candidate| aliases[&candidate.id] == actual.id)
                        .unwrap();
                    PredecessorCandidate {
                        id: actual.id.clone(),
                        kind: actual.kind,
                        disposition: serde_json::from_value(json!(expected.disposition)).unwrap(),
                        title: actual.title.clone(),
                        title_truncated: actual.title_truncated,
                        context_hint: actual.context_hint.clone(),
                        context_hint_truncated: actual.context_hint_truncated,
                        retired_at_us: expected.retired_at.as_deref().map(time),
                    }
                })
                .collect(),
        };
        let predecessor = serde_json::to_vec(&predecessor).unwrap();
        let projected = expected_predecessor_projection(&predecessor, &digest(&predecessor));
        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            serde_json::to_value(batch.input()).unwrap()
        );
        let content_bytes = model::BoundedRequest::new(prompt::INSTRUCTIONS, batch.input())
            .unwrap()
            .content_bytes();
        aggregate += content_bytes;
        assert!(aggregate <= CONTENT_LIMIT);
        assert_eq!(snapshot(&db, &source).await["foreground_observations"], 0);
        let prepared = PreparedCase {
            id: case.id.clone(),
            source,
            now_us,
            binding_id: batch.binding_id().into(),
            input_sha256: digest(serde_json::to_vec(batch.input()).unwrap()),
            content_bytes,
            aliases,
            event_aliases,
            message_aliases,
        };
        write(&dir.join("prepared.json"), &prepared);
        db.pool().close().await;
        entries.push(json!({"id":case.id,"input_sha256":prepared.input_sha256,
            "binding_id":prepared.binding_id,
            "database_sha256":digest(std::fs::read(dir.join("source.db")).unwrap()),
            "prepared_record_sha256":digest(std::fs::read(dir.join("prepared.json")).unwrap())}));
    }
    assert_eq!(corpus.cases.len(), CALL_LIMIT);
    let manifest = json!({"mode":"prepared","provider_calls":0,"protocol":{
        "corpus_sha256":digest(&corpus_bytes),"oracles_sha256":digest(&oracle_bytes)},"cases":entries});
    write(&root.path().join("manifest.json"), &manifest);
    let approved = digest(std::fs::read(root.path().join("manifest.json")).unwrap());
    let seeds = PairedSeeds::load(root.path().into(), &approved, &corpus_bytes, &oracle_bytes);
    let paired = tempfile::tempdir().unwrap();
    let case = &corpus.cases[0];
    let frozen_hash = digest(std::fs::read(root.path().join(&case.id).join("source.db")).unwrap());
    let (db, original) = seeds
        .copy_case(case, &paired.path().join("source.db"))
        .await;
    let mut revised_profile = profile;
    revised_profile.prompt_hash = Sha256::digest("revised scanner instructions").into();
    let batch = prepare(&db, &revised_profile, original.now_us).await;
    validate_prepared(
        case,
        &batch,
        &original.aliases,
        &original.event_aliases,
        &original.message_aliases,
    );
    assert_eq!(
        digest(serde_json::to_vec(batch.input()).unwrap()),
        original.input_sha256
    );
    assert_ne!(batch.binding_id(), original.binding_id);
    assert_eq!(
        frozen_hash,
        digest(std::fs::read(root.path().join(&case.id).join("source.db")).unwrap())
    );
    db.pool().close().await;
}

struct RecordingService {
    inner: Arc<dyn LlmService>,
    records: Mutex<Vec<Value>>,
}
#[async_trait::async_trait]
impl LlmService for RecordingService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let result = self.inner.complete(request).await;
        let record = match &result {
            Ok(response) => json!({"content":response.content,"end_turn":response.end_turn,
                "usage":response.usage,"usage_observation":response.usage_observation,
                "provider_replay_debug":format!("{:?}",response.provider_replay),
                "stream_telemetry_debug":format!("{:?}",response.stream_telemetry)}),
            Err(error) => {
                json!({"error_kind":format!("{:?}",error.kind),"error_detail":"provider detail withheld"})
            }
        };
        self.records.lock().unwrap().push(record);
        result
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
}

fn identity(service: &dyn LlmService, profile: &ScanProfile) -> Value {
    json!({"catalog_model":service.model_id(),"outbound_model":service.outbound_model_name(),"effort":"low",
        "connection":profile.connection,"endpoint_sha256":hex(model::endpoint_hash(service)),
        "prompt_sha256":digest(prompt::INSTRUCTIONS),"instruction_bytes":profile.instruction_bytes,
        "accepted_output_tokens":model::MAX_OUTPUT_TOKENS,"accepted_output_utf8_bytes":model::MAX_OUTPUT_BYTES,
        "requested_output_tokens":model::MAX_OUTPUT_TOKENS,"codex_generation_billing_cap":false,
        "timeout_seconds":model::TIMEOUT.as_secs(),"per_request_logical_content_bytes":model::MAX_CONTENT_BYTES,
        "max_calls":CALL_LIMIT,"max_aggregate_logical_content_bytes":CONTENT_LIMIT,
        "retries":0,"tool_count":0,
        "result_dto_source_sha256":digest(include_bytes!("../../../phoenix-db/src/coordinator_scanner.rs"))})
}
fn executable_hash() -> String {
    std::env::var("PHOENIX_SCANNER_QUALITY_BINARY_SHA256")
        .expect("retained binary wrapper must identify exact executable")
}

#[tokio::test]
#[ignore = "opt-in real scanner host/provider qualification; explicit prepare/live mode and retained binary required"]
#[allow(clippy::too_many_lines)]
async fn coordinator_scanner_quality() {
    let mode = std::env::var("PHOENIX_SCANNER_QUALITY_MODE").expect("prepare|live");
    assert!(matches!(mode.as_str(), "prepare" | "live"));
    let output = PathBuf::from(
        std::env::var("PHOENIX_SCANNER_QUALITY_OUTPUT").expect("fresh output directory"),
    );
    assert!(output.is_absolute());
    std::fs::create_dir(&output).expect("never overwrite evidence");
    let corpus_bytes = std::fs::read(root().join(CORPUS)).unwrap();
    let oracle_bytes = std::fs::read(root().join(ORACLES)).unwrap();
    let corpus: Corpus = serde_json::from_slice(&corpus_bytes).unwrap();
    assert_eq!(corpus.cases.len(), CALL_LIMIT);
    let registry = ModelRegistry::new(&LlmConfig::from_env(Arc::new(
        PhoenixRuntimeEnvironment::detect(),
    )));
    let service = model::eligible_service(&registry)
        .expect("strict actual Luna/Low route unavailable; no fallback");
    let connection = if service.uses_codex_bridge() {
        ScanConnection::Codex
    } else {
        ScanConnection::OpenAiResponses
    };
    let profile = super::profile(service.as_ref(), connection);
    let identity = identity(service.as_ref(), &profile);
    let static_manifest = json!({"identity":identity,"corpus_sha256":digest(&corpus_bytes),
        "oracles_sha256":digest(&oracle_bytes),"test_binary_sha256":executable_hash(),
        "compiled_source_commit":env!("PHOENIX_GIT_SHA"),
        "workspace_commit":String::from_utf8(std::process::Command::new("git").args(["rev-parse","HEAD"]).current_dir(root()).output().unwrap().stdout).unwrap().trim(),
        "oracle_shown_to_model":false,"inference_source":"actual Database::prepare_coordinator_scan ScanInput only",
        "quality_gates":serde_json::from_slice::<Value>(&corpus_bytes).unwrap()["gates"],
        "denominators":serde_json::from_slice::<Value>(&corpus_bytes).unwrap()["denominators"]});
    write(&output.join("protocol.json"), &static_manifest);
    std::fs::write(output.join("corpus-reference.json"), &corpus_bytes).unwrap();
    std::fs::write(output.join("oracle-reference.md"), &oracle_bytes).unwrap();
    std::fs::write(output.join("system.txt"), prompt::INSTRUCTIONS).unwrap();
    let mut total_bytes = 0;
    let mut cases = Vec::new();
    if mode == "prepare" {
        let seeds = std::env::var("PHOENIX_SCANNER_QUALITY_SEED_PREPARED")
            .ok()
            .map(|path| {
                let approved = std::env::var("PHOENIX_SCANNER_QUALITY_SEED_SHA256")
                    .expect("explicit approved original seed manifest SHA256 required");
                PairedSeeds::load(path.into(), &approved, &corpus_bytes, &oracle_bytes)
            });
        for case in &corpus.cases {
            let dir = output.join(&case.id);
            std::fs::create_dir(&dir).unwrap();
            let (db, original) = if let Some(seeds) = &seeds {
                let (db, original) = seeds.copy_case(case, &dir.join("source.db")).await;
                (db, Some(original))
            } else {
                (open(&dir.join("source.db")).await, None)
            };
            let (source, aliases, event_aliases, message_aliases) = if let Some(old) = &original {
                (
                    old.source.clone(),
                    old.aliases.clone(),
                    old.event_aliases.clone(),
                    old.message_aliases.clone(),
                )
            } else {
                fixture(&db, case).await
            };
            let now_us = original.as_ref().map_or_else(
                || time(&case.input.scan_context.captured_at),
                |old| old.now_us,
            );
            let batch = prepare(&db, &profile, now_us).await;
            validate_prepared(case, &batch, &aliases, &event_aliases, &message_aliases);
            let request = model::BoundedRequest::new(prompt::INSTRUCTIONS, batch.input()).unwrap();
            let input_bytes = serde_json::to_vec(batch.input()).unwrap();
            if let Some(original) = &original {
                assert_eq!(
                    serde_json::to_value(batch.input()).unwrap(),
                    serde_json::to_value(seeds.as_ref().unwrap().expected_input(original)).unwrap(),
                    "only the exact retirement-relative candidate projection may change"
                );
            }
            total_bytes += request.content_bytes();
            assert!(total_bytes <= CONTENT_LIMIT);
            let prepared = PreparedCase {
                id: case.id.clone(),
                source,
                now_us,
                binding_id: batch.binding_id().into(),
                input_sha256: digest(&input_bytes),
                content_bytes: request.content_bytes(),
                aliases,
                event_aliases,
                message_aliases,
            };
            write(&dir.join("input.json"), batch.input());
            write(
                &dir.join("before.json"),
                &snapshot(&db, &prepared.source).await,
            );
            write(&dir.join("prepared.json"), &prepared);
            db.pool().close().await;
            cases.push(json!({"id":case.id,"input_sha256":prepared.input_sha256,"content_bytes":prepared.content_bytes,
                "binding_id":prepared.binding_id,"database_sha256":digest(std::fs::read(dir.join("source.db")).unwrap()),
                "prepared_record_sha256":digest(std::fs::read(dir.join("prepared.json")).unwrap())}));
        }
        write(
            &output.join("manifest.json"),
            &json!({"mode":"prepared","provider_calls":0,"protocol":static_manifest,
            "seed_manifest_sha256":seeds.as_ref().map(|seed|&seed.manifest_sha256),
            "total_logical_content_bytes":total_bytes,"cases":cases}),
        );
    } else {
        let prepared_root = PathBuf::from(
            std::env::var("PHOENIX_SCANNER_QUALITY_PREPARED").expect("approved prepared directory"),
        );
        assert!(prepared_root.is_absolute());
        let manifest_bytes = std::fs::read(prepared_root.join("manifest.json")).unwrap();
        let approved_sha256 = std::env::var("PHOENIX_SCANNER_QUALITY_APPROVED_SHA256")
            .expect("explicit independently approved manifest SHA256 required");
        assert_eq!(
            digest(&manifest_bytes),
            approved_sha256,
            "exact prepared manifest must have explicit approval before any reservation/provider"
        );
        let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(
            manifest["protocol"], static_manifest,
            "frozen route/corpus/oracles/binary/protocol must match before any call"
        );
        assert!(manifest["total_logical_content_bytes"].as_u64().unwrap() <= CONTENT_LIMIT as u64);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(prepared_root.join("live-started"))
            .expect("one invocation per prepared corpus; no automatic retries");
        let recorder = RecordingService {
            inner: service,
            records: Mutex::new(Vec::new()),
        };
        let mut completed = 0;
        let mut failures = 0;
        for case in &corpus.cases {
            assert!(completed < CALL_LIMIT);
            let dir = output.join(&case.id);
            std::fs::create_dir(&dir).unwrap();
            let prepared_dir = prepared_root.join(&case.id);
            let prepared_bytes = std::fs::read(prepared_dir.join("prepared.json")).unwrap();
            let prepared: PreparedCase = serde_json::from_slice(&prepared_bytes).unwrap();
            let frozen_db_path = prepared_dir.join("source.db");
            let entry = manifest["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == case.id)
                .unwrap();
            assert_eq!(
                entry["prepared_record_sha256"],
                digest(&prepared_bytes),
                "approved source/alias mapping changed"
            );
            assert_eq!(
                entry["database_sha256"],
                digest(std::fs::read(&frozen_db_path).unwrap()),
                "retained DB altered before live"
            );
            let db_path = dir.join("source.db");
            std::fs::copy(&frozen_db_path, &db_path).unwrap();
            assert_eq!(
                entry["database_sha256"],
                digest(std::fs::read(&db_path).unwrap())
            );
            let db = open(&db_path).await;
            let batch = prepare(&db, &profile, prepared.now_us).await;
            assert_eq!(batch.binding_id(), prepared.binding_id);
            assert_eq!(
                digest(serde_json::to_vec(batch.input()).unwrap()),
                prepared.input_sha256
            );
            let request = model::BoundedRequest::new(prompt::INSTRUCTIONS, batch.input()).unwrap();
            assert_eq!(request.content_bytes(), prepared.content_bytes);
            total_bytes += request.content_bytes();
            assert!(total_bytes <= CONTENT_LIMIT);
            let before = snapshot(&db, &prepared.source).await;
            let reservation = db
                .reserve_coordinator_scan(batch, request.content_bytes(), prepared.now_us)
                .await
                .established()
                .expect("classified reservation")
                .unwrap();
            let ScanReservation::Reserved(attempt) = reservation else {
                panic!("frozen reservation not available: {reservation:?}")
            };
            let (result, metrics) = super::infer(&recorder, &request).await;
            completed += 1;
            let responses = std::mem::take(&mut *recorder.records.lock().unwrap());
            let raw = raw_proposals(&responses, attempt.input());
            let proposal = match &result {
                Ok(result) => {
                    json!({"closed_result":result,"source_audit":proposal_audit(result,attempt.input())})
                }
                Err(failure) => {
                    failures += 1;
                    json!({"adapter_failure":format!("{failure:?}")})
                }
            };
            write(
                &dir.join("proposal-before-apply.json"),
                &json!({"case":case.id,"proposal":proposal,
                "provider_responses":responses,"raw_proposals":raw,"elapsed_ms":metrics.elapsed_ms}),
            );
            let application = match result {
                Ok(result) => match db
                    .apply_coordinator_scan(&attempt, &result, metrics.clone(), prepared.now_us + 1)
                    .await
                    .established()
                    .expect("unclassified application; stop without cleanup")
                {
                    Ok(outcome) => match outcome {
                        ScanApplyOutcome::Applied {
                            created_item_ids,
                            changed_item_count,
                            registry_revision,
                        } => {
                            json!({"outcome":"applied","created_item_ids":created_item_ids,"changed_item_count":changed_item_count,"registry_revision":registry_revision})
                        }
                        ScanApplyOutcome::Rejected(reason) => {
                            json!({"outcome":"rejected","reason":reason})
                        }
                        ScanApplyOutcome::Conflict => json!({"outcome":"conflict"}),
                        ScanApplyOutcome::Replayed => json!({"outcome":"replayed"}),
                    },
                    Err(error) => {
                        db.fail_coordinator_scan(
                            &attempt,
                            phoenix_db::coordinator_scanner::ScanFailure::Storage(
                                error.to_string(),
                            ),
                            metrics.clone(),
                            prepared.now_us + 1,
                        )
                        .await
                        .established()
                        .expect("classified storage failure")
                        .unwrap();
                        json!({"outcome":"storage_failure","error":error.to_string()})
                    }
                },
                Err(failure) => {
                    db.fail_coordinator_scan(
                        &attempt,
                        failure,
                        metrics.clone(),
                        prepared.now_us + 1,
                    )
                    .await
                    .established()
                    .expect("classified failure")
                    .unwrap();
                    json!({"outcome":"inference_failed"})
                }
            };
            let after = snapshot(&db, &prepared.source).await;
            assert_eq!(before["messages"], after["messages"]);
            assert_eq!(
                before["foreground_observations"],
                after["foreground_observations"]
            );
            let usage=metrics.usage.map(|usage|json!({"input_tokens":usage.input_tokens,"cache_read_tokens":usage.cache_read_tokens,"cache_creation_tokens":usage.cache_creation_tokens,"output_tokens":usage.output_tokens}));
            write(
                &dir.join("record.json"),
                &json!({"case":case.id,"prepared":prepared,"provider_responses":responses,"raw_proposals":raw,
                "proposal":proposal,"application":application,"elapsed_ms":metrics.elapsed_ms,"usage":usage,"before":before,"after":after}),
            );
            db.pool().close().await;
            println!(
                "scanner quality {completed}/37 {} recorded; independent semantic review pending",
                case.id
            );
            cases.push(json!({"id":case.id,"record_sha256":digest(std::fs::read(dir.join("record.json")).unwrap())}));
        }
        write(
            &output.join("manifest.json"),
            &json!({"mode":"recorded_not_semantically_qualified","protocol":static_manifest,
            "prepared_manifest_sha256":approved_sha256,"provider_calls":completed,
            "adapter_failures":failures,"total_logical_content_bytes":total_bytes,"cases":cases}),
        );
    }
}
