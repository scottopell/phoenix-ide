use super::{DbError, DbResult, Migration, MIGRATION_119};
use sqlx::{Connection, Sqlite, SqlitePool, Transaction};

const INSERT_AUTHORITY: &str = "close_retirement_resource_dispatches_require_authority";
const INSERT_MEMBERSHIP: &str = "close_retirement_resource_dispatches_require_full_membership";
const UPDATE_MEMBERSHIP: &str =
    "close_retirement_resource_dispatches_updates_require_full_membership";
const CAPTURE: &str = include_str!("native_close_captured_loss_retry.sql");
const ORIGINAL: &str = include_str!("native_close_retry_dispatch.sql");
const WORKTREE: &str = "      AND effect.resource_kind = 'worktree' AND effect.identity_kind = 'worktree'
      AND effect.identity_codec = 'worktree_id_v1' AND effect.identity_value = captured.captured_worktree_identity
      AND captured.captured_worktree_fingerprint IS NOT NULL AND captured.captured_worktree_locator IS NOT NULL";

fn unexpected(name: &str) -> DbError {
    DbError::Serialization(format!(
        "migration 189 found unexpected {name} source shape"
    ))
}

fn trigger(source: &str, name: &str) -> DbResult<String> {
    let start = format!("CREATE TRIGGER {name}\n");
    let (_, tail) = source.split_once(&start).ok_or_else(|| unexpected(name))?;
    let (body, _) = tail.split_once("\nEND;").ok_or_else(|| unexpected(name))?;
    Ok(format!("{start}{body}\nEND"))
}

fn replace_exact(source: &str, name: &str, old: &str, new: &str) -> DbResult<String> {
    if source.matches(old).count() != 1 {
        return Err(unexpected(name));
    }
    Ok(source.replacen(old, new, 1))
}

async fn require_source(
    tx: &mut Transaction<'_, Sqlite>,
    name: &str,
    expected: &str,
) -> DbResult<()> {
    let installed: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
            .bind(name)
            .fetch_optional(&mut **tx)
            .await?;
    if installed.as_deref() != Some(expected) {
        return Err(unexpected(name));
    }
    Ok(())
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    // Reconstruct the full predecessor trigger: 119's active/non-native arm, 181's
    // native original arm, and 188's captured-loss arm. Never derive a new
    // authority from a fragment of unverified installed SQL.
    let original = ORIGINAL.trim_end();
    let captured = original
        .replace(
            "JOIN close_original_retirement_anchors anchor ON anchor.attempt_id = plan.attempt_id",
            "JOIN native_close_retry_snapshot_authorities anchor ON anchor.attempt_id = plan.attempt_id AND anchor.run_ordinal = plan.run_ordinal",
        )
        .replace(
            "close_expected_retirement_resources expected",
            "native_close_retry_resource_authorities expected",
        )
        .replace(
            "close_retirement_inventories inventory",
            "native_close_retry_scope_authorities inventory",
        );
    let native_188 = format!(
        "{original}\nUNION ALL\n{captured}\n      AND EXISTS (SELECT 1 FROM native_close_captured_loss_retry_plans capture_origin\n          WHERE capture_origin.attempt_id = plan.attempt_id AND capture_origin.run_ordinal = plan.run_ordinal)"
    );
    let legacy = replace_exact(
        &trigger(MIGRATION_119, INSERT_AUTHORITY)?,
        INSERT_AUTHORITY,
        "obligation.phase = 'completed'",
        "obligation.native_managed = 0 AND obligation.phase = 'completed'",
    )?;
    let suffix = ORIGINAL
        .get(original.len()..)
        .ok_or_else(|| unexpected(INSERT_AUTHORITY))?;
    let insert_188 = replace_exact(
        &legacy,
        INSERT_AUTHORITY,
        "\nBEGIN",
        &format!("\nAND NOT EXISTS (\n{native_188}{suffix}\n)\nBEGIN"),
    )?;

    // Tmux has only original-inventory authority. Leave both worktree arms
    // intact, including captured environment and pending-success predicates.
    let tmux = replace_exact(
        original,
        INSERT_AUTHORITY,
        WORKTREE,
        migration.sql.trim_end(),
    )?;
    let native_189 = format!("{native_188}\nUNION ALL\n{tmux}");
    let insert_189 = replace_exact(&insert_188, INSERT_AUTHORITY, &native_188, &native_189)?;

    // Keep 188's full membership check. Changing the dispatch key into a
    // completed native Close must also meet the INSERT authority; membership
    // alone would allow rewriting a row to an already discharged or unsupported
    // effect. Do not re-authorize timestamp-only updates of historical markers.
    let update_188 = trigger(CAPTURE, UPDATE_MEMBERSHIP)?;
    let update_189 = replace_exact(
        &update_188,
        UPDATE_MEMBERSHIP,
        "\nBEGIN",
        &format!(
            "\nOR ((NEW.attempt_id, NEW.scope, NEW.inspection_generation, NEW.inspection_fingerprint,
          NEW.resource_kind, NEW.identity_kind, NEW.identity_codec, NEW.identity_value)
      IS NOT (OLD.attempt_id, OLD.scope, OLD.inspection_generation, OLD.inspection_fingerprint,
              OLD.resource_kind, OLD.identity_kind, OLD.identity_codec, OLD.identity_value)
    AND EXISTS (SELECT 1 FROM close_obligations obligation
        WHERE obligation.attempt_id = NEW.attempt_id AND obligation.native_managed = 1
          AND obligation.phase = 'completed')
    AND NOT EXISTS (\n{native_189}\n))\nBEGIN"
        ),
    )?;

    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    require_source(&mut tx, INSERT_AUTHORITY, &insert_188).await?;
    require_source(
        &mut tx,
        INSERT_MEMBERSHIP,
        &trigger(CAPTURE, INSERT_MEMBERSHIP)?,
    )
    .await?;
    require_source(&mut tx, UPDATE_MEMBERSHIP, &update_188).await?;
    sqlx::raw_sql(
        "DROP TRIGGER close_retirement_resource_dispatches_require_authority;
         DROP TRIGGER close_retirement_resource_dispatches_updates_require_full_membership;",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("{insert_189};\n{update_189};")))
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
