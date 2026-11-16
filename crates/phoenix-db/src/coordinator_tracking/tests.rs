use super::*;
use phoenix_core::{
    domain::{
        db_schema::{MessageContent, SourceToolCall},
        llm_types::ContentBlock,
    },
    llm_language::LlmLanguage,
};

async fn setup() -> (Database, String) {
    let db = Database::open_in_memory().await.unwrap();
    let source = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    (db, source)
}
fn record(kind: TrackingKind, title: &str) -> TrackingMutation {
    TrackingMutation::Record {
        record: TrackingRecord {
            kind,
            title: title.into(),
            note: "Requested purpose; historical understanding, not worker status".into(),
            resumption: "Resume the user's question, preserving restrictions".into(),
            references: vec![TrackingReference::PullRequest {
                repository: "example/repo".into(),
                pr_number: 1,
            }],
            evidence: vec![],
        },
    }
}
async fn call(db: &Database, source: &str, mutation: &TrackingMutation) -> SourceToolCall {
    let call = SourceToolCall {
        message_id: uuid::Uuid::new_v4().to_string(),
        tool_use_id: uuid::Uuid::new_v4().to_string(),
    };
    db.add_message(
        &call.message_id,
        source,
        &MessageContent::Agent(vec![ContentBlock::ToolUse {
            id: call.tool_use_id.clone(),
            name: "tracking".into(),
            input: serde_json::json!({"request":mutation}),
        }]),
        None,
        None,
    )
    .await
    .unwrap();
    call
}
async fn apply(db: &Database, source: &str, mutation: TrackingMutation) -> TrackingReceipt {
    let call = call(db, source, &mutation).await;
    db.mutate_coordinator_tracking(source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn tracking_call_replay_conflict_and_structural_authority() {
    let (db, source) = setup().await;
    let mutation = record(TrackingKind::RequestedWork, "Ship useful memory");
    let call = call(&db, &source, &mutation).await;
    let first = db
        .mutate_coordinator_tracking(&source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    let replay = db
        .mutate_coordinator_tracking(&source, &call, &mutation)
        .await
        .established()
        .unwrap()
        .unwrap();
    assert_eq!(first.outcome, TrackingOutcome::Applied);
    assert_eq!(replay.outcome, TrackingOutcome::Replayed);
    assert_eq!(first.item_id, replay.item_id);
    let saved = db
        .read_coordinator_tracking(&source, &first.item_id, 0)
        .await
        .unwrap();
    let recording = saved.recorded_by_ember.unwrap();
    assert_eq!(recording.assistant_message_id, call.message_id);
    assert!(recording.source_available);
    assert!(db
        .mutate_coordinator_tracking(
            &source,
            &call,
            &record(TrackingKind::Bookmark, "Changed intent")
        )
        .await
        .established()
        .unwrap()
        .is_err());
    let ordinary = db
        .create_conversation("ordinary-tracker", "Ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    assert!(db
        .list_coordinator_tracking(&ordinary.id, None, false)
        .await
        .is_err());
    assert!(db
        .mutate_coordinator_tracking(&ordinary.id, &call, &mutation)
        .await
        .established()
        .unwrap()
        .is_err());
    let fake = SourceToolCall {
        message_id: "missing".into(),
        tool_use_id: "missing".into(),
    };
    assert!(db
        .mutate_coordinator_tracking(&source, &fake, &mutation)
        .await
        .established()
        .unwrap()
        .is_err());
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        1
    );
    sqlx::query("DELETE FROM messages WHERE message_id=?1")
        .bind(&call.message_id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        !db.read_coordinator_tracking(&source, &first.item_id, 0)
            .await
            .unwrap()
            .recorded_by_ember
            .unwrap()
            .source_available
    );
}
#[tokio::test]
async fn tracking_new_pause_invalidates_step_and_conflicts_stale_reconciliation() {
    let (db, source) = setup().await;
    let receipt = apply(
        &db,
        &source,
        record(TrackingKind::RequestedWork, "Deployment"),
    )
    .await;
    let evidence = TrackingEvidence {
        instance_id: db.instance_id().await.unwrap().to_string(),
        transcript_id: source.clone(),
        message_id: "deleted-pause-source".into(),
        quotation: "Pause deployment until I explicitly lift this restriction".into(),
    };
    let append = TrackingMutation::AppendEvidence {
        id: receipt.item_id.clone(),
        expected_revision: 1,
        evidence: vec![evidence.clone()],
    };
    let changed = apply(&db, &source, append).await;
    let detail = db
        .read_coordinator_tracking(&source, &receipt.item_id, 0)
        .await
        .unwrap();
    assert!(matches!(
        detail.item.resumption,
        TrackingResumption::ReviewRequired {
            latest_evidence_ordinal: 1,
            ..
        }
    ));
    assert!(matches!(
        detail.page_source_availability,
        TrackingSourceAvailability::SourceMissing
    ));
    let page = db
        .list_coordinator_tracking(&source, None, false)
        .await
        .unwrap();
    assert!(page.items[0].review_required);
    assert_eq!(page.review_required_count, 1);
    let duplicate = apply(
        &db,
        &source,
        TrackingMutation::AppendEvidence {
            id: receipt.item_id.clone(),
            expected_revision: changed.item_revision,
            evidence: vec![evidence],
        },
    )
    .await;
    assert_eq!(duplicate.outcome, TrackingOutcome::Unchanged);
    assert_eq!(duplicate.item_revision, changed.item_revision);
    let stale = TrackingMutation::Update {
        kind: TrackingKind::RequestedWork,
        id: receipt.item_id.clone(),
        expected_revision: changed.item_revision,
        title: "Deployment".into(),
        note: "Stopped".into(),
        resumption: "Keep paused".into(),
        reviewed_evidence_ordinal: 0,
        references: vec![],
    };
    let invocation = call(&db, &source, &stale).await;
    assert!(db
        .mutate_coordinator_tracking(&source, &invocation, &stale)
        .await
        .established()
        .unwrap()
        .is_err());
    apply(
        &db,
        &source,
        TrackingMutation::Update {
            kind: TrackingKind::RequestedWork,
            id: receipt.item_id.clone(),
            expected_revision: changed.item_revision,
            title: "Deployment".into(),
            note: "User pause retained".into(),
            resumption: "Wait for explicit pause lift".into(),
            reviewed_evidence_ordinal: 1,
            references: vec![],
        },
    )
    .await;
    assert!(
        matches!(db.read_coordinator_tracking(&source,&receipt.item_id,0).await.unwrap().item.resumption,TrackingResumption::Reviewed{resumption,..} if resumption.contains("explicit"))
    );
}
#[tokio::test]
async fn tracking_retirement_requires_reason_retains_history_and_source_independent_memory() {
    let (db, source) = setup().await;
    let receipt = apply(
        &db,
        &source,
        record(TrackingKind::Bookmark, "Return to an idea"),
    )
    .await;
    let bad = TrackingMutation::Retire {
        id: receipt.item_id.clone(),
        expected_revision: 1,
        disposition: TrackingRetirement::Dismissed,
        reason: "  ".into(),
    };
    let invocation = call(&db, &source, &bad).await;
    assert!(db
        .mutate_coordinator_tracking(&source, &invocation, &bad)
        .await
        .established()
        .unwrap()
        .is_err());
    assert!(sqlx::query("INSERT INTO coordinator_tracking_retirements(item_id,item_revision,disposition,reason,recorded_at_us) VALUES(?1,2,'resolved',NULL,1)").bind(&receipt.item_id).execute(db.pool()).await.is_err());
    assert!(sqlx::query(
        "UPDATE coordinator_tracking_items SET current_retirement_revision=2 WHERE id=?1"
    )
    .bind(&receipt.item_id)
    .execute(db.pool())
    .await
    .is_err());
    apply(
        &db,
        &source,
        TrackingMutation::Retire {
            id: receipt.item_id.clone(),
            expected_revision: 1,
            disposition: TrackingRetirement::Dismissed,
            reason: "User deliberately dropped investigation".into(),
        },
    )
    .await;
    assert_eq!(
        db.list_coordinator_tracking(&source, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
    let evidence = TrackingEvidence {
        instance_id: "removed-peer".into(),
        transcript_id: "old".into(),
        message_id: "old-question".into(),
        quotation: "Revisit this question".into(),
    };
    apply(
        &db,
        &source,
        TrackingMutation::Reopen {
            id: receipt.item_id.clone(),
            expected_revision: 2,
            evidence: vec![evidence],
        },
    )
    .await;
    let detail = db
        .read_coordinator_tracking(&source, &receipt.item_id, 0)
        .await
        .unwrap();
    assert_eq!(detail.retirement_history.len(), 1);
    assert_eq!(
        detail.retirement_history[0].reason,
        "User deliberately dropped investigation"
    );
    assert!(matches!(
        detail.item.resumption,
        TrackingResumption::ReviewRequired { .. }
    ));
}
#[tokio::test]
async fn tracking_bounded_pagination_does_not_drop_distinct_requests_or_claim_full_list() {
    let (db, source) = setup().await;
    for index in 0..11 {
        apply(
            &db,
            &source,
            record(
                if index % 2 == 0 {
                    TrackingKind::Bookmark
                } else {
                    TrackingKind::RequestedWork
                },
                &format!("Concern {index}"),
            ),
        )
        .await;
    }
    let first = db
        .list_coordinator_tracking(&source, None, false)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 8);
    assert_eq!(first.omitted_count, 3);
    assert!(first.has_more);
    assert!(serde_json::to_string(&first).unwrap().len() < 8192);
    let next = db
        .list_coordinator_tracking(&source, first.next_cursor.as_deref(), false)
        .await
        .unwrap();
    assert_eq!(next.items.len(), 3);
    assert_eq!(next.omitted_count, 0);
    assert!(!next.has_more);
    let ids = first
        .items
        .iter()
        .chain(&next.items)
        .map(|i| &i.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 11);
}
#[tokio::test]
async fn tracking_supported_restart_preserves_items_and_explicit_retirement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tracking.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    let source = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    let receipt = apply(
        &db,
        &source,
        record(TrackingKind::RequestedWork, "Outcome survives restart"),
    )
    .await;
    apply(
        &db,
        &source,
        TrackingMutation::Retire {
            id: receipt.item_id.clone(),
            expected_revision: 1,
            disposition: TrackingRetirement::Resolved,
            reason: "User confirmed outcome".into(),
        },
    )
    .await;
    db.pool.close().await;
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::run_pending_migrations(db.pool()).await.unwrap();
    let detail = db
        .read_coordinator_tracking(&source, &receipt.item_id, 0)
        .await
        .unwrap();
    assert!(
        matches!(detail.item.disposition,TrackingState::Resolved{reason} if reason=="User confirmed outcome")
    );
}

async fn observation(db: &Database, source: &str, now: i64) -> Option<crate::Message> {
    let seq = db.next_sequence_id(source).await.unwrap();
    db.append_coordinator_tracking_observation(source, seq, now)
        .await
        .established()
        .unwrap()
        .unwrap()
}
async fn input(db: &Database, source: &str, text: &str) {
    let seq = db.next_sequence_id(source).await.unwrap();
    db.add_message_with_seq_and_origin(
        &uuid::Uuid::new_v4().to_string(),
        source,
        seq,
        &MessageContent::User(crate::UserContent::new(text)),
        None,
        None,
        &phoenix_core::domain::db_schema::InputOrigin::UserApi,
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn tracking_observations_survive_continuation_without_rewriting_prefix_or_repeating_recent_items(
) {
    let (db, source) = setup().await;
    let request = apply(
        &db,
        &source,
        record(
            TrackingKind::RequestedWork,
            "Remember requested release outcome",
        ),
    )
    .await;
    let bookmark = apply(
        &db,
        &source,
        record(TrackingKind::Bookmark, "Return to the design question"),
    )
    .await;
    input(&db, &source, "Let's take an unrelated detour").await;
    let now = Utc::now().timestamp_micros();
    let prefix = db.get_messages(&source).await.unwrap();
    let first = observation(&db, &source, now).await.unwrap();
    assert!(first.content.to_json()["text"]
        .as_str()
        .unwrap()
        .contains("Remember requested release outcome"));
    let after = db.get_messages(&source).await.unwrap();
    for (earlier, current) in prefix.iter().zip(&after) {
        assert_eq!(earlier.message_id, current.message_id);
        assert_eq!(earlier.content, current.content);
    }
    for _ in 0..8 {
        input(&db, &source, "Still working on the detour").await;
        assert!(observation(&db, &source, now + 1_000_000).await.is_none());
    }
    let latest = db
        .read_coordinator_tracking(&source, &request.item_id, 0)
        .await
        .unwrap();
    assert_eq!(latest.item.revision, 1);
    db.update_conversation_state(
        &source,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "Detour only; original request intentionally absent".into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        crate::ContinueOutcome::Created(next) => next,
        other => panic!("{other:?}"),
    };
    let fresh = observation(&db, &successor.id, now + 2_000_000)
        .await
        .unwrap();
    let text = fresh.content.to_json()["text"].as_str().unwrap().to_owned();
    assert!(text.contains("Remember requested release outcome"));
    assert!(text.contains("Return to the design question"));
    assert_eq!(
        db.read_coordinator_tracking(&successor.id, &bookmark.item_id, 0)
            .await
            .unwrap()
            .item
            .kind,
        TrackingKind::Bookmark
    );
    assert!(db
        .append_coordinator_tracking_observation(&source, 1000, now)
        .await
        .established()
        .unwrap()
        .is_err());
    input(&db, &successor.id, "A new normal turn much later").await;
    let due = observation(
        &db,
        &successor.id,
        now + TRACKING_REVIEW_INTERVAL_US + 3_000_000,
    )
    .await
    .unwrap();
    let body = due.content.to_json()["text"].as_str().unwrap().to_owned();
    let parsed: TrackingObservation =
        serde_json::from_str(body.split_once('\n').unwrap().1).unwrap();
    assert_eq!(
        parsed.requested_item_due_for_review.unwrap().id,
        request.item_id
    );
    assert_eq!(
        db.read_coordinator_tracking(&successor.id, &request.item_id, 0)
            .await
            .unwrap()
            .item
            .reviewed_at_us,
        latest.item.reviewed_at_us,
        "Surfacing is not explicit review"
    );
    assert!(
        observation(
            &db,
            &successor.id,
            now + TRACKING_REVIEW_INTERVAL_US + 4_000_000
        )
        .await
        .is_none(),
        "same admitted input replay cannot select another cue"
    );
}
#[tokio::test]
async fn tracking_reference_updates_and_retirement_history_paginate_honestly() {
    let (db, source) = setup().await;
    let receipt = apply(
        &db,
        &source,
        record(TrackingKind::Bookmark, "Growing discussion"),
    )
    .await;
    let mut revision = 1;
    for index in 0..17 {
        let retired = apply(
            &db,
            &source,
            TrackingMutation::Retire {
                id: receipt.item_id.clone(),
                expected_revision: revision,
                disposition: TrackingRetirement::Dismissed,
                reason: format!("Deliberate decision {index}"),
            },
        )
        .await;
        let reopened = apply(
            &db,
            &source,
            TrackingMutation::Reopen {
                id: receipt.item_id.clone(),
                expected_revision: retired.item_revision,
                evidence: vec![TrackingEvidence {
                    instance_id: "peer".into(),
                    transcript_id: "history".into(),
                    message_id: format!("revisit-{index}"),
                    quotation: "User explicitly revisits this idea".into(),
                }],
            },
        )
        .await;
        revision = reopened.item_revision;
    }
    let detail = db
        .read_coordinator_tracking(&source, &receipt.item_id, 0)
        .await
        .unwrap();
    assert_eq!(detail.retirement_history.len(), 16);
    assert_eq!(detail.omitted_retirement_count, 1);
    let tail = db
        .read_coordinator_tracking_page(
            &source,
            &receipt.item_id,
            0,
            detail.next_retirement_revision,
        )
        .await
        .unwrap();
    assert_eq!(tail.retirement_history.len(), 1);
    assert_eq!(tail.omitted_retirement_count, 0);
    apply(
        &db,
        &source,
        TrackingMutation::Update {
            kind: TrackingKind::Bookmark,
            id: receipt.item_id.clone(),
            expected_revision: revision,
            title: "Growing discussion".into(),
            note: "The user has explicitly revisited it".into(),
            resumption: "Return to the question, not a commission".into(),
            reviewed_evidence_ordinal: 17,
            references: vec![TrackingReference::PullRequest {
                repository: "example/repo".into(),
                pr_number: 55,
            }],
        },
    )
    .await;
    assert_eq!(
        db.read_coordinator_tracking(&source, &receipt.item_id, 0)
            .await
            .unwrap()
            .references,
        vec![TrackingReference::PullRequest {
            repository: "example/repo".into(),
            pr_number: 55
        }]
    );
}

#[tokio::test]
async fn tracking_worst_case_escaped_titles_and_due_cue_fit_complete_context_bound() {
    let (db, source) = setup().await;
    for index in 0..9 {
        apply(
            &db,
            &source,
            record(
                TrackingKind::RequestedWork,
                &format!("{}x{index}", "\u{0001}".repeat(150)),
            ),
        )
        .await;
    }
    sqlx::query(
        "UPDATE coordinator_tracking_items SET revision=9223372036854775807,reviewed_at_us=0",
    )
    .execute(db.pool())
    .await
    .unwrap();
    input(&db, &source, "A normal turn with old requested work due").await;
    let message = observation(&db, &source, Utc::now().timestamp_micros())
        .await
        .unwrap();
    let text = message.content.to_json()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.len() <= 8192,
        "complete wrapper and payload are bounded: {}",
        text.len()
    );
    let parsed: TrackingObservation =
        serde_json::from_str(text.split_once('\n').unwrap().1).unwrap();
    assert!(parsed
        .captured_registry
        .items
        .iter()
        .all(|item| item.title_truncated));
    assert_eq!(parsed.captured_registry.omitted_count, 1);
    assert!(parsed.requested_item_due_for_review.is_some());
}

#[tokio::test]
async fn tracking_explicit_kind_change_preserves_evidence_and_pause_without_dispatching_work() {
    let (db, source) = setup().await;
    let saved = apply(
        &db,
        &source,
        record(TrackingKind::Bookmark, "Question later commissioned"),
    )
    .await;
    let evidence = TrackingEvidence {
        instance_id: db.instance_id().await.unwrap().to_string(),
        transcript_id: source.clone(),
        message_id: "restriction-source".into(),
        quotation: "Work is requested, but keep execution paused until I approve".into(),
    };
    let attached = apply(
        &db,
        &source,
        TrackingMutation::AppendEvidence {
            id: saved.item_id.clone(),
            expected_revision: 1,
            evidence: vec![evidence.clone()],
        },
    )
    .await;
    apply(
        &db,
        &source,
        TrackingMutation::Update {
            id: saved.item_id.clone(),
            expected_revision: attached.item_revision,
            kind: TrackingKind::RequestedWork,
            title: "Question later commissioned".into(),
            note: "User now requests the outcome; restriction remains".into(),
            resumption: "Keep execution paused until the user approves".into(),
            reviewed_evidence_ordinal: 1,
            references: vec![],
        },
    )
    .await;
    let detail = db
        .read_coordinator_tracking(&source, &saved.item_id, 0)
        .await
        .unwrap();
    assert_eq!(detail.item.kind, TrackingKind::RequestedWork);
    assert_eq!(detail.evidence[0].1, evidence);
    assert!(matches!(detail.item.disposition, TrackingState::Tracking));
    assert!(
        matches!(detail.item.resumption,TrackingResumption::Reviewed{resumption,..} if resumption.contains("paused"))
    );
    assert!(matches!(
        db.get_conversation(&source).await.unwrap().state,
        phoenix_core::domain::sm_state::ConvState::Idle
    ));
}
