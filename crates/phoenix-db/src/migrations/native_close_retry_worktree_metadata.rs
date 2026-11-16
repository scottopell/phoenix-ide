use sqlx::{Connection, SqlitePool};

use super::{DbError, DbResult, Migration};

const INSTALLED_AUTHORITY_PREFIX: &str = "WHEN EXISTS (SELECT 1 FROM close_obligations obligation\n    WHERE obligation.attempt_id = NEW.attempt_id AND obligation.phase = 'completed')\n AND NOT EXISTS (SELECT 1 FROM close_runs run JOIN close_run_retry_effects effect\n    ON effect.attempt_id = run.attempt_id AND effect.run_ordinal = run.run_ordinal\n    JOIN close_obligations obligation ON obligation.attempt_id = run.attempt_id\n    WHERE run.attempt_id = NEW.attempt_id AND run.status = 'running'\n      AND run.retry_evidence_kind = 'resource_plan'\n      AND run.run_ordinal = (SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = run.attempt_id)\n      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')\n      AND obligation.inspection_generation = NEW.inspection_generation\n      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint\n      AND effect.scope = NEW.scope AND effect.resource_kind = 'worktree'\n      AND effect.identity_kind = NEW.identity_kind AND effect.identity_codec = NEW.identity_codec\n      AND effect.identity_value = NEW.identity_value\n      AND NOT EXISTS (SELECT 1 FROM close_run_retry_successes success\n          WHERE success.attempt_id = effect.attempt_id AND success.run_ordinal = effect.run_ordinal\n            AND success.ordinal = effect.ordinal))";

fn installed_trigger_prefix(name: &str) -> Option<String> {
    let (header, abort) = match name {
        "close_worktree_cleanup_plan_require_retry_authority" => (
            "CREATE TRIGGER close_worktree_cleanup_plan_require_retry_authority\nBEFORE INSERT ON close_worktree_cleanup_plans\n",
            "worktree cleanup plan requires pending exact retry authority",
        ),
        "close_worktree_tombstone_require_retry_authority" => (
            "CREATE TRIGGER close_worktree_tombstone_require_retry_authority\nBEFORE UPDATE OF final_tombstone_root_codec, final_tombstone_root_value,\n    final_tombstone_root_device, final_tombstone_root_inode,\n    final_tombstone_object_device, final_tombstone_object_inode\nON close_worktree_cleanup_plans\n",
            "worktree tombstone requires pending exact retry authority",
        ),
        _ => return None,
    };
    Some(format!(
        "{header}{INSTALLED_AUTHORITY_PREFIX}\nBEGIN\n    SELECT RAISE(ABORT, '{abort}');\nEND"
    ))
}

fn is_known_installed_trigger(name: &str, definition: &str) -> bool {
    installed_trigger_prefix(name).is_some_and(|expected| {
        definition == expected || definition == format!("{expected};")
    })
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    for name in [
        "close_worktree_cleanup_plan_require_retry_authority",
        "close_worktree_tombstone_require_retry_authority",
    ] {
        let definition: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
        )
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        if !is_known_installed_trigger(name, &definition) {
            return Err(DbError::Serialization(format!(
                "migration 165 found unexpected {name} definition"
            )));
        }
        let replacement = INSTALLED_AUTHORITY_PREFIX
            .replace(
                "AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')",
                "AND obligation.native_managed = 0\n      AND obligation.close_outcome IN ('close_incomplete', 'archived_cleanup_attention')",
            );
        let rewritten = definition
            .replacen(INSTALLED_AUTHORITY_PREFIX, &replacement, 1)
            .replacen(
                "\nBEGIN",
                &format!("\nAND NOT EXISTS (\n{}\n)\nBEGIN", migration.sql),
                1,
            );
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER {name}")))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(rewritten))
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
