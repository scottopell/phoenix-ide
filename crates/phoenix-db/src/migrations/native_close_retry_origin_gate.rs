//! Keep captured-loss retry authority independent of unearned History status.
use super::{DbError, DbResult, Migration};
use sqlx::{Connection, SqlitePool};

const TRIGGER: &str = "native_close_captured_loss_retry_origin_requires_exact_stop";
const SOURCE_184: &str = include_str!("native_close_captured_loss_retry.sql");
const ARCHIVE_GATE: &str = "conversation.archived <> 1 OR ";

fn unexpected() -> DbError {
    DbError::Serialization(format!(
        "migration 186 found unexpected {TRIGGER} source shape"
    ))
}

fn corrected_source() -> DbResult<(String, String)> {
    let start = format!("CREATE TRIGGER {TRIGGER}\n");
    let (_, tail) = SOURCE_184.split_once(&start).ok_or_else(unexpected)?;
    let (body, _) = tail.split_once("\nEND;").ok_or_else(unexpected)?;
    let old = format!("{start}{body}\nEND");
    if old.matches(ARCHIVE_GATE).count() != 1 {
        return Err(unexpected());
    }
    let corrected = old.replacen(ARCHIVE_GATE, "", 1);
    Ok((old, corrected))
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let (expected_old, corrected) = corrected_source()?;
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    let installed: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
            .bind(TRIGGER)
            .fetch_optional(&mut *tx)
            .await?;
    if installed.as_deref() != Some(expected_old.as_str()) {
        return Err(unexpected());
    }
    sqlx::raw_sql("DROP TRIGGER native_close_captured_loss_retry_origin_requires_exact_stop;")
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("{corrected};")))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn predecessor() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        super::super::setup_real_175_for_test(&pool).await;
        super::super::run_through_for_test(&pool, 185)
            .await
            .unwrap();
        pool
    }

    async fn snapshot(pool: &SqlitePool) -> (Vec<(String, String)>, Vec<(i64, String)>) {
        let schema = sqlx::query_as(
            "SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name",
        )
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
    async fn real185_to186_preserves_origin_authority_and_is_idempotent() {
        let pool = predecessor().await;
        let (old, corrected) = corrected_source().unwrap();
        let installed: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
        )
        .bind(TRIGGER)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(installed, old);
        assert_eq!(
            super::super::run_through_for_test(&pool, 186)
                .await
                .unwrap(),
            1
        );
        let installed: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
        )
        .bind(TRIGGER)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(installed, corrected);
        let before = snapshot(&pool).await;
        assert_eq!(
            before.1.last(),
            Some(&(186, "native_close_retry_origin_gate".into()))
        );
        assert_eq!(
            super::super::run_through_for_test(&pool, 186)
                .await
                .unwrap(),
            0
        );
        assert_eq!(snapshot(&pool).await, before);
    }

    #[tokio::test]
    async fn missing_or_changed_origin_trigger_rolls_back186() {
        for changed in [false, true] {
            let pool = predecessor().await;
            sqlx::raw_sql(
                "DROP TRIGGER native_close_captured_loss_retry_origin_requires_exact_stop;",
            )
            .execute(&pool)
            .await
            .unwrap();
            if changed {
                sqlx::raw_sql("CREATE TRIGGER native_close_captured_loss_retry_origin_requires_exact_stop BEFORE INSERT ON native_close_captured_loss_retry_plans BEGIN SELECT 1; END;")
                    .execute(&pool).await.unwrap();
            }
            let before = snapshot(&pool).await;
            let error = super::super::run_through_for_test(&pool, 186)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("unexpected native_close_captured_loss_retry_origin_requires_exact_stop source shape"));
            assert_eq!(snapshot(&pool).await, before);
        }
    }

    #[test]
    fn source_rewrite_removes_only_archive_requirement() {
        let (old, corrected) = corrected_source().unwrap();
        assert_eq!(old.replacen(ARCHIVE_GATE, "", 1), corrected);
        assert!(!corrected.contains("conversation.archived"));
        assert!(corrected.contains("conversation.state_kind NOT IN ('Idle', 'Closed', 'Error')"));
        assert!(corrected.contains("failure.stop_certainty = 'shutdown_uncertain'"));
        assert!(corrected.contains("captured-loss retry requires exact effect-free captured stop"));
    }
}
