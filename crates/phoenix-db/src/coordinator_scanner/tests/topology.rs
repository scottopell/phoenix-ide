use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scanner_metadata_pages_visit_every_middle_member_and_revisit_old_generation_after_restart()
{
    use phoenix_core::domain::product_conversation::{
        ProductConversationId, ProductConversationKind,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged-chain.db");
    let db = Database::open_migrated_for_test(path.to_str().unwrap())
        .await
        .unwrap();
    let first = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap()
        .id;
    let mut ids = vec![first];
    let mut source_ids = Vec::new();
    for ordinal in 0..14 {
        let id = ids.last().unwrap();
        let tail: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id),0) FROM messages WHERE conversation_id=?",
        )
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        message(
            &db,
            id,
            tail + 1,
            &format!("Please remember outcome{ordinal}."),
            &InputOrigin::UserApi,
        )
        .await;
        source_ids.push(format!("{id}-{}", tail + 1));
        if ordinal < 13 {
            ids.push(continue_head(&db, id).await);
        }
    }
    let head = ids.last().unwrap().clone();
    let mut tx = db.pool().begin().await.unwrap();
    let topology = source::topology(&mut tx).await.unwrap().unwrap();
    let product: ProductConversationId = topology.owner.parse().unwrap();
    let aggregate = Database::get_product_conversation_on(
        &mut tx,
        &product,
        ProductConversationKind::Coordinator,
    )
    .await
    .unwrap();
    let mut scalar = source::member_page(&mut tx, &topology, 0).await.unwrap();
    scalar.extend(source::member_page(&mut tx, &topology, 8).await.unwrap());
    assert_eq!(scalar.len(), 14);
    for (member, canonical) in scalar.iter().zip(&aggregate.segments) {
        assert_eq!(member.id, canonical.transcript_row.conversation.id);
        assert_eq!(member.ordinal, canonical.transcript_row.segment_ordinal);
        assert_eq!(member.tail, canonical.transcript_row.tail_sequence_id);
        assert_eq!(
            member.generation,
            canonical.transcript_row.conversation.transcript_generation
        );
    }
    tx.commit().await.unwrap();
    let first = prepare(&db, 1000).await;
    assert_eq!(first.input.scan_context.transcript_id, head);
    let enrolled: Vec<String> = sqlx::query_scalar(
        "SELECT transcript_id FROM coordinator_scan_members ORDER BY continuation_ordinal",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(enrolled, [ids[..8].to_vec(), vec![head.clone()]].concat());
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
    let mut now = 1002;
    drain_empty(&db, &mut now, None).await;
    let covered = db
        .read_coordinator_scan_coverage(&head, None)
        .await
        .unwrap();
    assert_eq!(covered.summary.enrolled_members, 14);
    assert_eq!(covered.members.len(), 8);
    assert!(covered.has_more);
    assert_eq!(covered.omitted_members, 6);
    let rest = db
        .read_coordinator_scan_coverage(&head, covered.next_member)
        .await
        .unwrap();
    assert_eq!(
        rest.members
            .iter()
            .map(|m| &m.transcript_id)
            .collect::<Vec<_>>(),
        ids[8..].iter().collect::<Vec<_>>()
    );
    assert!(!rest.has_more);
    assert_eq!(rest.summary.pending_live_members, 0);
    assert_eq!(rest.summary.pending_historical_members, 0);
    db.update_message_display_data(
        &source_ids[10],
        &serde_json::json!({"changed":"old middle member"}),
    )
    .await
    .unwrap();
    let generation: i64 =
        sqlx::query_scalar("SELECT transcript_generation FROM conversations WHERE id=?")
            .bind(&ids[10])
            .fetch_one(db.pool())
            .await
            .unwrap();
    db.pool().close().await;
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    let mut revisited = false;
    for _ in 0..6 {
        now += 1;
        match db
            .prepare_coordinator_scan(Some(&profile()), now)
            .await
            .established()
            .unwrap()
            .unwrap()
        {
            ScanPreparation::Ready(batch) => {
                revisited |= batch.input.scan_context.transcript_id == ids[10]
                    && batch.input.scan_context.source_generation == generation;
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
            ScanPreparation::Progress | ScanPreparation::Idle => {}
            other @ ScanPreparation::Wait { .. } => panic!("{other:?}"),
        }
    }
    assert!(
        revisited,
        "middle source generation must be revisited without a new append/kick"
    );
    let after = db
        .read_coordinator_scan_coverage(&head, Some(7))
        .await
        .unwrap();
    assert_eq!(after.members[2].covered_generation, Some(generation));
    assert!(matches!(after.members[2].state, ScanCoverageState::Covered));
}

#[tokio::test]
async fn scanner_metadata_does_not_decode_irrelevant_runtime_or_handoff_blobs() {
    let (db, first) = setup().await;
    message(
        &db,
        &first,
        1,
        "Please remember the original request.",
        &InputOrigin::UserApi,
    )
    .await;
    db.add_message_with_seq(
        "historical-summary",
        &first,
        2,
        &MessageContent::continuation("Original historical handoff"),
        None,
        None,
    )
    .await
    .unwrap();
    let head = continue_head(&db, &first).await;
    let padding = "x".repeat(1024 * 1024);
    sqlx::query("UPDATE conversations SET state=?2,state_kind='tool_executing' WHERE id=?1")
        .bind(&first)
        .bind(serde_json::json!({"type":"tool_executing","padding":padding}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    let stored: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id=?")
        .bind(&first)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(serde_json::from_str::<phoenix_core::domain::sm_state::ConvState>(&stored).is_err());
    let changed = sqlx::query("UPDATE messages SET content=?1 WHERE message_type='continuation'")
        .bind(serde_json::json!({"not":"MessageContent","padding":padding}).to_string())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(changed.rows_affected() > 0);
    let mut now = 1000;
    let batch = prepare(&db, now).await;
    assert_eq!(batch.input.scan_context.transcript_id, first);
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
    drain_empty(&db, &mut now, None).await;
    let coverage = db
        .read_coordinator_scan_coverage(&head, None)
        .await
        .unwrap();
    assert_eq!(coverage.summary.enrolled_members, 2);
    assert_eq!(coverage.summary.pending_historical_members, 0);
}

#[tokio::test]
async fn scanner_external_unlink_rejects_reserved_detached_same_product_source() {
    let (db, first) = setup().await;
    message(
        &db,
        &first,
        1,
        "Please remember the detached request.",
        &InputOrigin::UserApi,
    )
    .await;
    let head = continue_head(&db, &first).await;
    let batch = prepare(&db, 1000).await;
    assert_eq!(batch.input.scan_context.transcript_id, first);
    let output = capture(&batch);
    let attempt = reserve(&db, batch, 1000).await;
    sqlx::query("DROP TRIGGER conversations_validate_product_continuation_on_update")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET continued_in_conv_id=NULL WHERE id=?")
        .bind(&first)
        .execute(db.pool())
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    let topology = source::topology(&mut tx).await.unwrap().unwrap();
    assert!(source::member(&mut tx, &topology, &first)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        source::member_page(&mut tx, &topology, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        db.apply_coordinator_scan(&attempt, &output, metrics(), 1001)
            .await
            .established()
            .unwrap()
            .unwrap(),
        ScanApplyOutcome::Conflict
    ));
    let coverage = db
        .read_coordinator_scan_coverage(&head, None)
        .await
        .unwrap();
    assert_eq!(coverage.members.len(), 1);
    assert_eq!(coverage.members[0].transcript_id, head);
    assert_eq!(coverage.summary.enrolled_members, 1);
    assert_eq!(
        db.list_coordinator_tracking(&head, None, false)
            .await
            .unwrap()
            .active_count,
        0
    );
}
