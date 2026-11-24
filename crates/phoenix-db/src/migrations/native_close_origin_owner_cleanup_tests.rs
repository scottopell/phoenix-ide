use super::*;

async fn predecessor() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    setup_real_175_for_test(&pool).await;
    run_through_for_test(&pool, 186).await.unwrap();
    pool
}

async fn snapshot(pool: &SqlitePool) -> (Vec<(String, String)>, Vec<(i64, String)>) {
    let schema =
        sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap();
    let ledger = sqlx::query_as("SELECT version, name FROM _migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap();
    (schema, ledger)
}

#[tokio::test]
async fn real186_to187_changes_only_owner_cleanup_and_is_idempotent() {
    let pool = predecessor().await;
    let before = snapshot(&pool).await;
    assert_eq!(run_through_for_test(&pool, 187).await.unwrap(), 1);
    let after = snapshot(&pool).await;
    let guards = [
        "native_close_completion_retry_admissions_no_delete",
        "native_close_completion_retry_source_revocations_no_delete",
        "native_close_completion_retry_origins_no_delete",
    ];
    let cleanup = "close_obligations_cleanup_native_completion_retry_owners";
    let unchanged_before: Vec<_> = before
        .0
        .iter()
        .filter(|(name, _)| !guards.contains(&name.as_str()))
        .collect();
    let unchanged_after: Vec<_> = after
        .0
        .iter()
        .filter(|(name, _)| !guards.contains(&name.as_str()) && name != cleanup)
        .collect();
    assert_eq!(unchanged_before, unchanged_after);
    assert_eq!(after.0.len(), before.0.len() + 1);
    for guard in guards {
        let sql = &after.0.iter().find(|(name, _)| name == guard).unwrap().1;
        assert!(sql.contains("WHEN EXISTS (\n    SELECT 1 FROM close_obligations\n    WHERE attempt_id = OLD.attempt_id\n)"));
    }
    let sql = &after.0.iter().find(|(name, _)| name == cleanup).unwrap().1;
    let admission = sql
        .find("DELETE FROM native_close_completion_retry_admissions")
        .unwrap();
    let revocation = sql
        .find("DELETE FROM native_close_completion_retry_source_revocations")
        .unwrap();
    let origin = sql
        .find("DELETE FROM native_close_completion_retry_origins")
        .unwrap();
    assert!(admission < revocation && revocation < origin);
    assert_eq!(sql.matches("WHERE attempt_id = OLD.attempt_id").count(), 3);
    assert_eq!(&after.1[..before.1.len()], before.1.as_slice());
    assert_eq!(
        after.1.last(),
        Some(&(187, "native_close_origin_owner_cleanup".into()))
    );
    assert_eq!(run_through_for_test(&pool, 187).await.unwrap(), 0);
    assert_eq!(snapshot(&pool).await, after);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn missing_predecessor_guard_rolls_back187() {
    let pool = predecessor().await;
    sqlx::raw_sql("DROP TRIGGER native_close_completion_retry_origins_no_delete;")
        .execute(&pool)
        .await
        .unwrap();
    let before = snapshot(&pool).await;
    assert!(run_through_for_test(&pool, 187).await.is_err());
    assert_eq!(snapshot(&pool).await, before);
}
