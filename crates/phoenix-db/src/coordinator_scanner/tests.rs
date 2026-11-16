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
            input_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 10,
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
