use super::*;
use phoenix_core::domain::instance_identity::InstanceId;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

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
    let local = initialize_fixture(&pool).await;
    (pool, local)
}

async fn initialize_fixture(pool: &SqlitePool) -> InstanceId {
    sqlx::raw_sql("PRAGMA foreign_keys=ON;
        CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE instance_identity(singleton_key INTEGER PRIMARY KEY CHECK(singleton_key=1),instance_id TEXT NOT NULL);
        CREATE TABLE products(id TEXT PRIMARY KEY);
        CREATE TABLE conversations(id TEXT PRIMARY KEY);
        CREATE TABLE slack_gateway_requests(request_id TEXT PRIMARY KEY);")
        .execute(pool).await.unwrap();
    let local = InstanceId::new();
    sqlx::query("INSERT INTO instance_identity VALUES(1,?1)")
        .bind(local.to_string())
        .execute(pool)
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
        .execute(pool)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(rename))
        .execute(pool)
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
        .execute(pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE INDEX custom_creation_index ON product_creation_jobs(objective);
        CREATE INDEX custom_rename_index ON product_conversation_rename_receipts(title);
        CREATE TABLE trigger_witness(value TEXT NOT NULL);
        CREATE TRIGGER custom_creation_trigger AFTER INSERT ON product_creation_jobs BEGIN INSERT INTO trigger_witness VALUES(NEW.request_id); END;
        CREATE TRIGGER custom_rename_trigger AFTER INSERT ON product_conversation_rename_receipts BEGIN INSERT INTO trigger_witness VALUES(NEW.operation_id); END;
        CREATE TABLE child_witness(request_id TEXT REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE, value TEXT NOT NULL);")
        .execute(pool).await.unwrap();
    local
}

async fn file_fixture(path: &std::path::Path) -> (SqlitePool, InstanceId) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    let local = initialize_fixture(&pool).await;
    (pool, local)
}

async fn reopen_fixture(path: &std::path::Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
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
    sqlx::query("UPDATE product_creation_jobs SET objective_origin='historical_internal', objective_source_instance_id=NULL WHERE request_id='local'")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_creation_jobs SET objective_origin='internal_conversation', objective_source_instance_id=?1 WHERE request_id='local'")
        .bind(local.to_string()).execute(&pool).await.unwrap();
    assert!(
        sqlx::query("UPDATE product_creation_jobs SET objective_origin='federated_conversation', objective_source_message_id=CAST('message' AS BLOB) WHERE request_id='local'")
            .execute(&pool)
            .await
            .is_err()
    );
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
    sqlx::query("INSERT INTO slack_gateway_requests VALUES('unexpected-slack')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query(
        "UPDATE product_creation_jobs SET objective_slack_request_id='unexpected-slack' WHERE request_id='api'"
    )
    .execute(&pool)
    .await
    .is_err());
    sqlx::query("INSERT INTO slack_gateway_requests VALUES(CAST(x'736c61636b2d626c6f62' AS BLOB))")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query(
        "UPDATE product_creation_jobs SET objective_origin='slack_gateway', objective_slack_request_id=CAST(x'736c61636b2d626c6f62' AS BLOB) WHERE request_id='api'"
    )
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
    sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id=NULL WHERE operation_id='operation'")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id=?1 WHERE operation_id='operation'")
        .bind(local.to_string()).execute(&pool).await.unwrap();
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

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn migration158_schema_and_historical_text_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("federated-conversation-actions.sqlite");
    let (pool, _) = file_fixture(&path).await;
    seed_creation(&pool, "historical-create", "internal_conversation").await;
    sqlx::query(
        "UPDATE product_creation_jobs
         SET objective_source_product_conversation_id=CAST(x'ff' AS TEXT)
         WHERE request_id='historical-create'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO product_conversation_rename_receipts VALUES
         ('historical-rename','target',0,'title',CAST(x'fe' AS TEXT),
          'transcript','message','tool','root','slug',1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let migration = Migration {
        version: 158,
        name: "federated_conversation_actions",
        sql: "",
    };
    run(&pool, &migration).await.unwrap();
    pool.close().await;

    let pool = reopen_fixture(&path).await;
    let creation: (String, Option<String>, String, Vec<u8>) = sqlx::query_as(
        "SELECT objective_origin, objective_source_instance_id,
                typeof(objective_source_product_conversation_id),
                CAST(objective_source_product_conversation_id AS BLOB)
         FROM product_creation_jobs WHERE request_id='historical-create'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(creation.0, "historical_internal");
    assert_eq!(creation.1, None);
    assert_eq!(creation.2, "text");
    assert_eq!(creation.3, vec![0xff]);

    let rename: (Option<String>, String, Vec<u8>) = sqlx::query_as(
        "SELECT source_instance_id, typeof(source_product_conversation_id),
                CAST(source_product_conversation_id AS BLOB)
         FROM product_conversation_rename_receipts
         WHERE operation_id='historical-rename'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rename.0, None);
    assert_eq!(rename.1, "text");
    assert_eq!(rename.2, vec![0xfe]);

    assert!(sqlx::query(
        "UPDATE product_creation_jobs
         SET objective_source_product_conversation_id=CAST('blob' AS BLOB)
         WHERE request_id='historical-create'",
    )
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE product_conversation_rename_receipts
         SET source_product_conversation_id=CAST('blob' AS BLOB)
         WHERE operation_id='historical-rename'",
    )
    .execute(&pool)
    .await
    .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_all(&pool)
            .await
            .unwrap(),
        vec!["ok"]
    );
}

#[test]
fn migration158_rewrites_full_provenance_checks_exactly() {
    let original = table_sql(
        include_str!("../retain_product_creation_receipts.sql"),
        "product_creation_jobs_phase1",
        "product_creation_jobs",
    );
    let expanded = qualify_schema(&original, true).unwrap();
    let expected = original
        .replace(
            CREATE_KIND_CHECK,
            "objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation', 'historical_internal', 'federated_conversation')",
        )
        .replace(CREATE_LEGACY_PROVENANCE_CHECK, &creation_provenance_check());
    assert_eq!(expanded, expected);
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
        assert!(expanded.contains(&format!(
            "{column} TEXT NOT NULL CHECK(typeof({column}) = 'text'\n        AND length({column}) > 0)"
        )));
    }
    let check = rename_provenance_check();
    let end = expected_original.rfind(')').unwrap();
    expected_original.insert_str(end, &format!(",\n    {check}\n"));
    assert_eq!(expanded, expected_original);
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
async fn migration158_reclassifies_legacy_incomplete_creation_locators_as_historical_internal() {
    let (pool, local) = fixture().await;
    seed_creation(&pool, "incomplete", "internal_conversation").await;
    let legacy_locators = ("source\0suffix", "\t", "\u{2003}", "tool");
    sqlx::query("UPDATE product_creation_jobs SET objective_source_product_conversation_id=?1, objective_source_transcript_id=?2, objective_source_message_id=?3, objective_source_tool_use_id=?4 WHERE request_id='incomplete'")
        .bind(legacy_locators.0)
        .bind(legacy_locators.1)
        .bind(legacy_locators.2)
        .bind(legacy_locators.3)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
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
    let provenance: (String, String, String, String, String, Option<String>) = sqlx::query_as("SELECT objective_origin, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, objective_source_instance_id FROM product_creation_jobs WHERE request_id='incomplete'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        provenance,
        (
            "historical_internal".into(),
            legacy_locators.0.into(),
            legacy_locators.1.into(),
            legacy_locators.2.into(),
            legacy_locators.3.into(),
            None,
        )
    );
    sqlx::query("UPDATE product_creation_jobs SET objective='retained historical objective' WHERE request_id='incomplete'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_origin='internal_conversation' WHERE request_id='incomplete'")
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_origin='federated_conversation', objective_source_instance_id=?1 WHERE request_id='incomplete'")
        .bind(local.to_string())
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_source_instance_id=?1 WHERE request_id='incomplete'")
        .bind(local.to_string())
        .execute(&pool)
        .await
        .is_err());
    sqlx::query("INSERT INTO slack_gateway_requests VALUES('slack-request')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query("UPDATE product_creation_jobs SET objective_slack_request_id='slack-request' WHERE request_id='incomplete'")
        .execute(&pool).await.is_err());
    for column in [
        "objective_source_product_conversation_id",
        "objective_source_transcript_id",
        "objective_source_message_id",
        "objective_source_tool_use_id",
    ] {
        for value in ["NULL", "CAST('blob' AS BLOB)", "'   '"] {
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE product_creation_jobs SET {column}={value} WHERE request_id='incomplete'"
            )))
                .execute(&pool)
                .await
                .is_err(),
                "historical_internal accepted {column}={value}"
            );
        }
    }
    let retained: (String, String, String, String) = sqlx::query_as("SELECT objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id FROM product_creation_jobs WHERE request_id='incomplete'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(
        retained,
        (
            legacy_locators.0.into(),
            legacy_locators.1.into(),
            legacy_locators.2.into(),
            legacy_locators.3.into()
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
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
async fn migration158_preserves_predecessor_valid_incomplete_rename_locators_unqualified() {
    let (pool, local) = fixture().await;
    let whitespace = ("\t", "\n", "\u{2003}", "\r\n");
    let bytes = ("source\0suffix", "transcript", "message", "工具");
    sqlx::query("INSERT INTO product_conversation_rename_receipts VALUES('legacy','target',0,'title',?1,?2,?3,?4,'root','slug',10),('bytes','target',0,'title',?5,?6,?7,?8,'root','slug',10)")
        .bind(whitespace.0).bind(whitespace.1).bind(whitespace.2).bind(whitespace.3)
        .bind(bytes.0).bind(bytes.1).bind(bytes.2).bind(bytes.3)
        .execute(&pool).await.unwrap();
    for invalid in ["", "\0suffix"] {
        assert!(sqlx::query("UPDATE product_conversation_rename_receipts SET source_message_id=?1 WHERE operation_id='legacy'")
            .bind(invalid).execute(&pool).await.is_err());
    }
    assert!(sqlx::query("UPDATE product_conversation_rename_receipts SET source_message_id=CAST('message' AS BLOB) WHERE operation_id='legacy'")
        .execute(&pool).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
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
    assert_unqualified_rename_locators(&pool, "bytes", bytes).await;
    for operation_id in ["legacy", "bytes"] {
        sqlx::query("UPDATE product_conversation_rename_receipts SET title='retained historical title' WHERE operation_id=?1")
            .bind(operation_id).execute(&pool).await.unwrap();
    }
    assert_unqualified_rename_locators(&pool, "legacy", whitespace).await;
    assert_unqualified_rename_locators(&pool, "bytes", bytes).await;
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    for operation_id in ["legacy", "bytes"] {
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
    assert_unqualified_rename_locators(&pool, "legacy", whitespace).await;
    assert_unqualified_rename_locators(&pool, "bytes", bytes).await;
}

#[tokio::test]
async fn migration158_preserves_invalid_utf8_creation_text_without_qualifying_it() {
    let (pool, _) = fixture().await;
    let columns = [
        "objective_source_product_conversation_id",
        "objective_source_transcript_id",
        "objective_source_message_id",
        "objective_source_tool_use_id",
    ];
    for (index, column) in columns.iter().enumerate() {
        let request = format!("invalid-creation-{index}");
        seed_creation(&pool, &request, "internal_conversation").await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE product_creation_jobs SET {column}=CAST(x'6162FF' AS TEXT) WHERE request_id=?1"
        )))
        .bind(&request)
        .execute(&pool)
        .await
        .unwrap();
    }
    let before: Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT request_id, CAST(objective_source_product_conversation_id AS BLOB), CAST(objective_source_transcript_id AS BLOB), CAST(objective_source_message_id AS BLOB), CAST(objective_source_tool_use_id AS BLOB) FROM product_creation_jobs ORDER BY request_id"
    ).fetch_all(&pool).await.unwrap();
    for (index, row) in before.iter().enumerate() {
        let locators = [&row.1, &row.2, &row.3, &row.4];
        assert_eq!(locators[index].as_slice(), b"ab\xff");
    }
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
    for (index, column) in columns.iter().enumerate() {
        let request = format!("invalid-creation-{index}");
        let (origin, instance, kind): (String, Option<String>, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT objective_origin, objective_source_instance_id, typeof({column}) FROM product_creation_jobs WHERE request_id=?1"
        ))).bind(&request).fetch_one(&pool).await.unwrap();
        assert_eq!(origin, "historical_internal");
        assert_eq!(instance, None);
        assert_eq!(kind, "text");
        sqlx::query(
            "UPDATE product_creation_jobs SET objective='still historical' WHERE request_id=?1",
        )
        .bind(&request)
        .execute(&pool)
        .await
        .unwrap();
    }
    let after: Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT request_id, CAST(objective_source_product_conversation_id AS BLOB), CAST(objective_source_transcript_id AS BLOB), CAST(objective_source_message_id AS BLOB), CAST(objective_source_tool_use_id AS BLOB) FROM product_creation_jobs ORDER BY request_id"
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(after, before);
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
}

#[tokio::test]
async fn migration158_preserves_invalid_utf8_rename_text_without_qualifying_it() {
    let (pool, _) = fixture().await;
    let columns = [
        "source_product_conversation_id",
        "source_transcript_id",
        "source_message_id",
        "source_tool_use_id",
    ];
    for (index, column) in columns.iter().enumerate() {
        let operation = format!("invalid-rename-{index}");
        sqlx::query("INSERT INTO product_conversation_rename_receipts VALUES(?1,'target',0,'title','source-product','source-transcript','source-message','source-tool','root','slug',10)")
            .bind(&operation).execute(&pool).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE product_conversation_rename_receipts SET {column}=CAST(x'6162FF' AS TEXT) WHERE operation_id=?1"
        )))
        .bind(&operation).execute(&pool).await.unwrap();
    }
    let before: Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT operation_id, CAST(source_product_conversation_id AS BLOB), CAST(source_transcript_id AS BLOB), CAST(source_message_id AS BLOB), CAST(source_tool_use_id AS BLOB) FROM product_conversation_rename_receipts ORDER BY operation_id"
    ).fetch_all(&pool).await.unwrap();
    for (index, row) in before.iter().enumerate() {
        let locators = [&row.1, &row.2, &row.3, &row.4];
        assert_eq!(locators[index].as_slice(), b"ab\xff");
    }
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
    for (index, column) in columns.iter().enumerate() {
        let operation = format!("invalid-rename-{index}");
        let (instance, kind): (Option<String>, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT source_instance_id, typeof({column}) FROM product_conversation_rename_receipts WHERE operation_id=?1"
        ))).bind(&operation).fetch_one(&pool).await.unwrap();
        assert_eq!(instance, None);
        assert_eq!(kind, "text");
        sqlx::query("UPDATE product_conversation_rename_receipts SET title='still historical' WHERE operation_id=?1")
            .bind(&operation).execute(&pool).await.unwrap();
    }
    let after: Vec<(String, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT operation_id, CAST(source_product_conversation_id AS BLOB), CAST(source_transcript_id AS BLOB), CAST(source_message_id AS BLOB), CAST(source_tool_use_id AS BLOB) FROM product_conversation_rename_receipts ORDER BY operation_id"
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(after, before);
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
}
