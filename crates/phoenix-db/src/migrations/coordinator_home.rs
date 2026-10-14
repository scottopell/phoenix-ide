use super::{DbError, DbResult, Migration};
use sqlx::{Connection, Sqlite, SqlitePool};

struct ForeignKeysGuard {
    connection: sqlx::pool::PoolConnection<Sqlite>,
    armed: bool,
}

impl Drop for ForeignKeysGuard {
    fn drop(&mut self) {
        if self.armed {
            self.connection.close_on_drop();
        }
    }
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = ForeignKeysGuard {
        connection: pool.acquire().await?,
        armed: true,
    };
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *guard.connection)
        .await?;
    let result = async {
        let mut tx = guard.connection.begin_with("BEGIN IMMEDIATE").await?;
        let result: DbResult<()> = async {
        let recorded: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE version = ?1)")
            .bind(migration.version)
            .fetch_one(&mut *tx)
            .await?;
        if recorded {
            return Ok(());
        }
        let table_sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'work_scopes'")
            .fetch_one(&mut *tx)
            .await?;
        let source = "CHECK (environment_kind IN ('allocated_worktree', 'unowned_cwd', 'none'))";
        if table_sql.matches(source).count() != 1 {
            return Err(DbError::Serialization("migration 127 found unexpected work_scopes environment CHECK".to_string()));
        }
        let table_sql = table_sql
            .replacen("CREATE TABLE work_scopes", "CREATE TABLE coordinator_home_work_scopes_new", 1)
            .replacen("CREATE TABLE \"work_scopes\"", "CREATE TABLE coordinator_home_work_scopes_new", 1)
            .replacen(source, "CHECK (environment_kind IN ('allocated_worktree', 'unowned_cwd', 'none', 'coordinator_home'))", 1);
        if !table_sql.starts_with("CREATE TABLE coordinator_home_work_scopes_new") {
            return Err(DbError::Serialization("migration 127 could not rebuild work_scopes".to_string()));
        }
        let objects: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT type, name, sql FROM sqlite_schema WHERE sql IS NOT NULL
             AND (type IN ('view', 'trigger') OR (type = 'index' AND tbl_name = 'work_scopes'))
             ORDER BY CASE type WHEN 'view' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, name",
        ).fetch_all(&mut *tx).await?;
        for (kind, name, _) in objects.iter().rev() {
            let escaped = name.replace('"', "\"\"");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP {kind} \"{escaped}\";")))
                .execute(&mut *tx).await?;
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(table_sql)).execute(&mut *tx).await?;
        sqlx::raw_sql("INSERT INTO coordinator_home_work_scopes_new SELECT * FROM work_scopes;
                       DROP TABLE work_scopes;
                       ALTER TABLE coordinator_home_work_scopes_new RENAME TO work_scopes;")
            .execute(&mut *tx).await?;
        for (_, name, sql) in objects {
            let sql = if matches!(name.as_str(), "work_scope_environment_shape_insert" | "work_scope_environment_shape_update") {
                if !sql.contains("NEW.environment_kind = 'unowned_cwd'") {
                    return Err(DbError::Serialization(format!("migration 127 found unexpected {name}")));
                }
                sql.replace("NEW.environment_kind = 'unowned_cwd'", "NEW.environment_kind IN ('unowned_cwd', 'coordinator_home')")
            } else {
                sql
            };
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&mut *tx).await?;
        }
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        let violations = sqlx::query("PRAGMA foreign_key_check").fetch_all(&mut *tx).await?;
        if !violations.is_empty() {
            return Err(DbError::Serialization("migration 127 found foreign key violations".to_string()));
        }
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
            .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
        Ok(())
        }.await;
        match result {
            Ok(()) => tx.commit().await.map_err(DbError::from),
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }.await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *guard.connection)
        .await?;
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *guard.connection)
        .await?;
    if foreign_keys != 1 {
        return Err(DbError::Serialization(
            "migration 127 could not restore foreign key enforcement".to_string(),
        ));
    }
    guard.armed = false;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    fn between<'a>(sql: &'a str, start: &str, end: &str) -> &'a str {
        let offset = sql.find(start).unwrap();
        sql[offset..].split_once(end).unwrap().0
    }

    async fn prior_schema() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO _migrations VALUES (126, 'retire_wake_feature_preserving_history');
             CREATE TABLE product_conversations (id TEXT PRIMARY KEY, kind TEXT NOT NULL);
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY, runtime_role TEXT NOT NULL,
                 work_scope_id TEXT REFERENCES work_scopes(id), coordinator_head INTEGER NOT NULL DEFAULT 0,
                 continued_in_conv_id TEXT REFERENCES conversations(id), archived INTEGER NOT NULL DEFAULT 0,
                 project_id TEXT, desired_base_branch TEXT
             );",
        ).execute(&pool).await.unwrap();
        // The rebuilt table and its shape constraints come from the shipped migration sources.
        let table = between(
            super::super::MIGRATION_053,
            "CREATE TABLE work_scopes_new (",
            "INSERT INTO work_scopes_new",
        );
        sqlx::raw_sql(table).execute(&pool).await.unwrap();
        sqlx::query("ALTER TABLE work_scopes_new RENAME TO work_scopes")
            .execute(&pool)
            .await
            .unwrap();
        for sql in [
            between(
                super::super::MIGRATION_054,
                "ALTER TABLE work_scopes ADD COLUMN environment_kind",
                "UPDATE work_scopes",
            ),
            between(
                super::super::MIGRATION_054,
                "CREATE VIEW work_scope_environments AS",
                "ALTER TABLE conversations ADD COLUMN",
            ),
            super::super::MIGRATION_064
                .split_once("WITH RECURSIVE")
                .unwrap()
                .0,
            between(
                super::super::MIGRATION_080,
                "CREATE TABLE product_conversation_work_scopes (",
                "INSERT INTO product_conversation_work_scopes",
            ),
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        sqlx::raw_sql(
            "INSERT INTO work_scopes (id, authority_kind, lifecycle, retired_at, retired_reason,
                                      created_at, updated_at, environment_kind, cwd, worktree_path,
                                      branch_name, base_branch, worktree_id, worktree_fingerprint)
             VALUES ('allocated', 'work', 'active', NULL, NULL, 'created', 'updated',
                     'allocated_worktree', '/ordinary/tree', '/ordinary/tree', 'branch', 'main', 'wt-id', 'wt-fingerprint'),
                    ('unowned', 'restricted_explore', 'active', NULL, NULL, 'created', 'updated',
                     'unowned_cwd', '/ordinary/cwd', NULL, NULL, NULL, NULL, NULL),
                    ('retired', 'work', 'retired', 'retired-at', 'retired-reason', 'created', 'updated',
                     'none', NULL, NULL, NULL, NULL, NULL, NULL);
             INSERT INTO product_conversations VALUES ('ordinary-product', 'ordinary'), ('coordinator-product', 'coordinator');
             INSERT INTO conversations (id, runtime_role, work_scope_id) VALUES ('ordinary', 'user', 'allocated');
             INSERT INTO conversations (id, runtime_role, work_scope_id, coordinator_head) VALUES ('coordinator', 'coordinator', NULL, 1);
             INSERT INTO conversations (id, runtime_role, work_scope_id, continued_in_conv_id) VALUES ('predecessor', 'coordinator', NULL, 'coordinator');
             INSERT INTO product_conversation_work_scopes VALUES ('allocated', 'ordinary-product');
             CREATE TABLE audit (id INTEGER PRIMARY KEY, scope_id TEXT REFERENCES work_scopes(id));
             CREATE VIEW z_scope_view AS SELECT * FROM work_scope_environments;
             CREATE VIEW a_dependent_view AS SELECT * FROM z_scope_view;
             CREATE TRIGGER view_audit INSTEAD OF DELETE ON a_dependent_view BEGIN
                 INSERT INTO audit(scope_id) VALUES (OLD.work_scope_id);
             END;
             CREATE TRIGGER scope_audit AFTER UPDATE ON work_scopes BEGIN
                 INSERT INTO audit(scope_id) VALUES (NEW.id);
             END;
             CREATE TRIGGER unrelated_audit AFTER UPDATE ON product_conversations BEGIN
                 INSERT INTO audit(scope_id) VALUES (NULL);
             END;",
        ).execute(&pool).await.unwrap();
        pool
    }

    async fn schema(pool: &SqlitePool) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT type, name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY type, name",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn scope_rows(pool: &SqlitePool) -> String {
        sqlx::query_scalar(
            "SELECT json_group_array(json_array(id, authority_kind, lifecycle, retired_at, retired_reason,
                     created_at, updated_at, environment_kind, cwd, worktree_path, branch_name, base_branch,
                     worktree_id, worktree_fingerprint)) FROM (SELECT * FROM work_scopes ORDER BY id)",
        ).fetch_one(pool).await.unwrap()
    }

    fn migration() -> &'static Migration {
        super::super::MIGRATIONS
            .iter()
            .find(|migration| migration.version == 127)
            .unwrap()
    }

    async fn assert_integrity(pool: &SqlitePool) {
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(pool)
                .await
                .unwrap(),
            1
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(pool)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
                .fetch_one(pool)
                .await
                .unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn preserves_prior_schema_rows_dependencies_and_constraints() {
        let pool = prior_schema().await;
        let before_schema = schema(&pool).await;
        let before_rows = scope_rows(&pool).await;
        assert!(sqlx::query("INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('premature-home', 'work', 'coordinator_home', '/home', 'now', 'now')")
            .execute(&pool).await.is_err());
        run(&pool, migration()).await.unwrap();
        assert_eq!(scope_rows(&pool).await, before_rows);
        for (kind, name, sql) in before_schema {
            if name == "work_scopes"
                || matches!(
                    name.as_str(),
                    "conversations_role_scope_insert" | "conversations_role_scope_update"
                )
            {
                continue;
            }
            let expected = if matches!(
                name.as_str(),
                "work_scope_environment_shape_insert" | "work_scope_environment_shape_update"
            ) {
                sql.replace(
                    "NEW.environment_kind = 'unowned_cwd'",
                    "NEW.environment_kind IN ('unowned_cwd', 'coordinator_home')",
                )
            } else {
                sql
            };
            let actual: String =
                sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = ?1 AND name = ?2")
                    .bind(kind)
                    .bind(&name)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(actual, expected, "{name}");
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_work_scope")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_integrity(&pool).await;
        let after = schema(&pool).await;
        run(&pool, migration()).await.unwrap();
        assert_eq!(schema(&pool).await, after);
        sqlx::query("INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('home', 'work', 'coordinator_home', '/owned/home', 'now', 'now')")
            .execute(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversations WHERE runtime_role = 'coordinator' AND work_scope_id = 'home'").fetch_one(&pool).await.unwrap(), 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM product_conversation_work_scopes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        for sql in [
            "DELETE FROM work_scopes WHERE id = 'allocated'",
            "INSERT INTO product_conversation_work_scopes VALUES ('home', 'coordinator-product')",
            "UPDATE work_scopes SET cwd = '/changed' WHERE id = 'home'",
            "UPDATE work_scopes SET lifecycle = 'active', retired_at = NULL, environment_kind = 'unknown' WHERE id = 'retired'",
            "UPDATE work_scopes SET worktree_id = 'invalid' WHERE id = 'unowned'",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        sqlx::query("DELETE FROM a_dependent_view WHERE work_scope_id = 'allocated'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET kind = kind WHERE id = 'ordinary-product'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );
        assert_integrity(&pool).await;
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_rebuild_objects_and_ledger_before_restoring_foreign_keys()
    {
        let pool = prior_schema().await;
        let before_schema = schema(&pool).await;
        let before_rows = scope_rows(&pool).await;
        let failing = Migration {
            version: 127,
            name: "injected-failure",
            sql: concat!(
                include_str!("coordinator_home.sql"),
                "\nSELECT * FROM injected_missing_table;"
            ),
        };
        assert!(run(&pool, &failing).await.is_err());
        assert_eq!(schema(&pool).await, before_schema);
        assert_eq!(scope_rows(&pool).await, before_rows);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 127")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_integrity(&pool).await;
        assert!(
            sqlx::query("DELETE FROM work_scopes WHERE id = 'allocated'")
                .execute(&pool)
                .await
                .is_err()
        );
        run(&pool, migration()).await.unwrap();
        assert_integrity(&pool).await;
    }
}
