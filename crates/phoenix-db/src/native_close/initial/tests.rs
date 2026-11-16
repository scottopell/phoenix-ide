use super::*;
use crate::workflow::AcceptAuthoritativeTurn;
use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
use phoenix_core::llm_language::LlmLanguage;
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};
use serde_json::json;

struct Fixture {
    db: Database,
    coordinator: String,
    source: SourceToolCall,
    target: ProductConversationId,
}

impl Fixture {
    async fn new() -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let target = Self::create_target(&db, "first-target").await;
        let source = SourceToolCall {
            message_id: "live-close-message".into(),
            tool_use_id: "live-close-call".into(),
        };
        let fixture = Self {
            db,
            coordinator: coordinator.id,
            source,
            target,
        };
        fixture.executing(&fixture.source, &fixture.target).await;
        fixture
    }

    async fn create_target(db: &Database, id: &str) -> ProductConversationId {
        db.create_conversation(id, id, "/nonexistent", true, None, None)
            .await
            .unwrap()
            .product_conversation_id
    }

    async fn executing(&self, source: &SourceToolCall, target: &ProductConversationId) {
        let input = json!({
            "action": "initial",
            "product_conversation_id": target.as_str()
        });
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                source.message_id.clone(),
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id.clone(),
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        };
        self.db
            .update_conversation_state(&self.coordinator, &state)
            .await
            .unwrap();
    }

    fn command(
        &self,
        source: SourceToolCall,
        target: &ProductConversationId,
    ) -> NativeCloseCommand {
        NativeCloseCommand::new(
            self.coordinator.clone(),
            source,
            NativeCloseIntent::Initial {
                target: target.clone(),
            },
        )
        .unwrap()
    }

    async fn apply(&self, command: &NativeCloseCommand) -> DbResult<NativeCloseInitialResult> {
        established(&self.db, command).await
    }
}

async fn established(
    db: &Database,
    command: &NativeCloseCommand,
) -> DbResult<NativeCloseInitialResult> {
    match db.apply_native_close_initial(command).await {
        LocalAuthorityResult::DurableFactEstablished(result) => result,
        LocalAuthorityResult::DurableFactUnclassified => {
            panic!("initial Close commit or rollback was not classified")
        }
    }
}

async fn count(db: &Database, table: &str) -> i64 {
    let sql = match table {
        "close_obligations" => "SELECT COUNT(*) FROM close_obligations",
        "close_runs" => "SELECT COUNT(*) FROM close_runs",
        "close_completion_retry_allowances" => {
            "SELECT COUNT(*) FROM close_completion_retry_allowances"
        }
        "native_close_initial_receipts" => "SELECT COUNT(*) FROM native_close_initial_receipts",
        "close_attempt_members" => "SELECT COUNT(*) FROM close_attempt_members",
        "close_attempt_participants" => "SELECT COUNT(*) FROM close_attempt_participants",
        _ => panic!("unexpected table"),
    };
    sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap()
}

async fn assert_one_operation(f: &Fixture, receipt: &NativeCloseReceipt, suppressed: i64) {
    assert_eq!(receipt.admitted.target, f.target);
    assert_eq!(receipt.admitted.run_ordinal, CloseRunOrdinal::INITIAL);
    for table in [
        "close_obligations",
        "close_runs",
        "close_completion_retry_allowances",
        "native_close_initial_receipts",
        "close_attempt_members",
        "close_attempt_participants",
    ] {
        assert_eq!(count(&f.db, table).await, 1, "{table}");
    }
    let marker: (i64, i64, i64) = sqlx::query_as(
        "SELECT o.native_managed, a.consumed, a.suppressed
         FROM close_obligations o JOIN close_completion_retry_allowances a
           ON a.attempt_id = o.attempt_id WHERE o.attempt_id = ?1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(marker, (1, 0, suppressed));
    let persisted: (String, String, i64) = sqlx::query_as(
        "SELECT attempt_id, product_conversation_id, run_ordinal
         FROM native_close_initial_receipts WHERE request_id = ?1",
    )
    .bind(receipt.request_id.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(persisted.0, receipt.admitted.attempt_id.as_str());
    assert_eq!(persisted.1, receipt.admitted.target.as_str());
    assert_eq!(persisted.2, 1);
}

async fn source_turn(db: &Database, conversation: &str, key: &str) -> u64 {
    let conversation: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(conversation).fetch_one(db.pool()).await.unwrap();
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: InputOrigin::UserApi,
            text: "source input".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "source input".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let result = db
        .workflow_repository()
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(key).unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(conversation),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .established()
        .expect("acceptance durability is established")
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = result.outcome else {
        panic!("turn was not created")
    };
    turn_id.0
}

async fn assert_turn_unsettled(db: &Database, turn_id: u64) {
    let authority: (i64, bool, Option<String>) = sqlx::query_as(
        "SELECT generation, owns_conversation, terminal_kind FROM durable_turns WHERE turn_id = ?1",
    )
    .bind(i64::try_from(turn_id).unwrap())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(authority, (0, true, None));
}

async fn assert_no_dispatch_effects(db: &Database, receipt: &NativeCloseReceipt) {
    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlements WHERE attempt_id = ?1 AND settled_at IS NOT NULL",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(queued, 0, "preparation must not dispatch work");
}

async fn assert_no_settlement_effects(db: &Database, receipt: &NativeCloseReceipt) {
    let attempt = receipt.admitted.attempt_id.as_str();
    for (table, sql) in [
        ("captures", "SELECT COUNT(*) FROM close_attempt_direct_turn_settlement_captures WHERE attempt_id = ?1"),
        ("settlements", "SELECT COUNT(*) FROM close_attempt_direct_turn_settlements WHERE attempt_id = ?1"),
    ] {
        let count: i64 = sqlx::query_scalar(sql).bind(attempt).fetch_one(db.pool()).await.unwrap();
        assert_eq!(count, 0, "unexpected {table}");
    }
}

async fn prepared_initial(
    f: &Fixture,
) -> (NativeCloseCommand, NativeCloseReceipt, ConvState) {
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    (command, receipt, live_source)
}

async fn initial_phase(f: &Fixture, receipt: &NativeCloseReceipt) -> String {
    sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
        .bind(receipt.admitted.attempt_id.as_str())
        .fetch_one(f.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn initial_settlement_advances_exact_quiescent_run_without_dispatch() {
    let f = Fixture::new().await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let settled = match f.db.settle_native_close_initial(&command, &receipt, &live_source).await {
        LocalAuthorityResult::DurableFactEstablished(result) => result.unwrap(),
        LocalAuthorityResult::DurableFactUnclassified => panic!("settlement outcome unclassified"),
    };
    assert_eq!(settled.run, receipt.admitted);
    assert_eq!(settled.phase, phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection);
    assert_eq!(initial_phase(&f, &receipt).await, "awaiting_retirement_inspection");
    assert_one_operation(&f, &receipt, 0).await;
    assert_no_dispatch_effects(&f.db, &receipt).await;
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
}

#[tokio::test]
async fn initial_settlement_requires_exact_released_turn_generation_not_latest_idle() {
    let f = Fixture::new().await;
    let turn = source_turn(&f.db, f.target.as_str(), "settlement-owning-turn").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");
    assert_turn_unsettled(&f.db, turn).await;
    // Model a durable release with the wrong generation; it cannot discharge the captured receipt.
    sqlx::query("UPDATE durable_turns SET generation = 2, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = ?1")
        .bind(i64::try_from(turn).unwrap()).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    sqlx::query("UPDATE durable_turns SET generation = 1 WHERE turn_id = ?1")
        .bind(i64::try_from(turn).unwrap()).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "awaiting_retirement_inspection");
    let unsettled = f.db.list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await.unwrap();
    assert!(unsettled.is_empty());
    assert_no_dispatch_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_settlement_rejects_mismatched_receipt_and_live_or_durable_source() {
    let f = Fixture::new().await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let mut wrong_request = receipt.clone();
    wrong_request.request_id = uuid::Uuid::new_v4();
    let mut wrong_run = receipt.clone();
    wrong_run.admitted.attempt_id = CloseAttemptId::parse("unrelated-attempt").unwrap();
    let mut wrong_target = receipt.clone();
    wrong_target.admitted.target = Fixture::create_target(&f.db, "settlement-other-target").await;
    for wrong in [&wrong_request, &wrong_run, &wrong_target] {
        assert!(matches!(
            f.db.settle_native_close_initial(&command, wrong, &live_source).await,
            LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
        ));
    }
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &ConvState::Idle).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");
    assert_no_dispatch_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_settlement_refuses_unprepared_or_cancelled_initial_run() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "awaiting_blocker_resolution");
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    f.db.cancel_close_before_retirement(receipt.admitted.attempt_id.as_str()).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "cancel_requested_during_settlement");
    assert_no_dispatch_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_settlement_rolls_back_reconciled_receipts_when_phase_write_fails() {
    let f = Fixture::new().await;
    let turn = source_turn(&f.db, f.target.as_str(), "settlement-rollback-turn").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    sqlx::query("UPDATE durable_turns SET generation = 1, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = ?1")
        .bind(i64::try_from(turn).unwrap()).execute(f.db.pool()).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_initial_settlement_phase BEFORE UPDATE OF phase ON close_obligations
         WHEN NEW.phase = 'awaiting_retirement_inspection'
         BEGIN SELECT RAISE(ABORT, 'injected phase failure after receipt reconciliation'); END",
    ).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");
    let unsettled = f.db.list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await.unwrap();
    assert_eq!(unsettled.len(), 1);
    assert_eq!(unsettled[0].turn_id, turn);
    assert_no_dispatch_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_execution_prepares_idle_run_once_without_settlement_effects() {
    let f = Fixture::new().await;
    let turn_id = source_turn(&f.db, f.target.as_str(), "initial-active-turn").await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let prepared = match f
        .db
        .prepare_native_close_initial_execution(&command, &receipt, &live_source)
        .await
    {
        LocalAuthorityResult::DurableFactEstablished(result) => result.unwrap(),
        LocalAuthorityResult::DurableFactUnclassified => panic!("preparation outcome unclassified"),
    };
    assert_eq!(prepared.run, receipt.admitted);
    assert_eq!(
        prepared.phase,
        phoenix_core::domain::close::ClosePhase::SettlingActiveWork
    );
    assert_one_operation(&f, &receipt, 0).await;
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(phase, "settling_active_work");
    assert_eq!(count(&f.db, "close_runs").await, 1);
    assert_eq!(count(&f.db, "close_completion_retry_allowances").await, 1);
    let captures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlement_captures WHERE attempt_id = ?1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(captures, 1);
    let targets = f
        .db
        .list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].turn_id, turn_id);
    assert_eq!(targets[0].expected_generation, 0);
    assert_turn_unsettled(&f.db, turn_id).await;
    assert_no_dispatch_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_execution_preserves_captured_busy_decision_without_confirming_work() {
    let f = Fixture::new().await;
    let busy_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    )
    .bind(f.target.as_str())
    .fetch_one(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&latest, &busy_state)
        .await
        .unwrap();
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    f.db.update_conversation_state(&latest, &ConvState::Idle)
        .await
        .unwrap();
    let prepared = match f
        .db
        .prepare_native_close_initial_execution(&command, &receipt, &live_source)
        .await
    {
        LocalAuthorityResult::DurableFactEstablished(result) => result.unwrap(),
        LocalAuthorityResult::DurableFactUnclassified => panic!("preparation outcome unclassified"),
    };
    assert_eq!(prepared.run, receipt.admitted);
    assert_eq!(
        prepared.phase,
        phoenix_core::domain::close::ClosePhase::AwaitingStopWorkConfirmation
    );
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(phase, "awaiting_stop_work_confirmation");
    assert_eq!(count(&f.db, "close_runs").await, 1);
    assert_eq!(count(&f.db, "native_close_initial_receipts").await, 1);
    assert_no_settlement_effects(&f.db, &receipt).await;
    assert_no_dispatch_effects(&f.db, &receipt).await;
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let repeated_phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(repeated_phase, "awaiting_stop_work_confirmation");
}

#[tokio::test]
async fn preparation_requires_the_exact_retained_receipt_and_target() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let mut wrong_target = receipt.clone();
    wrong_target.admitted.target = Fixture::create_target(&f.db, "other-target").await;
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &wrong_target, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let mut wrong_request = receipt.clone();
    wrong_request.request_id = uuid::Uuid::new_v4();
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &wrong_request, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let mut unrelated = receipt.clone();
    unrelated.admitted.attempt_id = CloseAttemptId::parse("unrelated-attempt").unwrap();
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &unrelated, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(phase, "awaiting_blocker_resolution");
    let prepared =
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await;
    assert!(matches!(
        prepared,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
}

#[tokio::test]
async fn preparation_rejects_stale_durable_source_with_held_snapshot() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let held_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &ConvState::Idle)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &held_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(count(&f.db, "close_obligations").await, 1);
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(phase, "awaiting_blocker_resolution");
    assert_no_settlement_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn live_coordinator_exact_source_admits_and_replays_without_refilling_allowance() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation")
    };
    assert_eq!(receipt.request_id, command.request_id());
    assert_one_operation(&f, &receipt, 0).await;
    assert_eq!(
        f.apply(&command).await.unwrap(),
        NativeCloseInitialResult::Replay(receipt.clone())
    );
    assert_one_operation(&f, &receipt, 0).await;
}

#[tokio::test]
async fn exact_receipt_replay_keeps_run_one_after_operation_progress() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation")
    };

    f.db.begin_close_idle_settlement(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();

    assert_eq!(
        f.apply(&command).await.unwrap(),
        NativeCloseInitialResult::Replay(receipt.clone())
    );
    assert_one_operation(&f, &receipt, 0).await;
}

#[tokio::test]
async fn same_source_changed_target_conflicts_instead_of_starting_second_operation() {
    let f = Fixture::new().await;
    let original = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&original).await.unwrap() else {
        panic!("expected a new operation")
    };
    let other = Fixture::create_target(&f.db, "second-target").await;
    f.executing(&f.source, &other).await;
    let changed = f.command(f.source.clone(), &other);
    assert_eq!(original.request_id(), changed.request_id());
    assert_ne!(original.fingerprint(), changed.fingerprint());
    assert!(matches!(
        f.apply(&changed).await,
        Err(DbError::CloseFoundationConflict(message)) if message.contains("request identity conflict")
    ));
    assert_one_operation(&f, &receipt, 1).await;
}

#[tokio::test]
async fn idle_or_replaced_coordinator_refuses_even_exact_historical_replay() {
    for replaced in [false, true] {
        let f = Fixture::new().await;
        let command = f.command(f.source.clone(), &f.target);
        let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap()
        else {
            panic!("expected a new operation")
        };
        if replaced {
            sqlx::query("UPDATE conversations SET coordinator_head = 0 WHERE id = ?1")
                .bind(&f.coordinator)
                .execute(f.db.pool())
                .await
                .unwrap();
            assert!(matches!(
                f.apply(&command).await,
                Err(DbError::CurrentCoordinatorRequired)
            ));
        } else {
            f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
                .await
                .unwrap();
            assert!(matches!(
                f.apply(&command).await,
                Err(DbError::CloseFoundationPrecondition(_))
            ));
        }
        assert_one_operation(&f, &receipt, i64::from(!replaced)).await;
    }
}

#[tokio::test]
async fn new_source_tuple_must_reference_retained_operation_without_new_allowance() {
    let f = Fixture::new().await;
    let original = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&original).await.unwrap() else {
        panic!("expected a new operation")
    };
    let second_source = SourceToolCall {
        message_id: "next-close-message".into(),
        tool_use_id: "next-close-call".into(),
    };
    f.executing(&second_source, &f.target).await;
    let second = f.command(second_source, &f.target);
    assert_ne!(second.request_id(), original.request_id());
    assert_eq!(
        f.apply(&second).await.unwrap(),
        NativeCloseInitialResult::ExistingOperationNeedsExactReference(receipt.admitted.clone())
    );
    assert_one_operation(&f, &receipt, 1).await;
}

#[tokio::test]
async fn failed_receipt_insert_rolls_back_capture_run_allowance_marker_and_topology() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    sqlx::query(
        "CREATE TRIGGER fail_initial_receipt BEFORE INSERT ON native_close_initial_receipts
         BEGIN SELECT RAISE(ABORT, 'injected receipt failure after close capture'); END",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    assert!(f.apply(&command).await.is_err());
    for table in [
        "close_obligations",
        "close_runs",
        "close_completion_retry_allowances",
        "native_close_initial_receipts",
        "close_attempt_members",
        "close_attempt_participants",
    ] {
        assert_eq!(count(&f.db, table).await, 0, "{table}");
    }
    let archived: i64 =
        sqlx::query_scalar("SELECT archived FROM conversations WHERE id = 'first-target'")
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(archived, 0);
}

#[tokio::test]
async fn idle_preparation_rolls_back_phase_when_late_capture_write_fails() {
    let f = Fixture::new().await;
    let turn_id = source_turn(&f.db, f.target.as_str(), "rollback-active-turn").await;
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    sqlx::query(
        "CREATE TRIGGER fail_initial_settlement_capture BEFORE INSERT ON close_attempt_direct_turn_settlements
         BEGIN SELECT RAISE(ABORT, 'injected capture failure'); END",
    ).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(phase, "awaiting_blocker_resolution");
    assert_no_settlement_effects(&f.db, &receipt).await;
    assert_turn_unsettled(&f.db, turn_id).await;
}

#[tokio::test]
async fn concurrent_identical_commands_commit_once_and_return_one_replay() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    let (first, second) = tokio::join!(f.apply(&command), f.apply(&command));
    let first = first.unwrap();
    let second = second.unwrap();
    let receipt = match (&first, &second) {
        (
            NativeCloseInitialResult::NewlyAdmitted(receipt),
            NativeCloseInitialResult::Replay(replay),
        )
        | (
            NativeCloseInitialResult::Replay(replay),
            NativeCloseInitialResult::NewlyAdmitted(receipt),
        ) => {
            assert_eq!(receipt, replay);
            receipt
        }
        _ => panic!("expected exactly one admission and one replay: {first:?}, {second:?}"),
    };
    assert_one_operation(&f, receipt, 0).await;
}

#[tokio::test]
async fn failed_topology_capture_rolls_back_all_initial_admission_rows() {
    let f = Fixture::new().await;
    let command = f.command(f.source.clone(), &f.target);
    sqlx::query(
        "CREATE TRIGGER fail_native_topology_seal BEFORE UPDATE OF topology_sealed ON close_obligations
         WHEN NEW.topology_sealed = 1
         BEGIN SELECT RAISE(ABORT, 'injected capture failure'); END",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    assert!(f.apply(&command).await.is_err());
    for table in [
        "close_obligations",
        "close_runs",
        "close_completion_retry_allowances",
        "native_close_initial_receipts",
        "close_attempt_members",
        "close_attempt_participants",
    ] {
        assert_eq!(count(&f.db, table).await, 0, "{table}");
    }
}
