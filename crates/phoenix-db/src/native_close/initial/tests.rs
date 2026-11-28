use super::*;
use crate::workflow::AcceptAuthoritativeTurn;
use phoenix_core::domain::close::{ClosePhase, RetiredResourceKind, RetirementFailureReason};
use phoenix_core::domain::db_schema::InputOrigin;
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
    SubmittedDirectTurnIdentity,
};
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
use phoenix_core::llm_language::LlmLanguage;
use phoenix_core::work_scope::WorkScopeId;
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};
use sqlx::Connection;
use std::collections::BTreeSet;
use uuid::Uuid;

struct Fixture {
    db: Database,
    _temp_dir: Option<tempfile::TempDir>,
    db_path: Option<String>,
    coordinator: String,
    source: SourceToolCall,
    target: ProductConversationId,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_database(Database::open_in_memory().await.unwrap(), None).await
    }

    async fn new_file_backed() -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("initial-entry.sqlite");
        let path = path.to_string_lossy().into_owned();
        let db = Database::open(&path).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        Self::with_database(db, Some((temp_dir, path))).await
    }

    async fn with_database(
        db: Database,
        file_backing: Option<(tempfile::TempDir, String)>,
    ) -> Self {
        let (temp_dir, db_path) = match file_backing {
            Some((dir, path)) => (Some(dir), Some(path)),
            None => (None, None),
        };
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
            _temp_dir: temp_dir,
            db_path,
            coordinator: coordinator.id,
            source,
            target,
        };
        fixture.executing(&fixture.source, &fixture.target).await;
        fixture
    }

    async fn create_target(db: &Database, id: &str) -> ProductConversationId {
        let target = db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        target.product_conversation_id
    }

    async fn executing(&self, source: &SourceToolCall, target: &ProductConversationId) {
        let input = NativeCloseIntent::Initial {
            target: target.clone(),
        }
        .source_input();
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

async fn prepared_initial(f: &Fixture) -> (NativeCloseCommand, NativeCloseReceipt, ConvState) {
    let command = f.command(f.source.clone(), &f.target);
    let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
        panic!("expected a new operation");
    };
    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await,
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

async fn initial_inventory_request(
    f: &Fixture,
    receipt: &NativeCloseReceipt,
) -> CaptureCloseRetirementInventoryRequest {
    let attempt_id = &receipt.admitted.attempt_id;
    let snapshot =
        f.db.get_close_obligation(attempt_id.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
    let scopes =
        f.db.list_close_attempt_scopes(attempt_id.as_str())
            .await
            .unwrap()
            .into_iter()
            .map(|scope| {
                let worktree = match scope.captured_worktree {
                    Some(phoenix_core::domain::close::CapturedWorktreeIdentity::Resolved(
                        identity,
                    )) => Some(identity),
                    Some(phoenix_core::domain::close::CapturedWorktreeIdentity::Unresolved {
                        ..
                    }) => {
                        panic!("fixture scope identity must be resolved")
                    }
                    None => panic!("fixture target must have an allocated worktree"),
                };
                crate::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.scope,
                    inventory: phoenix_core::domain::close::CloseOwnedResourceInventory {
                        worktree,
                        work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(),
                        tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(),
                        browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                }
            })
            .collect();
    CaptureCloseRetirementInventoryRequest {
        attempt_id: attempt_id.clone(),
        snapshot,
        scopes,
    }
}

async fn assert_no_initial_inspection_artifacts(f: &Fixture, receipt: &NativeCloseReceipt) {
    for sql in [
        "SELECT COUNT(*) FROM close_retirement_inspections WHERE attempt_id = ?1",
        "SELECT COUNT(*) FROM close_retirement_inventories WHERE attempt_id = ?1",
        "SELECT COUNT(*) FROM close_expected_retirement_resources WHERE attempt_id = ?1",
        "SELECT COUNT(*) FROM close_original_retirement_anchors WHERE attempt_id = ?1",
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "unexpected inspection or inventory artifact: {sql}"
        );
    }
}

#[tokio::test]
async fn initial_effect_admission_rejects_retained_losses_even_in_anomalous_phase() {
    let f = Fixture::new().await;
    let scope = crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await.established().unwrap().unwrap();
    let inspection = ReplaceCloseInspectionRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
            scope,
            snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
                "loss-generation", "loss-fingerprint",
            ).unwrap(),
            losses: vec![phoenix_core::domain::close::CloseLossItem::UntrackedNonIgnoredPath(
                phoenix_core::domain::close::GitPathIdentity::from_bytes(b"retained-loss".to_vec()),
            )],
        }],
    };
    f.db.replace_native_close_initial_inspection(
        &command, &receipt, &live_source, inspection, None,
    ).await.established().unwrap().unwrap();
    assert_eq!(initial_phase(&f, &receipt).await, ClosePhase::AwaitingLossConfirmation.as_str());

    // Reproduce legacy/anomalous retained state while preserving the loss-confirmation phase guard.
    sqlx::query("UPDATE close_obligations SET phase = 'retirement_requested' WHERE attempt_id = ?1")
        .bind(receipt.admitted.attempt_id.as_str()).execute(f.db.pool()).await.unwrap();
    let retained_losses = f.db.list_close_retirement_losses(receipt.admitted.attempt_id.as_str())
        .await.unwrap();
    assert!(!retained_losses.is_empty());
    let before_captures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlement_captures WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source).await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, ClosePhase::RetirementRequested.as_str());
    let after_captures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlement_captures WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(after_captures, before_captures);
}

#[tokio::test]
async fn initial_loss_inspection_is_retained_and_terminalized_atomically_without_dispatch() {
    let f = Fixture::new().await;
    let scope = crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await.established().unwrap().unwrap();
    let request = ReplaceCloseInspectionRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
            scope,
            snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
                "loss-generation", "loss-fingerprint",
            ).unwrap(),
            losses: vec![
                phoenix_core::domain::close::CloseLossItem::UntrackedNonIgnoredPath(
                    phoenix_core::domain::close::GitPathIdentity::from_bytes(b"dirty-file".to_vec()),
                ),
                phoenix_core::domain::close::CloseLossItem::IgnoredPath(
                    phoenix_core::domain::close::GitPathIdentity::from_bytes(b"ignored-file".to_vec()),
                ),
            ],
        }],
    };
    let result = f.db.terminalize_native_close_initial_losses(
        &command, &receipt, &live_source, request, None, "unconfirmed dirty-file loss",
    ).await.established().unwrap().unwrap();
    assert_eq!(result.phase(), ClosePhase::Completed);
    assert_eq!(result.close_outcome(), Some(phoenix_core::domain::close::CloseCompletionOutcome::CloseIncomplete));
    assert_eq!(initial_phase(&f, &receipt).await, ClosePhase::Completed.as_str());
    assert_eq!(f.db.list_close_retirement_losses(receipt.admitted.attempt_id.as_str()).await.unwrap().len(), 2);
    let certainty: String = sqlx::query_scalar(
        "SELECT stop_certainty FROM close_cleanup_failures WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(certainty, "shutdown_uncertain");
    let dispatches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(dispatches, 0);
}

#[tokio::test]
async fn initial_loss_terminalization_guards_authority_and_rolls_back_late_failure() {
    let f = Fixture::new().await;
    let scope = crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await.established().unwrap().unwrap();
    let request = ReplaceCloseInspectionRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
            scope,
            snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
                "loss-rollback-generation", "loss-rollback-fingerprint",
            ).unwrap(),
            losses: vec![phoenix_core::domain::close::CloseLossItem::UntrackedNonIgnoredPath(
                phoenix_core::domain::close::GitPathIdentity::from_bytes(b"retained-loss".to_vec()),
            )],
        }],
    };
    let before = initial_finalization_snapshot(&f, &receipt).await;
    let mut wrong_receipt = receipt.clone();
    wrong_receipt.request_id = Uuid::new_v4();
    for (proof, source) in [(&wrong_receipt, &live_source), (&receipt, &ConvState::Idle)] {
        assert!(matches!(
            f.db.terminalize_native_close_initial_losses(
                &command, proof, source, request.clone(), None, "unconfirmed loss",
            ).await,
            LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
        ));
        assert_eq!(initial_finalization_snapshot(&f, &receipt).await, before);
        assert_eq!(initial_phase(&f, &receipt).await, ClosePhase::AwaitingRetirementInspection.as_str());
        assert_no_initial_inspection_artifacts(&f, &receipt).await;
    }

    sqlx::query(
        "CREATE TRIGGER reject_initial_loss_terminalization BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected initial loss terminalization failure'); END",
    ).execute(f.db.pool()).await.unwrap();
    let rollback = f.db.terminalize_native_close_initial_losses(
        &command, &receipt, &live_source, request.clone(), None, "unconfirmed loss",
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Err(error)) = rollback else {
        panic!("late terminalization failure must be a definitely rolled-back error");
    };
    assert!(error.to_string().contains("injected initial loss terminalization failure"));
    assert_eq!(initial_finalization_snapshot(&f, &receipt).await, before);
    assert_no_initial_inspection_artifacts(&f, &receipt).await;
    sqlx::query("DROP TRIGGER reject_initial_loss_terminalization")
        .execute(f.db.pool()).await.unwrap();

    let terminal = f.db.terminalize_native_close_initial_losses(
        &command, &receipt, &live_source, request, None, "unconfirmed loss",
    ).await.established().unwrap().unwrap();
    assert_eq!(terminal.phase(), ClosePhase::Completed);
    assert_eq!(f.db.list_close_retirement_losses(receipt.admitted.attempt_id.as_str()).await.unwrap().len(), 1);
    let dispatches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(dispatches, 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn initial_inspection_and_inventory_require_exact_live_source_and_receipt() {
    let f = Fixture::new().await;
    let target_scope =
        crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    let captured_scopes =
        f.db.list_close_attempt_scopes(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
    assert_eq!(captured_scopes.len(), 1);
    assert_eq!(captured_scopes[0].scope, target_scope);
    let inspection = ReplaceCloseInspectionRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
            scope: target_scope.clone(),
            snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
                "native-generation-1",
                "native-fingerprint-1",
            )
            .unwrap(),
            losses: vec![],
        }],
    };
    let mut wrong = receipt.clone();
    wrong.request_id = Uuid::new_v4();
    for (proof, source) in [(&wrong, &live_source), (&receipt, &ConvState::Idle)] {
        assert!(matches!(
            f.db.replace_native_close_initial_inspection(
                &command,
                proof,
                source,
                inspection.clone(),
                None,
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_no_initial_inspection_artifacts(&f, &receipt).await;
        assert_eq!(
            initial_phase(&f, &receipt).await,
            ClosePhase::AwaitingRetirementInspection.as_str()
        );
    }
    let mut wrong_inspection = inspection.clone();
    wrong_inspection.attempt_id = CloseAttemptId::parse("different-initial-attempt").unwrap();
    assert!(matches!(
        f.db.replace_native_close_initial_inspection(
            &command,
            &receipt,
            &live_source,
            wrong_inspection,
            None,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_no_initial_inspection_artifacts(&f, &receipt).await;
    f.db.replace_native_close_initial_inspection(
        &command,
        &receipt,
        &live_source,
        inspection.clone(),
        None,
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    assert_eq!(
        initial_phase(&f, &receipt).await,
        ClosePhase::RetirementRequested.as_str()
    );
    let inventory = initial_inventory_request(&f, &receipt).await;
    assert_eq!(inventory.scopes.len(), 1);
    assert_eq!(inventory.scopes[0].scope, target_scope);
    assert!(inventory.scopes[0].inventory.worktree.is_some());
    for (proof, source) in [(&wrong, &live_source), (&receipt, &ConvState::Idle)] {
        assert!(matches!(
            f.db.capture_native_close_initial_retirement_inventory(
                &command,
                proof,
                source,
                inventory.clone(),
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_no_initial_inspection_artifacts_after_inspection(&f, &receipt).await;
    }
    let mut wrong_attempt = inventory.clone();
    wrong_attempt.attempt_id = CloseAttemptId::parse("different-initial-attempt").unwrap();
    assert!(matches!(
        f.db.capture_native_close_initial_retirement_inventory(
            &command,
            &receipt,
            &live_source,
            wrong_attempt,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_no_initial_inspection_artifacts_after_inspection(&f, &receipt).await;
    let captured =
        f.db.capture_native_close_initial_retirement_inventory(
            &command,
            &receipt,
            &live_source,
            inventory.clone(),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(captured.len(), 2);
    assert!(captured
        .iter()
        .all(|resource| resource.scope == target_scope));
    assert_eq!(
        captured
            .iter()
            .filter(|resource| resource.resource.kind() == RetiredResourceKind::WorkScope)
            .count(),
        1,
    );
    assert_eq!(
        captured
            .iter()
            .filter(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
            .count(),
        1,
    );
    assert_eq!(
        f.db.capture_native_close_initial_retirement_inventory(
            &command,
            &receipt,
            &live_source,
            inventory,
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        captured
    );
}

async fn captured_initial_retirement(
    f: &Fixture,
) -> (
    NativeCloseCommand,
    NativeCloseReceipt,
    ConvState,
    Vec<CloseExpectedRetirementResource>,
) {
    captured_initial_retirement_with_pty_sessions(f, BTreeSet::new()).await
}

async fn captured_initial_retirement_with_pty_sessions(
    f: &Fixture,
    pty_sessions: BTreeSet<phoenix_core::domain::close::OpaqueIdentity>,
) -> (
    NativeCloseCommand,
    NativeCloseReceipt,
    ConvState,
    Vec<CloseExpectedRetirementResource>,
) {
    let scope = crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    f.db.replace_native_close_initial_inspection(
        &command,
        &receipt,
        &live_source,
        ReplaceCloseInspectionRequest {
            attempt_id: receipt.admitted.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope,
                snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
                    "final-generation",
                    "final-fingerprint",
                )
                .unwrap(),
                losses: vec![],
            }],
        },
        None,
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let before = initial_completion_writes(f, &receipt).await;
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_completion_writes(f, &receipt).await, before);
    let mut inventory = initial_inventory_request(f, &receipt).await;
    inventory.scopes[0].inventory.pty_sessions = pty_sessions;
    // A valid inspected resource is not dispatch authority until the complete
    // original inventory and anchor have been captured atomically.
    let before_dispatch = initial_dispatch_writes(f, &receipt).await;
    let uncaptured = RecordCloseRetirementDispatchRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: inventory.scopes[0].scope.clone(),
        snapshot: inventory.snapshot.clone(),
        resource: RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            phoenix_core::domain::close::LossItemIdentity::Worktree(
                inventory.scopes[0].inventory.worktree.clone().unwrap(),
            ),
        )
        .unwrap(),
    };
    assert!(matches!(
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            uncaptured,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_dispatch_writes(f, &receipt).await, before_dispatch);
    assert_eq!(initial_completion_writes(f, &receipt).await, before);
    let expected =
        f.db.capture_native_close_initial_retirement_inventory(
            &command,
            &receipt,
            &live_source,
            inventory,
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    (command, receipt, live_source, expected)
}

fn initial_dispatch_request(
    receipt: &NativeCloseReceipt,
    resource: &CloseExpectedRetirementResource,
) -> RecordCloseRetirementDispatchRequest {
    RecordCloseRetirementDispatchRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: resource.scope.clone(),
        snapshot: resource.snapshot.clone(),
        resource: resource.resource.clone(),
    }
}

#[tokio::test]
// Keep pre-dispatch refusal, affine transfer, and completed reentry on the same resource.
#[allow(clippy::too_many_lines)]
async fn initial_worktree_entry_requires_exact_dispatch_and_refuses_completed_reentry() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let entered = std::cell::Cell::new(0);
    let transferred = std::cell::Cell::new(false);
    // Model the independently retained owner: writer-fence replay cannot mint
    // another physical owner before a positive retirement proof is written.
    let handoff = || {
        if transferred.replace(true) {
            return NativeCloseInitialResourceEntryDecision::Refused;
        }
        entered.set(entered.get() + 1);
        NativeCloseInitialResourceEntryDecision::Entered
    };
    let undispatched =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            handoff,
        )
        .await;
    assert!(matches!(
        undispatched,
        NativeCloseInitialResourceEntryResult::Rejected {
            error: DbError::CloseFoundationPrecondition(_),
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert_eq!(entered.get(), 0);
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let wrong_snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        "another-generation",
        "another-fingerprint",
    )
    .unwrap();
    let mismatched =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            wrong_snapshot,
            &worktree.resource,
            handoff,
        )
        .await;
    assert!(matches!(
        mismatched,
        NativeCloseInitialResourceEntryResult::Rejected {
            error: DbError::CloseFoundationPrecondition(_),
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert_eq!(entered.get(), 0);
    let admitted =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            handoff,
        )
        .await;
    assert!(matches!(
        admitted,
        NativeCloseInitialResourceEntryResult::Invoked {
            decision: NativeCloseInitialResourceEntryDecision::Entered,
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert_eq!(entered.get(), 1);
    let pending_replay =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            handoff,
        )
        .await;
    assert!(matches!(
        pending_replay,
        NativeCloseInitialResourceEntryResult::Invoked {
            decision: NativeCloseInitialResourceEntryDecision::Refused,
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert_eq!(
        entered.get(),
        1,
        "retained owner refuses duplicate pre-proof entry"
    );
    record_initial_retirement(&f, &command, &receipt, &live_source, worktree).await;
    let completed =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            handoff,
        )
        .await;
    assert!(matches!(
        completed,
        NativeCloseInitialResourceEntryResult::Rejected {
            error: DbError::CloseFoundationPrecondition(_),
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert_eq!(
        entered.get(),
        1,
        "completed proof cannot grant another entry"
    );
}

#[tokio::test]
async fn first_effect_refuses_missing_dispatch_mismatched_binding_and_completed_resource() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let invoked = std::cell::Cell::new(false);
    let callback = || {
        invoked.set(true);
    };
    let undispatched =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            callback,
        )
        .await;
    assert!(matches!(
        undispatched,
        Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Confirmed,
            ..
        })
    ));
    assert!(!invoked.get());
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let foreign_snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        "foreign-generation",
        "foreign-fingerprint",
    )
    .unwrap();
    for (scope, snapshot) in [
        (
            WorkScopeId::parse("foreign-scope").unwrap(),
            worktree.snapshot.clone(),
        ),
        (worktree.scope.clone(), foreign_snapshot),
    ] {
        let rejected =
            f.db.with_native_close_initial_worktree_first_effect(
                &command,
                &receipt,
                &live_source,
                scope,
                snapshot,
                &worktree.resource,
                callback,
            )
            .await;
        assert!(matches!(
            rejected,
            Err(NativeCloseInitialFirstEffectError::Rejected {
                release: NativeCloseInitialEntryRelease::Confirmed,
                ..
            })
        ));
        assert!(!invoked.get());
    }
    record_initial_retirement(&f, &command, &receipt, &live_source, worktree).await;
    let completed =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            callback,
        )
        .await;
    assert!(matches!(
        completed,
        Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Confirmed,
            ..
        })
    ));
    assert!(!invoked.get());
}

#[tokio::test]
async fn first_effect_callback_is_refused_on_source_loss_and_preserves_exact_result() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();

    let invoked = std::cell::Cell::new(false);
    let refused =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &ConvState::Idle,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            || {
                invoked.set(true);
                "must-not-run"
            },
        )
        .await;
    assert!(matches!(
        refused,
        Err(NativeCloseInitialFirstEffectError::Rejected {
            error: DbError::CloseFoundationPrecondition(_),
            release: NativeCloseInitialEntryRelease::Confirmed,
        })
    ));
    assert!(!invoked.get());

    let outcome =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            || String::from("exact real-effect outcome"),
        )
        .await;
    match outcome {
        Ok((result, NativeCloseInitialEntryRelease::Confirmed)) => {
            assert_eq!(result, "exact real-effect outcome");
        }
        _ => panic!("expected callback result and confirmed reservation release"),
    }
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let revoked =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            || {
                invoked.set(true);
            },
        )
        .await;
    assert!(matches!(
        revoked,
        Err(NativeCloseInitialFirstEffectError::Rejected {
            release: NativeCloseInitialEntryRelease::Confirmed,
            ..
        })
    ));
    assert!(
        !invoked.get(),
        "stale live source cannot bypass durable revocation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_effect_callback_holds_writer_reservation_against_source_update() {
    let f = Fixture::new_file_backed().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();

    let path = f.db_path.as_deref().expect("file-backed fixture");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::ZERO);
    let mut competing_writer = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let conversation_id = f.coordinator.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let (busy_tx, busy_rx) = std::sync::mpsc::sync_channel::<Result<i32, String>>(1);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (retry_tx, retry_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let competing = tokio::spawn(async move {
        start_rx.await.unwrap();
        let error = sqlx::query("UPDATE conversations SET updated_at = updated_at WHERE id = ?1")
            .bind(&conversation_id)
            .execute(&mut competing_writer)
            .await
            .unwrap_err();
        let code = error
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .and_then(|code| code.parse::<i32>().ok());
        let _ = busy_tx.send(code.ok_or_else(|| format!("expected SQLite code 5, got {error}")));
        release_rx.await.map_err(|error| error.to_string())?;
        sqlx::query("UPDATE conversations SET updated_at = updated_at WHERE id = ?1")
            .bind(&conversation_id)
            .execute(&mut competing_writer)
            .await
            .map_err(|error| error.to_string())?;
        let _ = retry_tx.send(Ok(()));
        Ok::<(), String>(())
    });

    let outcome =
        f.db.with_native_close_initial_worktree_first_effect(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            || {
                start_tx.send(()).unwrap();
                let code = busy_rx
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .expect("competing writer should acknowledge while callback is held")
                    .expect("competing UPDATE should report SQLITE_BUSY (5)");
                assert_eq!(code, 5);
                Err::<(), _>("callback-result-sentinel")
            },
        )
        .await;
    assert!(matches!(
        outcome,
        Ok((
            Err("callback-result-sentinel"),
            NativeCloseInitialEntryRelease::Confirmed
        ))
    ));
    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), retry_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), competing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn initial_worktree_entry_rejects_durable_source_revocation_before_handoff() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let invoked = std::cell::Cell::new(false);
    let result =
        f.db.enter_native_close_initial_worktree_resource(
            &command,
            &receipt,
            &live_source,
            worktree.scope.clone(),
            worktree.snapshot.clone(),
            &worktree.resource,
            || {
                invoked.set(true);
                NativeCloseInitialResourceEntryDecision::Entered
            },
        )
        .await;
    assert!(matches!(
        result,
        NativeCloseInitialResourceEntryResult::Rejected {
            error: DbError::CloseFoundationPrecondition(_),
            release: NativeCloseInitialEntryRelease::Confirmed,
        }
    ));
    assert!(!invoked.get());
}

async fn initial_dispatch_writes(
    f: &Fixture,
    receipt: &NativeCloseReceipt,
) -> (i64, Option<i64>, i64) {
    sqlx::query_as(
        "SELECT COUNT(*), MAX(dispatched_at_us),
                (SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1)
         FROM close_retirement_resource_dispatches WHERE attempt_id = ?1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn initial_dispatch_records_exact_resources_and_replays_without_effects_or_generic_bypass() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let completion_before = initial_completion_writes(&f, &receipt).await;
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, (0, None, 0));
    for (index, resource) in expected.iter().enumerate() {
        let request = initial_dispatch_request(&receipt, resource);
        let before = initial_dispatch_writes(&f, &receipt).await;
        assert!(matches!(
            f.db.record_close_retirement_dispatch(request.clone()).await,
            Err(DbError::CloseFoundationPrecondition(_))
        ));
        assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
        for replay in [false, true] {
            let before_dispatch = initial_dispatch_writes(&f, &receipt).await;
            assert!(matches!(
                f.db.record_native_close_initial_retirement_dispatch(
                    &command,
                    &receipt,
                    &live_source,
                    request.clone(),
                )
                .await,
                LocalAuthorityResult::DurableFactEstablished(Ok(_))
            ));
            let after = initial_dispatch_writes(&f, &receipt).await;
            assert_eq!(after.0, i64::try_from(index + 1).unwrap());
            assert!(after.1.is_some());
            assert_eq!(after.2, 0);
            if replay {
                assert_eq!(after, before_dispatch);
            }
        }
        // Generic dispatch must refuse even an already durable native dispatch.
        let before = initial_dispatch_writes(&f, &receipt).await;
        assert!(matches!(
            f.db.record_close_retirement_dispatch(request).await,
            Err(DbError::CloseFoundationPrecondition(_))
        ));
        assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
    }
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        completion_before
    );
}

#[tokio::test]
async fn process_batch_replay_does_not_commit_other_first_dispatches() {
    use phoenix_core::domain::close::OpaqueIdentity;
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement_with_pty_sessions(
        &f,
        ["test-pty-a", "test-pty-b"]
            .into_iter()
            .map(|id| OpaqueIdentity::parse(id).unwrap())
            .collect(),
    )
    .await;
    let requests: Vec<_> = expected
        .iter()
        .filter(|item| item.resource.kind() == RetiredResourceKind::PtySession)
        .map(|item| initial_dispatch_request(&receipt, item))
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            requests[1].clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(CloseRetirementDispatch::First(_)))
    ));
    let before = initial_dispatch_writes(&f, &receipt).await;
    assert_eq!(before.0, 1);
    assert!(matches!(
        f.db.record_native_close_initial_process_dispatches(
            &command,
            &receipt,
            &live_source,
            &requests,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(CloseRetirementBatchDispatch::Replay))
    ));
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
}

#[tokio::test]
async fn concurrent_original_worktree_dispatches_yield_one_first_owner() {
    let f = Fixture::new_file_backed().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let request = initial_dispatch_request(&receipt, worktree);
    let dispatch = || {
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
    };
    let (left, right) = tokio::join!(dispatch(), dispatch());
    let left = left.established().unwrap().unwrap();
    let right = right.established().unwrap().unwrap();
    let owner = match (left, right) {
        (CloseRetirementDispatch::First(owner), CloseRetirementDispatch::Replay)
        | (CloseRetirementDispatch::Replay, CloseRetirementDispatch::First(owner)) => owner,
        other => panic!("expected exactly one first dispatch, got {other:?}"),
    };
    let claimed = owner.into_request();
    assert_eq!(claimed.attempt_id, request.attempt_id);
    assert_eq!(claimed.scope, request.scope);
    assert_eq!(claimed.snapshot, request.snapshot);
    assert_eq!(claimed.resource, request.resource);
    assert!(matches!(
        dispatch().await.established().unwrap().unwrap(),
        CloseRetirementDispatch::Replay
    ));
    let (dispatches, _, receipts) = initial_dispatch_writes(&f, &receipt).await;
    assert_eq!((dispatches, receipts), (1, 0));
}

#[tokio::test]
async fn initial_dispatch_revalidates_live_and_durable_source_before_replay() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let request = initial_dispatch_request(&receipt, &expected[0]);
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        request.clone(),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let before = initial_dispatch_writes(&f, &receipt).await;
    let completion_before = initial_completion_writes(&f, &receipt).await;
    assert!(matches!(
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &ConvState::Idle,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        completion_before
    );
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    // Durable source loss may legitimately stop the run. Snapshot after that fence.
    let before = initial_dispatch_writes(&f, &receipt).await;
    let completion_before = initial_completion_writes(&f, &receipt).await;
    assert!(matches!(
        f.db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &live_source,
            request,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        completion_before
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn initial_dispatch_rejects_mismatched_receipt_and_full_inventory_identity_without_writes() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let resource = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let request = initial_dispatch_request(&receipt, resource);
    let mut wrong_receipt = receipt.clone();
    wrong_receipt.request_id = Uuid::new_v4();
    let mut wrong_run = receipt.clone();
    wrong_run.admitted.attempt_id = CloseAttemptId::parse("different-initial-attempt").unwrap();
    let mut wrong_ordinal = receipt.clone();
    wrong_ordinal.admitted.run_ordinal = CloseRunOrdinal::parse(2).unwrap();
    let mut wrong_attempt = request.clone();
    wrong_attempt.attempt_id = wrong_run.admitted.attempt_id.clone();
    let mut wrong_scope = request.clone();
    wrong_scope.scope = WorkScopeId::parse("different-scope").unwrap();
    let mut wrong_generation = request.clone();
    wrong_generation.snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        "different-generation",
        request.snapshot.fingerprint(),
    )
    .unwrap();
    let mut wrong_fingerprint = request.clone();
    wrong_fingerprint.snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        request.snapshot.generation(),
        "different-fingerprint",
    )
    .unwrap();
    let phoenix_core::domain::close::LossItemIdentity::Worktree(worktree) =
        request.resource.identity()
    else {
        panic!("expected full worktree identity");
    };
    let mut wrong_identity = request.clone();
    wrong_identity.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            WorktreeId::parse("different-worktree").unwrap(),
            worktree.fingerprint().clone(),
            worktree.locator().clone(),
        )),
    )
    .unwrap();
    let mut wrong_worktree_fingerprint = request.clone();
    wrong_worktree_fingerprint.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            worktree.id().clone(),
            WorktreeFingerprint::parse("different-worktree-fingerprint").unwrap(),
            worktree.locator().clone(),
        )),
    )
    .unwrap();
    let mut wrong_locator = request.clone();
    wrong_locator.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            worktree.id().clone(),
            worktree.fingerprint().clone(),
            GitPathIdentity::from_bytes(b"/different/worktree".to_vec()),
        )),
    )
    .unwrap();
    for replay in [false, true] {
        if replay {
            f.db.record_native_close_initial_retirement_dispatch(
                &command,
                &receipt,
                &live_source,
                request.clone(),
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        }
        let before = initial_dispatch_writes(&f, &receipt).await;
        let completion_before = initial_completion_writes(&f, &receipt).await;
        for proof in [&wrong_receipt, &wrong_run, &wrong_ordinal] {
            assert!(matches!(
                f.db.record_native_close_initial_retirement_dispatch(
                    &command,
                    proof,
                    &live_source,
                    request.clone(),
                )
                .await,
                LocalAuthorityResult::DurableFactEstablished(Err(
                    DbError::CloseFoundationPrecondition(_)
                ))
            ));
            assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
        }
        for mismatch in [
            &wrong_attempt,
            &wrong_scope,
            &wrong_generation,
            &wrong_fingerprint,
            &wrong_identity,
            &wrong_worktree_fingerprint,
            &wrong_locator,
        ] {
            assert!(matches!(
                f.db.record_native_close_initial_retirement_dispatch(
                    &command,
                    &receipt,
                    &live_source,
                    mismatch.clone(),
                )
                .await,
                LocalAuthorityResult::DurableFactEstablished(Err(
                    DbError::CloseFoundationPrecondition(_)
                ))
            ));
            assert_eq!(initial_dispatch_writes(&f, &receipt).await, before);
        }
        assert_eq!(
            initial_completion_writes(&f, &receipt).await,
            completion_before
        );
    }
}

async fn record_initial_retirement(
    f: &Fixture,
    command: &NativeCloseCommand,
    receipt: &NativeCloseReceipt,
    live_source: &ConvState,
    resource: &CloseExpectedRetirementResource,
) {
    f.db.record_native_close_initial_retirement_dispatch(
        command,
        receipt,
        live_source,
        initial_dispatch_request(receipt, resource),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    f.db.record_native_close_initial_retirement_evidence(
        command,
        receipt,
        live_source,
        crate::RecordCloseRetirementEvidenceRequest {
            attempt_id: receipt.admitted.attempt_id.clone(),
            snapshot: resource.snapshot.clone(),
            scope: resource.scope.clone(),
            resource: resource.resource.clone(),
            outcome: phoenix_core::domain::close::RetirementOutcome::Retired,
            detail: None,
        },
    )
    .await
    .established()
    .unwrap()
    .unwrap();
}

async fn initial_evidence_writes(
    f: &Fixture,
    receipt: &NativeCloseReceipt,
) -> (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
) {
    sqlx::query_as(
        "SELECT COUNT(*), MAX(proof_kind), MAX(detail), MAX(created_at), MAX(updated_at),
                (SELECT COUNT(*) FROM close_retirement_resource_history WHERE attempt_id = ?1),
                (SELECT MAX(recorded_at) FROM close_retirement_resource_history WHERE attempt_id = ?1)
         FROM close_retirement_resources WHERE attempt_id = ?1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One retained proof exercises authority-before-replay mismatches.
async fn initial_retirement_evidence_is_source_bound_replayable_and_not_generic() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let resource = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let request = crate::RecordCloseRetirementEvidenceRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        snapshot: resource.snapshot.clone(),
        scope: resource.scope.clone(),
        resource: resource.resource.clone(),
        outcome: phoenix_core::domain::close::RetirementOutcome::Retired,
        detail: None,
    };
    let evidence_before = initial_evidence_writes(&f, &receipt).await;
    let dispatch_before = initial_dispatch_writes(&f, &receipt).await;
    let completion_before = initial_completion_writes(&f, &receipt).await;

    // Evidence cannot establish success before this exact resource's dispatch intent.
    assert!(matches!(
        f.db.record_native_close_initial_retirement_evidence(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_evidence_writes(&f, &receipt).await, evidence_before);
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, dispatch_before);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        completion_before
    );

    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, resource),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let completion_after_dispatch = initial_completion_writes(&f, &receipt).await;
    assert!(matches!(
        f.db.record_native_close_initial_retirement_evidence(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(()))
    ));
    let evidence_after = initial_evidence_writes(&f, &receipt).await;
    assert_eq!(evidence_after.0, 1);
    assert_eq!(evidence_after.1.as_deref(), Some("retired"));
    // This composite snapshot also counts evidence: take its replay baseline
    // after the first legitimate proof, not merely after dispatch admission.
    let dispatch_after = initial_dispatch_writes(&f, &receipt).await;

    // A byte-for-byte replay is idempotent: evidence history, dispatch timestamp,
    // and completion state remain unchanged.
    assert!(matches!(
        f.db.record_native_close_initial_retirement_evidence(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(()))
    ));
    assert_eq!(initial_evidence_writes(&f, &receipt).await, evidence_after);
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, dispatch_after);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        completion_after_dispatch
    );

    let other_target = Fixture::create_target(&f.db, "second-target").await;
    let mut wrong_request_id = receipt.clone();
    wrong_request_id.request_id = Uuid::new_v4();
    let mut wrong_ordinal = receipt.clone();
    wrong_ordinal.admitted.run_ordinal = CloseRunOrdinal::parse(2).unwrap();
    let mut wrong_target = receipt.clone();
    wrong_target.admitted.target = other_target.clone();
    let wrong_command_target = f.command(f.source.clone(), &other_target);
    let before_rejections = (
        initial_evidence_writes(&f, &receipt).await,
        initial_dispatch_writes(&f, &receipt).await,
        initial_completion_writes(&f, &receipt).await,
    );
    for (proof, candidate_command) in [
        (&wrong_request_id, &command),
        (&wrong_ordinal, &command),
        (&wrong_target, &command),
        (&receipt, &wrong_command_target),
    ] {
        assert!(matches!(
            f.db.record_native_close_initial_retirement_evidence(
                candidate_command,
                proof,
                &live_source,
                request.clone(),
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(
            initial_evidence_writes(&f, &receipt).await,
            before_rejections.0
        );
        assert_eq!(
            initial_dispatch_writes(&f, &receipt).await,
            before_rejections.1
        );
        assert_eq!(
            initial_completion_writes(&f, &receipt).await,
            before_rejections.2
        );
    }

    let phoenix_core::domain::close::LossItemIdentity::Worktree(worktree) =
        request.resource.identity()
    else {
        panic!("expected full worktree identity");
    };
    let mut wrong_worktree_id = request.clone();
    wrong_worktree_id.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            WorktreeId::parse("different-worktree").unwrap(),
            worktree.fingerprint().clone(),
            worktree.locator().clone(),
        )),
    )
    .unwrap();
    let mut wrong_worktree_fingerprint = request.clone();
    wrong_worktree_fingerprint.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            worktree.id().clone(),
            WorktreeFingerprint::parse("different-worktree-fingerprint").unwrap(),
            worktree.locator().clone(),
        )),
    )
    .unwrap();
    let mut wrong_worktree_locator = request.clone();
    wrong_worktree_locator.resource = RetiredResourceIdentity::parse(
        RetiredResourceKind::Worktree,
        phoenix_core::domain::close::LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
            worktree.id().clone(),
            worktree.fingerprint().clone(),
            GitPathIdentity::from_bytes(b"/different/worktree".to_vec()),
        )),
    )
    .unwrap();
    let mut wrong_snapshot = request.clone();
    wrong_snapshot.snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        "stale-generation",
        request.snapshot.fingerprint(),
    )
    .unwrap();
    let mut changed_detail = request.clone();
    changed_detail.detail = Some("different result".into());
    let mut residual = request.clone();
    residual.outcome = phoenix_core::domain::close::RetirementOutcome::Residual {
        residual_reason: RetirementFailureReason::RemovalFailed,
    };
    for mismatch in [
        wrong_worktree_id,
        wrong_worktree_fingerprint,
        wrong_worktree_locator,
        wrong_snapshot,
        changed_detail,
        residual,
    ] {
        assert!(matches!(
            f.db.record_native_close_initial_retirement_evidence(
                &command,
                &receipt,
                &live_source,
                mismatch,
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(initial_evidence_writes(&f, &receipt).await, evidence_after);
        assert_eq!(initial_dispatch_writes(&f, &receipt).await, dispatch_after);
        assert_eq!(
            initial_completion_writes(&f, &receipt).await,
            completion_after_dispatch
        );
    }
    assert!(matches!(
        f.db.record_close_retirement_evidence(request.clone()).await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    assert_eq!(initial_evidence_writes(&f, &receipt).await, evidence_after);

    // The durable source fence wins over replay even after success evidence exists.
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let fenced_before = (
        initial_evidence_writes(&f, &receipt).await,
        initial_dispatch_writes(&f, &receipt).await,
        initial_completion_writes(&f, &receipt).await,
    );
    assert!(matches!(
        f.db.record_native_close_initial_retirement_evidence(
            &command,
            &receipt,
            &live_source,
            request,
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_evidence_writes(&f, &receipt).await, fenced_before.0);
    assert_eq!(initial_dispatch_writes(&f, &receipt).await, fenced_before.1);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        fenced_before.2
    );
}

#[tokio::test]
async fn initial_retirement_evidence_adopts_exact_prior_retirement_without_replacing_it() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let resource = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::WorkScope)
        .unwrap();
    let request = crate::RecordCloseRetirementEvidenceRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        snapshot: resource.snapshot.clone(),
        scope: resource.scope.clone(),
        resource: resource.resource.clone(),
        outcome: phoenix_core::domain::close::RetirementOutcome::AbsenceAdopted {
            absence_basis: phoenix_core::domain::close::AbsenceBasis::SameAttemptPriorRetirement,
        },
        detail: None,
    };
    let before = initial_evidence_writes(&f, &receipt).await;
    assert!(matches!(
        f.db.record_native_close_initial_retirement_evidence(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_evidence_writes(&f, &receipt).await, before);

    record_initial_retirement(&f, &command, &receipt, &live_source, resource).await;
    let retired = initial_evidence_writes(&f, &receipt).await;
    assert_eq!(retired.0, 1);
    assert_eq!(retired.1.as_deref(), Some("retired"));
    f.db.record_native_close_initial_retirement_evidence(
        &command,
        &receipt,
        &live_source,
        request.clone(),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let adopted = initial_evidence_writes(&f, &receipt).await;
    assert_eq!(adopted.0, 1);
    assert_eq!(adopted.1.as_deref(), Some("retired"));
    assert_eq!(adopted.5, 2);
    assert_eq!(adopted.4, retired.4);
    f.db.record_native_close_initial_retirement_evidence(&command, &receipt, &live_source, request)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(initial_evidence_writes(&f, &receipt).await, adopted);
}

async fn initial_completion_writes(
    f: &Fixture,
    receipt: &NativeCloseReceipt,
) -> (String, i64, i64, String) {
    let attempt = receipt.admitted.attempt_id.as_str();
    let phase = initial_phase(f, receipt).await;
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id = ?1")
        .bind(format!("close-outcome:{attempt}"))
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    let archived: i64 = sqlx::query_scalar("SELECT archived FROM conversations WHERE id = ?1")
        .bind("first-target")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    let lifecycle: String =
        sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(f.target.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    (phase, messages, archived, lifecycle)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn initial_completion_requires_source_receipt_and_all_concrete_proofs() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    assert_eq!(expected.len(), 2);
    let work_scope = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::WorkScope)
        .unwrap();
    let worktree = expected
        .iter()
        .find(|resource| resource.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    assert_eq!(work_scope.scope, worktree.scope);
    assert_eq!(work_scope.snapshot, worktree.snapshot);
    let before = initial_completion_writes(&f, &receipt).await;
    let mut finalization_before = initial_finalization_snapshot(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_close_retirement(&receipt.admitted.attempt_id)
            .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    let mut generic_tx = f.db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        crate::Database::complete_close_retirement_tx(
            &mut generic_tx,
            &receipt.admitted.attempt_id
        )
        .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    generic_tx.rollback().await.unwrap();
    assert_eq!(initial_completion_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    assert_eq!(before.0, ClosePhase::RetirementRequested.as_str());
    assert_eq!(before.1, 0);
    assert_eq!(before.2, 0);
    assert_eq!(before.3, "open");
    let mut wrong = receipt.clone();
    wrong.request_id = Uuid::new_v4();
    let mut wrong_run = receipt.clone();
    wrong_run.admitted.attempt_id = CloseAttemptId::parse("other-initial-attempt").unwrap();
    for proof in [&wrong, &wrong_run] {
        assert!(matches!(
            f.db.complete_native_close_initial(&command, proof, &live_source)
                .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(initial_completion_writes(&f, &receipt).await, before);
        assert_eq!(
            initial_finalization_snapshot(&f, &receipt).await,
            finalization_before
        );
    }
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &ConvState::Idle)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_completion_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    assert_eq!(initial_completion_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    record_initial_retirement(&f, &command, &receipt, &live_source, worktree).await;
    finalization_before = initial_finalization_snapshot(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    assert_eq!(initial_completion_writes(&f, &receipt).await, before);
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    record_initial_retirement(&f, &command, &receipt, &live_source, work_scope).await;
    finalization_before = initial_finalization_snapshot(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_close_retirement(&receipt.admitted.attempt_id)
            .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    let mut generic_tx = f.db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(matches!(
        crate::Database::complete_close_retirement_tx(
            &mut generic_tx,
            &receipt.admitted.attempt_id
        )
        .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    generic_tx.rollback().await.unwrap();
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        finalization_before
    );
    let completed =
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap();
    assert_eq!(completed.phase(), ClosePhase::Completed);
    assert_eq!(
        initial_completion_writes(&f, &receipt).await,
        (
            ClosePhase::Completed.as_str().to_owned(),
            1,
            1,
            "history".into()
        )
    );
    assert_eq!(
        f.db.get_close_run(&phoenix_core::domain::close::CloseRunRef {
            attempt_id: receipt.admitted.attempt_id.clone(),
            ordinal: CloseRunOrdinal::INITIAL,
        })
        .await
        .unwrap()
        .status,
        phoenix_core::domain::close::CloseRunStatus::Completed
    );
    let replay_before = initial_finalization_snapshot(&f, &receipt).await;
    let mut wrong_target = receipt.clone();
    wrong_target.admitted.target =
        ProductConversationId::parse("foreign-completed-target").unwrap();
    let mut wrong_attempt = receipt.clone();
    wrong_attempt.admitted.attempt_id = CloseAttemptId::parse("foreign-completed-attempt").unwrap();
    for invalid_receipt in [&wrong_target, &wrong_attempt] {
        assert!(matches!(
            f.db.complete_native_close_initial(&command, invalid_receipt, &live_source)
                .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(
            initial_finalization_snapshot(&f, &receipt).await,
            replay_before
        );
    }
    let conflicting_command = f.command(f.source.clone(), &wrong_target.admitted.target);
    assert_eq!(conflicting_command.request_id(), command.request_id());
    assert_ne!(conflicting_command.fingerprint(), command.fingerprint());
    assert!(matches!(
        f.db.complete_native_close_initial(&conflicting_command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        replay_before
    );
    let mut wrong_receipt = receipt.clone();
    wrong_receipt.request_id = Uuid::new_v4();
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &wrong_receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        replay_before
    );
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &ConvState::Idle)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        replay_before
    );
    assert!(matches!(
        f.db.complete_close_retirement(&receipt.admitted.attempt_id)
            .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        replay_before
    );
    let replay =
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap();
    assert_eq!(replay, completed);
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        replay_before
    );
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let after_source_loss = initial_finalization_snapshot(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        after_source_loss
    );
}

#[tokio::test]
async fn initial_completion_rejects_durably_lost_source_without_writes() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, _) = captured_initial_retirement(&f).await;
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    // Source loss classifies the running attempt; only finalization's subsequent
    // writes must be absent, not the legitimate source-fence classification.
    let before = initial_completion_writes(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_completion_writes(&f, &receipt).await, before);
}

async fn assert_no_initial_inspection_artifacts_after_inspection(
    f: &Fixture,
    receipt: &NativeCloseReceipt,
) {
    for sql in [
        "SELECT COUNT(*) FROM close_retirement_inventories WHERE attempt_id = ?1",
        "SELECT COUNT(*) FROM close_expected_retirement_resources WHERE attempt_id = ?1",
        "SELECT COUNT(*) FROM close_original_retirement_anchors WHERE attempt_id = ?1",
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "unexpected inventory artifact: {sql}");
    }
}

#[tokio::test]
async fn prepared_native_initial_is_not_generic_settlement_work() {
    let f = Fixture::new().await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    assert!(f
        .db
        .list_pending_generic_close_settlements(None)
        .await
        .unwrap()
        .is_empty());
    let attempt = receipt.admitted.attempt_id.as_str();
    for result in [
        f.db.begin_close_idle_settlement(attempt).await,
        f.db.begin_close_active_work_settlement(attempt).await,
        f.db.cancel_close_before_retirement(attempt).await,
        f.db.request_close_settlement_cancellation(attempt).await,
    ] {
        assert!(matches!(
            result,
            Err(DbError::CloseFoundationPrecondition(message))
                if message.contains("owned by native initial Close settlement")
        ));
    }
    assert!(f
        .db
        .advance_close_settlement_when_quiescent(receipt.admitted.attempt_id.as_str())
        .await
        .is_err());
    assert_eq!(
        initial_phase(&f, &receipt).await,
        ClosePhase::SettlingActiveWork.as_str()
    );
    assert_no_dispatch_effects(&f.db, &receipt).await;
    assert_one_operation(&f, &receipt, 0).await;

    // The exact source-owned path remains available; exclusion does not strand
    // this run behind a generic-worker-only advancement API.
    let established =
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap();
    assert_eq!(established.phase, ClosePhase::AwaitingRetirementInspection);
}

#[tokio::test]
async fn initial_settlement_advances_exact_quiescent_run_without_dispatch() {
    let f = Fixture::new().await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let settled = match f
        .db
        .settle_native_close_initial(&command, &receipt, &live_source)
        .await
    {
        LocalAuthorityResult::DurableFactEstablished(result) => result.unwrap(),
        LocalAuthorityResult::DurableFactUnclassified => panic!("settlement outcome unclassified"),
    };
    assert_eq!(settled.run, receipt.admitted);
    assert_eq!(
        settled.phase,
        phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection
    );
    assert_eq!(
        initial_phase(&f, &receipt).await,
        "awaiting_retirement_inspection"
    );
    assert_one_operation(&f, &receipt, 0).await;
    assert_no_dispatch_effects(&f.db, &receipt).await;
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
}

#[tokio::test]
async fn initial_settlement_requires_exact_released_turn_generation_not_latest_idle() {
    let f = Fixture::new().await;
    let turn = source_turn(&f.db, f.target.as_str(), "settlement-owning-turn").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");
    assert_turn_unsettled(&f.db, turn).await;
    // Model a durable release with the wrong generation; it cannot discharge the captured receipt.
    sqlx::query("UPDATE durable_turns SET generation = 2, terminal_kind = 'Completed', owns_conversation = 0 WHERE turn_id = ?1")
        .bind(i64::try_from(turn).unwrap()).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    sqlx::query("UPDATE durable_turns SET generation = 1 WHERE turn_id = ?1")
        .bind(i64::try_from(turn).unwrap())
        .execute(f.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    assert_eq!(
        initial_phase(&f, &receipt).await,
        "awaiting_retirement_inspection"
    );
    let unsettled = f
        .db
        .list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
    assert!(unsettled.is_empty());
    let settled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlements WHERE attempt_id = ?1 AND settled_at IS NOT NULL",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(
        settled, 1,
        "the exact released generation must be reconciled"
    );
    assert_one_operation(&f, &receipt, 0).await;
}

#[tokio::test]
async fn initial_inspection_failure_terminalizes_exact_captured_scope_and_outboxes_once() {
    let f = Fixture::new().await;
    let captured_scope =
        crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    let result =
        f.db.terminalize_native_close_initial_inspection_failure(
            &command,
            &receipt,
            &live_source,
            &captured_scope,
            RetirementFailureReason::IdentityNotProven,
            "worktree identity could not be verified",
        )
        .await;
    let obligation = result.established().unwrap().unwrap();
    assert_eq!(obligation.phase(), ClosePhase::Completed);
    let failures =
        f.db.list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].occurrence.reason,
        RetirementFailureReason::IdentityNotProven
    );
    assert_eq!(
        failures[0].occurrence.authority,
        crate::CloseCleanupFailureAuthority::CapturedScope {
            scope: captured_scope.clone(),
            resource: phoenix_core::domain::close::RetiredResourceIdentity::parse(
                RetiredResourceKind::WorkScope,
                phoenix_core::domain::close::LossItemIdentity::Opaque(
                    phoenix_core::domain::close::OpaqueIdentity::parse(captured_scope.as_str())
                        .unwrap(),
                ),
            )
            .unwrap(),
        }
    );
    assert_eq!(failures[0].occurrence.remaining_resources.len(), 2);
    assert!(failures[0]
        .occurrence
        .remaining_resources
        .iter()
        .all(|resource| resource.disposition == crate::CloseCleanupResourceDisposition::Unknown));
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(failures[0].occurrence.failure_occurrence_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);
}

#[tokio::test]
async fn initial_inspection_unresolved_worktree_keeps_only_captured_scope_unknown() {
    let f = Fixture::new().await;
    // Capture an allocated worktree whose stable identity is still unresolved.
    // Failure classification must not invent an exact Worktree resource.
    let scope =
        f.db.get_conversation("first-target")
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
    sqlx::query(
        "UPDATE work_scopes
         SET environment_kind = 'allocated_worktree', worktree_path = '/tmp/unresolved-worktree',
             worktree_id = NULL, worktree_fingerprint = NULL
         WHERE id = ?1",
    )
    .bind(scope.as_str())
    .execute(f.db.pool())
    .await
    .unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let captured =
        f.db.list_close_attempt_scopes(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].scope, scope);
    assert!(matches!(
        &captured[0].captured_worktree,
        Some(phoenix_core::domain::close::CapturedWorktreeIdentity::Unresolved { .. })
    ));
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    let completed =
        f.db.terminalize_native_close_initial_inspection_failure(
            &command,
            &receipt,
            &live_source,
            &scope,
            RetirementFailureReason::IdentityNotProven,
            "captured allocated worktree has no resolved identity",
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(completed.phase(), ClosePhase::Completed);
    let failures =
        f.db.list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
    assert_eq!(failures.len(), 1);
    let resources = &failures[0].occurrence.remaining_resources;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].scope, scope);
    assert_eq!(resources[0].resource.kind(), RetiredResourceKind::WorkScope);
    assert_eq!(
        resources[0].disposition,
        crate::CloseCleanupResourceDisposition::Unknown
    );
    assert!(f
        .db
        .terminalize_native_close_initial_inspection_failure(
            &command,
            &receipt,
            &live_source,
            &scope,
            RetirementFailureReason::IdentityNotProven,
            "replayed classification cannot create another occurrence",
        )
        .await
        .established()
        .unwrap()
        .is_err());
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE mandatory_failure_occurrence_id = ?1",
    )
    .bind(failures[0].occurrence.failure_occurrence_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 1);
}

#[tokio::test]
async fn initial_inspection_failure_refuses_wrong_authority_without_writes_and_rolls_back_outbox_failure(
) {
    let f = Fixture::new().await;
    let captured_scope =
        crate::close_foundation::allocate_scope_worktree(&f.db, "first-target").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    f.db.settle_native_close_initial(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    let wrong_receipt = NativeCloseReceipt {
        request_id: Uuid::new_v4(),
        ..receipt.clone()
    };
    let wrong_scope = WorkScopeId::parse("not-captured-scope").unwrap();
    for (proof, source, scope) in [
        (&receipt, &ConvState::Idle, &captured_scope),
        (&wrong_receipt, &live_source, &captured_scope),
        (&receipt, &live_source, &wrong_scope),
    ] {
        assert!(matches!(
            f.db.terminalize_native_close_initial_inspection_failure(
                &command,
                proof,
                source,
                scope,
                RetirementFailureReason::IdentityNotProven,
                "inspection failed",
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(count(&f.db, "close_obligations").await, 1);
        assert_eq!(count(&f.db, "close_runs").await, 1);
        let failures =
            f.db.list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
                .await
                .unwrap();
        assert!(failures.is_empty());
    }
    sqlx::query(
        "CREATE TRIGGER reject_initial_close_failure_outbox BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected failure outbox error'); END",
    ).execute(f.db.pool()).await.unwrap();
    assert!(matches!(
        f.db.terminalize_native_close_initial_inspection_failure(
            &command,
            &receipt,
            &live_source,
            &captured_scope,
            RetirementFailureReason::IdentityNotProven,
            "inspection failed",
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    assert_eq!(
        initial_phase(&f, &receipt).await,
        ClosePhase::AwaitingRetirementInspection.as_str()
    );
    assert!(f
        .db
        .list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap()
        .is_empty());
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind = 'mandatory_close_failure'",
    ).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(outbox_count, 0);
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
            f.db.settle_native_close_initial(&command, wrong, &live_source)
                .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
    }
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &ConvState::Idle)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "completed");
    let failures =
        f.db.list_close_cleanup_failures(receipt.admitted.attempt_id.as_str())
            .await
            .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].occurrence.authority,
        crate::CloseCleanupFailureAuthority::ExplicitlySuppressed
    );
    assert!(failures[0].occurrence.remaining_resources.is_empty());
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
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_phase(&f, &receipt).await,
        "awaiting_blocker_resolution"
    );
    assert!(matches!(
        f.db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    f.db.cancel_close_before_retirement(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_phase(&f, &receipt).await,
        "cancel_requested_during_settlement"
    );
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
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(_))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");
    let unsettled = f
        .db
        .list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await
        .unwrap();
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
    let attempt = receipt.admitted.attempt_id.as_str();
    for result in [
        f.db.begin_close_active_work_settlement(attempt).await,
        f.db.cancel_close_before_retirement(attempt).await,
    ] {
        assert!(matches!(
            result,
            Err(DbError::CloseFoundationPrecondition(message))
                if message.contains("owned by native initial Close settlement")
        ));
    }
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM close_obligations WHERE attempt_id = ?1")
            .bind(receipt.admitted.attempt_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert!(matches!(
        f.db.settle_native_close_initial(&command, &receipt, &live_source)
            .await,
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
    assert_eq!(phase, "completed");
    assert_no_settlement_effects(&f.db, &receipt).await;
}

#[tokio::test]
async fn initial_receipt_replay_and_source_fence_accept_both_exact_retained_shapes() {
    for legacy in [false, true] {
        let f = Fixture::new().await;
        let command = f.command(f.source.clone(), &f.target);
        let input = if legacy {
            command.intent().legacy_source_input()
        } else {
            command.intent().source_input()
        };
        let command = NativeCloseCommand::new_with_source_input(
            f.coordinator.clone(),
            f.source.clone(),
            command.intent().clone(),
            &input,
        )
        .unwrap();
        let state = crate::native_close::tests::executing_state(
            command.source(),
            input.clone(),
            input.clone(),
        );
        f.db.update_conversation_state(&f.coordinator, &state)
            .await
            .unwrap();
        let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
            panic!("expected admission for legacy={legacy}");
        };
        // Rewriting the unchanged source exercises the durable source fence.
        f.db.update_conversation_state(&f.coordinator, &state)
            .await
            .unwrap();
        assert_eq!(
            f.db.get_conversation(&f.coordinator).await.unwrap().state,
            state
        );
        let NativeCloseInitialResult::Replay(replay) = f.apply(&command).await.unwrap() else {
            panic!("expected exact replay for legacy={legacy}");
        };
        assert_eq!(replay, receipt);
        assert_one_operation(&f, &receipt, 0).await;
        let opposite = if legacy {
            command.intent().source_input()
        } else {
            command.intent().legacy_source_input()
        };
        let coherent_opposite = crate::native_close::tests::executing_state(
            command.source(),
            opposite.clone(),
            opposite,
        );
        // The fence must select the original shape from the saved fingerprint,
        // not adopt a coherent replacement carrying the same source IDs.
        f.db.update_conversation_state(&f.coordinator, &coherent_opposite)
            .await
            .unwrap();
        assert!(f.apply(&command).await.is_err());
        assert_one_operation(&f, &receipt, 1).await;
    }
}

#[tokio::test]
async fn coherent_live_and_durable_opposite_wire_shapes_refuse_both_directions() {
    for legacy in [false, true] {
        let f = Fixture::new().await;
        let intent = NativeCloseIntent::Initial {
            target: f.target.clone(),
        };
        let flat = intent.legacy_source_input();
        let wrapped = intent.source_input();
        let (input, opposite) = if legacy {
            (flat, wrapped)
        } else {
            (wrapped, flat)
        };
        let command = NativeCloseCommand::new_with_source_input(
            f.coordinator.clone(),
            f.source.clone(),
            intent.clone(),
            &input,
        )
        .unwrap();
        let matching = crate::native_close::tests::executing_state(
            &f.source,
            input.clone(),
            input.clone(),
        );
        f.db.update_conversation_state(&f.coordinator, &matching)
            .await
            .unwrap();
        let NativeCloseInitialResult::NewlyAdmitted(receipt) = f.apply(&command).await.unwrap() else {
            panic!("expected admission");
        };
        let opposite_state = crate::native_close::tests::executing_state(
            &f.source,
            opposite.clone(),
            opposite.clone(),
        );
        // Durable source matches the command; coherent live source does not.
        let mut tx = f.db.pool().begin().await.unwrap();
        assert!(require_initial_identity_tx(&mut tx, &command, &receipt, &opposite_state)
            .await
            .is_err());
        assert_eq!(
            super::super::receipts::read_receipt_tx(&mut tx, &command)
                .await
                .unwrap(),
            Some(receipt.admitted.clone())
        );
        tx.rollback().await.unwrap();

        // Bypass the fence deliberately to test durable authority independently.
        sqlx::query("UPDATE conversations SET state = ?2, state_kind = ?3 WHERE id = ?1")
            .bind(&f.coordinator)
            .bind(serde_json::to_string(&opposite_state).unwrap())
            .bind(crate::conv_state_kind(&opposite_state))
            .execute(f.db.pool())
            .await
            .unwrap();
        let mut tx = f.db.pool().begin().await.unwrap();
        assert!(require_initial_identity_tx(&mut tx, &command, &receipt, &matching)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert!(f.apply(&command).await.is_err());
        let opposite_command = NativeCloseCommand::new_with_source_input(
            f.coordinator.clone(),
            f.source.clone(),
            intent,
            &opposite,
        )
        .unwrap();
        assert_eq!(opposite_command.request_id(), command.request_id());
        assert_ne!(opposite_command.fingerprint(), command.fingerprint());
        let mut tx = f.db.pool().begin().await.unwrap();
        assert!(super::super::receipts::read_receipt_tx(&mut tx, &opposite_command)
            .await
            .is_err());
        tx.rollback().await.unwrap();
    }
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

    let live_source = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let prepared = f
        .db
        .prepare_native_close_initial_execution(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(prepared.phase, ClosePhase::SettlingActiveWork);

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

#[tokio::test]
async fn try_settle_waits_for_captured_direct_turn_then_advances_after_exact_release() {
    let f = Fixture::new().await;
    let turn_id = source_turn(&f.db, f.target.as_str(), "try-settle-turn").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;

    assert!(matches!(
        f.db.try_settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(
            NativeCloseInitialSettlement::WaitingForQuiescence
        ))
    ));
    assert_eq!(initial_phase(&f, &receipt).await, "settling_active_work");

    assert_turn_unsettled(&f.db, turn_id).await;
    // Terminal-looking work with the wrong generation is still pending, not proof.
    sqlx::query(
        "UPDATE durable_turns SET generation = 2, terminal_kind = 'Completed', owns_conversation = 0
         WHERE turn_id = ?1",
    )
    .bind(i64::try_from(turn_id).unwrap())
    .execute(f.db.pool())
    .await
    .unwrap();
    assert_eq!(
        f.db.try_settle_native_close_initial(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap(),
        NativeCloseInitialSettlement::WaitingForQuiescence
    );

    sqlx::query("UPDATE durable_turns SET generation = 1 WHERE turn_id = ?1")
        .bind(i64::try_from(turn_id).unwrap())
        .execute(f.db.pool())
        .await
        .unwrap();
    assert_eq!(
        f.db.try_settle_native_close_initial(&command, &receipt, &live_source)
            .await
            .established()
            .unwrap()
            .unwrap(),
        NativeCloseInitialSettlement::Advanced(NativeCloseInitialExecution {
            run: receipt.admitted.clone(),
            phase: ClosePhase::AwaitingRetirementInspection,
        })
    );
    assert_one_operation(&f, &receipt, 0).await;
}

#[tokio::test]
async fn try_settle_rejects_stale_source_and_wrong_receipt_before_waiting_classification() {
    let f = Fixture::new().await;
    let _turn_id = source_turn(&f.db, f.target.as_str(), "try-settle-stale-turn").await;
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let mut wrong_receipt = receipt.clone();
    wrong_receipt.request_id = Uuid::new_v4();
    assert!(matches!(
        f.db.try_settle_native_close_initial(&command, &wrong_receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let stale_source = ConvState::Idle;
    assert!(matches!(
        f.db.try_settle_native_close_initial(&command, &receipt, &stale_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    assert!(matches!(
        f.db.try_settle_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
}

async fn initial_finalization_snapshot(f: &Fixture, receipt: &NativeCloseReceipt) -> Vec<String> {
    let attempt = receipt.admitted.attempt_id.as_str();
    let mut snapshot = failure_boundary_snapshot(&f.db, attempt).await;
    for (sql, bind) in [
        ("SELECT json_group_array(json_array(scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us)) FROM (SELECT scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us FROM close_retirement_resource_dispatches WHERE attempt_id = ?1 ORDER BY scope, resource_kind, identity_value)", attempt),
        ("SELECT json_group_array(json_array(id, archived, updated_at)) FROM (SELECT id, archived, updated_at FROM conversations WHERE id IN (SELECT conversation_id FROM close_attempt_participants WHERE attempt_id = ?1) ORDER BY id)", attempt),
        ("SELECT json_group_array(json_array(message_id, conversation_id, sequence_id, message_type, content, created_at)) FROM messages WHERE message_id = 'close-outcome:' || ?1", attempt),
        ("SELECT json_group_array(json_array(scope, resource_kind, identity_kind, identity_codec, identity_value, created_at, updated_at)) FROM (SELECT scope, resource_kind, identity_kind, identity_codec, identity_value, created_at, updated_at FROM close_retirement_resources WHERE attempt_id = ?1 ORDER BY scope, resource_kind, identity_kind, identity_codec, identity_value)", attempt),
        ("SELECT json_group_array(json_array(id, ordinary_lifecycle)) FROM product_conversations WHERE id = ?1", f.target.as_str()),
    ] {
        snapshot.push(
            sqlx::query_scalar(sql)
                .bind(bind)
                .fetch_one(f.db.pool())
                .await
                .unwrap(),
        );
    }
    snapshot
}

async fn failure_boundary_snapshot(db: &Database, attempt: &str) -> Vec<String> {
    let queries = [
        "SELECT json_group_array(json_array(phase, close_outcome, inspection_generation, inspection_fingerprint, created_at, updated_at, completed_at, native_managed)) FROM close_obligations WHERE attempt_id = ?1",
        "SELECT json_group_array(json_array(message_id, content)) FROM messages WHERE message_id = 'close-outcome:' || ?1",
        "SELECT json_group_array(json_array(run_ordinal, status, created_at_us, ended_at_us)) FROM (SELECT run_ordinal, status, created_at_us, ended_at_us FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal)",
        "SELECT json_group_array(json_array(scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail)) FROM (SELECT scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail FROM close_retirement_resources WHERE attempt_id = ?1 ORDER BY scope, resource_kind, identity_value)",
        "SELECT json_group_array(json_array(history_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, recorded_at)) FROM (SELECT history_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, recorded_at FROM close_retirement_resource_history WHERE attempt_id = ?1 ORDER BY history_id)",
        "SELECT json_group_array(json_array(failure_occurrence_id, cleanup_run_ordinal, source_product_conversation_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, reason, detail, stop_certainty, confirmed_at_us, occurred_at_us, authority_kind)) FROM (SELECT failure_occurrence_id, cleanup_run_ordinal, source_product_conversation_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, reason, detail, stop_certainty, confirmed_at_us, occurred_at_us, authority_kind FROM close_cleanup_failures WHERE attempt_id = ?1 ORDER BY cleanup_run_ordinal)",
        "SELECT json_group_array(json_array(failure_occurrence_id, ordinal, scope, resource_kind, identity_kind, identity_codec, identity_value, disposition)) FROM (SELECT child.failure_occurrence_id, child.ordinal, child.scope, child.resource_kind, child.identity_kind, child.identity_codec, child.identity_value, child.disposition FROM close_cleanup_failure_resources child JOIN close_cleanup_failures failure USING (failure_occurrence_id) WHERE failure.attempt_id = ?1 ORDER BY child.failure_occurrence_id, child.ordinal)",
        "SELECT json_group_array(json_array(event_id, route_kind, mandatory_failure_occurrence_id, occurred_at_us)) FROM (SELECT event.event_id, event.route_kind, event.mandatory_failure_occurrence_id, event.occurred_at_us FROM coordinator_watch_events event JOIN close_cleanup_failures failure ON failure.failure_occurrence_id = event.mandatory_failure_occurrence_id WHERE failure.attempt_id = ?1 ORDER BY event.event_id)",
    ];
    let mut snapshot = Vec::with_capacity(queries.len());
    for sql in queries {
        snapshot.push(
            sqlx::query_scalar(sql)
                .bind(attempt)
                .fetch_one(db.pool())
                .await
                .unwrap(),
        );
    }
    snapshot
}

#[tokio::test]
async fn native_initial_resource_failure_is_source_bound_and_replays_read_only() {
    check_initial_resource_failure(0).await;
    check_initial_resource_failure(1).await;
}

#[allow(clippy::too_many_lines)] // Keep fresh failure, replay and no-mutation checks together.
async fn check_initial_resource_failure(failed_index: usize) {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    assert!(expected.len() >= 2);
    let failed = &expected[failed_index];
    for successful in &expected[..failed_index] {
        record_initial_retirement(&f, &command, &receipt, &live_source, successful).await;
    }
    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, failed),
    )
    .await
    .established()
    .unwrap()
    .unwrap();

    let remaining = expected[failed_index..]
        .iter()
        .map(|item| crate::CloseCleanupFailureResource {
            scope: item.scope.clone(),
            resource: item.resource.clone(),
            disposition: if item.scope == failed.scope && item.resource == failed.resource {
                crate::close_foundation::CloseCleanupResourceDisposition::Failed
            } else {
                crate::close_foundation::CloseCleanupResourceDisposition::Unattempted
            },
        })
        .collect::<Vec<_>>();
    let request = crate::TerminalizeInitialCloseCleanupFailureRequest {
        failure_occurrence_id: phoenix_core::domain::close::CloseRunRef::initial(
            receipt.admitted.attempt_id.clone(),
        )
        .failure_occurrence_id(),
        attempt_id: receipt.admitted.attempt_id.clone(),
        source_product_conversation_id: f.target.clone(),
        authority: crate::CloseCleanupFailureAuthority::ExpectedResource {
            scope: failed.scope.clone(),
            snapshot: failed.snapshot.clone(),
            resource: failed.resource.clone(),
        },
        remaining_resources: remaining,
        reason: RetirementFailureReason::RemovalFailed,
        detail: "native initial cleanup failure".into(),
        stop_certainty: phoenix_core::domain::close::CloseStopCertainty::ShutdownUncertain,
        occurred_at_us: chrono::Utc::now().timestamp_micros(),
    };
    let before_refusals =
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await;
    let mut omitted = request.clone();
    omitted.remaining_resources.pop();
    let mut extra = request.clone();
    let duplicate = extra.remaining_resources[0].clone();
    extra.remaining_resources.push(duplicate);
    let mut wrong_disposition = request.clone();
    wrong_disposition.remaining_resources[0].disposition =
        crate::close_foundation::CloseCleanupResourceDisposition::Unattempted;
    let mut wrong_snapshot = request.clone();
    let crate::CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } =
        &mut wrong_snapshot.authority
    else {
        unreachable!()
    };
    *snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        format!("{}-wrong", snapshot.generation()),
        snapshot.fingerprint(),
    )
    .unwrap();
    let mut wrong_timestamp = request.clone();
    wrong_timestamp.occurred_at_us = -1;
    for invalid in [
        omitted,
        extra,
        wrong_disposition,
        wrong_snapshot,
        wrong_timestamp,
    ] {
        assert!(matches!(
            f.db.terminalize_native_close_initial_resource_failure(
                &command,
                &receipt,
                &live_source,
                invalid,
            )
            .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::CloseFoundationPrecondition(_)
            ))
        ));
        assert_eq!(
            failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
            before_refusals,
            "refused request must preserve evidence, history, failure children, outbox and run facts",
        );
    }
    assert!(matches!(
        f.db.terminalize_initial_close_cleanup_failure(&request)
            .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    assert_eq!(
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
        before_refusals,
        "generic failure route must not bypass native source authority",
    );

    sqlx::query(
        "CREATE TRIGGER reject_native_close_failure_outbox BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.route_kind = 'mandatory_close_failure'
         BEGIN SELECT RAISE(ABORT, 'injected mandatory outbox failure'); END",
    )
    .execute(f.db.pool())
    .await
    .unwrap();
    let rollback =
        f.db.terminalize_native_close_initial_resource_failure(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Err(error)) = rollback else {
        panic!("injected mandatory delivery failure must roll back");
    };
    assert!(error
        .to_string()
        .contains("injected mandatory outbox failure"));
    assert_eq!(
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
        before_refusals,
        "mandatory outbox failure must roll back failure, children and stopped run",
    );
    sqlx::query("DROP TRIGGER reject_native_close_failure_outbox")
        .execute(f.db.pool())
        .await
        .unwrap();

    let terminal =
        f.db.terminalize_native_close_initial_resource_failure(
            &command,
            &receipt,
            &live_source,
            request.clone(),
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(terminal.phase(), ClosePhase::Completed);
    let retained: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_cleanup_failures WHERE attempt_id = ?1 AND cleanup_run_ordinal = 1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(retained, 1);
    let before_replay =
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await;
    let stale_source = ConvState::Idle;
    assert!(matches!(
        f.db.terminalize_native_close_initial_resource_failure(
            &command,
            &receipt,
            &stale_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
        before_replay,
        "stale source must not mutate committed failure evidence",
    );
    let mut wrong_receipt = receipt.clone();
    wrong_receipt.request_id = Uuid::new_v4();
    assert!(matches!(
        f.db.terminalize_native_close_initial_resource_failure(
            &command,
            &wrong_receipt,
            &live_source,
            request.clone(),
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
        before_replay,
        "wrong receipt must not mutate committed failure evidence",
    );
    assert!(matches!(
        f.db.terminalize_initial_close_cleanup_failure(&request)
            .await,
        Err(DbError::CloseFoundationPrecondition(_))
    ));
    f.db.terminalize_native_close_initial_resource_failure(
        &command,
        &receipt,
        &live_source,
        request.clone(),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let stopped_before = initial_finalization_snapshot(&f, &receipt).await;
    assert!(matches!(
        f.db.complete_native_close_initial(&command, &receipt, &live_source)
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        initial_finalization_snapshot(&f, &receipt).await,
        stopped_before
    );
    let after_replay = failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await;
    assert_eq!(before_replay, after_replay);
    assert!(matches!(
        f.db.terminalize_native_close_initial_resource_failure(
            &command,
            &receipt,
            &live_source,
            crate::TerminalizeInitialCloseCleanupFailureRequest {
                detail: "conflicting replay".into(),
                ..request
            },
        )
        .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(
        failure_boundary_snapshot(&f.db, receipt.admitted.attempt_id.as_str()).await,
        before_replay,
    );
}

#[tokio::test]
// Check dispatch and live-source rejection against the same original worktree metadata.
#[allow(clippy::too_many_lines)]
async fn initial_worktree_metadata_requires_exact_original_dispatched_authority_and_live_source() {
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let plan = || crate::RecordCloseWorktreeCleanupPlanRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: worktree.scope.clone(),
        snapshot: worktree.snapshot.clone(),
        resource: worktree.resource.clone(),
        administrative_dir_incarnation: "initial-authority-test".into(),
        administrative_dir: std::path::PathBuf::from("/tmp/initial-authority-admin"),
    };

    let before_dispatch =
        f.db.close_worktree_cleanup_plan(
            &receipt.admitted.attempt_id,
            &worktree.scope,
            &worktree.snapshot,
            &worktree.resource,
        )
        .await
        .unwrap();
    assert!(before_dispatch.is_none());
    let (result, lease) = f
        .db
        .record_native_close_initial_worktree_cleanup_plan_with_entry(
            &command,
            &receipt,
            &live_source,
            plan(),
            || Ok(()),
        )
        .await;
    assert!(lease.is_none());
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert!(f
        .db
        .close_worktree_cleanup_plan(
            &receipt.admitted.attempt_id,
            &worktree.scope,
            &worktree.snapshot,
            &worktree.resource,
        )
        .await
        .unwrap()
        .is_none());

    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let wrong_snapshot = crate::RecordCloseWorktreeCleanupPlanRequest {
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot::parse(
            "foreign-generation",
            "foreign-fingerprint",
        )
        .unwrap(),
        ..plan()
    };
    let (result, lease) = f
        .db
        .record_native_close_initial_worktree_cleanup_plan_with_entry(
            &command,
            &receipt,
            &live_source,
            wrong_snapshot,
            || Ok(()),
        )
        .await;
    assert!(lease.is_none());
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert!(f
        .db
        .close_worktree_cleanup_plan(
            &receipt.admitted.attempt_id,
            &worktree.scope,
            &worktree.snapshot,
            &worktree.resource,
        )
        .await
        .unwrap()
        .is_none());

    let (result, lease) = f
        .db
        .record_native_close_initial_worktree_cleanup_plan_with_entry(
            &command,
            &receipt,
            &live_source,
            plan(),
            || Ok(()),
        )
        .await;
    assert!(lease.is_some());
    result.established().unwrap().unwrap();
    assert!(f
        .db
        .close_worktree_cleanup_plan(
            &receipt.admitted.attempt_id,
            &worktree.scope,
            &worktree.snapshot,
            &worktree.resource,
        )
        .await
        .unwrap()
        .is_some());

    f.db.update_conversation_state(&f.coordinator, &ConvState::Idle)
        .await
        .unwrap();
    let stopped_plan = crate::RecordCloseWorktreeCleanupPlanRequest {
        administrative_dir_incarnation: "must-not-write".into(),
        ..plan()
    };
    let (result, lease) = f
        .db
        .record_native_close_initial_worktree_cleanup_plan_with_entry(
            &command,
            &receipt,
            &live_source,
            stopped_plan,
            || Ok(()),
        )
        .await;
    assert!(lease.is_none());
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    let retained =
        f.db.close_worktree_cleanup_plan(
            &receipt.admitted.attempt_id,
            &worktree.scope,
            &worktree.snapshot,
            &worktree.resource,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.administrative_dir_incarnation,
        "initial-authority-test"
    );
}

#[tokio::test]
async fn initial_work_scope_retirement_rejects_unresolved_originals_then_commits_proof_atomically()
{
    let f = Fixture::new().await;
    let (command, receipt, live_source, expected) = captured_initial_retirement(&f).await;
    let worktree = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap();
    let work_scope = expected
        .iter()
        .find(|item| item.resource.kind() == RetiredResourceKind::WorkScope)
        .unwrap();
    let precondition = || {
        phoenix_core::work_scope::WorkScopeRetirementPrecondition::
        after_runtime_inventory_found_no_live_resource(work_scope.scope.clone())
    };
    let lifecycle = || async {
        sqlx::query_scalar::<_, String>("SELECT lifecycle FROM work_scopes WHERE id = ?1")
            .bind(work_scope.scope.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap()
    };

    f.db.record_native_close_initial_retirement_dispatch(
        &command,
        &receipt,
        &live_source,
        initial_dispatch_request(&receipt, worktree),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let scope_dispatches = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM close_retirement_resource_dispatches
             WHERE attempt_id = ?1 AND scope = ?2 AND resource_kind = 'work_scope'",
        )
        .bind(receipt.admitted.attempt_id.as_str())
        .bind(work_scope.scope.as_str())
        .fetch_one(f.db.pool())
        .await
        .unwrap()
    };
    assert_eq!(lifecycle().await, "active");
    let unresolved =
        f.db.retire_work_scope_for_native_close_initial(
            &command,
            &receipt,
            &live_source,
            work_scope.scope.clone(),
            work_scope.snapshot.clone(),
            work_scope.resource.clone(),
            precondition(),
            "retire only after original resources resolve",
        )
        .await;
    assert!(matches!(
        unresolved,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ));
    assert_eq!(lifecycle().await, "active");
    let scope_proofs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1
         AND scope = ?2 AND resource_kind = 'work_scope'",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .bind(work_scope.scope.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(scope_proofs, 0);
    assert_eq!(scope_dispatches().await, 0);

    record_initial_retirement(&f, &command, &receipt, &live_source, worktree).await;
    let retired =
        f.db.retire_work_scope_for_native_close_initial(
            &command,
            &receipt,
            &live_source,
            work_scope.scope.clone(),
            work_scope.snapshot.clone(),
            work_scope.resource.clone(),
            precondition(),
            "original resources resolved",
        )
        .await;
    assert!(matches!(
        retired,
        LocalAuthorityResult::DurableFactEstablished(Ok(
            phoenix_core::work_scope::WorkScopeRetirementOutcome::Retired
        ))
    ));
    assert_eq!(lifecycle().await, "retired");
    assert_eq!(scope_dispatches().await, 1);
    let proof: (String, Option<String>) = sqlx::query_as(
        "SELECT proof_kind, detail FROM close_retirement_resources
         WHERE attempt_id = ?1 AND scope = ?2 AND resource_kind = 'work_scope'",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .bind(work_scope.scope.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(proof.0, "retired");
    assert_eq!(proof.1.as_deref(), Some("original resources resolved"));
}

#[tokio::test]
async fn native_initial_control_route_confirms_stop_and_captures_settlement_targets() {
    let f = Fixture::new().await;
    let source_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(f.target.as_str()).fetch_one(f.db.pool()).await.unwrap();
    let turn = source_turn(&f.db, f.target.as_str(), "native-control-turn").await;
    f.db.update_conversation_state(&latest, &source_state).await.unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let latest = TranscriptConversationId::parse(latest).unwrap();
    let CloseTargetControlResolution::ParkedNativeInitial(route) = f
        .db
        .resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
        .await
        .unwrap()
    else {
        panic!("expected parked Initial source route");
    };
    assert_eq!(route.run_ordinal, CloseRunOrdinal::INITIAL);
    let result = f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::ConfirmStopWork,
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(obligation)) = result else {
        panic!("expected confirmed stop settlement: {result:?}");
    };
    assert_eq!(obligation.phase(), phoenix_core::domain::close::ClosePhase::SettlingActiveWork);
    assert_eq!(
        f.db.resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
            .await
            .unwrap(),
        CloseTargetControlResolution::NativeInitialSettlementInProgress,
    );
    let wrong_transcript = TranscriptConversationId::parse(f.coordinator.clone()).unwrap();
    assert!(f
        .db
        .resolve_close_target_control(&f.target, &wrong_transcript, &receipt.admitted.attempt_id)
        .await
        .is_err());
    let captures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_attempt_direct_turn_settlement_captures WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(captures, 1);
    let captured = f.db.list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await.unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].turn_id, turn);
    assert_eq!(captured[0].conversation_id, latest.as_str());
}

#[tokio::test]
async fn native_initial_control_cancel_terminalizes_only_unencumbered_stop_confirmation() {
    let f = Fixture::new().await;
    let source_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(f.target.as_str()).fetch_one(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&latest, &source_state).await.unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let latest = TranscriptConversationId::parse(latest).unwrap();
    let CloseTargetControlResolution::ParkedNativeInitial(route) = f
        .db
        .resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
        .await
        .unwrap()
    else {
        panic!("expected parked Initial source route");
    };
    let result = f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::Cancel,
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(obligation)) = result else {
        panic!("expected cancellation: {result:?}");
    };
    assert_eq!(obligation.phase(), phoenix_core::domain::close::ClosePhase::Completed);
    assert_eq!(obligation.close_outcome(), Some(phoenix_core::domain::close::CloseCompletionOutcome::Cancelled));
    let (status, created_at_us, ended_at_us): (String, i64, Option<i64>) = sqlx::query_as(
        "SELECT status, created_at_us, ended_at_us FROM close_runs
         WHERE attempt_id = ?1 AND run_ordinal = 1",
    )
    .bind(receipt.admitted.attempt_id.as_str())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(status, "completed");
    assert!(ended_at_us.is_some_and(|ended_at| ended_at >= created_at_us));
    assert_no_settlement_effects(&f.db, &receipt).await;
    assert!(f.db.resolve_close_target_control(
        &f.target, &latest, &receipt.admitted.attempt_id,
    ).await.is_err());
}

#[tokio::test]
async fn native_initial_control_refuses_cancel_after_stop_settlement_begins() {
    let f = Fixture::new().await;
    let source_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(f.target.as_str()).fetch_one(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&latest, &source_state).await.unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let latest = TranscriptConversationId::parse(latest).unwrap();
    let CloseTargetControlResolution::ParkedNativeInitial(route) = f
        .db
        .resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
        .await
        .unwrap()
    else {
        panic!("expected parked Initial source route");
    };
    let confirmed = f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::ConfirmStopWork,
    ).await;
    assert!(matches!(confirmed, LocalAuthorityResult::DurableFactEstablished(Ok(_))));
    let cancelled = f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::Cancel,
    ).await;
    assert!(matches!(cancelled, LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))));
}

#[tokio::test]
async fn settlement_dispatch_no_actor_terminalizes_only_the_captured_target() {
    let f = Fixture::new().await;
    let turn_id = source_turn(&f.db, f.target.as_str(), "settlement-no-actor-turn").await;
    let source_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(f.target.as_str()).fetch_one(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&latest, &source_state).await.unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let latest = TranscriptConversationId::parse(latest).unwrap();
    let CloseTargetControlResolution::ParkedNativeInitial(route) = f
        .db
        .resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
        .await
        .unwrap()
    else {
        panic!("expected parked Initial source route");
    };
    f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::ConfirmStopWork,
    ).await.established().unwrap().unwrap();
    let targets = f
        .db
        .prepare_native_close_initial_settlement_dispatch(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].turn_id, turn_id);

    let mut stale = targets[0].clone();
    stale.expected_generation += 1;
    let rejected = f.db.dispatch_native_close_initial_settlement_turn::<()>(
        &command, &receipt, &live_source, &stale,
        || panic!("stale generation must be refused before enqueue"),
    ).await.established().unwrap();
    assert!(matches!(rejected, Err(DbError::CloseFoundationPrecondition(_))));
    assert_turn_unsettled(&f.db, turn_id).await;

    let mut callback_called = false;
    assert_eq!(
        f.db.dispatch_native_close_initial_settlement_turn::<()>(
            &command,
            &receipt,
            &live_source,
            &targets[0],
            || {
                callback_called = true;
                NativeCloseInitialTurnDispatch::NoLiveActor
            },
        )
        .await
        .established()
        .unwrap()
        .unwrap(),
        NativeCloseInitialTurnDispatchResult::Released
    );
    assert!(callback_called);
    let (generation, terminal, owns_conversation): (i64, Option<String>, i64) = sqlx::query_as(
        "SELECT generation, terminal_kind, owns_conversation FROM durable_turns WHERE turn_id = ?1",
    )
    .bind(i64::try_from(turn_id).unwrap())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(generation, 1);
    assert_eq!(terminal.as_deref(), Some("Cancelled"));
    assert_eq!(owns_conversation, 0);
    assert!(f.db.list_unsettled_close_direct_turn_settlement_targets(receipt.admitted.attempt_id.as_str())
        .await.unwrap().is_empty());
}

#[tokio::test]
async fn settlement_stop_claim_rejects_revoked_source_and_competing_delivery() {
    let f = Fixture::new().await;
    let turn_id = source_turn(&f.db, f.target.as_str(), "settlement-no-actor-turn").await;
    let source_state = f.db.get_conversation(&f.coordinator).await.unwrap().state;
    let latest: String = sqlx::query_scalar(
        "SELECT id FROM conversations WHERE product_conversation_id = ?1 AND parent_conversation_id IS NULL",
    ).bind(f.target.as_str()).fetch_one(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&latest, &source_state).await.unwrap();
    let (command, receipt, live_source) = prepared_initial(&f).await;
    let latest = TranscriptConversationId::parse(latest).unwrap();
    let CloseTargetControlResolution::ParkedNativeInitial(route) = f
        .db
        .resolve_close_target_control(&f.target, &latest, &receipt.admitted.attempt_id)
        .await
        .unwrap()
    else {
        panic!("expected parked Initial source route");
    };
    f.db.apply_native_initial_close_control(
        &command, &receipt, &live_source, &route, super::super::NativeCloseInitialControl::ConfirmStopWork,
    ).await.established().unwrap().unwrap();
    let targets = f
        .db
        .prepare_native_close_initial_settlement_dispatch(&command, &receipt, &live_source)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].turn_id, turn_id);

    let target = &targets[0];
    let (revoked, lease) = f.db.claim_native_close_initial_settlement_stop(
        &command, &receipt, &live_source, target,
        || Err::<(), _>(DbError::CloseFoundationPrecondition("source revoked while queued".into())),
    ).await;
    assert!(lease.is_none());
    assert!(matches!(revoked, LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))));
    let claim = || f.db.claim_native_close_initial_settlement_stop(
        &command, &receipt, &live_source, target, || Ok(()),
    );
    let ((a, _), (b, _)) = tokio::join!(claim(), claim());
    assert_ne!(a.established().unwrap().unwrap(), b.established().unwrap().unwrap());
    let (duplicate, lease) = f.db.claim_native_close_initial_settlement_stop::<()>(
        &command, &receipt, &live_source, target, || panic!("duplicate must not enter the source"),
    ).await;
    assert!(!duplicate.established().unwrap().unwrap());
    assert!(lease.is_none());
    let fallback = f.db.dispatch_native_close_initial_settlement_turn::<()>(
        &command, &receipt, &live_source, target, || NativeCloseInitialTurnDispatch::NoLiveActor,
    ).await.established().unwrap();
    assert!(matches!(fallback, Err(DbError::CloseFoundationPrecondition(_))));
    assert_turn_unsettled(&f.db, turn_id).await;
}
