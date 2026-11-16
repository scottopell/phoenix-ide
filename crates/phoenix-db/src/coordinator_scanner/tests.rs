use super::*;
use crate::{coordinator_tracking::TrackingResumption, Database, MessageContent, UserContent};
use phoenix_core::llm_language::LlmLanguage;
use sqlx::Row;

fn profile() -> ScanProfile {
    ScanProfile {
        connection: ScanConnection::Codex,
        endpoint_hash: [1; 32],
        prompt_hash: [2; 32],
        instruction_bytes: 64,
    }
}
fn metrics() -> ScanMetrics {
    ScanMetrics {
        elapsed_ms: 10,
        usage: Some(ScanUsage {
            input_tokens: Some(20),
            cache_read_tokens: Some(0),
            cache_creation_tokens: Some(0),
            output_tokens: Some(10),
        }),
    }
}
async fn setup() -> (Database, String) {
    let db = Database::open_in_memory().await.unwrap();
    let source = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    (db, source)
}
async fn message(db: &Database, source: &str, sequence: i64, text: &str, origin: &InputOrigin) {
    db.add_message_with_seq_and_origin(
        &format!("{source}-{sequence}"),
        source,
        sequence,
        &MessageContent::User(UserContent::new(text)),
        None,
        None,
        origin,
    )
    .await
    .unwrap();
}
async fn prepare(db: &Database, now: i64) -> PreparedScanBatch {
    for _ in 0..20 {
        match db
            .prepare_coordinator_scan(Some(&profile()), now)
            .await
            .established()
            .unwrap()
            .unwrap()
        {
            ScanPreparation::Ready(batch) => return batch,
            ScanPreparation::Progress => {}
            other => panic!("unexpected preparation: {other:?}"),
        }
    }
    panic!("preparation did not progress")
}
async fn reserve(db: &Database, batch: PreparedScanBatch, now: i64) -> ReservedScanAttempt {
    let bytes = serde_json::to_vec(batch.input()).unwrap().len() + 64;
    match db
        .reserve_coordinator_scan(batch, bytes, now)
        .await
        .established()
        .unwrap()
        .unwrap()
    {
        ScanReservation::Reserved(attempt) => attempt,
        other => panic!("unexpected reservation: {other:?}"),
    }
}
fn capture(batch: &PreparedScanBatch) -> ScanResult {
    let fragment = &batch.input.fragments[0];
    ScanResult::Complete {
        actions: vec![ScanAction::New {
            kind: TrackingKind::RequestedWork,
            title: "Cobalt totals question".into(),
            note: "Ask the requested period before any work".into(),
            resumption: "Ask which period; do not start external work".into(),
            quotes: vec![ScanQuote {
                fragment_id: fragment.fragment_id.clone(),
                quotation: fragment.text.clone(),
            }],
        }],
    }
}

#[tokio::test]
async fn scanner_exact_source_atomic_capture_replay_and_unknown_provenance() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please ask which Cobalt period before any work.",
        &InputOrigin::UnknownHistorical,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    assert_eq!(batch.input.scan_context.lane, ScanLane::Historical);
    assert!(matches!(
        batch.input.fragments[0].origin,
        InputOrigin::UnknownHistorical
    ));
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1000).await;
    let first = db
        .apply_coordinator_scan(&attempt, &output, metrics(), 1001)
        .await
        .established()
        .unwrap()
        .unwrap();
    let ScanApplyOutcome::Applied {
        created_item_ids,
        registry_revision,
        ..
    } = first
    else {
        panic!("capture not applied")
    };
    assert_eq!(registry_revision, 1);
    assert_eq!(created_item_ids.len(), 1);
    let detail = db
        .read_coordinator_tracking(&source, &created_item_ids[0], 0)
        .await
        .unwrap();
    assert!(matches!(
        detail.item.resumption,
        TrackingResumption::ReviewRequired { .. }
    ));
    assert!(detail.item.reviewed_at_us.is_none());
    assert!(detail.recorded_by_ember.is_none());
    let origin: String = sqlx::query_scalar(
        "SELECT origin_kind FROM coordinator_scan_evidence_origins WHERE item_id=?1",
    )
    .bind(&created_item_ids[0])
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(origin, "unknown_historical");
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1002)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Replayed
    ));
    let page = db
        .read_coordinator_scan_coverage(&source, None)
        .await
        .unwrap();
    assert!(matches!(page.members[0].state, ScanCoverageState::Covered));
}
#[tokio::test]
async fn scanner_invalid_quote_and_generation_conflict_advance_neither_items_nor_coverage() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let mut output = capture(&batch);
    let ScanResult::Complete { actions } = &mut output;
    let ScanAction::New { quotes, .. } = &mut actions[0] else {
        unreachable!()
    };
    quotes[0].quotation = "fabricated source".into();
    let attempt = reserve(&db, batch, 1000).await;
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1001)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Rejected(_)
    ));
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
    assert!(matches!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .state,
        ScanCoverageState::Incomplete
    ));
    let batch = prepare(&db, 1002).await;
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1002).await;
    sqlx::query(
        "UPDATE conversations SET transcript_generation=transcript_generation+1 WHERE id=?1",
    )
    .bind(&source)
    .execute(db.pool())
    .await
    .unwrap();
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1003)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Conflict
    ));
    let row = sqlx::query("SELECT state,input_tokens FROM coordinator_scan_attempts WHERE id=?1")
        .bind(attempt.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "conflict");
    assert_eq!(row.get::<i64, _>("input_tokens"), 20);
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
    assert!(matches!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .state,
        ScanCoverageState::GenerationStale
    ));
}
#[tokio::test]
async fn scanner_reservation_crash_recapture_charges_retry_and_later_recovers() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let binding = batch.binding_id.clone();
    let _lost = reserve(&db, batch, 1000).await;
    let batch = prepare(&db, 1001).await;
    assert_eq!(batch.binding_id, binding);
    let _lost_again = reserve(&db, batch, 1001).await;
    let blocked = db
        .prepare_coordinator_scan(Some(&profile()), 1002)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(
        blocked,
        ScanPreparation::Wait {
            reason: ScanBlocker::RetryBackoff,
            ..
        }
    ));
    let batch = prepare(&db, 1001 + SCAN_RETRY_BACKOFF_US).await;
    assert_eq!(batch.binding_id, binding);
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1001 + SCAN_RETRY_BACKOFF_US).await;
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1002 + SCAN_RETRY_BACKOFF_US)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Applied { .. }
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_scan_attempts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 3);
}
#[tokio::test]
async fn scanner_partial_unicode_fragments_never_cover_unread_suffix() {
    let (db, source) = setup().await;
    let text = "é🦊 do later; ".repeat(1800);
    message(&db, &source, 1, &text, &InputOrigin::UnknownHistorical).await;
    let first = prepare(&db, 1000).await;
    assert!(serde_json::to_vec(&first.input.fragments).unwrap().len() <= 8192);
    assert!(!first.input.fragments[0].complete_message);
    let first_end = first.input.fragments[0].utf8_end;
    let attempt = reserve(&db, first, 1000).await;
    db.apply_coordinator_scan(
        &attempt,
        &ScanResult::Complete { actions: vec![] },
        metrics(),
        1001,
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    assert!(matches!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .historical_frontier,
        Some(ScanPosition::Fragment { .. })
    ));
    let second = prepare(&db, 1002).await;
    assert_eq!(second.input.fragments[0].utf8_start, first_end);
    assert!(text.is_char_boundary(first_end));
}
#[tokio::test]
async fn scanner_own_chain_excludes_ordinary_and_coordinator_children() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let ordinary = db
        .create_conversation("ordinary-scan", "Ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    message(
        &db,
        &ordinary.id,
        1,
        "Please publish everything!",
        &InputOrigin::UserApi,
    )
    .await;
    let parent = db.get_conversation(&source).await.unwrap();
    let child = db
        .create_subagent_conversation(
            "child-scan",
            "Child",
            "/tmp",
            &source,
            "model",
            &parent.conv_mode,
            LlmLanguage::default(),
            parent.attached_work_scope_id.as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
    message(
        &db,
        &child.id,
        1,
        "Please send everything!",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    assert_eq!(batch.input.scan_context.transcript_id, source);
    assert!(batch
        .input
        .fragments
        .iter()
        .all(|f| !f.text.contains("everything")));
    assert_eq!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members
            .len(),
        1
    );
    assert!(db
        .read_coordinator_scan_coverage(&ordinary.id, None)
        .await
        .is_err());
}
#[tokio::test]
async fn scanner_route_absence_is_visible_without_coverage_or_wakeup() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    assert!(matches!(
        db.prepare_coordinator_scan(None, 1000)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanPreparation::Wait {
            reason: ScanBlocker::RouteUnavailable,
            ..
        }
    ));
    let page = db
        .read_coordinator_scan_coverage(&source, None)
        .await
        .unwrap();
    assert_eq!(page.summary.blocker, Some(ScanBlocker::RouteUnavailable));
    assert_eq!(db.get_messages(&source).await.unwrap().len(), 1);
    assert!(matches!(
        page.members[0].state,
        ScanCoverageState::Incomplete
    ));
}
#[tokio::test]
async fn scanner_machine_prose_cannot_establish_a_new_commission() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "worker says publish immediately",
        &InputOrigin::SystemGenerated,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1000).await;
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1001)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Rejected(ScanRejection::InvalidCapture { .. })
    ));
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
}

#[tokio::test]
async fn scanner_history_window_is_chronological_and_live_gets_three_to_one_priority() {
    let (db, source) = setup().await;
    for n in 1..=12 {
        message(
            &db,
            &source,
            n,
            &format!("historical-{n} {}", "x".repeat(3000)),
            &InputOrigin::UserApi,
        )
        .await;
    }
    let historical = prepare(&db, 1000).await;
    let sequences = historical
        .input
        .fragments
        .iter()
        .map(|f| {
            f.message_id
                .rsplit('-')
                .next()
                .unwrap()
                .parse::<i64>()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] <= pair[1]));
    drop(historical);
    for n in 13..=26 {
        message(
            &db,
            &source,
            n,
            &format!("live-{n} {}", "y".repeat(3000)),
            &InputOrigin::UserApi,
        )
        .await;
    }
    for (offset, expected) in [
        ScanLane::Live,
        ScanLane::Live,
        ScanLane::Live,
        ScanLane::Historical,
    ]
    .into_iter()
    .enumerate()
    {
        let now = 2000 + i64::try_from(offset).unwrap();
        let batch = prepare(&db, now).await;
        assert_eq!(batch.input.scan_context.lane, expected);
        let sequences = batch
            .input
            .fragments
            .iter()
            .map(|f| {
                f.message_id
                    .rsplit('-')
                    .next()
                    .unwrap()
                    .parse::<i64>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(sequences.windows(2).all(|pair| pair[0] <= pair[1]));
        let attempt = reserve(&db, batch, now).await;
        db.apply_coordinator_scan(
            &attempt,
            &ScanResult::Complete { actions: vec![] },
            metrics(),
            now,
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    }
}
#[tokio::test]
async fn scanner_malformed_and_oversized_source_remain_pending_without_dropping_history() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UnknownHistorical,
    )
    .await;
    sqlx::query("UPDATE messages SET content='not-json' WHERE conversation_id=?1")
        .bind(&source)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        db.prepare_coordinator_scan(Some(&profile()), 1000)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanPreparation::Wait {
            reason: ScanBlocker::SourceUnavailable,
            ..
        }
    ));
    let before = db
        .read_coordinator_scan_coverage(&source, None)
        .await
        .unwrap();
    assert!(matches!(
        before.members[0].historical_frontier,
        Some(ScanPosition::Boundary { sequence_id: 1 })
    ));
    let content = serde_json::to_string(
        &MessageContent::User(UserContent::new("x".repeat(1_048_577))).to_stored_json(),
    )
    .unwrap();
    sqlx::query("UPDATE messages SET content=?2 WHERE conversation_id=?1")
        .bind(&source)
        .bind(content)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        db.prepare_coordinator_scan(Some(&profile()), 1001)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanPreparation::Wait {
            reason: ScanBlocker::SourceUnavailable,
            ..
        }
    ));
    assert_eq!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .historical_frontier,
        before.members[0].historical_frontier
    );
}
#[tokio::test]
async fn scanner_budget_charges_crashes_and_keeps_partially_observed_usage() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let attempt = reserve(&db, batch, 1000).await;
    let partial = ScanMetrics {
        elapsed_ms: 20,
        usage: Some(ScanUsage {
            input_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            output_tokens: Some(12),
        }),
    };
    db.fail_coordinator_scan(
        &attempt,
        ScanFailure::Provider("temporary outage".into()),
        partial,
        1001,
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let row =
        sqlx::query("SELECT input_tokens,output_tokens FROM coordinator_scan_attempts WHERE id=?1")
            .bind(attempt.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(row.get::<Option<i64>, _>("input_tokens"), None);
    assert_eq!(row.get::<Option<i64>, _>("output_tokens"), Some(12));
    for step in 1..4 {
        let mut route = profile();
        route.prompt_hash = [step + 10; 32];
        for _ in 0..2 {
            let batch = match db
                .prepare_coordinator_scan(Some(&route), 2000)
                .await
                .established()
                .unwrap()
                .unwrap()
            {
                ScanPreparation::Ready(b) => b,
                ScanPreparation::Wait {
                    reason: ScanBlocker::HistoricalBudgetPaused,
                    ..
                } => {
                    let count: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_scan_attempts")
                            .fetch_one(db.pool())
                            .await
                            .unwrap();
                    assert_eq!(count, 4);
                    return;
                }
                other => panic!("{other:?}"),
            };
            let bytes = serde_json::to_vec(batch.input()).unwrap().len() + 64;
            match db
                .reserve_coordinator_scan(batch, bytes, 2000)
                .await
                .established()
                .unwrap()
                .unwrap()
            {
                ScanReservation::Reserved(_) => {}
                ScanReservation::Paused {
                    reason: ScanBlocker::HistoricalBudgetPaused,
                    ..
                } => {
                    let count: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM coordinator_scan_attempts")
                            .fetch_one(db.pool())
                            .await
                            .unwrap();
                    assert_eq!(count, 4);
                    return;
                }
                other => panic!("{other:?}"),
            }
        }
    }
    panic!("historical budget never paused")
}
#[tokio::test]
async fn scanner_file_restart_keeps_reserved_attempt_and_retry_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scanner.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    let source = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UnknownHistorical,
    )
    .await;
    let first = prepare(&db, 1000).await;
    let binding = first.binding_id.clone();
    let _lost = reserve(&db, first, 1000).await;
    db.pool().close().await;
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    let second = prepare(&db, 1001).await;
    assert_eq!(second.binding_id, binding);
    let _lost = reserve(&db, second, 1001).await;
    db.pool().close().await;
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    assert!(matches!(
        db.prepare_coordinator_scan(Some(&profile()), 1002)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanPreparation::Wait {
            reason: ScanBlocker::RetryBackoff,
            ..
        }
    ));
    let later = prepare(&db, 1001 + SCAN_RETRY_BACKOFF_US).await;
    let output = capture(&later);
    let attempt = reserve(&db, later, 1001 + SCAN_RETRY_BACKOFF_US).await;
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1002 + SCAN_RETRY_BACKOFF_US)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Applied { .. }
    ));
}

async fn native(
    db: &Database,
    source: &str,
    mutation: crate::coordinator_tracking::TrackingMutation,
) -> crate::coordinator_tracking::TrackingReceipt {
    use phoenix_core::domain::{
        db_schema::SourceToolCall,
        llm_types::ContentBlock,
        sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput},
    };
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: uuid::Uuid::new_v4().to_string(),
    };
    let input = serde_json::json!({"request":mutation});
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
    let result = db
        .mutate_coordinator_tracking(source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    db.update_conversation_state(source, &ConvState::Idle)
        .await
        .unwrap();
    result
}
fn append_output(batch: &PreparedScanBatch, id: &str) -> ScanResult {
    ScanResult::Complete {
        actions: vec![ScanAction::Append {
            item_id: id.into(),
            quotes: vec![ScanQuote {
                fragment_id: batch.input.fragments[0].fragment_id.clone(),
                quotation: batch.input.fragments[0].text.clone(),
            }],
        }],
    }
}
#[tokio::test]
async fn scanner_unchanged_generation_rescan_dedups_but_new_restriction_invalidates_review() {
    use crate::coordinator_tracking::TrackingMutation;
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1000).await;
    let ScanApplyOutcome::Applied {
        created_item_ids, ..
    } = db
        .apply_coordinator_scan(&attempt, &output, metrics(), 1001)
        .await
        .established()
        .unwrap()
        .unwrap()
    else {
        panic!("not captured")
    };
    let id = &created_item_ids[0];
    native(
        &db,
        &source,
        TrackingMutation::Update {
            id: id.clone(),
            expected_revision: 1,
            kind: TrackingKind::RequestedWork,
            title: "Cobalt".into(),
            note: "User's Cobalt question".into(),
            resumption: "Ask which period before any work".into(),
            reviewed_evidence_ordinal: 1,
            references: vec![],
        },
    )
    .await;
    let before = db.read_coordinator_tracking(&source, id, 0).await.unwrap();
    assert!(matches!(
        before.item.resumption,
        TrackingResumption::Reviewed { .. }
    ));
    db.update_message_display_data(
        &format!("{source}-1"),
        &serde_json::json!({"display_only":"changed"}),
    )
    .await
    .unwrap();
    let batch = prepare(&db, 2000).await;
    assert_eq!(batch.input.scan_context.source_generation, 2);
    let output = append_output(&batch, id);
    let attempt = reserve(&db, batch, 2000).await;
    let ScanApplyOutcome::Applied {
        changed_item_count,
        registry_revision,
        ..
    } = db
        .apply_coordinator_scan(&attempt, &output, metrics(), 2001)
        .await
        .established()
        .unwrap()
        .unwrap()
    else {
        panic!("not applied")
    };
    assert_eq!(changed_item_count, 0);
    assert_eq!(registry_revision, before.registry_revision);
    let detail = db.read_coordinator_tracking(&source, id, 0).await.unwrap();
    assert_eq!(detail.item.revision, before.item.revision);
    assert!(matches!(
        detail.item.resumption,
        TrackingResumption::Reviewed { .. }
    ));
    assert_eq!(detail.evidence.len(), 1);
    message(
        &db,
        &source,
        2,
        "Pause Cobalt. Do not start until I explicitly resume.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 3000).await;
    let output = append_output(&batch, id);
    let attempt = reserve(&db, batch, 3000).await;
    db.apply_coordinator_scan(&attempt, &output, metrics(), 3001)
        .await
        .established()
        .unwrap()
        .unwrap();
    let detail = db.read_coordinator_tracking(&source, id, 0).await.unwrap();
    assert!(matches!(
        detail.item.resumption,
        TrackingResumption::ReviewRequired { .. }
    ));
    assert_eq!(detail.evidence.len(), 2);
    assert!(detail.evidence[1].1.quotation.contains("Pause"));
    assert_eq!(detail.item.revision, before.item.revision + 1);
}
#[tokio::test]
async fn scanner_fresh_renewal_stays_visible_without_reopening_retired_record() {
    use crate::coordinator_tracking::{TrackingMutation, TrackingRetirement, TrackingState};
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 1000).await;
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1000).await;
    let ScanApplyOutcome::Applied {
        created_item_ids, ..
    } = db
        .apply_coordinator_scan(&attempt, &output, metrics(), 1001)
        .await
        .established()
        .unwrap()
        .unwrap()
    else {
        panic!("not captured")
    };
    let old = &created_item_ids[0];
    native(
        &db,
        &source,
        TrackingMutation::Retire {
            id: old.clone(),
            expected_revision: 1,
            disposition: TrackingRetirement::Dismissed,
            reason: "User abandoned the earlier request".into(),
        },
    )
    .await;
    message(
        &db,
        &source,
        2,
        "I now renew Cobalt. Please ask which new period before any work.",
        &InputOrigin::UserApi,
    )
    .await;
    let batch = prepare(&db, 2000).await;
    assert!(batch
        .input
        .candidates
        .iter()
        .any(|c| c.id == *old && c.disposition == TrackingDisposition::Dismissed));
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 2000).await;
    let ScanApplyOutcome::Applied {
        created_item_ids, ..
    } = db
        .apply_coordinator_scan(&attempt, &output, metrics(), 2001)
        .await
        .established()
        .unwrap()
        .unwrap()
    else {
        panic!("renewal not captured")
    };
    let renewed = &created_item_ids[0];
    assert_ne!(old, renewed);
    let old_detail = db.read_coordinator_tracking(&source, old, 0).await.unwrap();
    assert!(
        matches!(old_detail.item.disposition,TrackingState::Dismissed{ref reason}if reason.contains("abandoned"))
    );
    let page = db
        .list_coordinator_tracking(&source, None, false)
        .await
        .unwrap();
    assert_eq!(page.active_count, 1);
    assert_eq!(page.items[0].id, *renewed);
    assert!(page.items[0].review_required);
    let observation = db
        .append_coordinator_tracking_observation(&source, 100, 2002)
        .await
        .established()
        .unwrap()
        .unwrap()
        .unwrap();
    let text = observation.content.to_json()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains(renewed));
    assert!(text.contains("requested_item_due_for_review"));
}
#[tokio::test]
async fn scanner_two_inflight_batches_cas_one_winner_and_no_omitted_source() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    let first = prepare(&db, 1000).await;
    let second = prepare(&db, 1001).await;
    assert_eq!(first.binding_id, second.binding_id);
    let output = capture(&first);
    let first = reserve(&db, first, 1000).await;
    let second = reserve(&db, second, 1001).await;
    assert!(matches!(
        db.apply_coordinator_scan(&first, &output, metrics(), 1002)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Applied { .. }
    ));
    let replay = db
        .apply_coordinator_scan(&second, &output, metrics(), 1003)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert!(matches!(replay, ScanApplyOutcome::Replayed));
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        1
    );
    assert!(matches!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .state,
        ScanCoverageState::Covered
    ));
}

#[tokio::test]
async fn scanner_acceptance_requires_known_bounded_output_metrics() {
    let (db, source) = setup().await;
    message(
        &db,
        &source,
        1,
        "Please retain Cobalt.",
        &InputOrigin::UserApi,
    )
    .await;
    for (n, output_tokens) in [None, Some(2049)].into_iter().enumerate() {
        let now = 1000 + i64::try_from(n).unwrap();
        let batch = prepare(&db, now).await;
        let output = capture(&batch);
        let attempt = reserve(&db, batch, now).await;
        let metrics = ScanMetrics {
            elapsed_ms: 10,
            usage: Some(ScanUsage {
                input_tokens: Some(20),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                output_tokens,
            }),
        };
        assert!(matches!(
            db.apply_coordinator_scan(&attempt, &output, metrics, now)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ScanApplyOutcome::Rejected(ScanRejection::InvalidOutput { .. })
        ));
    }
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
    assert!(matches!(
        db.read_coordinator_scan_coverage(&source, None)
            .await
            .unwrap()
            .members[0]
            .state,
        ScanCoverageState::Incomplete
    ));
}
