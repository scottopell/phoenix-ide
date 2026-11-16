use super::*;
use sqlx::Connection;

async fn observed_prepare(
    db: &Database,
    route: &ScanProfile,
    now: i64,
) -> (ScanPreparation, usize) {
    let mut connection = db.pool().acquire().await.unwrap();
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let mut allowance = source::SourceReadAllowance::new();
    let result = storage::prepare_with_allowance(&mut tx, Some(route), now, &mut allowance)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (result, allowance.remaining())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scanner_paused_spending_loads_no_poisoned_prose_and_recovers_after_window() {
    const HOUR: i64 = 3_600_000_000;
    for live in [false, true] {
        let (db, owner) = setup().await;
        message(
            &db,
            &owner,
            1,
            "Please remember the old request.",
            &InputOrigin::UserApi,
        )
        .await;
        drop(prepare(&db, 1000).await);
        if live {
            message(
                &db,
                &owner,
                2,
                "Please remember the new request.",
                &InputOrigin::UserApi,
            )
            .await;
        }
        for number in 0..if live { 16 } else { 4 } {
            let mut route = profile();
            route.prompt_hash = [number; 32];
            let ScanPreparation::Ready(batch) = db
                .prepare_coordinator_scan(Some(&route), 2000)
                .await
                .established()
                .unwrap()
                .unwrap()
            else {
                panic!("budget filled early")
            };
            reserve(&db, batch, 2000).await;
        }
        let original: Vec<(String, String)> =
            sqlx::query_as("SELECT message_id,content FROM messages WHERE conversation_id=?")
                .bind(&owner)
                .fetch_all(db.pool())
                .await
                .unwrap();
        sqlx::query("UPDATE messages SET content='not-json' WHERE conversation_id=?")
            .bind(&owner)
            .execute(db.pool())
            .await
            .unwrap();
        let (paused, remaining) = observed_prepare(&db, &profile(), 3000).await;
        assert_eq!(
            remaining, 32,
            "known spending pause must precede all source projection"
        );
        let ScanPreparation::Wait { reason, .. } = paused else {
            panic!("missing spending pause")
        };
        assert_eq!(
            reason,
            if live {
                ScanBlocker::GlobalBudgetPaused
            } else {
                ScanBlocker::HistoricalBudgetPaused
            }
        );
        let (unblocked, remaining) = observed_prepare(&db, &profile(), HOUR + 3000).await;
        assert!(remaining < 32);
        assert!(matches!(
            unblocked,
            ScanPreparation::Wait {
                reason: ScanBlocker::SourceUnavailable,
                ..
            }
        ));
        for (id, content) in original {
            sqlx::query("UPDATE messages SET content=?2 WHERE message_id=?1")
                .bind(id)
                .bind(content)
                .execute(db.pool())
                .await
                .unwrap();
        }
        assert!(matches!(
            db.prepare_coordinator_scan(Some(&profile()), HOUR + 4000)
                .await
                .established()
                .unwrap()
                .unwrap(),
            ScanPreparation::Ready(_)
        ));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scanner_one_allowance_spans_backoff_malformed_members_and_both_lanes() {
    let (db, first) = setup().await;
    let mut owners = vec![first];
    for index in 0..4 {
        let id = owners.last().unwrap();
        for sequence in 1..=8 {
            message(
                &db,
                id,
                sequence,
                "Please retain the historical request.",
                &InputOrigin::UserApi,
            )
            .await;
        }
        if index < 3 {
            owners.push(continue_head(&db, id).await);
        }
    }
    drop(prepare(&db, 1000).await);
    for id in &owners {
        for sequence in 9..=12 {
            message(
                &db,
                id,
                sequence,
                "Please retain the live request.",
                &InputOrigin::UserApi,
            )
            .await;
        }
    }
    let head = owners.last().unwrap();
    for now in [2000, 2001] {
        sqlx::query("UPDATE coordinator_scan_members SET selected_at_us=CASE WHEN transcript_id=?1 THEN 0 ELSE 1000000 END").bind(head).execute(db.pool()).await.unwrap();
        let batch = prepare(&db, now).await;
        assert_eq!(batch.input.scan_context.transcript_id, *head);
        assert_eq!(batch.input.scan_context.lane, ScanLane::Live);
        let attempt = reserve(&db, batch, now).await;
        db.fail_coordinator_scan(
            &attempt,
            ScanFailure::Provider("Temporary synthetic outage".into()),
            metrics(),
            now,
        )
        .await
        .established()
        .unwrap()
        .unwrap();
    }
    let resume: i64 = sqlx::query_scalar(
        "SELECT resumes_at_us FROM coordinator_scan_work WHERE transcript_id=? AND lane='live'",
    )
    .bind(head)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(resume > 3000);
    sqlx::query("UPDATE coordinator_scan_members SET selected_at_us=0")
        .execute(db.pool())
        .await
        .unwrap();
    let original: Vec<(String, String)> = sqlx::query_as("SELECT message_id,content FROM messages")
        .fetch_all(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE messages SET content='not-json' WHERE conversation_id<>?1 OR sequence_id<=8",
    )
    .bind(head)
    .execute(db.pool())
    .await
    .unwrap();
    let before = db.read_coordinator_scan_coverage(head, None).await.unwrap();
    let (pending, remaining) = observed_prepare(&db, &profile(), 3000).await;
    assert_eq!(
        remaining, 0,
        "four live windows consume16 rows, then only16 historical rows may be fetched"
    );
    assert!(matches!(
        pending,
        ScanPreparation::Wait {
            reason: ScanBlocker::SourceUnavailable,
            ..
        }
    ));
    let after = db.read_coordinator_scan_coverage(head, None).await.unwrap();
    for (a, b) in before.members.iter().zip(&after.members) {
        assert_eq!(a.live_frontier, b.live_frontier);
        assert_eq!(a.historical_frontier, b.historical_frontier);
    }
    for (id, content) in original {
        sqlx::query("UPDATE messages SET content=?2 WHERE message_id=?1")
            .bind(id)
            .bind(content)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let mut now = resume + 1;
    drain_empty(&db, &mut now, None).await;
    let final_page = db.read_coordinator_scan_coverage(head, None).await.unwrap();
    assert_eq!(final_page.summary.pending_live_members, 0);
    assert_eq!(final_page.summary.pending_historical_members, 0);
}
