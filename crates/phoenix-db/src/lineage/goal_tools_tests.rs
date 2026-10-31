use super::*;
use crate::Database;
use phoenix_core::llm_language::LlmLanguage;

async fn seed_legacy_goal(db: &Database) -> String {
    let conversation = db
        .create_conversation("migration-owner", "owner", "/tmp", true, None, None)
        .await
        .unwrap();
    let owner = conversation.product_conversation_id.to_string();
    sqlx::query("INSERT INTO conversation_goal_ids(goal_id,objective,owner_id) VALUES ('legacy-goal','preserve objective',?)")
        .bind(&owner).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,host_action_id) VALUES (1,?,'generated_host','legacy-create')")
        .bind(&owner).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,1,'legacy-goal','underway',NULL,'legacy-create','create',NULL,1,1,1)")
        .bind(&owner).execute(db.pool()).await.unwrap();
    owner
}

async fn seed_real_145_receipt(db: &Database, owner: &str) -> String {
    let coordinator = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap();
    sqlx::query("INSERT INTO workflows(workflow_id,profile_kind,profile_version,runtime_acceptance_enabled,external_acceptance_enabled,version,generation,status,snapshot_codec_family,snapshot_codec_version,snapshot_payload,created_at,updated_at) VALUES (1,'direct_turn',1,0,1,1,0,'Active','direct_turn.snapshot',1,X'01',1,1)")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO durable_turns(turn_id,workflow_id,conversation_id,client_turn_key,prepared_fingerprint,prepared_payload,disposition,generation,owns_conversation,origin_kind,canonical_message_id,materialized) VALUES (1,1,?,'migration-turn','migration-fingerprint',X'01','Runtime',0,1,'user_api',NULL,0)")
        .bind(&coordinator.id).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO messages(message_id,conversation_id,sequence_id,message_type,content,created_at) VALUES ('migration-agent',?,1,'agent','{}','2026-10-31T00:00:00Z')")
        .bind(&coordinator.id).execute(db.pool()).await.unwrap();
    let source = coordinator.product_conversation_id.to_string();
    sqlx::query("INSERT INTO conversation_goal_actors(actor_id,owner_id,kind,source_product_id,transcript_id,workflow_id,turn_id,message_id,tool_use_id) VALUES (2,?,'coordinator',?,?,1,1,'migration-agent','migration-tool')")
        .bind(owner).bind(&source).bind(&coordinator.id).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_preparations(reference,owner_id,caller_conversation_id,source_product_id,workflow_id,turn_id,message_id,tool_use_id,operation,expected_version,observed_goal_id,objective,disposition,reason) VALUES (zeroblob(16),?,?,?,1,1,'migration-agent','migration-tool','state',1,'legacy-goal',NULL,'blocked','waiting')")
        .bind(owner).bind(&coordinator.id).bind(&source).execute(db.pool()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_coordinator_receipts(preparation_reference,owner_id,outcome,result_version,recorded_at_us) VALUES (zeroblob(16),?,'accepted',2,2)")
        .bind(owner).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO conversation_goal_history(owner_id,version,goal_id,state,reason,action_id,change_kind,report_turn_id,requested_actor_id,applied_actor_id,recorded_at_us) VALUES (?,2,'legacy-goal','blocked','waiting','prepared-state','state',NULL,2,2,2)")
        .bind(owner).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    source
}

async fn assert_goal_rows(db: &Database, owner: &str, has_prepared: bool) {
    let identity: (String, String) =
        sqlx::query_as("SELECT goal_id,objective FROM conversation_goal_ids WHERE owner_id=?")
            .bind(owner)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        identity,
        ("legacy-goal".into(), "preserve objective".into())
    );
    let history: Vec<(i64, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT version,state,reason,requested_actor_id,applied_actor_id FROM conversation_goal_history WHERE owner_id=? ORDER BY version",
    ).bind(owner).fetch_all(db.pool()).await.unwrap();
    let mut expected = vec![(1, "underway".into(), None, 1, 1)];
    if has_prepared {
        expected.push((2, "blocked".into(), Some("waiting".into()), 2, 2));
    }
    assert_eq!(history, expected);
    if has_prepared {
        let provenance: (String, i64, i64, String, String) = sqlx::query_as(
            "SELECT outcome,result_version,workflow_id,message_id,tool_use_id FROM conversation_goal_coordinator_receipt_history JOIN conversation_goal_coordinator_receipts USING(preparation_reference,owner_id,result_version) WHERE owner_id=?",
        ).bind(owner).fetch_one(db.pool()).await.unwrap();
        assert_eq!(
            provenance,
            (
                "accepted".into(),
                2,
                1,
                "migration-agent".into(),
                "migration-tool".into()
            )
        );
        let mismatches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_goal_coordinator_history_mismatch",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(mismatches, 0);
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
}

async fn table_data(pool: &SqlitePool) -> Vec<(String, Vec<Vec<String>>)> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap();
    let mut data = Vec::new();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&table)
                .fetch_all(pool)
                .await
                .unwrap();
        let projection = columns
            .iter()
            .map(|column| format!("quote(\"{}\")", column.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {projection} FROM \"{}\"",
            table.replace('"', "\"\"")
        )))
        .fetch_all(pool)
        .await
        .unwrap();
        let mut values: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                (0..columns.len())
                    .map(|index| row.get::<String, _>(index))
                    .collect()
            })
            .collect();
        values.sort();
        data.push((table, values));
    }
    data
}

#[tokio::test]
async fn canonical_goal_tools_prefixes_upgrade_reopen_and_remain_idempotent() {
    for through in 142..=149 {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("goal-tools.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_through_for_test(db.pool(), through)
            .await
            .unwrap();
        let owner = seed_legacy_goal(&db).await;
        let source = if through >= 145 {
            Some(seed_real_145_receipt(&db, &owner).await)
        } else {
            None
        };
        db.pool().close().await;
        drop(db);

        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(
            crate::migrations::run_pending_migrations(db.pool())
                .await
                .unwrap(),
            149 - through
        );
        assert_goal_rows(&db, &owner, source.is_some()).await;
        let tail: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _migrations WHERE version>=143 ORDER BY version",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(tail, vec![143, 144, 145, 146, 147, 148, 149]);
        let before = snapshot(db.pool()).await;
        assert_eq!(
            crate::migrations::run_pending_migrations(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(snapshot(db.pool()).await, before);
        db.pool().close().await;
        drop(db);

        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(
            crate::migrations::run_pending_migrations(db.pool())
                .await
                .unwrap(),
            0
        );
        assert_goal_rows(&db, &owner, source.is_some()).await;
        if let Some(source) = source {
            let before_delete = snapshot(db.pool()).await;
            let data_before_delete = table_data(db.pool()).await;
            let error = sqlx::query("DELETE FROM product_conversations WHERE id=?")
                .bind(&source)
                .execute(db.pool())
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Coordinator transcript is outside ordinary Delete"),
                "{error}"
            );
            assert_eq!(snapshot(db.pool()).await, before_delete);
            assert_eq!(table_data(db.pool()).await, data_before_delete);
            assert_goal_rows(&db, &owner, true).await;
        }
        sqlx::query("DELETE FROM product_conversations WHERE id=?")
            .bind(&owner)
            .execute(db.pool())
            .await
            .unwrap();
        for table in [
            "conversation_goal_preparations",
            "conversation_goal_coordinator_receipts",
            "conversation_goal_history",
            "conversation_goal_actors",
            "conversation_goal_ids",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(count, 0, "owner deletion left {table} at prefix{through}");
        }
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
        db.pool().close().await;
    }
}

#[tokio::test]
async fn fresh_goal_tools_database_applies_complete_registry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fresh.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap() as usize,
        crate::migrations::migration_identities().count()
    );
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        0
    );
    let owner = seed_legacy_goal(&db).await;
    seed_real_145_receipt(&db, &owner).await;
    assert_goal_rows(&db, &owner, true).await;
}

async fn seed_legacy_tools_retention(
    db: &Database,
    owner: &str,
) -> phoenix_core::domain::db_schema::Conversation {
    let transcript: String =
        sqlx::query_scalar("SELECT id FROM conversations WHERE product_conversation_id=?")
            .bind(owner)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let author = db
        .create_conversation("migration-author", "author", "/tmp", true, None, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO product_creation_jobs(request_id,product_conversation_id,cwd,objective,llm_language,status,accepted_at_unix_micros,updated_at_unix_micros,published_product_id,published_conversation_id) VALUES ('joint-create',?,'/tmp','retained objective','phoenix-native','published',1,1,?,?)")
        .bind(owner).bind(owner).bind(&transcript).execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO product_creation_job_images(request_id,ordinal,media_type,data) VALUES ('joint-create',0,'image/png','payload')")
        .execute(db.pool()).await.unwrap();
    sqlx::query("INSERT INTO product_conversation_rename_receipts(operation_id,product_conversation_id,name_revision_before,title,source_product_conversation_id,source_transcript_id,source_message_id,source_tool_use_id,naming_root_transcript_id,slug_at_commit,committed_at_unix_micros) VALUES ('joint-rename',?,0,'retained title',?,?,'rename-message','rename-tool',?,'retained-slug',1)")
        .bind(owner).bind(author.product_conversation_id.as_str()).bind(&author.id).bind(&transcript).execute(db.pool()).await.unwrap();
    author
}

#[tokio::test]
async fn real_145_provenance_and_tools_retention_survive_upgrade_delete_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retention.db");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    crate::migrations::run_through_for_test(db.pool(), 144)
        .await
        .unwrap();
    let owner = seed_legacy_goal(&db).await;
    let author = seed_legacy_tools_retention(&db, &owner).await;
    assert_eq!(
        crate::migrations::run_through_for_test(db.pool(), 145)
            .await
            .unwrap(),
        1
    );
    seed_real_145_receipt(&db, &owner).await;
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        4
    );
    sqlx::query("INSERT INTO product_creation_initial_execution_receipts(request_id,terminal_kind,failure_reason) VALUES ('joint-create','Completed',NULL)")
        .execute(db.pool()).await.unwrap();
    assert_goal_rows(&db, &owner, true).await;
    assert_tools_retention(&db, false).await;
    db.pool().close().await;
    drop(db);

    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_goal_rows(&db, &owner, true).await;
    assert_tools_retention(&db, false).await;
    let attribution_before: (String, String) = sqlx::query_as("SELECT source_product_conversation_id,source_transcript_id FROM product_conversation_rename_receipts WHERE operation_id='joint-rename'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        attribution_before,
        (
            author.product_conversation_id.to_string(),
            author.id.clone()
        )
    );
    sqlx::query("DELETE FROM product_conversations WHERE id=?")
        .bind(author.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
    let author_transcripts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE id=?")
            .bind(&author.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(author_transcripts, 0);
    let attribution_after: (String, String) = sqlx::query_as("SELECT source_product_conversation_id,source_transcript_id FROM product_conversation_rename_receipts WHERE operation_id='joint-rename'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(attribution_after, attribution_before);
    assert_goal_rows(&db, &owner, true).await;
    assert_tools_retention(&db, false).await;
    sqlx::query("DELETE FROM product_conversations WHERE id=?")
        .bind(&owner)
        .execute(db.pool())
        .await
        .unwrap();
    assert_tools_retention(&db, true).await;
    for table in [
        "conversation_goal_ids",
        "conversation_goal_actors",
        "conversation_goal_history",
        "conversation_goal_preparations",
        "conversation_goal_coordinator_receipts",
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 0, "owner deletion left {table}");
    }
    db.pool().close().await;
    drop(db);
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    assert_eq!(
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_tools_retention(&db, true).await;
    let attribution_reopened: (String, String) = sqlx::query_as("SELECT source_product_conversation_id,source_transcript_id FROM product_conversation_rename_receipts WHERE operation_id='joint-rename'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(attribution_reopened, attribution_before);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
}

async fn assert_tools_retention(db: &Database, retired: bool) {
    let creation: (String, String) = sqlx::query_as(
        "SELECT status,objective FROM product_creation_jobs WHERE request_id='joint-create'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(creation, ("published".into(), "retained objective".into()));
    let rename: String = sqlx::query_scalar(
        "SELECT title FROM product_conversation_rename_receipts WHERE operation_id='joint-rename'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(rename, "retained title");
    let terminal: String = sqlx::query_scalar("SELECT terminal_kind FROM product_creation_initial_execution_receipts WHERE request_id='joint-create'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(terminal, "Completed");
    let images: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM product_creation_job_images WHERE request_id='joint-create'),(SELECT COUNT(*) FROM product_creation_image_replay_identity WHERE request_id='joint-create'),(SELECT COUNT(*) FROM product_creation_retired_images WHERE request_id='joint-create')")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(images, (i64::from(!retired), 1, i64::from(retired)));
    let identity: String = sqlx::query_scalar("SELECT data_sha256 FROM product_creation_image_replay_identity WHERE request_id='joint-create' AND ordinal=0")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        identity,
        "239F59ED55E737C77147CF55AD0C1B030B6D7EE748A7426952F9B852D5A935E5"
    );
}

#[tokio::test]
async fn unpublished_tools_d3_tail_exposes_missing_145_without_mutation() {
    // d3a1497 is an unpublished integration input, not a supported shipped sparse predecessor.
    let pool = real_prefix(142).await;
    crate::migrations::run_unpublished_tools_d3_for_test(&pool)
        .await
        .unwrap();
    let tail: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _migrations WHERE version>=143 ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(tail, vec![143, 144, 146, 147, 148, 149]);
    let absent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name='conversation_goal_preparations'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(absent, 0);
    let error = super::super::preflight(&pool, crate::migrations::migration_identities())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing canonical predecessor"), "{error}");
    refuses_without_mutation(&pool).await;
}
