use std::borrow::Cow;

use sqlx::{Row, Sqlite, SqliteConnection, SqlitePool, Transaction};

use super::{run_pending_migrations, run_through_for_test, MIGRATIONS};
use crate::{Database, DbError, DbResult};

const PROVENANCE: &str = include_str!("agent_creation_provenance.sql");
const RECEIPTS: &str = include_str!("product_conversation_rename_receipts.sql");
const FAULTS: &[&str] = &[
    "DROP TABLE product_creation_jobs;\n",
    "INSERT INTO product_creation_job_images (request_id, ordinal, media_type, data) SELECT request_id, ordinal, media_type, data FROM _phase1_images_before;\n",
    "INSERT INTO _phase1_creation_guard SELECT 'no outgoing FK violations', NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);\n",
];

pub(super) async fn body_sql<'a>(
    tx: &mut Transaction<'_, Sqlite>,
    original: &'a str,
) -> DbResult<Cow<'a, str>> {
    if original != PROVENANCE {
        return Ok(Cow::Borrowed(original));
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='_conversation_tools_fault')",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !exists {
        return Ok(Cow::Borrowed(original));
    }
    let values: Vec<i64> =
        sqlx::query_scalar("SELECT boundary FROM temp._conversation_tools_fault")
            .fetch_all(&mut **tx)
            .await?;
    let [boundary] = values.as_slice() else {
        return Err(DbError::Serialization(
            "invalid test fault cardinality".into(),
        ));
    };
    let index = usize::try_from(*boundary)
        .map_err(|_| DbError::Serialization("invalid test fault index".into()))?;
    let Some(anchor) = FAULTS.get(index) else {
        return Err(DbError::Serialization("invalid test fault boundary".into()));
    };
    assert_eq!(original.matches(anchor).count(), 1);
    let at = original.find(anchor).unwrap() + anchor.len();
    let mut modified = original.to_owned();
    modified.insert_str(at, "CREATE TEMP TABLE _phase1_injected_failure (n INTEGER CONSTRAINT conversation_tools_fault CHECK(n=1)); INSERT INTO _phase1_injected_failure VALUES(0);\n");
    Ok(Cow::Owned(modified))
}

async fn fixture(prefix: u32, populated: bool) -> SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options)
        .await
        .unwrap();
    Database::from_pool_for_tests(pool.clone(), String::new())
        .run_migrations()
        .await
        .unwrap();
    run_through_for_test(&pool, prefix).await.unwrap();
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _migrations ORDER BY version")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        versions,
        MIGRATIONS
            .iter()
            .filter(|m| m.version <= prefix)
            .map(|m| i64::from(m.version))
            .collect::<Vec<_>>()
    );
    sqlx::raw_sql("CREATE TEMP TABLE _conversation_tools_connection (marker TEXT); INSERT INTO _conversation_tools_connection VALUES('original');")
        .execute(&pool).await.unwrap();
    if populated {
        seed(&pool, prefix).await;
    }
    pool
}

async fn seed(pool: &SqlitePool, prefix: u32) {
    for (id, origin) in [
        ("old-user", "user_api"),
        ("old-history", "unknown_historical"),
    ] {
        sqlx::query("INSERT INTO product_creation_jobs(request_id, product_conversation_id, cwd, objective, model, llm_language, status, accepted_at_unix_micros, updated_at_unix_micros, objective_origin, pin_exact_checkout_oid, pin_logical_base, pin_freshness) VALUES(?1, ?2, '/fixture', ?3, 'removed-model', 'caveman', 'accepted', 1, 2, ?4, 'frozen-oid', 'main', 'fresh')")
            .bind(id).bind(format!("pc_{id}")).bind(" literal 東京\0objective \n").bind(origin).execute(pool).await.unwrap();
    }
    if prefix >= 142 {
        sqlx::raw_sql("INSERT INTO slack_gateway_requests(request_id, workspace, owner, channel, message_ts, thread_ts, kind, instruction, context, source_url, accepted_at_unix_micros, input_id, observed_at_unix_micros, trigger_kind) VALUES('slack-request', 'ws', 'owner', 'channel', '1', '1', 'baseline', 'literal', 'context', 'https://example.invalid', 1, 'input', 1, 'prefix');
            INSERT INTO product_creation_jobs(request_id, product_conversation_id, cwd, objective, llm_language, status, accepted_at_unix_micros, updated_at_unix_micros, objective_origin, objective_slack_request_id) VALUES('old-slack', 'pc_old_slack', '/fixture', 'Slack objective', 'phoenix-native', 'accepted', 3, 4, 'slack_gateway', 'slack-request');")
            .execute(pool).await.unwrap();
    }
    for id in if prefix >= 142 {
        vec!["old-user", "old-history", "old-slack"]
    } else {
        vec!["old-user", "old-history"]
    } {
        sqlx::query("INSERT INTO product_creation_job_images VALUES(?1, 0, 'image/png', ?2)")
            .bind(id)
            .bind("bytes東京\0image")
            .execute(pool)
            .await
            .unwrap();
        for (generation, status) in [
            (1_i64, "reserved"),
            (2, "present"),
            (3, "cleanup_required"),
            (4, "released"),
            (5, "conflict"),
        ] {
            sqlx::query("INSERT INTO product_creation_resource_reservations(id, request_id, generation, repository_identity, resource_identity, status, created_at_unix_micros, updated_at_unix_micros, ownership_token) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 5, 6, ?7)")
                .bind(format!("{id}-{generation}")).bind(id).bind(generation).bind("repo東京\0identity")
                .bind(format!("resource-{id}-{generation}")).bind(status).bind((generation > 1).then_some("token東京"))
                .execute(pool).await.unwrap();
        }
    }
}

const TABLES: &[&str] = &[
    "product_creation_jobs",
    "product_creation_job_images",
    "product_creation_resource_reservations",
];
#[derive(Debug, PartialEq)]
struct Snapshot {
    columns: Vec<Vec<String>>,
    data: Vec<Vec<Vec<String>>>,
    schema: Vec<(String, String, String, Option<String>)>,
    ledger: Vec<(i64, String, String)>,
}
async fn project(conn: &mut SqliteConnection, table: &str, columns: &[String]) -> Vec<Vec<String>> {
    let fields = columns
        .iter()
        .map(|c| format!("typeof(\"{c}\") || ':' || hex(CAST(\"{c}\" AS BLOB))"))
        .collect::<Vec<_>>()
        .join(",");
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {fields} FROM {table} ORDER BY 1"
    )))
    .fetch_all(conn)
    .await
    .unwrap();
    let mut rows = rows
        .iter()
        .map(|row| {
            (0..columns.len())
                .map(|i| row.get::<String, _>(i))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows
}
async fn snapshot(conn: &mut SqliteConnection) -> Snapshot {
    let mut columns = Vec::new();
    let mut data = Vec::new();
    for table in TABLES {
        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                .bind(*table)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        data.push(project(conn, table, &names).await);
        columns.push(names);
    }
    Snapshot {
        columns,
        data,
        schema: sqlx::query_as(
            "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap(),
        ledger: sqlx::query_as("SELECT version,name,applied_at FROM _migrations ORDER BY version")
            .fetch_all(conn)
            .await
            .unwrap(),
    }
}
async fn preserved(conn: &mut SqliteConnection, before: &Snapshot) {
    for (i, table) in TABLES.iter().enumerate() {
        assert_eq!(
            project(conn, table, &before.columns[i]).await,
            before.data[i],
            "typed row bytes: {table}"
        );
    }
    for (kind, name, table, sql) in &before.schema {
        if kind == "table"
            && ["product_creation_jobs", "product_conversations"].contains(&table.as_str())
        {
            continue;
        }
        let actual: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type=?1 AND name=?2")
                .bind(kind)
                .bind(name)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(&actual, sql, "unchanged schema object {name}");
    }
    let new_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('product_creation_jobs')")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(new_count as usize, before.columns[0].len() + 4);
}
async fn clean(conn: &mut SqliteConnection) {
    let marker: String =
        sqlx::query_scalar("SELECT marker FROM temp._conversation_tools_connection")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(marker, "original");
    let leftovers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_temp_master WHERE name LIKE '_phase1_%'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(leftovers, 0);
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(fk, 1);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(conn)
        .await
        .unwrap()
        .is_empty());
}

#[test]
fn registered_identity_order_and_exact_bodies() {
    let identities = [
        (142, "slack_gateway_v1"),
        (143, "agent_creation_provenance"),
        (144, "product_conversation_rename_receipts"),
    ];
    for (version, name) in identities {
        let entries = MIGRATIONS
            .iter()
            .filter(|m| m.version == version)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, name);
    }
    for (bytes, version) in [(PROVENANCE, 143), (RECEIPTS, 144)] {
        let entries = MIGRATIONS
            .iter()
            .filter(|m| m.sql == bytes)
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, version);
    }
    assert!(MIGRATIONS
        .windows(2)
        .all(|pair| pair[0].version < pair[1].version));
    assert_eq!(
        MIGRATIONS
            .iter()
            .map(|m| m.name)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        MIGRATIONS.len()
    );
}

#[tokio::test]
async fn registered_slack_origin_decodes_and_replays_after_upgrade() {
    use crate::{ProductCreationAcceptOutcome, ProductCreationOrigin};

    let pool = fixture(142, true).await;
    let db = Database::from_pool_for_tests(pool.clone(), String::new());
    assert_eq!(run_pending_migrations(&pool).await.unwrap(), 2);
    let retained = db
        .get_product_creation_job("old-slack")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.intent.origin,
        ProductCreationOrigin::SlackGateway {
            request_id: "slack-request".into(),
        }
    );
    assert!(matches!(
        db.accept_product_creation("old-slack", &retained.intent)
            .await
            .unwrap(),
        ProductCreationAcceptOutcome::Replayed(_)
    ));
    sqlx::raw_sql("INSERT INTO slack_gateway_bindings(binding_id, workspace, owner, channel, thread_ts, kind, creation_id) VALUES('new-binding', 'ws', 'owner', 'channel', '2', 'explicit', 'new-slack');
        INSERT INTO slack_gateway_requests(request_id, workspace, owner, channel, message_ts, thread_ts, kind, instruction, context, source_url, binding_id, accepted_at_unix_micros, input_id, observed_at_unix_micros, trigger_kind) VALUES('new-slack-source', 'ws', 'owner', 'channel', '2', '2', 'explicit', 'literal', 'context', 'https://example.invalid', 'new-binding', 5, 'new-input', 5, 'prefix');")
        .execute(&pool).await.unwrap();
    let mut intent = retained.intent;
    intent.images.clear();
    intent.origin = ProductCreationOrigin::SlackGateway {
        request_id: "new-slack-source".into(),
    };
    let ProductCreationAcceptOutcome::Accepted(accepted) = db
        .accept_product_creation("new-slack", &intent)
        .await
        .unwrap()
    else {
        panic!("expected new Slack acceptance");
    };
    let bound_product: String = sqlx::query_scalar(
        "SELECT product_id FROM slack_gateway_bindings WHERE binding_id='new-binding'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bound_product, accepted.product_conversation_id.as_str());
    assert_eq!(accepted.intent.origin, intent.origin);
    assert_eq!(
        db.get_product_creation_job("new-slack")
            .await
            .unwrap()
            .unwrap()
            .intent
            .origin,
        intent.origin
    );
}

#[tokio::test]
async fn registered_fresh_upgrade_slack_children_ledger_and_reentry() {
    let fresh = Database::open_in_memory().await.unwrap();
    assert_eq!(run_pending_migrations(fresh.pool()).await.unwrap(), 0);
    for prefix in [128, 131, 142] {
        for populated in [false, true] {
            let pool = fixture(prefix, populated).await;
            run_through_for_test(&pool, 142).await.unwrap();
            let before = snapshot(&mut pool.acquire().await.unwrap()).await;
            assert_eq!(run_pending_migrations(&pool).await.unwrap(), 2);
            let mut conn = pool.acquire().await.unwrap();
            preserved(&mut conn, &before).await;
            clean(&mut conn).await;
            let ledger = snapshot(&mut conn).await.ledger;
            assert_eq!(
                ledger
                    .iter()
                    .filter(|(v, _, _)| *v >= 143)
                    .map(|(v, n, _)| (*v, n.as_str()))
                    .collect::<Vec<_>>(),
                [
                    (143, "agent_creation_provenance"),
                    (144, "product_conversation_rename_receipts")
                ]
            );
            drop(conn);
            assert_eq!(run_pending_migrations(&pool).await.unwrap(), 0);
            assert_eq!(
                snapshot(&mut pool.acquire().await.unwrap()).await.ledger,
                ledger
            );
        }
    }
}

#[tokio::test]
async fn registered_body_faults_rollback_and_same_connection_retry() {
    for boundary in 0..3_i64 {
        let pool = fixture(142, true).await;
        let before = snapshot(&mut pool.acquire().await.unwrap()).await;
        sqlx::raw_sql("CREATE TEMP TABLE _conversation_tools_fault(boundary INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO temp._conversation_tools_fault VALUES(?1)")
            .bind(boundary)
            .execute(&pool)
            .await
            .unwrap();
        let error = run_pending_migrations(&pool).await.unwrap_err();
        assert!(
            error.to_string().contains("conversation_tools_fault"),
            "{error}"
        );
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(snapshot(&mut conn).await, before);
        clean(&mut conn).await;
        sqlx::raw_sql("DROP TABLE temp._conversation_tools_fault")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 2);
        let mut conn = pool.acquire().await.unwrap();
        preserved(&mut conn, &before).await;
        clean(&mut conn).await;
    }
}

#[tokio::test]
async fn registered_ledger_failure_rolls_back_body_then_reenters() {
    let pool = fixture(142, true).await;
    sqlx::raw_sql("CREATE TRIGGER tools_ledger_fault BEFORE INSERT ON _migrations WHEN NEW.version=143 BEGIN SELECT RAISE(ABORT,'tools ledger fault'); END;").execute(&pool).await.unwrap();
    let before = snapshot(&mut pool.acquire().await.unwrap()).await;
    let error = run_pending_migrations(&pool).await.unwrap_err();
    assert!(error.to_string().contains("tools ledger fault"), "{error}");
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(snapshot(&mut conn).await, before);
    clean(&mut conn).await;
    sqlx::raw_sql("DROP TRIGGER tools_ledger_fault")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(run_pending_migrations(&pool).await.unwrap(), 2);
}

#[tokio::test]
async fn unsupported_lineage_refuses_before_schema_or_row_mutation() {
    let pool = fixture(142, true).await;
    sqlx::query("UPDATE _migrations SET name='slack_gateway_first_observation' WHERE version=142")
        .execute(&pool)
        .await
        .unwrap();
    let before = snapshot(&mut pool.acquire().await.unwrap()).await;
    assert!(run_pending_migrations(&pool).await.is_err());
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(snapshot(&mut conn).await, before);
    clean(&mut conn).await;
}
