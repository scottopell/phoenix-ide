use super::*;
use phoenix_core::domain::instance_identity::InstanceId;
use sqlx::sqlite::SqlitePoolOptions;

fn table_sql(script: &str, name: &str, destination: &str) -> String {
    let start = script.find(&format!("CREATE TABLE {name} (")).unwrap();
    let end = start + script.get(start..).unwrap().find("\n);").unwrap() + 3;
    script
        .get(start..end)
        .unwrap()
        .replacen(name, destination, 1)
}

async fn fixture() -> (SqlitePool, InstanceId) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql("PRAGMA foreign_keys=ON;
        CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE instance_identity(singleton_key INTEGER PRIMARY KEY CHECK(singleton_key=1),instance_id TEXT NOT NULL);
        CREATE TABLE products(id TEXT PRIMARY KEY);
        CREATE TABLE conversations(id TEXT PRIMARY KEY);
        CREATE TABLE slack_gateway_requests(request_id TEXT PRIMARY KEY);")
        .execute(&pool).await.unwrap();
    let local = InstanceId::new();
    sqlx::query("INSERT INTO instance_identity VALUES(1,?1)")
        .bind(local.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let create = table_sql(
        include_str!("../retain_product_creation_receipts.sql"),
        "product_creation_jobs_phase1",
        "product_creation_jobs",
    );
    let rename = table_sql(
        include_str!("../retain_product_conversation_rename_receipts.sql"),
        "product_conversation_rename_receipts_retained",
        "product_conversation_rename_receipts",
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(create))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(rename))
        .execute(&pool)
        .await
        .unwrap();
    let retained = include_str!("../retain_product_creation_receipts.sql");
    let start = retained
        .find("CREATE TRIGGER creation_slack_origin_insert")
        .unwrap();
    let end = retained
        .get(start..)
        .unwrap()
        .find("INSERT INTO product_creation_job_images")
        .unwrap()
        + start;
    sqlx::raw_sql(sqlx::AssertSqlSafe(retained.get(start..end).unwrap()))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE INDEX custom_creation_index ON product_creation_jobs(objective);
        CREATE INDEX custom_rename_index ON product_conversation_rename_receipts(title);
        CREATE TABLE trigger_witness(value TEXT NOT NULL);
        CREATE TRIGGER custom_creation_trigger AFTER INSERT ON product_creation_jobs BEGIN INSERT INTO trigger_witness VALUES(NEW.request_id); END;
        CREATE TRIGGER custom_rename_trigger AFTER INSERT ON product_conversation_rename_receipts BEGIN INSERT INTO trigger_witness VALUES(NEW.operation_id); END;
        CREATE TABLE child_witness(request_id TEXT REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE, value TEXT NOT NULL);")
        .execute(&pool).await.unwrap();
    (pool, local)
}

async fn seed_creation(pool: &SqlitePool, request: &str, kind: &str) {
    sqlx::query("INSERT INTO product_creation_jobs(request_id,product_conversation_id,cwd,objective,llm_language,status,accepted_at_unix_micros,updated_at_unix_micros,objective_origin,objective_source_product_conversation_id,objective_source_transcript_id,objective_source_message_id,objective_source_tool_use_id) VALUES(?1,?2,'/repo','objective','phoenix-native','accepted',1,1,?3,?4,?5,?6,?7)")
        .bind(request).bind(format!("product-{request}")).bind(kind)
        .bind((kind == "internal_conversation").then_some("historical-source-product"))
        .bind((kind == "internal_conversation").then_some("historical-source-transcript"))
        .bind((kind == "internal_conversation").then_some("historical-source-message"))
        .bind((kind == "internal_conversation").then_some("historical-source-tool"))
        .execute(pool).await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn migration158_qualifies_only_complete_local_native_history_and_preserves_schema() {
    let (pool, local) = fixture().await;
    for (request, kind) in [
        ("local", "internal_conversation"),
        ("api", "user_api"),
        ("absent", "unknown_historical"),
    ] {
        seed_creation(&pool, request, kind).await;
    }
    sqlx::query("UPDATE product_creation_jobs SET objective_source_product_conversation_id='  product:α  ', objective_source_transcript_id=' transcript/root ', objective_source_message_id='message:assistant', objective_source_tool_use_id='tool/use' WHERE request_id='local'")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO product_conversation_rename_receipts VALUES('operation','target',0,'title','  source:β  ',' transcript/root ','message:assistant','tool/use','deleted-root','retained-slug',10);
        INSERT INTO child_witness VALUES('local','preserved child');")
        .execute(&pool).await.unwrap();
    let before: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let migration = Migration {
        version: 158,
        name: "federated_conversation_actions",
        sql: "",
    };
    run(&pool, &migration).await.unwrap();
    let qualifications: Vec<(String, Option<String>)> = sqlx::query_as("SELECT request_id,objective_source_instance_id FROM product_creation_jobs ORDER BY request_id").fetch_all(&pool).await.unwrap();
    assert_eq!(
        qualifications,
        vec![
            ("absent".into(), None),
            ("api".into(), None),
            ("local".into(), Some(local.to_string()))
        ]
    );
    let creation_source: (String, String, String, String) = sqlx::query_as("SELECT objective_source_product_conversation_id,objective_source_transcript_id,objective_source_message_id,objective_source_tool_use_id FROM product_creation_jobs WHERE request_id='local'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        creation_source,
        (
            "  product:α  ".into(),
            " transcript/root ".into(),
            "message:assistant".into(),
            "tool/use".into()
        )
    );
    let receipt: (String, String, String, String, String, String) = sqlx::query_as("SELECT source_instance_id,source_product_conversation_id,source_transcript_id,source_message_id,source_tool_use_id,slug_at_commit FROM product_conversation_rename_receipts WHERE operation_id='operation'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        receipt,
        (
            local.to_string(),
            "  source:β  ".into(),
            " transcript/root ".into(),
            "message:assistant".into(),
            "tool/use".into(),
            "retained-slug".into()
        )
    );
    let after: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (kind, name, sql) in &before {
        let changed = after
            .iter()
            .find(|item| &item.0 == kind && &item.1 == name)
            .unwrap();
        if name != "product_creation_jobs" && name != "product_conversation_rename_receipts" {
            assert_eq!(&changed.2, sql, "schema object {name} changed");
        } else {
            let mut conn = pool.acquire().await.unwrap();
            let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
                sqlx::query_as(sqlx::AssertSqlSafe(format!("PRAGMA table_info({name})")))
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(columns.last().unwrap().2, "TEXT");
        }
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM child_witness")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trigger_witness")
            .fetch_one(&pool)
            .await
            .unwrap(),
        4
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    for invalid in [format!("{local}\0"), format!("{local}\0suffix")] {
        assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_instance_id=?1 WHERE request_id='local'")
            .bind(&invalid).execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id=?1 WHERE operation_id='operation'")
            .bind(&invalid).execute(&pool).await.is_err());
    }
    // New foreign provenance is valid without creating any destination-local source rows.
    sqlx::query("UPDATE product_creation_jobs SET objective_origin='federated_conversation',objective_source_instance_id=?1 WHERE request_id='local'").bind(InstanceId::new().to_string()).execute(&pool).await.unwrap();
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_instance_id=NULL WHERE request_id='local'").execute(&pool).await.is_err());
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_instance_id='invalid' WHERE request_id='local'").execute(&pool).await.is_err());
    assert!(sqlx::query(
        "UPDATE product_creation_jobs SET objective_source_instance_id=?1 WHERE request_id='api'"
    )
    .bind(local.to_string())
    .execute(&pool)
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE product_creation_jobs SET attempt_count=0 WHERE request_id='api'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query(
        "UPDATE product_creation_jobs SET published_product_id='missing' WHERE request_id='api'"
    )
    .execute(&pool)
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id=NULL WHERE operation_id='operation'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE product_conversation_rename_receipts SET name_revision_before=-1")
            .execute(&pool)
            .await
            .is_err()
    );
    for invalid in ["", "  ", "\t", "\u{2003}", "tool\0suffix"] {
        assert!(sqlx::query("UPDATE product_conversation_rename_receipts SET source_tool_use_id=?1 WHERE operation_id='operation'")
            .bind(invalid).execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_message_id=?1 WHERE request_id='local'")
            .bind(invalid).execute(&pool).await.is_err());
    }
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_message_id=NULL WHERE request_id='local'")
        .execute(&pool).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version=158")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[test]
fn migration158_preserves_original_constraint_bodies_and_does_not_add_source_fks() {
    let original = table_sql(
        include_str!("../retain_product_creation_receipts.sql"),
        "product_creation_jobs_phase1",
        "product_creation_jobs",
    );
    let expanded = qualify_schema(&original, true).unwrap();
    let expected_original = original.replace(CREATE_KIND_CHECK,"objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation', 'federated_conversation')").replace(CREATE_NATIVE_CHECK,"objective_origin IN ('internal_conversation', 'federated_conversation')");
    let end = expected_original.rfind(')').unwrap();
    assert!(expanded.starts_with(expected_original.get(..end).unwrap()));
    assert_eq!(
        expanded.matches("REFERENCES").count(),
        original.matches("REFERENCES").count()
    );
    let original = table_sql(
        include_str!("../retain_product_conversation_rename_receipts.sql"),
        "product_conversation_rename_receipts_retained",
        "product_conversation_rename_receipts",
    );
    let expanded = qualify_schema(&original, false).unwrap();
    let mut expected_original = original;
    for column in [
        "source_product_conversation_id",
        "source_transcript_id",
        "source_message_id",
        "source_tool_use_id",
    ] {
        expected_original = expected_original.replace(&format!("AND length({column}) > 0"), "");
        assert!(expanded.contains(&format!(
            "{column} TEXT NOT NULL CHECK(typeof({column}) = 'text'"
        )));
    }
    let end = expected_original.rfind(')').unwrap();
    assert!(expanded.starts_with(expected_original.get(..end).unwrap()));
    assert!(!expanded.contains("REFERENCES"));
    assert!(qualify_schema("CREATE TABLE unexpected(x TEXT)", true).is_err());
    assert!(qualify_schema("CREATE TABLE unexpected(x TEXT)", false).is_err());
}

#[tokio::test]
async fn migration158_failure_rolls_back_columns_and_restores_schema_guard() {
    let (pool, _) = fixture().await;
    sqlx::query(
        "ALTER TABLE product_creation_jobs RENAME COLUMN objective_origin TO unexpected_origin",
    )
    .execute(&pool)
    .await
    .unwrap();
    let migration = Migration {
        version: 158,
        name: "federated_conversation_actions",
        sql: "",
    };
    assert!(run(&pool, &migration).await.is_err());
    for table in [
        "product_creation_jobs",
        "product_conversation_rename_receipts",
    ] {
        let columns: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT name FROM pragma_table_info('{table}')"
        )))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!columns
            .iter()
            .any(|name| name == "objective_source_instance_id" || name == "source_instance_id"));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn migration158_rejects_incomplete_historical_locators_transactionally() {
    let (pool, _) = fixture().await;
    seed_creation(&pool, "incomplete", "internal_conversation").await;
    sqlx::query("PRAGMA ignore_check_constraints=ON")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE product_creation_jobs SET objective_source_product_conversation_id=?1, objective_source_message_id=?2, objective_source_tool_use_id=?3 WHERE request_id='incomplete'")
        .bind("source\0suffix")
        .bind("\t")
        .bind("\u{2003}")
        .execute(&pool).await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints=OFF")
        .execute(&pool)
        .await
        .unwrap();
    let migration = Migration {
        version: 158,
        name: "federated_conversation_actions",
        sql: "",
    };
    assert!(run(&pool, &migration).await.is_err());
    let creation_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('product_creation_jobs')")
            .fetch_all(&pool)
            .await
            .unwrap();
    let rename_columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('product_conversation_rename_receipts')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!creation_columns
        .iter()
        .any(|name| name == "objective_source_instance_id"));
    assert!(!rename_columns
        .iter()
        .any(|name| name == "source_instance_id"));
    let source_locators: (String, String, String) = sqlx::query_as("SELECT objective_source_product_conversation_id, objective_source_message_id, objective_source_tool_use_id FROM product_creation_jobs WHERE request_id='incomplete'")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(
        source_locators,
        ("source\0suffix".into(), "\t".into(), "\u{2003}".into())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

async fn assert_unqualified_rename_locators(
    pool: &SqlitePool,
    operation_id: &str,
    expected: (&str, &str, &str, &str),
) {
    let retained: (String, String, String, String, Option<String>) = sqlx::query_as(
        "SELECT source_product_conversation_id,source_transcript_id,source_message_id,source_tool_use_id,source_instance_id FROM product_conversation_rename_receipts WHERE operation_id=?1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        retained,
        (
            expected.0.into(),
            expected.1.into(),
            expected.2.into(),
            expected.3.into(),
            None
        )
    );
}

#[tokio::test]
async fn migration158_preserves_malformed_rename_locators_as_unqualified_history() {
    let (pool, local) = fixture().await;
    let whitespace = ("\t", "\n", "\u{2003}", "\r\n");
    let incomplete = ("", "  ", "message", "tool");
    let bytes = ("\0", "\0transcript", "message\0suffix", "工具");
    let mut conn = pool.acquire().await.unwrap();
    // The empty and NUL-prefixed historical values violate the old length checks.
    sqlx::query("PRAGMA ignore_check_constraints=ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO product_conversation_rename_receipts VALUES('legacy','target',0,'title',?1,?2,?3,?4,'root','slug',10),('incomplete','target',0,'title',?5,?6,?7,?8,'root','slug',10),('bytes','target',0,'title',?9,?10,?11,?12,'root','slug',10)")
        .bind(whitespace.0).bind(whitespace.1).bind(whitespace.2).bind(whitespace.3)
        .bind(incomplete.0).bind(incomplete.1).bind(incomplete.2).bind(incomplete.3)
        .bind(bytes.0).bind(bytes.1).bind(bytes.2).bind(bytes.3)
        .execute(&mut *conn).await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints=OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    run(
        &pool,
        &Migration {
            version: 158,
            name: "federated_conversation_actions",
            sql: "",
        },
    )
    .await
    .unwrap();
    assert_unqualified_rename_locators(&pool, "legacy", whitespace).await;
    assert_unqualified_rename_locators(&pool, "incomplete", incomplete).await;
    assert_unqualified_rename_locators(&pool, "bytes", bytes).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA ignore_check_constraints")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    for operation_id in ["legacy", "incomplete", "bytes"] {
        assert!(sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id=?1 WHERE operation_id=?2")
            .bind(local.to_string())
            .bind(operation_id)
            .execute(&pool)
            .await
            .is_err());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version=158")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert!(run(
        &pool,
        &Migration {
            version: 158,
            name: "federated_conversation_actions",
            sql: ""
        }
    )
    .await
    .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version=158")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let source_instance: Option<String> = sqlx::query_scalar("SELECT source_instance_id FROM product_conversation_rename_receipts WHERE operation_id='incomplete'").fetch_one(&pool).await.unwrap();
    assert_eq!(source_instance, None);
}
