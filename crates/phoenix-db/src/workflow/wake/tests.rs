use std::{str::FromStr, time::Duration};

use chrono::{TimeZone, Utc};
use phoenix_workflow::{
    wake_profile::{
        BashResourceIdentity, BashTerminalEvidence, BashTerminalStatus, TmuxResourceIdentity,
        TmuxTerminalEvidence, TmuxTerminalStatus, WakeRegistrationIntent, WakeResourceIdentity,
        WakeTerminalEvidence, WakeTerminalPayload, WorkScopeIdentity, WorkScopeKind,
    },
    Timestamp,
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};

use super::*;
use crate::run_pending_migrations;

fn scope() -> WorkScopeIdentity {
    WorkScopeIdentity {
        kind: WorkScopeKind::Conversation,
        stable_key: "conv-wake".to_owned(),
    }
}

fn bash() -> WakeResourceIdentity {
    WakeResourceIdentity::Bash(BashResourceIdentity {
        work_scope: scope(),
        handle_id: "bash-handle".to_owned(),
    })
}

fn tmux() -> WakeResourceIdentity {
    WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
        work_scope: scope(),
        server_generation: "tmux-generation".to_owned(),
        window_id: "@7".to_owned(),
    })
}

async fn pool() -> SqlitePool {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE conversations (id TEXT PRIMARY KEY, slug TEXT UNIQUE, cwd TEXT NOT NULL DEFAULT '/tmp', parent_conversation_id TEXT, user_initiated BOOLEAN NOT NULL DEFAULT 1, state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', created_at TEXT NOT NULL DEFAULT '2025-01-01', updated_at TEXT NOT NULL DEFAULT '2025-01-01', archived BOOLEAN NOT NULL DEFAULT 0, model TEXT, conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', steering_queue TEXT NOT NULL DEFAULT '[]');
         CREATE TABLE messages (message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, sequence_id INTEGER NOT NULL, message_type TEXT NOT NULL, content TEXT NOT NULL, display_data TEXT, usage_data TEXT, created_at TEXT NOT NULL, FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE);
         INSERT INTO conversations (id, slug) VALUES ('conv-wake', 'conv-wake');",
    )
    .execute(&pool)
    .await
    .unwrap();
    run_pending_migrations(&pool).await.unwrap();
    pool
}

fn registration(resource: WakeResourceIdentity) -> WakeRegistrationRequest {
    let accepted_at = Utc.timestamp_opt(1_000, 0).single().unwrap();
    WakeRegistrationRequest {
        idempotency_key: "register-key".to_owned(),
        intent_fingerprint: format!("fingerprint-{resource:?}"),
        workflow_id: "wake-workflow".to_owned(),
        transition_id: "wake-transition".to_owned(),
        binding_id: "wake-binding".to_owned(),
        authority_scope: "conversation:conv-wake".to_owned(),
        intent: WakeRegistrationIntent {
            contract_id: "wake-contract".to_owned(),
            conversation_id: "conv-wake".to_owned(),
            registration_scope: scope(),
            resource,
            registering_tool_use_id: "tool-use".to_owned(),
            registered_at: Timestamp(1_000),
            expires_at: Timestamp(1_100),
        },
        fence_version: 1,
        accepted_at,
    }
}

async fn registered(resource: WakeResourceIdentity) -> (SqlitePool, WorkflowRepository) {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let result = WakeWorkflowAdapter::new(&repo)
        .register(&registration(resource))
        .await
        .unwrap();
    assert!(matches!(result, WakeRegistrationResult::New { .. }));
    (pool, repo)
}

async fn claimed(repo: &WorkflowRepository) -> ClaimedWakeEffect {
    let adapter = WakeWorkflowAdapter::new(repo);
    let now = Utc.timestamp_opt(1_001, 0).single().unwrap();
    let due = adapter.due(now).await.unwrap();
    assert_eq!(due.len(), 1);
    adapter
        .claim(
            &due[0],
            "claim-token".to_owned(),
            "wake-worker".to_owned(),
            now,
            Utc.timestamp_opt(1_050, 0).single().unwrap(),
        )
        .await
        .unwrap()
        .expect("wake effect claimed")
}

async fn terminalize_bash(
    repo: &WorkflowRepository,
    claim: ClaimedWakeEffect,
    workflow_id: &str,
    contract_id: &str,
    receipt_id: &str,
    inbox_id: &str,
) {
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match bash() {
            WakeResourceIdentity::Bash(identity) => identity,
            WakeResourceIdentity::TmuxWindow(_) | WakeResourceIdentity::Subagent(_) => {
                unreachable!()
            }
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(1_020),
        exit_code: Some(0),
        duration_ms: Some(20),
        signal_number: None,
        kill_signal_sent: None,
        tail_start_offset: 0,
        tail_end_offset: 0,
        tail_truncated_before: false,
        tail_offsets: vec![],
        final_tail: vec![],
    });
    assert!(matches!(
        WakeWorkflowAdapter::new(repo)
            .record_terminal_evidence(
                &WakeObservationRequest {
                    observation_id: format!("observation-{workflow_id}"),
                    authority: claim.authority.clone(),
                    attempt_id: claim.attempt_id.clone(),
                    evidence: evidence.clone(),
                    recorded_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
                },
                &WakeTerminalReceiptRequest {
                    receipt_id: receipt_id.to_owned(),
                    reducer_inbox_id: inbox_id.to_owned(),
                    authority: claim.authority,
                    attempt_id: claim.attempt_id,
                    terminal: WakeTerminalPayload::Fired {
                        contract_id: contract_id.to_owned(),
                        resource: bash(),
                        evidence,
                        resolved_at: Timestamp(1_020),
                    },
                    accepted_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
                    origin: DurableReceiptOrigin::Execution,
                },
            )
            .await
            .unwrap(),
        AcceptReceiptResult::Accepted { .. }
    ));
}

#[tokio::test]
async fn terminal_evidence_rejects_a_fired_receipt_with_different_evidence_atomically() {
    let (pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match bash() {
            WakeResourceIdentity::Bash(identity) => identity,
            WakeResourceIdentity::TmuxWindow(_) | WakeResourceIdentity::Subagent(_) => {
                unreachable!()
            }
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(1_020),
        exit_code: Some(0),
        duration_ms: Some(20_000),
        signal_number: None,
        kill_signal_sent: None,
        tail_start_offset: 0,
        tail_end_offset: 1,
        tail_truncated_before: false,
        tail_offsets: vec![0],
        final_tail: vec!["observed".to_owned()],
    });
    let mut receipt_evidence = evidence.clone();
    let WakeTerminalEvidence::Bash(receipt_bash_evidence) = &mut receipt_evidence else {
        unreachable!()
    };
    receipt_bash_evidence.final_tail = vec!["different".to_owned()];

    let result = WakeWorkflowAdapter::new(&repo)
        .record_terminal_evidence(
            &WakeObservationRequest {
                observation_id: "mismatched-observation".to_owned(),
                authority: claim.authority.clone(),
                attempt_id: claim.attempt_id.clone(),
                evidence,
                recorded_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
            },
            &WakeTerminalReceiptRequest {
                receipt_id: "mismatched-receipt".to_owned(),
                reducer_inbox_id: "mismatched-inbox".to_owned(),
                authority: claim.authority,
                attempt_id: claim.attempt_id,
                terminal: WakeTerminalPayload::Fired {
                    contract_id: "wake-contract".to_owned(),
                    resource: bash(),
                    evidence: receipt_evidence,
                    resolved_at: Timestamp(1_021),
                },
                accepted_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
                origin: DurableReceiptOrigin::Execution,
            },
        )
        .await
        .unwrap();

    assert_eq!(result, AcceptReceiptResult::StaleAuthority);
    let writes: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM workflow_observations WHERE id = 'mismatched-observation') \
              + (SELECT COUNT(*) FROM workflow_receipts WHERE id = 'mismatched-receipt') \
              + (SELECT COUNT(*) FROM workflow_reducer_inbox WHERE id = 'mismatched-inbox')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(writes, 0);
}

#[tokio::test]
async fn terminal_evidence_requires_a_fired_receipt() {
    let (_pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match bash() {
            WakeResourceIdentity::Bash(identity) => identity,
            WakeResourceIdentity::TmuxWindow(_) | WakeResourceIdentity::Subagent(_) => {
                unreachable!()
            }
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(1_020),
        exit_code: Some(0),
        duration_ms: None,
        signal_number: None,
        kill_signal_sent: None,
        tail_start_offset: 0,
        tail_end_offset: 0,
        tail_truncated_before: false,
        tail_offsets: Vec::new(),
        final_tail: Vec::new(),
    });
    let result = WakeWorkflowAdapter::new(&repo)
        .record_terminal_evidence(
            &WakeObservationRequest {
                observation_id: "observation".to_owned(),
                authority: claim.authority.clone(),
                attempt_id: claim.attempt_id.clone(),
                evidence,
                recorded_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
            },
            &WakeTerminalReceiptRequest {
                receipt_id: "receipt".to_owned(),
                reducer_inbox_id: "inbox".to_owned(),
                authority: claim.authority,
                attempt_id: claim.attempt_id,
                terminal: WakeTerminalPayload::Expired {
                    contract_id: "wake-contract".to_owned(),
                    resource: bash(),
                    resolved_at: Timestamp(1_021),
                },
                accepted_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
                origin: DurableReceiptOrigin::Execution,
            },
        )
        .await
        .unwrap();
    assert_eq!(result, AcceptReceiptResult::StaleAuthority);
}

#[tokio::test]
async fn fired_evidence_after_expiry_is_rejected_in_atomic_and_direct_receipt_paths() {
    let (pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match bash() {
            WakeResourceIdentity::Bash(identity) => identity,
            WakeResourceIdentity::TmuxWindow(_) | WakeResourceIdentity::Subagent(_) => {
                unreachable!()
            }
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(1_101),
        exit_code: Some(0),
        duration_ms: None,
        signal_number: None,
        kill_signal_sent: None,
        tail_start_offset: 0,
        tail_end_offset: 0,
        tail_truncated_before: false,
        tail_offsets: vec![],
        final_tail: vec![],
    });
    let receipt = WakeTerminalReceiptRequest {
        receipt_id: "late-receipt".to_owned(),
        reducer_inbox_id: "late-inbox".to_owned(),
        authority: claim.authority.clone(),
        attempt_id: claim.attempt_id.clone(),
        terminal: WakeTerminalPayload::Fired {
            contract_id: "wake-contract".to_owned(),
            resource: bash(),
            evidence: evidence.clone(),
            resolved_at: Timestamp(1_101),
        },
        accepted_at: Utc.timestamp_opt(1_101, 0).single().unwrap(),
        origin: DurableReceiptOrigin::Execution,
    };
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert_eq!(
        adapter
            .record_terminal_evidence(
                &WakeObservationRequest {
                    observation_id: "late-observation".to_owned(),
                    authority: claim.authority,
                    attempt_id: claim.attempt_id,
                    evidence,
                    recorded_at: Utc.timestamp_opt(1_101, 0).single().unwrap(),
                },
                &receipt,
            )
            .await
            .unwrap(),
        AcceptReceiptResult::StaleAuthority
    );
    assert_eq!(
        adapter.accept_terminal_receipt(&receipt).await.unwrap(),
        AcceptReceiptResult::StaleAuthority
    );
    let writes: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM workflow_observations WHERE id = 'late-observation') \
              + (SELECT COUNT(*) FROM workflow_receipts WHERE id = 'late-receipt')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(writes, 0);
}

#[tokio::test]
async fn continuation_owner_transfer_requires_the_persisted_edge_and_replays() {
    let (pool, repo) = registered(bash()).await;
    sqlx::query("INSERT INTO conversations (id, slug) VALUES ('successor', 'successor'), ('unrelated', 'unrelated')")
        .execute(&pool)
        .await
        .unwrap();
    let adapter = WakeWorkflowAdapter::new(&repo);
    let error = adapter
        .transfer_conversation_owner("conv-wake", "unrelated")
        .await
        .unwrap_err();
    assert!(matches!(error, WorkflowRepositoryError::CorruptState(_)));
    assert!(adapter
        .has_pending_for_conversation("conv-wake")
        .await
        .unwrap());

    sqlx::query(
        "UPDATE conversations SET continued_in_conv_id = 'successor' WHERE id = 'conv-wake'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        adapter
            .transfer_conversation_owner("conv-wake", "successor")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        adapter
            .transfer_conversation_owner("conv-wake", "successor")
            .await
            .unwrap(),
        0
    );
    assert!(!adapter
        .has_pending_for_conversation("conv-wake")
        .await
        .unwrap());
    assert!(adapter
        .has_pending_for_conversation("successor")
        .await
        .unwrap());
}

#[tokio::test]
async fn pending_lookup_and_scope_rekey_follow_live_resource_ownership() {
    let (pool, repo) = registered(bash()).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert!(adapter
        .has_pending_for_conversation("conv-wake")
        .await
        .unwrap());
    let worktree = WorkScopeIdentity {
        kind: WorkScopeKind::Worktree,
        stable_key: "/repo/worktree".to_owned(),
    };
    assert_eq!(
        adapter
            .rekey_scope_for_conversation("conv-wake", &scope(), &worktree)
            .await
            .unwrap(),
        1
    );
    let row = sqlx::query(
        "SELECT registration_scope_kind, registration_scope_stable_key, bash_work_scope_kind, bash_work_scope_stable_key FROM wake_workflow_bindings WHERE contract_id = 'wake-contract'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "worktree");
    assert_eq!(row.get::<String, _>(1), "/repo/worktree");
    assert_eq!(row.get::<String, _>(2), "worktree");
    assert_eq!(row.get::<String, _>(3), "/repo/worktree");
}

#[tokio::test]
async fn concurrent_exact_protocol_selection_ensure_is_idempotent() {
    let pool = pool().await;
    let first_repo = WorkflowRepository::new(pool.clone());
    let second_repo = WorkflowRepository::new(pool.clone());
    let first = WakeWorkflowAdapter::new(&first_repo);
    let second = WakeWorkflowAdapter::new(&second_repo);
    let at = Utc.timestamp_opt(1_000, 0).single().unwrap();
    let (left, right) = tokio::join!(
        first.ensure_protocol_selection(at),
        second.ensure_protocol_selection(at)
    );
    left.unwrap();
    right.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workflow_protocol_selections WHERE id = ?1"
        )
        .bind(SELECTION_ID)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workflow_profile_codecs WHERE selection_id = ?1"
        )
        .bind(SELECTION_ID)
        .fetch_one(&pool)
        .await
        .unwrap(),
        6
    );
}

#[tokio::test]
async fn ensure_protocol_selection_atomically_replaces_existing_accepting_selection() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let competing = DurableProtocolSelectionRegistration {
        selection_id: "wake-competing".to_owned(),
        profile_id: phoenix_workflow::wake_profile::PROFILE_ID.to_owned(),
        selector_identity: "competing-selector".to_owned(),
        selector_version: 9,
        protocol_version: phoenix_workflow::wake_profile::PROTOCOL_VERSION,
        authority: phoenix_workflow::SemanticAuthority::EngineProtocol,
        accepting: true,
        runtime_acceptance_enabled: true,
        external_acceptance_enabled: true,
        registered_at: Utc.timestamp_opt(900, 0).single().unwrap(),
        drained_at: None,
        supported_codecs: vec![],
        executor_kinds: vec![],
    };
    repo.register_protocol_selection(&competing).await.unwrap();

    WakeWorkflowAdapter::new(&repo)
        .ensure_protocol_selection(Utc.timestamp_opt(1_000, 0).single().unwrap())
        .await
        .unwrap();

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT id, accepting, selector_version FROM workflow_protocol_selections ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("wake-competing".to_owned(), 0, 9),
            (SELECTION_ID.to_owned(), 1, 1),
        ]
    );
}

#[tokio::test]
async fn same_id_with_incompatible_exact_selection_fails() {
    let pool = pool().await;
    let repo = WorkflowRepository::new(pool.clone());
    let mut incompatible = DurableProtocolSelectionRegistration {
        selection_id: SELECTION_ID.to_owned(),
        profile_id: phoenix_workflow::wake_profile::PROFILE_ID.to_owned(),
        selector_identity: SELECTOR_IDENTITY.to_owned(),
        selector_version: 1,
        protocol_version: phoenix_workflow::wake_profile::PROTOCOL_VERSION,
        authority: phoenix_workflow::SemanticAuthority::EngineProtocol,
        accepting: true,
        runtime_acceptance_enabled: true,
        external_acceptance_enabled: true,
        registered_at: Utc.timestamp_opt(900, 0).single().unwrap(),
        drained_at: None,
        supported_codecs: vec![],
        executor_kinds: vec![],
    };
    incompatible.selector_version = 2;
    repo.register_protocol_selection(&incompatible)
        .await
        .unwrap();
    assert!(matches!(
        WakeWorkflowAdapter::new(&repo)
            .ensure_protocol_selection(Utc.timestamp_opt(1_000, 0).single().unwrap())
            .await,
        Err(WorkflowRepositoryError::ProtocolSelectionIncompatible { .. })
    ));
}

#[tokio::test]
async fn registration_selects_protocol_is_retryable_and_installs_exact_deadline_intent() {
    let (pool, repo) = registered(bash()).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    let replay = adapter.register(&registration(bash())).await.unwrap();
    let WakeRegistrationResult::Replay { receipt, .. } = replay else {
        panic!("same external request must replay");
    };
    assert!(receipt.payload.contains("register-key"));
    assert!(receipt.payload.contains("1100"));

    let selection = sqlx::query(
        "SELECT external_acceptance_enabled, runtime_acceptance_enabled FROM workflow_protocol_selections WHERE id = ?1",
    )
    .bind(SELECTION_ID)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(selection.get::<i64, _>("external_acceptance_enabled"), 1);
    assert_eq!(selection.get::<i64, _>("runtime_acceptance_enabled"), 1);

    let effect = sqlx::query(
        "SELECT kind, role, ambiguity_policy, intent_payload, status FROM workflow_effects WHERE workflow_id = 'wake-workflow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(effect.get::<String, _>("kind"), "observe_handle");
    assert_eq!(effect.get::<String, _>("role"), "required");
    assert_eq!(
        effect.get::<String, _>("ambiguity_policy"),
        "observable_reconciliation"
    );
    assert_eq!(effect.get::<String, _>("status"), "eligible");
    assert!(effect.get::<String, _>("intent_payload").contains("1100"));

    let mut conflict = registration(bash());
    conflict.intent_fingerprint = "different-intent".to_owned();
    assert_eq!(
        adapter.register(&conflict).await.unwrap(),
        WakeRegistrationResult::Conflict
    );
    let workflow_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflows")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(workflow_count, 1);
}

#[tokio::test]
async fn registration_fence_is_a_shared_compare_and_increment_gate() {
    let (pool, repo) = registered(bash()).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM wake_registration_fences WHERE conversation_id = 'conv-wake'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );

    let mut stale = registration(tmux());
    stale.idempotency_key = "stale-key".to_owned();
    stale.intent_fingerprint = "stale-fingerprint".to_owned();
    stale.workflow_id = "stale-workflow".to_owned();
    stale.transition_id = "stale-transition".to_owned();
    stale.binding_id = "stale-binding".to_owned();
    stale.intent.contract_id = "stale-contract".to_owned();
    assert_eq!(
        adapter.register(&stale).await.unwrap(),
        WakeRegistrationResult::Retryable
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflows")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );

    stale.fence_version = 2;
    assert!(matches!(
        adapter.register(&stale).await.unwrap(),
        WakeRegistrationResult::New { .. }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM wake_registration_fences WHERE conversation_id = 'conv-wake'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        3
    );
}

#[tokio::test]
async fn registration_replay_accepts_progressed_effect_state() {
    let (pool, repo) = registered(bash()).await;
    sqlx::query(
        "UPDATE workflow_effects SET status = 'retry_wait', next_eligible_at = '1970-01-01T00:18:00+00:00' \
         WHERE id = 'wake-observe:wake-workflow'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        WakeWorkflowAdapter::new(&repo)
            .register(&registration(bash()))
            .await
            .unwrap(),
        WakeRegistrationResult::Replay { .. }
    ));
}

#[tokio::test]
async fn registration_replay_uses_stable_intent_not_fresh_acceptance_time() {
    let (_pool, repo) = registered(bash()).await;
    let mut retry = registration(bash());
    retry.accepted_at = Utc.timestamp_opt(9_999, 0).single().unwrap();
    assert!(matches!(
        WakeWorkflowAdapter::new(&repo)
            .register(&retry)
            .await
            .unwrap(),
        WakeRegistrationResult::Replay { .. }
    ));
}

#[tokio::test]
async fn registration_replay_repairs_each_missing_graph_component() {
    for (table, delete_sql, count_sql) in [
        (
            "wake_workflow_bindings",
            "DELETE FROM wake_workflow_bindings WHERE workflow_id = 'wake-workflow'",
            "SELECT COUNT(*) FROM wake_workflow_bindings WHERE workflow_id = 'wake-workflow'",
        ),
        (
            "workflow_barrier_members",
            "DELETE FROM workflow_barrier_members WHERE barrier_id = 'wake-registration:wake-workflow'",
            "SELECT COUNT(*) FROM workflow_barrier_members WHERE barrier_id = 'wake-registration:wake-workflow'",
        ),
        (
            "workflow_barriers",
            "DELETE FROM workflow_barrier_members WHERE barrier_id = 'wake-registration:wake-workflow'; DELETE FROM workflow_barriers WHERE workflow_id = 'wake-workflow'",
            "SELECT COUNT(*) FROM workflow_barriers WHERE workflow_id = 'wake-workflow'",
        ),
        (
            "workflow_effects",
            "DELETE FROM wake_workflow_bindings WHERE workflow_id = 'wake-workflow'; DELETE FROM workflow_barrier_members WHERE barrier_id = 'wake-registration:wake-workflow'; DELETE FROM workflow_effects WHERE workflow_id = 'wake-workflow'",
            "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 'wake-workflow'",
        ),
        (
            "workflow_transitions",
            "DELETE FROM wake_workflow_bindings WHERE workflow_id = 'wake-workflow'; DELETE FROM workflow_barrier_members WHERE barrier_id = 'wake-registration:wake-workflow'; DELETE FROM workflow_barriers WHERE workflow_id = 'wake-workflow'; DELETE FROM workflow_effects WHERE workflow_id = 'wake-workflow'; DELETE FROM workflow_transitions WHERE workflow_id = 'wake-workflow'",
            "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 'wake-workflow'",
        ),
        (
            "wake_registration_fences",
            "DELETE FROM wake_workflow_bindings WHERE workflow_id = 'wake-workflow'; DELETE FROM wake_registration_fences WHERE conversation_id = 'conv-wake'",
            "SELECT COUNT(*) FROM wake_registration_fences WHERE conversation_id = 'conv-wake'",
        ),
    ] {
        let (pool, repo) = registered(bash()).await;
        sqlx::raw_sql(delete_sql)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed deleting {table}: {error}"));
        let replay = WakeWorkflowAdapter::new(&repo)
            .register(&registration(bash()))
            .await
            .unwrap();
        assert!(matches!(replay, WakeRegistrationResult::Replay { .. }));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(count_sql)
                .fetch_one(&pool)
                .await
                .unwrap(),
            1,
            "replay did not repair {table}"
        );
    }
}

#[tokio::test]
async fn registration_failpoints_roll_back_every_registration_row() {
    for failpoint in [
        WakeRegistrationFailpoint::AfterExternalAcceptance,
        WakeRegistrationFailpoint::AfterInitialTransition,
        WakeRegistrationFailpoint::AfterTypedBinding,
    ] {
        let pool = pool().await;
        let repo = WorkflowRepository::new(pool.clone());
        let adapter = WakeWorkflowAdapter::new(&repo);
        let error = adapter
            .register_with_failpoint(&registration(bash()), Some(failpoint))
            .await
            .unwrap_err();
        assert!(matches!(error, WorkflowRepositoryError::CorruptState(_)));
        let count: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM external_acceptance_bindings) \
                  + (SELECT COUNT(*) FROM workflows) + (SELECT COUNT(*) FROM workflow_transitions) \
                  + (SELECT COUNT(*) FROM workflow_effects) + (SELECT COUNT(*) FROM workflow_barriers) \
                  + (SELECT COUNT(*) FROM workflow_barrier_members) \
                  + (SELECT COUNT(*) FROM wake_workflow_bindings) \
                  + (SELECT COUNT(*) FROM wake_registration_fences)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "registration rows leaked at {failpoint:?}");
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bash_observation_and_terminal_receipt_require_same_exact_authority() {
    let (pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
        identity: match bash() {
            WakeResourceIdentity::Bash(identity) => identity,
            WakeResourceIdentity::TmuxWindow(_) | WakeResourceIdentity::Subagent(_) => {
                unreachable!()
            }
        },
        status: BashTerminalStatus::Exited,
        occurred_at: Timestamp(1_020),
        exit_code: Some(0),
        duration_ms: Some(20_000),
        signal_number: None,
        kill_signal_sent: None,
        tail_start_offset: 41,
        tail_end_offset: 57,
        tail_truncated_before: true,
        tail_offsets: vec![41, 49],
        final_tail: vec!["first".to_owned(), "second".to_owned()],
    });
    let adapter = WakeWorkflowAdapter::new(&repo);
    let observation = WakeObservationRequest {
        observation_id: "wake-observation".to_owned(),
        authority: claim.authority.clone(),
        attempt_id: claim.attempt_id.clone(),
        evidence: evidence.clone(),
        recorded_at: Utc.timestamp_opt(1_021, 0).single().unwrap(),
    };
    let receipt = WakeTerminalReceiptRequest {
        receipt_id: "wake-receipt".to_owned(),
        reducer_inbox_id: "wake-inbox".to_owned(),
        authority: claim.authority,
        attempt_id: claim.attempt_id,
        terminal: WakeTerminalPayload::Fired {
            contract_id: "wake-contract".to_owned(),
            resource: bash(),
            evidence,
            resolved_at: Timestamp(1_020),
        },
        accepted_at: Utc.timestamp_opt(1_022, 0).single().unwrap(),
        origin: DurableReceiptOrigin::Execution,
    };
    let accepted = adapter
        .record_terminal_evidence(&observation, &receipt)
        .await
        .unwrap();
    assert!(matches!(accepted, AcceptReceiptResult::Accepted { .. }));
    let observation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_observations WHERE id = 'wake-observation' AND authoritative = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(observation_count, 1);
    let state = sqlx::query(
        "SELECT e.status, i.requires_runtime_acceptance, i.delivery_status \
         FROM workflow_effects e JOIN workflow_reducer_inbox i ON i.workflow_id = e.workflow_id \
         WHERE e.id = 'wake-observe:wake-workflow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.get::<String, _>("status"), "receipted");
    assert_eq!(state.get::<i64, _>("requires_runtime_acceptance"), 1);
    assert_eq!(state.get::<String, _>("delivery_status"), "pending");
    let claim_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(claim_count, 0);
    let terminal_projection = sqlx::query(
        "SELECT contract_id, resource_kind, status, bash_status, bash_duration_ms, \
                bash_tail_start_offset, bash_tail_end_offset, bash_tail_truncated_before \
         FROM wake_terminal_receipts WHERE receipt_id = 'wake-receipt'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        terminal_projection.get::<String, _>("contract_id"),
        "wake-contract"
    );
    assert_eq!(
        terminal_projection.get::<String, _>("resource_kind"),
        "bash"
    );
    assert_eq!(terminal_projection.get::<String, _>("status"), "fired");
    assert_eq!(
        terminal_projection.get::<String, _>("bash_status"),
        "exited"
    );
    assert_eq!(
        terminal_projection.get::<i64, _>("bash_duration_ms"),
        20_000
    );
    assert_eq!(
        terminal_projection.get::<i64, _>("bash_tail_start_offset"),
        41
    );
    assert_eq!(
        terminal_projection.get::<i64, _>("bash_tail_end_offset"),
        57
    );
    assert_eq!(
        terminal_projection.get::<i64, _>("bash_tail_truncated_before"),
        1
    );
    let tail: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT ordinal, offset, line FROM wake_terminal_receipt_bash_tail \
         WHERE receipt_id = 'wake-receipt' ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tail,
        vec![(0, 41, "first".to_owned()), (1, 49, "second".to_owned())]
    );
    let wake_inbox = sqlx::query(
        "SELECT workflow_id, contract_id, terminal_receipt_id, conversation_id, sequence, consumed_at \
         FROM wake_observation_inbox WHERE id = 'wake-inbox'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(wake_inbox.get::<String, _>("workflow_id"), "wake-workflow");
    assert_eq!(wake_inbox.get::<String, _>("contract_id"), "wake-contract");
    assert_eq!(
        wake_inbox.get::<String, _>("terminal_receipt_id"),
        "wake-receipt"
    );
    assert_eq!(wake_inbox.get::<String, _>("conversation_id"), "conv-wake");
    assert_eq!(wake_inbox.get::<i64, _>("sequence"), 1);
    assert_eq!(wake_inbox.get::<Option<String>, _>("consumed_at"), None);

    let workflow: (String, String) =
        sqlx::query_as("SELECT status, snapshot_payload FROM workflows WHERE id = 'wake-workflow'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(workflow.0, "completed");
    let snapshot: Value = serde_json::from_str(&workflow.1).unwrap();
    assert_eq!(snapshot["runtime_availability"], "terminal");
    assert_eq!(snapshot["terminal"]["type"], "fired");
    let obligation: (String, i64) =
        sqlx::query_as("SELECT status, snapshot_upper_bound FROM wake_runtime_obligations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(obligation, ("owed".to_owned(), 1));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wake_runtime_obligation_items")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn tmux_typed_observation_rejects_mismatched_resource() {
    let (pool, repo) = registered(tmux()).await;
    let claim = claimed(&repo).await;
    let tmux_identity = match tmux() {
        WakeResourceIdentity::TmuxWindow(identity) => identity,
        WakeResourceIdentity::Bash(_) | WakeResourceIdentity::Subagent(_) => unreachable!(),
    };
    let adapter = WakeWorkflowAdapter::new(&repo);
    let wrong = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
        identity: TmuxResourceIdentity {
            window_id: "@8".to_owned(),
            ..tmux_identity.clone()
        },
        status: TmuxTerminalStatus::ExitMarkerObserved,
        occurred_at: Timestamp(1_010),
        exit_code: Some(0),
        duration_ms: Some(10_000),
        final_tail: vec!["tmux done".to_owned()],
    });
    assert_eq!(
        adapter
            .record_observation(&WakeObservationRequest {
                observation_id: "wrong".to_owned(),
                authority: claim.authority.clone(),
                attempt_id: claim.attempt_id.clone(),
                evidence: wrong,
                recorded_at: Utc.timestamp_opt(1_011, 0).single().unwrap(),
            })
            .await
            .unwrap(),
        RecordObservationResult::StaleAuthority
    );
    let right = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
        identity: tmux_identity,
        status: TmuxTerminalStatus::WindowKilled,
        occurred_at: Timestamp(1_012),
        exit_code: None,
        duration_ms: Some(12_000),
        final_tail: vec!["killed".to_owned()],
    });
    assert!(matches!(
        adapter
            .record_observation(&WakeObservationRequest {
                observation_id: "right".to_owned(),
                authority: claim.authority,
                attempt_id: claim.attempt_id,
                evidence: right,
                recorded_at: Utc.timestamp_opt(1_013, 0).single().unwrap(),
            })
            .await
            .unwrap(),
        RecordObservationResult::Recorded { .. }
    ));
    let payload: String = sqlx::query_scalar(
        "SELECT payload FROM workflow_observations WHERE id = 'right' AND authoritative = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(payload.contains("tmux_window"));
    assert!(payload.contains("window_killed"));
}

#[tokio::test]
async fn next_deadline_excludes_receipted_and_invalidated_observe_effects() {
    let (pool, repo) = registered(bash()).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert_eq!(
        adapter.next_deadline().await.unwrap(),
        Some(Utc.timestamp_opt(1_100, 0).single().unwrap())
    );

    sqlx::query(
        "UPDATE workflow_effects SET status = 'receipted' WHERE id = 'wake-observe:wake-workflow'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(adapter.next_deadline().await.unwrap(), None);

    sqlx::query("UPDATE workflow_effects SET status = 'invalidated' WHERE id = 'wake-observe:wake-workflow'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(adapter.next_deadline().await.unwrap(), None);
}

#[tokio::test]
async fn next_deadline_uses_live_claim_lease_and_retry_actionable_time() {
    let (_pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert_eq!(
        adapter.next_deadline().await.unwrap(),
        Some(Utc.timestamp_opt(1_050, 0).single().unwrap())
    );

    let retry_at = Utc.timestamp_opt(1_030, 0).single().unwrap();
    assert_eq!(
        adapter
            .schedule_retry(
                &claim.authority,
                Utc.timestamp_opt(1_010, 0).single().unwrap(),
                retry_at,
            )
            .await
            .unwrap(),
        ReconcileEffectResult::ScheduledRetry
    );
    assert_eq!(adapter.next_deadline().await.unwrap(), Some(retry_at));
}

#[tokio::test]
async fn retry_promotes_only_the_exact_due_deadline() {
    let (_pool, repo) = registered(bash()).await;
    let claim = claimed(&repo).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    let now = Utc.timestamp_opt(1_010, 0).single().unwrap();
    let deadline = Utc.timestamp_opt(1_030, 0).single().unwrap();
    assert_eq!(
        adapter
            .schedule_retry(&claim.authority, now, deadline)
            .await
            .unwrap(),
        ReconcileEffectResult::ScheduledRetry
    );
    assert!(adapter
        .due(Utc.timestamp_opt(1_029, 0).single().unwrap())
        .await
        .unwrap()
        .is_empty());
    let due = adapter.due(deadline).await.unwrap();
    assert_eq!(due.len(), 1);
    let DueEffect::RetryWait {
        next_eligible_at, ..
    } = &due[0]
    else {
        panic!("exact deadline must expose retry wait");
    };
    assert_eq!(*next_eligible_at, deadline);
    assert!(adapter
        .promote_exact_deadline(&due[0], deadline)
        .await
        .unwrap());
    assert!(!adapter
        .promote_exact_deadline(&due[0], deadline)
        .await
        .unwrap());
}

#[tokio::test]
async fn cancellation_creates_delivery_obligation_without_llm_resume() {
    let (pool, repo) = registered(bash()).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    assert!(adapter
        .cancel(&WakeCancellationRequest {
            workflow_id: "wake-workflow".to_owned(),
            observe_effect_id: "wake-observe:wake-workflow".to_owned(),
            reducer_inbox_id: "cancel-inbox".to_owned(),
            transition_id: "cancel-transition".to_owned(),
            expected_version: 1,
            expected_generation: 0,
            contract_id: "wake-contract".to_owned(),
            resource: bash(),
            resolved_at: Timestamp(1_015),
            committed_at: Utc.timestamp_opt(1_016, 0).single().unwrap(),
        })
        .await
        .unwrap());
    let row = sqlx::query(
        "SELECT w.status AS workflow_status, w.generation, e.status AS effect_status, i.receipt_id, i.requires_runtime_acceptance \
         FROM workflows w JOIN workflow_effects e ON e.workflow_id = w.id \
         JOIN workflow_reducer_inbox i ON i.workflow_id = w.id WHERE w.id = 'wake-workflow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("workflow_status"), "cancelled");
    assert_eq!(row.get::<i64, _>("generation"), 1);
    assert_eq!(row.get::<String, _>("effect_status"), "invalidated");
    assert_eq!(
        row.get::<Option<String>, _>("receipt_id"),
        Some("wake-cancel-receipt:wake-workflow".to_owned())
    );
    assert_eq!(row.get::<i64, _>("requires_runtime_acceptance"), 0);
    let snapshot: String =
        sqlx::query_scalar("SELECT snapshot_payload FROM workflows WHERE id = 'wake-workflow'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snapshot["contract_id"], "wake-contract");
    assert_eq!(snapshot["conversation_id"], "conv-wake");
    assert_eq!(snapshot["registering_tool_use_id"], "tool-use");
    assert_eq!(snapshot["cancelled"], true);
    assert_eq!(snapshot["runtime_availability"], "terminal");
    assert_eq!(snapshot["terminal"]["type"], "cancelled");
    assert_eq!(snapshot["terminal"]["reason"], "explicit_cancel");
    assert_eq!(snapshot["terminal"]["resolved_at"], 1_015);
    let projection: (String, String, String) = sqlx::query_as(
        "SELECT r.status, r.cancellation_reason, i.terminal_receipt_id \
         FROM wake_terminal_receipts r JOIN wake_observation_inbox i \
           ON i.terminal_receipt_id = r.receipt_id \
         WHERE i.id = 'cancel-inbox'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(projection.0, "cancelled");
    assert_eq!(projection.1, "explicit_cancel");
    assert_eq!(projection.2, "wake-cancel-receipt:wake-workflow");
    let normalized_resolved_at: String = sqlx::query_scalar(
        "SELECT resolved_at FROM wake_terminal_receipts WHERE receipt_id = 'wake-cancel-receipt:wake-workflow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        normalized_resolved_at,
        Utc.timestamp_opt(1_015, 0).single().unwrap().to_rfc3339()
    );
    let runtime_obligations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM wake_runtime_obligations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(runtime_obligations, 1, "cancellation must be delivered");
    let output = adapter.owed_tool_results("conv-wake").await.unwrap();
    assert_eq!(output.len(), 1);
    let payload: Value = serde_json::from_str(&output[0].2).unwrap();
    assert_eq!(payload["status"], "cancelled");

    let owed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_owed_acceptance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owed, 0);
}

#[tokio::test]
async fn cancellation_suppresses_only_linked_obligation_and_preserves_same_conversation_peer() {
    let (pool, repo) = registered(bash()).await;
    let first_claim = claimed(&repo).await;
    terminalize_bash(
        &repo,
        first_claim,
        "wake-workflow",
        "wake-contract",
        "receipt-first",
        "inbox-first",
    )
    .await;

    let mut second = registration(bash());
    second.idempotency_key = "register-second".to_owned();
    second.intent_fingerprint = "fingerprint-second".to_owned();
    second.workflow_id = "wake-workflow-second".to_owned();
    second.transition_id = "wake-transition-second".to_owned();
    second.binding_id = "wake-binding-second".to_owned();
    second.intent.contract_id = "wake-contract-second".to_owned();
    second.fence_version = 2;
    assert!(matches!(
        WakeWorkflowAdapter::new(&repo)
            .register(&second)
            .await
            .unwrap(),
        WakeRegistrationResult::New { .. }
    ));
    let second_claim = claimed(&repo).await;
    terminalize_bash(
        &repo,
        second_claim,
        "wake-workflow-second",
        "wake-contract-second",
        "receipt-second",
        "inbox-second",
    )
    .await;

    sqlx::query("UPDATE workflows SET status = 'active', version = 1, generation = 0 WHERE id = 'wake-workflow'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_effects SET status = 'eligible' WHERE id = 'wake-observe:wake-workflow'",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("DELETE FROM wake_runtime_obligations")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO wake_runtime_obligations \
         (id, conversation_id, snapshot_upper_bound, status, created_at) VALUES \
         ('obligation-first', 'conv-wake', 1, 'owed', '1970-01-01T00:17:01+00:00'), \
         ('obligation-second', 'conv-wake', 2, 'owed', '1970-01-01T00:17:02+00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wake_runtime_obligation_items (obligation_id, ordinal, inbox_item_id) VALUES \
         ('obligation-first', 0, 'inbox-first'), \
         ('obligation-second', 0, 'inbox-second')",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(WakeWorkflowAdapter::new(&repo)
        .cancel(&WakeCancellationRequest {
            workflow_id: "wake-workflow".to_owned(),
            observe_effect_id: "wake-observe:wake-workflow".to_owned(),
            reducer_inbox_id: "cancel-inbox-scoped".to_owned(),
            transition_id: "cancel-transition-scoped".to_owned(),
            expected_version: 1,
            expected_generation: 0,
            contract_id: "wake-contract".to_owned(),
            resource: bash(),
            resolved_at: Timestamp(1_030),
            committed_at: Utc.timestamp_opt(1_030, 0).single().unwrap(),
        })
        .await
        .unwrap());

    let statuses: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM wake_runtime_obligations ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        statuses,
        vec![
            ("obligation-first".to_owned(), "suppressed".to_owned()),
            ("obligation-second".to_owned(), "owed".to_owned()),
        ]
    );
}

#[tokio::test]
async fn cancellation_revokes_live_claim_and_rejects_wrong_effect_id() {
    let (pool, repo) = registered(bash()).await;
    let _claim = claimed(&repo).await;
    let adapter = WakeWorkflowAdapter::new(&repo);
    let mut request = WakeCancellationRequest {
        workflow_id: "wake-workflow".to_owned(),
        observe_effect_id: "wrong-effect".to_owned(),
        reducer_inbox_id: "cancel-inbox".to_owned(),
        transition_id: "cancel-transition".to_owned(),
        expected_version: 1,
        expected_generation: 0,
        contract_id: "wake-contract".to_owned(),
        resource: bash(),
        resolved_at: Timestamp(1_015),
        committed_at: Utc.timestamp_opt(1_015, 0).single().unwrap(),
    };
    assert!(!adapter.cancel(&request).await.unwrap());
    request.observe_effect_id = "wake-observe:wake-workflow".to_owned();
    request.contract_id = "wrong-contract".to_owned();
    assert!(!adapter.cancel(&request).await.unwrap());
    request.contract_id = "wake-contract".to_owned();
    request.resource = WakeResourceIdentity::Bash(BashResourceIdentity {
        work_scope: scope(),
        handle_id: "wrong-handle".to_owned(),
    });
    assert!(!adapter.cancel(&request).await.unwrap());
    request.resource = bash();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workflows WHERE id = 'wake-workflow'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "active"
    );
    request.observe_effect_id = "wake-observe:wake-workflow".to_owned();
    assert!(adapter.cancel(&request).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_claims")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workflow_barriers")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "invalidated"
    );
}

#[tokio::test]
async fn cancellation_losing_to_terminal_receipt_rolls_back_without_cancel_event() {
    let (pool, repo) = registered(bash()).await;
    sqlx::query(
        "UPDATE workflow_effects SET status = 'receipted' WHERE id = 'wake-observe:wake-workflow'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let cancelled = WakeWorkflowAdapter::new(&repo)
        .cancel(&WakeCancellationRequest {
            workflow_id: "wake-workflow".to_owned(),
            observe_effect_id: "wake-observe:wake-workflow".to_owned(),
            reducer_inbox_id: "cancel-inbox".to_owned(),
            transition_id: "cancel-transition".to_owned(),
            expected_version: 1,
            expected_generation: 0,
            contract_id: "wake-contract".to_owned(),
            resource: bash(),
            resolved_at: Timestamp(1_015),
            committed_at: Utc.timestamp_opt(1_015, 0).single().unwrap(),
        })
        .await
        .unwrap();
    assert!(!cancelled);
    let state: (String, i64) =
        sqlx::query_as("SELECT status, generation FROM workflows WHERE id = 'wake-workflow'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("active".to_owned(), 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workflow_reducer_inbox WHERE id = 'cancel-inbox'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}
