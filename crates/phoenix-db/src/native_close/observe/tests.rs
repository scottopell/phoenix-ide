use super::*;
use phoenix_core::domain::close::{
    CloseAttemptId, ClosePhase, CloseRunOrdinal, TranscriptConversationId,
};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
use phoenix_core::llm_language::LlmLanguage;

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
        Self {
            db,
            coordinator: coordinator.id,
            source,
            target,
        }
    }

    async fn create_target(db: &Database, id: &str) -> ProductConversationId {
        db.create_conversation(id, id, "/nonexistent", true, None, None)
            .await
            .unwrap()
            .product_conversation_id
    }

    async fn executing(&self, command: &NativeCloseCommand) {
        let source = command.source();
        let input = command.intent().source_input();
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

    fn command(&self, referenced: &NativeCloseRunRef) -> NativeCloseCommand {
        NativeCloseCommand::new(
            self.coordinator.clone(),
            self.source.clone(),
            NativeCloseIntent::Observe {
                target: referenced.target.clone(),
                attempt_id: referenced.attempt_id.clone(),
                run_ordinal: referenced.run_ordinal,
            },
        )
        .unwrap()
    }

    async fn begin(&self) -> NativeCloseRunRef {
        let obligation = self
            .db
            .begin_close_foundation(
                &self.target,
                &TranscriptConversationId::parse("first-target").unwrap(),
                "retained-attempt",
            )
            .await
            .unwrap();
        NativeCloseRunRef {
            target: self.target.clone(),
            attempt_id: obligation.attempt_id().clone(),
            run_ordinal: CloseRunOrdinal::INITIAL,
        }
    }

    async fn apply(&self, command: &NativeCloseCommand) -> DbResult<NativeCloseObserveResult> {
        match self.db.apply_native_close_observe(command).await {
            LocalAuthorityResult::DurableFactEstablished(result) => result,
            LocalAuthorityResult::DurableFactUnclassified => {
                panic!("unclassified observation transaction")
            }
        }
    }
}

#[tokio::test]
async fn observation_does_not_transfer_generic_settlement_ownership() {
    for adopt in [false, true] {
        let f = Fixture::new().await;
        let referenced = f.begin().await;
        let attempt = referenced.attempt_id.as_str();
        f.db.begin_close_idle_settlement(attempt).await.unwrap();
        let mut pending =
            f.db.list_pending_generic_close_settlements(None)
                .await
                .unwrap();
        assert_eq!(pending.len(), 1);
        let admitted = pending.pop().unwrap();
        assert_eq!(admitted.obligation().attempt_id(), &referenced.attempt_id);

        if adopt {
            let command = f.command(&referenced);
            f.executing(&command).await;
            assert!(matches!(
                f.apply(&command).await.unwrap(),
                NativeCloseObserveResult::Observed(_)
            ));
        }
        // Observation adoption may happen after selection. It must not revoke
        // the existing generic driver or turn its receipt into initial authority.
        assert!(f
            .db
            .validate_generic_close_settlement(&admitted)
            .await
            .unwrap());
        assert_eq!(
            f.db.list_pending_generic_close_settlements(None)
                .await
                .unwrap()
                .len(),
            1
        );
        let advanced =
            f.db.advance_close_settlement_when_quiescent(attempt)
                .await
                .unwrap();
        assert_eq!(advanced.phase(), ClosePhase::AwaitingRetirementInspection);
        assert!(!f
            .db
            .validate_generic_close_settlement(&admitted)
            .await
            .unwrap());
    }
}

#[tokio::test]
async fn exact_observation_duplicate_replays_one_immutable_receipt_without_admission() {
    let f = Fixture::new().await;
    let referenced = f.begin().await;
    let command = f.command(&referenced);
    f.executing(&command).await;
    let NativeCloseObserveResult::Observed(first) = f.apply(&command).await.unwrap() else {
        panic!("first observation must be new");
    };
    let NativeCloseObserveResult::Replay(replay) = f.apply(&command).await.unwrap() else {
        panic!("duplicate must replay");
    };
    assert_eq!(first.receipt, replay.receipt);
    assert_eq!(replay.receipt.referenced, referenced);
    assert_eq!(
        replay.current_referenced_run_status,
        CloseRunStatus::Running
    );
    assert_eq!(
        replay.current_operation.phase(),
        ClosePhase::AwaitingBlockerResolution
    );
    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM close_obligations),
                (SELECT COUNT(*) FROM close_runs),
                (SELECT COUNT(*) FROM native_close_initial_receipts WHERE intent = 'observe'),
                (SELECT COUNT(*) FROM close_completion_retry_allowances),
                (SELECT COUNT(*) FROM close_original_retirement_anchors),
                (SELECT COUNT(*) FROM close_run_retry_effects)",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(counts, (1, 1, 1, 1, 0, 0));
    for statement in [
        "UPDATE native_close_initial_receipts SET run_ordinal = 2",
        "UPDATE native_close_initial_receipts SET intent = 'initial'",
        "DELETE FROM native_close_initial_receipts",
        "UPDATE close_obligations SET native_managed = 0",
    ] {
        assert!(sqlx::query(statement).execute(f.db.pool()).await.is_err());
    }
}

#[tokio::test]
async fn wrong_target_missing_attempt_and_missing_run_never_fall_back_or_adopt() {
    let f = Fixture::new().await;
    let retained = f.begin().await;
    let other = Fixture::create_target(&f.db, "other-target").await;
    for referenced in [
        NativeCloseRunRef {
            target: other,
            ..retained.clone()
        },
        NativeCloseRunRef {
            attempt_id: CloseAttemptId::parse("missing-attempt").unwrap(),
            ..retained.clone()
        },
        NativeCloseRunRef {
            run_ordinal: CloseRunOrdinal::parse(2).unwrap(),
            ..retained.clone()
        },
    ] {
        let command = f.command(&referenced);
        f.executing(&command).await;
        assert!(matches!(
            f.apply(&command).await,
            Err(DbError::CloseFoundationPrecondition(_))
        ));
    }
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM native_close_initial_receipts),
                (SELECT native_managed FROM close_obligations),
                (SELECT COUNT(*) FROM close_runs)",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 1));
}

#[tokio::test]
async fn revoked_source_is_rejected_before_receipt_replay() {
    let f = Fixture::new().await;
    let referenced = f.begin().await;
    let command = f.command(&referenced);
    f.executing(&command).await;
    f.apply(&command).await.unwrap();
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    assert!(matches!(
        f.apply(&command).await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_close_initial_receipts")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn receipt_insert_failure_rolls_back_adoption_without_refilling_allowance() {
    let f = Fixture::new().await;
    let referenced = f.begin().await;
    sqlx::query("UPDATE close_completion_retry_allowances SET consumed = 1, suppressed = 1")
        .execute(f.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_observe_receipt BEFORE INSERT ON native_close_initial_receipts
         WHEN NEW.intent = 'observe' BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    let command = f.command(&referenced);
    f.executing(&command).await;
    assert!(f.apply(&command).await.is_err());
    let state: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT o.native_managed, a.consumed, a.suppressed,
                (SELECT COUNT(*) FROM native_close_initial_receipts)
         FROM close_obligations o JOIN close_completion_retry_allowances a ON a.attempt_id = o.attempt_id",
    ).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(state, (0, 1, 1, 0));
    sqlx::query("DROP TRIGGER fail_observe_receipt")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.apply(&command).await.unwrap();
    f.apply(&command).await.unwrap();
    let state: (i64, i64, i64) = sqlx::query_as(
        "SELECT o.native_managed, a.consumed, a.suppressed
         FROM close_obligations o JOIN close_completion_retry_allowances a ON a.attempt_id = o.attempt_id",
    ).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(state, (1, 1, 1));
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM close_runs),
                (SELECT COUNT(*) FROM close_original_retirement_anchors),
                (SELECT COUNT(*) FROM close_run_retry_effects)",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(counts, (1, 0, 0));
}

#[tokio::test]
async fn observing_legacy_operation_without_allowance_does_not_create_one() {
    let f = Fixture::new().await;
    let referenced = f.begin().await;
    // Removing the post-feature child represents an operation predating that child.
    sqlx::query("DROP TRIGGER close_completion_retry_allowances_no_direct_delete")
        .execute(f.db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM close_completion_retry_allowances")
        .execute(f.db.pool())
        .await
        .unwrap();
    let command = f.command(&referenced);
    f.executing(&command).await;
    f.apply(&command).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_completion_retry_allowances")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
}
