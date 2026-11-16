use super::*;
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
use phoenix_core::llm_language::LlmLanguage;
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
    assert_one_operation(&f, &receipt, 0).await;
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
