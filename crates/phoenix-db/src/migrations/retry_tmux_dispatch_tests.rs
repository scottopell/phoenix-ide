//! Upgrade the real migration 188 schema; never manufacture the expected trigger body.
use super::*;
use sqlx::sqlite::SqlitePoolOptions;

async fn predecessor() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    setup_real_179_for_test(&pool).await;
    run_through_for_test(&pool, 188).await.unwrap();
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 188);
    pool
}

async fn schema(pool: &SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn ledger(pool: &SqlitePool) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT version, name FROM _migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn trigger(pool: &SqlitePool, name: &str) -> String {
    sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn real188_to189_preserves_worktree_authority_and_adds_exact_original_tmux() {
    let pool = predecessor().await;
    let name = "close_retirement_resource_dispatches_require_authority";
    let before = trigger(&pool, name).await;
    let membership_name = "close_retirement_resource_dispatches_require_full_membership";
    let membership = trigger(&pool, membership_name).await;
    let original = include_str!("native_close_retry_dispatch.sql").trim_end();
    assert!(before.contains(original));
    assert_eq!(run_through_for_test(&pool, 189).await.unwrap(), 1);
    let after = trigger(&pool, name).await;
    assert!(after.contains(original), "retain exact worktree authority");
    assert!(after
        .contains("AND effect.resource_kind = 'tmux_server' AND effect.identity_kind = 'opaque'"));
    assert!(after.contains("AND effect.identity_codec = 'opaque_string_v1'"));
    assert_eq!(trigger(&pool, membership_name).await, membership);
    let applied_schema = schema(&pool).await;
    let applied_ledger = ledger(&pool).await;
    assert_eq!(
        applied_ledger.last(),
        Some(&(189, "native_close_retry_tmux_dispatch".into()))
    );
    assert_eq!(run_through_for_test(&pool, 189).await.unwrap(), 0);
    assert_eq!(schema(&pool).await, applied_schema);
    assert_eq!(ledger(&pool).await, applied_ledger);
    let violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

#[tokio::test]
async fn missing_or_changed_dispatch_trigger_rolls_back189() {
    for changed in [false, true] {
        let pool = predecessor().await;
        sqlx::raw_sql("DROP TRIGGER close_retirement_resource_dispatches_require_authority;")
            .execute(&pool)
            .await
            .unwrap();
        if changed {
            sqlx::raw_sql(
                "CREATE TRIGGER close_retirement_resource_dispatches_require_authority
                BEFORE INSERT ON close_retirement_resource_dispatches BEGIN SELECT 1; END;",
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        let before = schema(&pool).await;
        let before_ledger = ledger(&pool).await;
        let error = run_through_for_test(&pool, 189).await.unwrap_err();
        assert!(error.to_string().contains(
            "unexpected close_retirement_resource_dispatches_require_authority source shape"
        ));
        assert_eq!(schema(&pool).await, before);
        assert_eq!(ledger(&pool).await, before_ledger);
    }
}
