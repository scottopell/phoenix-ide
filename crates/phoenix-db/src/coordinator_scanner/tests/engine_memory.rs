use super::*;
use sqlx::Connection;

async fn set_length_limit(connection: &mut sqlx::SqliteConnection, limit: i32) -> i32 {
    let mut handle = connection.lock_handle().await.unwrap();
    // SAFETY: SQLx's handle lock excludes concurrent access to this connection.
    unsafe {
        libsqlite3_sys::sqlite3_limit(
            handle.as_raw_handle().as_ptr(),
            libsqlite3_sys::SQLITE_LIMIT_LENGTH,
            limit,
        )
    }
}
fn assert_engine_too_big(result: Result<sqlx::sqlite::SqliteRow, sqlx::Error>) {
    let error = result.unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("18"),
        "expected SQLite SQLITE_TOOBIG, got {error}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn scanner_utf8_engine_length_guard_never_materializes_excluded_or_oversized_source() {
    const ENGINE_LIMIT: i32 = 2 * 1024 * 1024;
    const PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
    for kind in ["tool", "user", "utf8"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("engine-size.db");
        let db = Database::open_migrated_for_test(path.to_str().unwrap())
            .await
            .unwrap();
        let encoding: String = sqlx::query_scalar("PRAGMA encoding")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(encoding, "UTF-8");
        let owner = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap()
            .id;
        let small = "Résumé 東京 Café";
        let content = match kind {
            "tool" => MessageContent::tool("large-tool-result", "x".repeat(PAYLOAD_BYTES), false),
            "user" => MessageContent::User(UserContent::new("x".repeat(PAYLOAD_BYTES))),
            "utf8" => MessageContent::User(UserContent::new(small)),
            _ => unreachable!(),
        };
        db.add_message_with_seq_and_origin(
            "sized-source",
            &owner,
            1,
            &content,
            None,
            None,
            &InputOrigin::UserApi,
        )
        .await
        .unwrap();
        drop(content);
        let mut connection = db.pool().acquire().await.unwrap().detach();
        let original = set_length_limit(&mut connection, ENGINE_LIMIT).await;
        let size: i64 = sqlx::query_scalar(
            "SELECT octet_length(content) FROM messages WHERE message_id='sized-source'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        if kind == "utf8" {
            assert!(size < i64::from(ENGINE_LIMIT));
        } else {
            assert!(size > i64::try_from(PAYLOAD_BYTES).unwrap());
            assert_engine_too_big(
                sqlx::query("SELECT content FROM messages WHERE message_id='sized-source'")
                    .fetch_one(&mut connection)
                    .await,
            );
            assert_engine_too_big(sqlx::query("SELECT length(CAST(content AS BLOB)) FROM messages WHERE message_id='sized-source'").fetch_one(&mut connection).await);
        }
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let mut allowance = source::SourceReadAllowance::new();
        let prepared =
            storage::prepare_with_allowance(&mut tx, Some(&profile()), 1000, &mut allowance)
                .await
                .unwrap();
        assert_eq!(allowance.remaining(), 31);
        match kind {
            "tool" => assert!(matches!(prepared, ScanPreparation::Progress)),
            "user" => assert!(matches!(
                prepared,
                ScanPreparation::Wait {
                    reason: ScanBlocker::SourceUnavailable,
                    ..
                }
            )),
            "utf8" => {
                let ScanPreparation::Ready(batch) = prepared else {
                    panic!("bounded UTF8 prose must remain readable")
                };
                assert_eq!(batch.input.fragments.len(), 1);
                assert_eq!(batch.input.fragments[0].text, small);
                assert!(batch.input.fragments[0].complete_message);
            }
            _ => unreachable!(),
        }
        let frontier: i64 = sqlx::query_scalar(
            "SELECT historical_seq FROM coordinator_scan_members WHERE transcript_id=?",
        )
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            frontier,
            i64::from(kind != "tool"),
            "oversized prose must not advance coverage"
        );
        tx.commit().await.unwrap();
        assert_eq!(
            set_length_limit(&mut connection, original).await,
            ENGINE_LIMIT
        );
        connection.close().await.unwrap();
        let coverage = db
            .read_coordinator_scan_coverage(&owner, None)
            .await
            .unwrap();
        assert_eq!(
            coverage.summary.pending_historical_members,
            usize::from(kind != "tool")
        );
        assert_eq!(
            db.list_coordinator_tracking(&owner, None, false)
                .await
                .unwrap()
                .active_count,
            0
        );
        db.pool().close().await;
    }
}
