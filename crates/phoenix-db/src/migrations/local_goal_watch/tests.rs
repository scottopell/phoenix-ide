use super::*;
use crate::Database;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Row;

async fn run_through_capture(pool: &sqlx::SqlitePool) -> DbResult<u32> {
    let end = super::super::MIGRATIONS
        .iter()
        .position(|migration| migration.version == 135)
        .unwrap()
        + 1;
    super::super::run_migration_list(pool, &super::super::MIGRATIONS[..end]).await
}

async fn fixture() -> (Database, String) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool, "migration-135-memory".into());
    let index = super::super::MIGRATIONS
        .iter()
        .position(|m| m.version == 135)
        .unwrap();
    db.run_migrations().await.unwrap();
    super::super::run_migration_list(db.pool(), &super::super::MIGRATIONS[..index])
        .await
        .unwrap();
    let source = db
        .create_conversation("pre135", "pre135", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    let watch: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id=?",
    )
    .bind(source.product_conversation_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    for (id, kind, outcome, reason) in [
        ("old-terminal", "creation", "completed", None),
        (
            "old-question",
            "question_request",
            "awaiting_user_response",
            Some("question_request"),
        ),
        (
            "old-approval",
            "task_approval_wait",
            "awaiting_task_approval",
            Some("task_approval_wait"),
        ),
    ] {
        sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,terminal_reason,occurred_at_us) VALUES (?,?,?,?,0,?,?,?,(SELECT max(enrolled_at_us)+1 FROM coordinator_watches))")
            .bind(id).bind(watch).bind(kind).bind(id).bind(&source.id).bind(outcome).bind(reason).execute(db.pool()).await.unwrap();
    }
    let target = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let events = ["old-terminal", "old-question", "old-approval"];
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at,origin_kind,origin_subscription_event_id) VALUES ('historical-message',?1,1,'user','{ \"text\": \"preserve exact bytes\" }','2026-01-01T00:00:00Z','subscription_event',?2)")
        .bind(&target.id).bind(events[0]).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO steering_messages(message_id,conversation_id,ordinal,text,origin_kind,origin_subscription_event_id) VALUES ('historical-steering',?1,0,'preserve steering','subscription_event',?2)")
        .bind(&target.id).bind(events[1]).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,7,'Active','direct_turn.snapshot',1,X'0001FF',10,20)").execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,origin_subscription_event_id) VALUES (1,1,?1,'historical-key','historical-fingerprint',X'FF000A','Steering',7,0,'subscription_event',?2)")
        .bind(&target.id).bind(events[2]).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,delivery_state) VALUES ('retired-wake',?,'wake','old-wake',0,?,'completed',20,'suppressed')")
        .bind(watch).bind(&source.id).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO coordinator_event_feature_retirements(event_id,reason) VALUES ('retired-wake','WakeFeatureRetired')").execute(db.pool()).await.unwrap();
    let closing = db
        .create_conversation("closing", "closing", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO close_obligations(attempt_id,product_conversation_id,phase,created_at,updated_at) VALUES ('close-attempt',?,'awaiting_blocker_resolution','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')")
        .bind(closing.product_conversation_id.as_str()).execute(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO close_cleanup_failures(failure_occurrence_id,attempt_id,cleanup_run_ordinal,source_product_conversation_id,reason,detail,stop_certainty,occurred_at_us,authority_kind) VALUES ('close-failure:1:close-attempt','close-attempt',1,?,'interrupted','retained exact failure','shutdown_uncertain',30,'attempt_interrupted')")
        .bind(closing.product_conversation_id.as_str()).execute(&mut *tx).await.unwrap();
    assert!(
        crate::coordinator_watches::append_mandatory_close_failure_event_tx(
            &mut tx,
            "close-failure:1:close-attempt",
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    (db, source.id)
}

async fn schema(pool: &SqlitePool) -> Vec<(String, String, String, i64)> {
    sqlx::query_as("SELECT type,name,coalesce(sql,''),rootpage FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")
        .fetch_all(pool).await.unwrap()
}

async fn ledger(pool: &SqlitePool) -> Vec<(i64, String, String)> {
    sqlx::query_as("SELECT version,name,applied_at FROM _migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn retained_rows(pool: &SqlitePool) -> Vec<(String, Vec<Vec<String>>)> {
    let mut snapshot = Vec::new();
    for table in [
        "coordinator_watch_events",
        "coordinator_watches",
        "subscription_delivery_identities",
        "messages",
        "steering_messages",
        "durable_turns",
        "workflows",
        "coordinator_event_feature_retirements",
        "close_obligations",
        "close_runs",
        "close_cleanup_failures",
        "conversation_goal_ids",
        "conversation_goal_history",
        "conversation_goal_actors",
        "conversation_goal_bindings",
    ] {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info(?) WHERE name NOT IN ('goal_capture_kind','goal_capture_event_id') ORDER BY cid",
        ).bind(table).fetch_all(pool).await.unwrap();
        assert!(!columns.is_empty(), "missing retained table {table}");
        let values = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect::<Vec<_>>()
            .join(",");
        let order = (1..=columns.len())
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {values} FROM {table} ORDER BY {order}"
        )))
        .fetch_all(pool)
        .await
        .unwrap();
        snapshot.push((
            table.to_owned(),
            rows.iter()
                .map(|row| {
                    (0..columns.len())
                        .map(|index| row.get::<String, _>(index))
                        .collect()
                })
                .collect(),
        ));
    }
    snapshot
}

#[tokio::test]
async fn populated_forward_upgrade_preserves_outbox_roots_delivery_references_and_legacy_facts() {
    let (db, _) = fixture().await;
    let before_schema = schema(db.pool()).await;
    let before_rows = retained_rows(db.pool()).await;
    let before_ledger = ledger(db.pool()).await;
    let identities: Vec<(String,Option<String>,Option<String>)> = sqlx::query_as("SELECT id,local_event_id,remote_admission_id FROM subscription_delivery_identities ORDER BY id")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(identities.len(), 5);
    let before: Vec<(String,String,String,String,Option<String>)> = sqlx::query_as("SELECT event_id,source_occurrence_kind,terminal_kind,delivery_state,accepted_transcript_id FROM coordinator_watch_events ORDER BY event_id")
        .fetch_all(db.pool()).await.unwrap();
    run_through_capture(db.pool()).await.unwrap();
    let after_schema = schema(db.pool()).await;
    assert_eq!(retained_rows(db.pool()).await, before_rows);
    for (kind, name, sql, root) in before_schema {
        let after = after_schema
            .iter()
            .find(|row| row.0 == kind && row.1 == name)
            .unwrap();
        assert_eq!(after.3, root, "root moved: {name}");
        if name != "coordinator_watch_events" {
            assert_eq!(&after.2, &sql, "prior object changed: {name}");
        }
    }
    let after_ledger = ledger(db.pool()).await;
    assert_eq!(&after_ledger[..before_ledger.len()], &before_ledger);
    assert_eq!(after_ledger.last().unwrap().0, 135);
    let after: Vec<(String,String,String,String,Option<String>)> = sqlx::query_as("SELECT event_id,source_occurrence_kind,terminal_kind,delivery_state,accepted_transcript_id FROM coordinator_watch_events ORDER BY event_id")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(before, after);
    let after_ids: Vec<(String,Option<String>,Option<String>)> = sqlx::query_as("SELECT id,local_event_id,remote_admission_id FROM subscription_delivery_identities ORDER BY id")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(identities, after_ids);
    for table in ["messages", "steering_messages", "durable_turns"] {
        let preserved = after_schema
            .iter()
            .find(|row| row.1 == table && row.0 == "table")
            .unwrap();
        assert!(preserved
            .2
            .contains("REFERENCES subscription_delivery_identities(id)"));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM coordinator_event_feature_retirements")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM coordinator_watch_goal_captures")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    let presentations = db
        .coordinator_watch_event_presentations(&[
            "old-terminal".into(),
            "old-question".into(),
            "old-approval".into(),
        ])
        .await
        .unwrap();
    assert!(presentations.values().all(|event| matches!(
        event.goal_capture,
        Some(phoenix_core::domain::watch_goal::WatchGoalCapture::LegacyNotCaptured)
    )));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pragma_foreign_key_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn forced_after_schema_and_ddl_failure_restores_everything_and_can_retry() {
    let (db, _) = fixture().await;
    let before = schema(db.pool()).await;
    let before_rows = retained_rows(db.pool()).await;
    let before_version: i64 = sqlx::query_scalar("PRAGMA schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let before_ledger = ledger(db.pool()).await;
    let sql = concat!(
        include_str!("../../coordinator_watches/goal_capture.sql"),
        "\nSELECT missing_migration_135_function();"
    );
    let failed = Migration {
        version: 135,
        name: "local_watch_goal_capture",
        sql,
    };
    assert!(run(db.pool(), &failed).await.is_err());
    assert_eq!(schema(db.pool()).await, before);
    assert_eq!(retained_rows(db.pool()).await, before_rows);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA schema_version")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        before_version
    );
    assert_eq!(ledger(db.pool()).await, before_ledger);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    run_through_capture(db.pool()).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn unexpected_check_fails_before_partial_new_schema() {
    let empty = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE coordinator_watch_events(event_id TEXT PRIMARY KEY)")
        .execute(&empty)
        .await
        .unwrap();
    let migration = super::super::MIGRATIONS
        .iter()
        .find(|m| m.version == 135)
        .unwrap();
    assert!(run(&empty, migration).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(&empty)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_schema WHERE name='coordinator_watch_goal_captures'"
        )
        .fetch_one(&empty)
        .await
        .unwrap(),
        0
    );
}

const CACHED_INSERT: &str = "INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) VALUES (?,?,?,?,?,?,?,?)";
async fn assert_cached_shapes_rejected(
    retained: &mut sqlx::SqliteConnection,
    watch: i64,
    source: &crate::Conversation,
) {
    let old_shape_error = sqlx::query(CACHED_INSERT)
        .bind("cached-goal-change")
        .bind(watch)
        .bind("goal_change")
        .bind(source.product_conversation_id.as_str())
        .bind(1_i64)
        .bind(&source.id)
        .bind("goal_changed")
        .bind(3_i64)
        .execute(&mut *retained)
        .await
        .unwrap_err();
    assert!(old_shape_error
        .to_string()
        .contains("goal capture requires eligible local subscription"));
    let packet_shape_error = sqlx::query(CACHED_INSERT)
        .bind("cached-wrong-packet")
        .bind(watch)
        .bind("creation")
        .bind("cached-wrong-packet")
        .bind(0_i64)
        .bind(&source.id)
        .bind("goal_changed")
        .bind(3_i64)
        .execute(&mut *retained)
        .await
        .unwrap_err();
    let packet_shape_error = packet_shape_error.as_database_error().unwrap();
    assert_eq!(packet_shape_error.code().as_deref(), Some("275"));
    assert!(packet_shape_error
        .message()
        .starts_with("CHECK constraint failed:"));
    assert!(packet_shape_error
        .message()
        .contains("((source_occurrence_kind = 'goal_change') = (terminal_kind = 'goal_changed'))"));
}

#[tokio::test]
async fn another_pooled_connection_reparses_the_new_schema() {
    let dir = tempfile::tempdir().unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.path().join("watch.db"))
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    let db = Database::from_pool_for_tests(pool.clone(), "migration-135-file".into());
    db.run_migrations().await.unwrap();
    let index = super::super::MIGRATIONS
        .iter()
        .position(|m| m.version == 135)
        .unwrap();
    super::super::run_migration_list(&pool, &super::super::MIGRATIONS[..index])
        .await
        .unwrap();
    let source = db
        .create_conversation("cache135", "cache135", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    let watch: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id=?",
    )
    .bind(source.product_conversation_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut retained = pool.acquire().await.unwrap();
    let before: i64 = sqlx::query_scalar("PRAGMA schema_version")
        .fetch_one(&mut *retained)
        .await
        .unwrap();
    sqlx::query(CACHED_INSERT)
        .bind("cached-before")
        .bind(watch)
        .bind("creation")
        .bind("cached-before")
        .bind(0_i64)
        .bind(&source.id)
        .bind("completed")
        .bind(1_i64)
        .execute(&mut *retained)
        .await
        .unwrap();
    sqlx::query("SELECT event_id,terminal_kind FROM coordinator_watch_events")
        .fetch_all(&mut *retained)
        .await
        .unwrap();
    run_through_capture(&pool).await.unwrap();
    let after: i64 = sqlx::query_scalar("PRAGMA schema_version")
        .fetch_one(&mut *retained)
        .await
        .unwrap();
    assert!(after > before);
    sqlx::query(CACHED_INSERT)
        .bind("cached-after")
        .bind(watch)
        .bind("creation")
        .bind("cached-after")
        .bind(0_i64)
        .bind(&source.id)
        .bind("completed")
        .bind(2_i64)
        .execute(&mut *retained)
        .await
        .unwrap();
    assert_cached_shapes_rejected(&mut retained, watch, &source).await;
    let mut tx = sqlx::Acquire::begin(&mut *retained).await.unwrap();
    sqlx::query("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us,goal_capture_kind) VALUES ('cached-goal-change',?,'goal_change',?,1,?,'goal_changed',3,'captured')")
        .bind(watch).bind(source.product_conversation_id.as_str()).bind(&source.id)
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO coordinator_watch_goal_captures(event_id,kind,owner_id,owner_version,goal_id,objective,state,usage_kind) VALUES ('cached-goal-change','saved_goal',?,1,'cached-goal','cached goal objective','underway','unavailable_not_captured')")
        .bind(source.product_conversation_id.as_str()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT terminal_kind FROM coordinator_watch_events WHERE event_id='cached-goal-change'")
        .fetch_one(&mut *retained).await.unwrap(), "goal_changed");
    sqlx::query(
        "SELECT event_id,goal_capture_kind,goal_capture_event_id FROM coordinator_watch_events",
    )
    .fetch_all(&mut *retained)
    .await
    .unwrap();
    assert_retained_connection_integrity(&mut retained).await;
}

async fn assert_retained_connection_integrity(retained: &mut sqlx::SqliteConnection) {
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&mut *retained)
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
            .fetch_one(&mut *retained)
            .await
            .unwrap(),
        0
    );
}
