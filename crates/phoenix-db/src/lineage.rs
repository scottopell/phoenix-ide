use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::{DbError, DbResult};

struct ColumnMarker {
    table: &'static str,
    column: &'static str,
    declared_type: &'static str,
}

const MARKERS: &[(u32, &[ColumnMarker])] = &[
    (
        130,
        &[ColumnMarker {
            table: "continuation_handoff_candidates",
            column: "candidate_text",
            declared_type: "TEXT",
        }],
    ),
    (
        131,
        &[
            ColumnMarker {
                table: "request_capacity_plans",
                column: "plan_id",
                declared_type: "TEXT",
            },
            ColumnMarker {
                table: "turn_usage",
                column: "capacity_plan_id",
                declared_type: "TEXT",
            },
        ],
    ),
    (
        132,
        &[
            ColumnMarker {
                table: "messages",
                column: "origin_source_instance_id",
                declared_type: "TEXT",
            },
            ColumnMarker {
                table: "steering_messages",
                column: "origin_source_instance_id",
                declared_type: "TEXT",
            },
            ColumnMarker {
                table: "durable_turns",
                column: "origin_source_instance_id",
                declared_type: "TEXT",
            },
        ],
    ),
    (
        133,
        &[ColumnMarker {
            table: "federation_subscriptions",
            column: "id",
            declared_type: "TEXT",
        }],
    ),
    (
        134,
        &[ColumnMarker {
            table: "conversation_goal_ids",
            column: "goal_id",
            declared_type: "TEXT",
        }],
    ),
    (
        135,
        &[ColumnMarker {
            table: "federation_peer_connections",
            column: "notes",
            declared_type: "TEXT",
        }],
    ),
    (
        136,
        &[ColumnMarker {
            table: "conversation_goal_user_requests",
            column: "request_id",
            declared_type: "BLOB",
        }],
    ),
    (
        137,
        &[ColumnMarker {
            table: "coordinator_watch_goal_captures",
            column: "event_id",
            declared_type: "TEXT",
        }],
    ),
    (
        138,
        &[ColumnMarker {
            table: "conversation_goal_requests",
            column: "request_id",
            declared_type: "TEXT",
        }],
    ),
    (
        139,
        &[ColumnMarker {
            table: "conversation_goal_start_materializations",
            column: "start_id",
            declared_type: "INTEGER",
        }],
    ),
];

const TABLE_GROUPS: &[(u32, &[&str])] = &[
    (130, &["continuation_handoff_candidates"]),
    (
        131,
        &["request_capacity_plans", "request_prefix_change_causes"],
    ),
    (
        133,
        &[
            "federation_subscription_create_fences",
            "federation_subscriptions",
            "federation_terminal_identities",
            "federation_terminal_payloads",
            "federation_delivery_authorizations",
            "federation_terminal_occurrences",
            "federation_terminal_current",
            "federation_remote_subscriptions",
            "federation_pending_unsubscribes",
            "federation_remote_admissions",
            "federation_remote_navigation",
            "federation_received_facts",
            "federation_received_gaps",
            "subscription_delivery_identities",
        ],
    ),
    (
        134,
        &[
            "conversation_goal_ids",
            "conversation_goal_actors",
            "conversation_goal_history",
            "conversation_goal_bindings",
        ],
    ),
    (
        136,
        &[
            "conversation_goal_control_requests",
            "conversation_goal_controls",
            "conversation_goal_control_bindings",
            "conversation_goal_user_requests",
        ],
    ),
    (137, &["coordinator_watch_goal_captures"]),
    (
        138,
        &[
            "conversation_goal_limits",
            "conversation_goal_requests",
            "conversation_goal_request_checkpoints",
            "conversation_goal_starts",
            "conversation_goal_start_canonical_inputs",
        ],
    ),
    (139, &["conversation_goal_start_materializations"]),
];

pub(crate) async fn preflight(
    pool: &SqlitePool,
    identities: impl Iterator<Item = (u32, &'static str)>,
) -> DbResult<()> {
    let canonical: BTreeMap<_, _> = identities.collect();
    let ledger_kind: Option<String> =
        sqlx::query_scalar("SELECT type FROM sqlite_schema WHERE name = '_migrations'")
            .fetch_optional(pool)
            .await?;
    let mut recorded = BTreeMap::new();
    match ledger_kind.as_deref() {
        None => {}
        Some("table") => {
            let columns: Vec<(String, String, i64, i64)> = sqlx::query_as(
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info('_migrations') ORDER BY cid",
            ).fetch_all(pool).await?;
            let valid_version = columns.iter().any(|(name, ty, _, pk)| {
                name == "version" && ty.eq_ignore_ascii_case("INTEGER") && *pk == 1
            });
            let valid_name = columns.iter().any(|(name, ty, required, _)| {
                name == "name" && ty.eq_ignore_ascii_case("TEXT") && *required == 1
            });
            let valid_timestamp = columns.iter().any(|(name, ty, required, _)| {
                name == "applied_at" && ty.eq_ignore_ascii_case("TEXT") && *required == 1
            });
            if !(valid_version && valid_name && valid_timestamp) {
                return Err(conflict("malformed migration ledger schema"));
            }
            let invalid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE typeof(version) != 'integer' OR typeof(name) != 'text' OR typeof(applied_at) != 'text')")
                .fetch_one(pool).await?;
            if invalid {
                return Err(conflict("malformed stored migration identity"));
            }
            let rows: Vec<(i64, String)> =
                sqlx::query_as("SELECT version, name FROM _migrations ORDER BY version")
                    .fetch_all(pool)
                    .await?;
            for (number, name) in rows {
                let version =
                    u32::try_from(number).map_err(|_| conflict("invalid migration version"))?;
                if canonical.get(&version).copied() != Some(name.as_str()) {
                    return Err(conflict(&format!("stored migration {version}/{name:?} differs from the canonical identity; old branch, unknown and mixed lineages are unsupported")));
                }
                if recorded.insert(version, name).is_some() {
                    return Err(conflict("duplicate migration identity"));
                }
            }
        }
        Some(_) => return Err(conflict("migration ledger is not a table")),
    }
    let highest = recorded.keys().next_back().copied().unwrap_or(0);
    if highest >= 130 && !(129..=highest).all(|v| recorded.contains_key(&v)) {
        return Err(conflict(
            "partial post-129 ledger has a missing canonical predecessor",
        ));
    }
    for &(version, markers) in MARKERS {
        let mut any = false;
        let mut all = true;
        for marker in markers {
            let (present, valid) = column_marker(pool, marker).await?;
            any |= present;
            all &= valid;
        }
        if recorded.contains_key(&version) {
            if !all {
                return Err(conflict(&format!(
                    "canonical ledger {version} lacks its schema evidence"
                )));
            }
        } else if any {
            return Err(conflict(&format!(
                "schema evidence for {version} exists without its canonical ledger identity"
            )));
        }
    }
    for &(version, tables) in TABLE_GROUPS {
        for table in tables {
            let kind: Option<String> =
                sqlx::query_scalar("SELECT type FROM sqlite_schema WHERE name = ?")
                    .bind(table)
                    .fetch_optional(pool)
                    .await?;
            let present = match kind.as_deref() {
                None => false,
                Some("table") => true,
                Some(_) => return Err(conflict(&format!("schema marker {table} is not a table"))),
            };
            if present != recorded.contains_key(&version) {
                return Err(conflict(&format!(
                    "partial migration-{version} table group: {table}"
                )));
            }
        }
    }
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' AND name IN ('federation_accept_turn', 'federation_accept_steering')",
    ).fetch_all(pool).await?;
    if triggers.len() != if recorded.contains_key(&133) { 2 } else { 0 } {
        return Err(conflict(
            "partial or orphaned federation acceptance triggers",
        ));
    }
    let stop_guards = triggers
        .iter()
        .filter(|(_, sql)| sql.contains("federation subscription locally stopped"))
        .count();
    if recorded.contains_key(&140) {
        if stop_guards != 2 {
            return Err(conflict(
                "canonical ledger 140 lacks both local-Stop guards",
            ));
        }
    } else if stop_guards != 0 {
        return Err(conflict(
            "local-Stop schema exists without canonical ledger 140",
        ));
    }
    Ok(())
}

async fn column_marker(pool: &SqlitePool, marker: &ColumnMarker) -> DbResult<(bool, bool)> {
    let kind: Option<String> = sqlx::query_scalar("SELECT type FROM sqlite_schema WHERE name = ?")
        .bind(marker.table)
        .fetch_optional(pool)
        .await?;
    if kind.as_deref().is_some_and(|kind| kind != "table") {
        return Err(conflict(&format!(
            "schema marker {} is not a table",
            marker.table
        )));
    }
    let declared: Option<String> =
        sqlx::query_scalar("SELECT type FROM pragma_table_info(?) WHERE name = ?")
            .bind(marker.table)
            .bind(marker.column)
            .fetch_optional(pool)
            .await?;
    if declared.is_none()
        && kind.is_some()
        && TABLE_GROUPS
            .iter()
            .any(|(_, tables)| tables.contains(&marker.table))
    {
        return Err(conflict(&format!(
            "partial schema marker {} lacks {}",
            marker.table, marker.column
        )));
    }
    let present = declared.is_some();
    let valid = declared.is_some_and(|ty| ty.eq_ignore_ascii_case(marker.declared_type));
    Ok((present, valid))
}

fn conflict(message: &str) -> DbError {
    DbError::Serialization(format!("database migration lineage conflict: {message}; refused before migration or ledger mutation; no automatic conversion is supported"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::Row;

    const OLD: [&str; 9] = [
        "normalized_remote_input_origin",
        "bounded_federated_terminal_subscriptions",
        "durable_conversation_goal_storage",
        "federation_peer_operator_notes",
        "conversation_goal_user_control",
        "local_goal_watch",
        "conversation_goal_execution_requests",
        "conversation_goal_start_materialization",
        "federation_local_stop_acceptance",
    ];

    async fn pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn ledger(pool: &SqlitePool) {
        sqlx::raw_sql("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL DEFAULT (datetime('now'))); CREATE TABLE retained_payload(value TEXT); INSERT INTO retained_payload VALUES ('unchanged');")
            .execute(pool).await.unwrap();
    }

    async fn row(pool: &SqlitePool, version: u32, name: &str) {
        sqlx::query("INSERT INTO _migrations(version,name) VALUES (?,?)")
            .bind(version)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn canonical_row(pool: &SqlitePool, version: u32) {
        let name = crate::migrations::migration_identities()
            .find(|(v, _)| *v == version)
            .unwrap()
            .1;
        row(pool, version, name).await;
    }

    async fn shape(pool: &SqlitePool, through: u32) {
        let mut tables: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for &(_, markers) in MARKERS.iter().filter(|(v, _)| *v <= through) {
            for marker in markers {
                tables
                    .entry(marker.table)
                    .or_default()
                    .push((marker.column, marker.declared_type));
            }
        }
        for (table, columns) in tables {
            let definitions = columns
                .into_iter()
                .map(|(name, ty)| format!("{name} {ty}"))
                .collect::<Vec<_>>()
                .join(",");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TABLE {table}({definitions})"
            )))
            .execute(pool)
            .await
            .unwrap();
        }
        for &(_, group) in TABLE_GROUPS
            .iter()
            .filter(|(version, _)| *version <= through)
        {
            for table in group {
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                    "CREATE TABLE IF NOT EXISTS {table}(fixture_value TEXT)"
                )))
                .execute(pool)
                .await
                .unwrap();
            }
        }
        if through >= 133 {
            let reason = if through >= 140 {
                "federation subscription locally stopped"
            } else {
                "prior acceptance guard"
            };
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE TABLE trigger_fixture(value TEXT); CREATE TRIGGER federation_accept_turn AFTER INSERT ON trigger_fixture BEGIN SELECT RAISE(ABORT,'{reason}'); END; CREATE TRIGGER federation_accept_steering AFTER UPDATE ON trigger_fixture BEGIN SELECT RAISE(ABORT,'{reason}'); END;")))
                .execute(pool).await.unwrap();
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Snapshot {
        schema: Vec<(String, String, Option<String>)>,
        ledger_rows: Option<Vec<Vec<String>>>,
        retained_payload: Option<Vec<String>>,
    }

    async fn snapshot(pool: &SqlitePool) -> Snapshot {
        let schema: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT type, name, sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(pool)
                .await
                .unwrap();
        let ledger_rows = if schema.iter().any(|(_, name, _)| name == "_migrations") {
            let columns: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM pragma_table_info('_migrations') ORDER BY cid",
            )
            .fetch_all(pool)
            .await
            .unwrap();
            let projection = columns
                .iter()
                .map(|name| format!("quote(\"{}\")", name.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",");
            let ordering = (1..=columns.len())
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {projection} FROM _migrations ORDER BY {ordering}"
            )))
            .fetch_all(pool)
            .await
            .unwrap();
            Some(
                rows.iter()
                    .map(|row| {
                        (0..columns.len())
                            .map(|i| row.get::<String, _>(i))
                            .collect()
                    })
                    .collect(),
            )
        } else {
            None
        };
        let retained_payload = if schema.iter().any(|(_, name, _)| name == "retained_payload") {
            Some(
                sqlx::query_scalar("SELECT value FROM retained_payload ORDER BY value")
                    .fetch_all(pool)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        Snapshot {
            schema,
            ledger_rows,
            retained_payload,
        }
    }

    #[tokio::test]
    async fn snapshot_preserves_ledger_absence_empty_and_malformed_values() {
        let pool = pool().await;
        let absent = snapshot(&pool).await;
        assert_eq!(absent.ledger_rows, None);
        sqlx::raw_sql("CREATE TABLE _migrations(version, name, applied_at)")
            .execute(&pool)
            .await
            .unwrap();
        let empty = snapshot(&pool).await;
        assert_eq!(empty.ledger_rows, Some(Vec::new()));
        assert_ne!(absent, empty);
        sqlx::raw_sql(
            "INSERT INTO _migrations VALUES (X'80FF', NULL, 42), ('130', 'unknown', 1.5)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let populated = snapshot(&pool).await;
        assert_eq!(
            populated.ledger_rows,
            Some(vec![
                vec!["'130'".to_owned(), "'unknown'".to_owned(), "1.5".to_owned()],
                vec!["X'80FF'".to_owned(), "NULL".to_owned(), "42".to_owned()],
            ])
        );
        refuses_without_mutation(&pool).await;
    }

    async fn refuses_without_mutation(pool: &SqlitePool) {
        let before = snapshot(pool).await;
        assert!(preflight(pool, crate::migrations::migration_identities())
            .await
            .is_err());
        assert_eq!(snapshot(pool).await, before);
        assert!(crate::migrations::run_pending_migrations(pool)
            .await
            .is_err());
        assert_eq!(snapshot(pool).await, before);
        let db = crate::Database::from_pool_for_tests(pool.clone(), String::new());
        assert!(db.run_migrations().await.is_err());
        assert_eq!(snapshot(pool).await, before);
    }

    async fn real_prefix(through: u32) -> SqlitePool {
        let pool = pool().await;
        let db = crate::Database::from_pool_for_tests(pool.clone(), String::new());
        db.run_migrations().await.unwrap();
        crate::migrations::run_through_for_test(&pool, through)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE retained_payload(value TEXT); INSERT INTO retained_payload VALUES ('unchanged');")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn fresh_and_each_real_canonical_prefix_are_admitted() {
        let fresh = pool().await;
        preflight(&fresh, crate::migrations::migration_identities())
            .await
            .unwrap();
        for through in 129..=140 {
            let pool = real_prefix(through).await;
            let start_tables: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('conversation_goal_starts', 'conversation_goal_start_canonical_inputs')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(start_tables, if through >= 138 { 2 } else { 0 });
            let nonexistent_usage_table: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'conversation_goal_request_usage'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(nonexistent_usage_table, 0);
            let before = snapshot(&pool).await;
            preflight(&pool, crate::migrations::migration_identities())
                .await
                .unwrap_or_else(|error| panic!("canonical prefix {through}: {error}"));
            assert_eq!(snapshot(&pool).await, before);
        }
    }

    #[tokio::test]
    async fn real_control_sql_tables_missing_or_orphaned_refuse_before_both_entrypoints() {
        let control_sql = include_str!("conversation_goals/control_candidate.sql");
        let tables: Vec<_> = control_sql
            .lines()
            .filter_map(|line| line.strip_prefix("CREATE TABLE "))
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(tables.len(), 3);
        for table in tables {
            let pool = real_prefix(136).await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
                .execute(&pool)
                .await
                .unwrap();
            refuses_without_mutation(&pool).await;

            // Retain the actual column/constraint DDL, not the preflight's marker list.
            let definition = control_sql
                .split(";\n")
                .find(|statement| {
                    statement
                        .lines()
                        .any(|line| line == format!("CREATE TABLE {table} ("))
                })
                .unwrap();
            for through in [131, 135] {
                let pool = real_prefix(through).await;
                sqlx::raw_sql(sqlx::AssertSqlSafe(definition.to_owned()))
                    .execute(&pool)
                    .await
                    .unwrap();
                refuses_without_mutation(&pool).await;
            }
        }
    }

    #[tokio::test]
    async fn malformed_marker_objects_and_column_types_refuse_before_both_entrypoints() {
        for ddl in [
            "CREATE VIEW continuation_handoff_candidates AS SELECT 'not a table' candidate_text",
            "CREATE TABLE continuation_handoff_candidates(candidate_text INTEGER)",
            "CREATE TABLE continuation_handoff_candidates(unrecognized TEXT)",
            "CREATE TABLE messages(origin_source_instance_id INTEGER)",
        ] {
            let pool = pool().await;
            ledger(&pool).await;
            sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
            refuses_without_mutation(&pool).await;
        }
        let pool = real_prefix(136).await;
        sqlx::raw_sql("DROP TABLE conversation_goal_user_requests; CREATE TABLE conversation_goal_user_requests(request_id TEXT);")
            .execute(&pool).await.unwrap();
        refuses_without_mutation(&pool).await;
    }

    #[tokio::test]
    async fn every_old_branch_prefix_and_isolated_tail_identity_refuses_before_both_entrypoints() {
        for length in 1..=OLD.len() {
            let pool = pool().await;
            ledger(&pool).await;
            canonical_row(&pool, 129).await;
            for (offset, name) in OLD.iter().take(length).enumerate() {
                row(&pool, 130 + u32::try_from(offset).unwrap(), name).await;
            }
            refuses_without_mutation(&pool).await;
        }
        for (offset, name) in OLD.iter().enumerate() {
            let pool = pool().await;
            ledger(&pool).await;
            row(&pool, 130 + u32::try_from(offset).unwrap(), name).await;
            refuses_without_mutation(&pool).await;
        }
    }

    #[tokio::test]
    async fn mixed_unknown_partial_and_schema_ambiguous_lineages_refuse_without_mutation() {
        for scenario in 0..6 {
            let pool = pool().await;
            ledger(&pool).await;
            canonical_row(&pool, 129).await;
            match scenario {
                0 => {
                    canonical_row(&pool, 130).await;
                    row(&pool, 131, OLD[1]).await;
                    shape(&pool, 130).await;
                }
                1 => {
                    row(&pool, 130, "unknown").await;
                }
                2 => {
                    canonical_row(&pool, 132).await;
                }
                3 => {
                    canonical_row(&pool, 130).await;
                }
                4 => {
                    shape(&pool, 132).await;
                }
                5 => {
                    canonical_row(&pool, 130).await;
                    shape(&pool, 132).await;
                }
                _ => unreachable!(),
            }
            refuses_without_mutation(&pool).await;
        }
    }

    #[tokio::test]
    async fn missing_ledger_never_accepts_even_a_complete_unledgered_schema() {
        let pool = pool().await;
        shape(&pool, 140).await;
        let before: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(&pool)
                .await
                .unwrap();
        refuses_without_mutation(&pool).await;
        let after = sqlx::query_as("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn malformed_ledger_and_partial_unledgered_origin_columns_are_rejected() {
        for ddl in [
            "CREATE VIEW _migrations AS SELECT 130 version, 'continuation_handoff_candidates' name",
            "CREATE TABLE _migrations(version TEXT, name TEXT, applied_at TEXT)",
            "CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            "CREATE TABLE steering_messages(origin_source_instance_id TEXT)",
        ] {
            let pool = pool().await;
            sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
            refuses_without_mutation(&pool).await;
        }
    }

    #[tokio::test]
    async fn orphaned_acceptance_trigger_refuses_before_both_entrypoints() {
        let pool = pool().await;
        ledger(&pool).await;
        sqlx::raw_sql("CREATE TABLE trigger_fixture(value TEXT); CREATE TRIGGER federation_accept_turn AFTER INSERT ON trigger_fixture BEGIN SELECT 1; END;")
            .execute(&pool).await.unwrap();
        refuses_without_mutation(&pool).await;
    }

    #[tokio::test]
    async fn partial_marker_tables_and_orphan_group_members_refuse_before_both_entrypoints() {
        for (sql, expected_tables) in [
            (
                include_str!("conversation_goals/execution_candidate.sql"),
                3,
            ),
            (include_str!("conversation_goals/start_candidate.sql"), 2),
        ] {
            let tables: Vec<_> = sql
                .lines()
                .filter_map(|line| line.strip_prefix("CREATE TABLE "))
                .map(|line| line.split_whitespace().next().unwrap())
                .collect();
            assert_eq!(tables.len(), expected_tables);
            for table in tables {
                let pool = real_prefix(138).await;
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
                    .execute(&pool)
                    .await
                    .unwrap();
                refuses_without_mutation(&pool).await;

                let definition = sql
                    .split(";\n")
                    .find(|statement| {
                        statement
                            .lines()
                            .any(|line| line == format!("CREATE TABLE {table} ("))
                    })
                    .unwrap();
                let pool = real_prefix(137).await;
                sqlx::raw_sql(sqlx::AssertSqlSafe(definition.to_owned()))
                    .execute(&pool)
                    .await
                    .unwrap();
                refuses_without_mutation(&pool).await;
            }
        }
        for table in [
            "continuation_handoff_candidates",
            "request_prefix_change_causes",
            "federation_received_gaps",
            "conversation_goal_ids",
            "conversation_goal_limits",
            "conversation_goal_start_materializations",
        ] {
            let pool = pool().await;
            ledger(&pool).await;
            canonical_row(&pool, 129).await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TABLE {table}(unrecognized TEXT)"
            )))
            .execute(&pool)
            .await
            .unwrap();
            refuses_without_mutation(&pool).await;
        }
        let pool = pool().await;
        ledger(&pool).await;
        for version in 129..=138 {
            canonical_row(&pool, version).await;
        }
        shape(&pool, 138).await;
        sqlx::raw_sql("DROP TABLE conversation_goal_limits")
            .execute(&pool)
            .await
            .unwrap();
        refuses_without_mutation(&pool).await;
    }

    #[tokio::test]
    async fn real_base_bootstrap_remains_admissible_and_numbered_migrations_are_idempotent() {
        let pool = pool().await;
        let db = crate::Database::from_pool_for_tests(pool.clone(), String::new());
        db.run_migrations().await.unwrap();
        crate::migrations::run_pending_migrations(&pool)
            .await
            .unwrap();
        preflight(&pool, crate::migrations::migration_identities())
            .await
            .unwrap();
        let before = snapshot(&pool).await;
        preflight(&pool, crate::migrations::migration_identities())
            .await
            .unwrap();
        assert_eq!(
            crate::migrations::run_pending_migrations(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(snapshot(&pool).await, before);

        db.run_migrations().await.unwrap();
        let after_bootstrap = snapshot(&pool).await;
        assert!(!before
            .schema
            .iter()
            .any(|(_, name, _)| name == "idx_messages_conversation"));
        let mut expected_schema = before.schema.clone();
        expected_schema.push((
            "index".to_owned(),
            "idx_messages_conversation".to_owned(),
            Some(
                "CREATE INDEX idx_messages_conversation ON messages(conversation_id, sequence_id)"
                    .to_owned(),
            ),
        ));
        expected_schema.sort();
        assert_eq!(after_bootstrap.schema, expected_schema);
        assert_eq!(after_bootstrap.ledger_rows, before.ledger_rows);
        assert_eq!(after_bootstrap.retained_payload, before.retained_payload);
        preflight(&pool, crate::migrations::migration_identities())
            .await
            .unwrap();
        assert_eq!(
            crate::migrations::run_pending_migrations(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(snapshot(&pool).await, after_bootstrap);
    }
}
