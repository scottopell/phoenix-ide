use super::observe_running_close_runs;
use crate::db::{Database, GenericCloseAdmission};
use phoenix_core::domain::close::{CloseAttemptId, TranscriptConversationId};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

const CHILD_MODE: &str = "PHOENIX_TEST_CLOSE_STARTUP_MODE";
const DATABASE_PATH: &str = "PHOENIX_TEST_CLOSE_STARTUP_DATABASE";
const MARKER_PATH: &str = "PHOENIX_TEST_CLOSE_STARTUP_MARKER";
const COMPLETE: &str = "close-startup-process-reopen-complete";
const ATTEMPT: &str = "startup-close-process-reopen-attempt";

#[tokio::test]
async fn process_exit_and_reopen_observes_persisted_generic_close_without_redrive() {
    if let Some(mode) = std::env::var_os(CHILD_MODE) {
        run_child(mode.to_string_lossy().as_ref()).await;
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("phoenix.db");
    let marker = directory.path().join("admission-committed");
    let test_name = concat!(
        module_path!(),
        "::process_exit_and_reopen_observes_persisted_generic_close_without_redrive"
    );
    let test_name = test_name.strip_prefix("phoenix_ide::").unwrap_or(test_name);

    let mut abandoned = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD_MODE, "admit-and-wait")
        .env(DATABASE_PATH, &database)
        .env(MARKER_PATH, &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        if let Some(status) = abandoned.try_wait().unwrap() {
            let output = abandoned.wait_with_output().await.unwrap();
            panic!(
                "admission child exited before its commit marker ({status}):\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(Instant::now() < deadline, "admission child did not commit in time");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    abandoned.start_kill().unwrap();
    let killed = tokio::time::timeout(Duration::from_secs(30), abandoned.wait_with_output())
        .await
        .expect("killed child did not reap")
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(killed.status.signal(), Some(libc::SIGKILL));
    }
    #[cfg(not(unix))]
    assert!(!killed.status.success());

    for mode in ["reopen-and-classify", "verify-reopen-idempotence"] {
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
                .env(CHILD_MODE, mode)
                .env(DATABASE_PATH, &database)
                .env(MARKER_PATH, &marker)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("fresh child exceeded deadline")
        .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{mode}: {stdout}\n{}", String::from_utf8_lossy(&output.stderr));
        assert!(stdout.lines().any(|line| line == COMPLETE), "{mode} did not complete: {stdout}");
        assert!(stdout.contains("1 passed; 0 failed; 0 ignored"), "unexpected {mode} result: {stdout}");
    }
}

async fn run_child(mode: &str) {
    let database_path = std::env::var(DATABASE_PATH).expect("database path must be supplied");
    let marker_path = std::env::var(MARKER_PATH).expect("marker path must be supplied");
    let db = Database::open(&database_path).await.unwrap();
    crate::db::run_pending_migrations(db.pool()).await.unwrap();
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    match mode {
        "admit-and-wait" => {
            let conversation = db
                .create_conversation("startup-close", "startup-close", "/tmp", true, None, None)
                .await
                .unwrap();
            let product_id = conversation.product_conversation_id;
            let transcript_id = TranscriptConversationId::parse("startup-close").unwrap();
            let attempt = CloseAttemptId::parse(ATTEMPT).unwrap();
            assert!(matches!(
                db.begin_generic_close_settlement_classified(&product_id, &transcript_id, &attempt)
                    .await.established().expect("classified fixture transaction")
                    .unwrap(),
                GenericCloseAdmission::InsertedSettling(_)
            ));
            let running: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1 AND status = 'running'",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(running, 1, "generic admission must durably own the exact running run");
            std::fs::write(marker_path, b"generic close admission committed").unwrap();
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
        "reopen-and-classify" => {
            assert!(std::path::Path::new(&marker_path).exists());
            assert_eq!(db.list_running_close_runs().await.unwrap().len(), 1);
            let mut commits = 0;
            assert_eq!(observe_running_close_runs(&db, || commits += 1).await.unwrap(), 1);
            assert_eq!(commits, 1);
            assert!(db.list_running_close_runs().await.unwrap().is_empty());

            assert_eq!(observe_running_close_runs(&db, || commits += 1).await.unwrap(), 0);
            assert_eq!(commits, 1, "reopening an already classified run must be idempotent");

            let run_status: String = sqlx::query_scalar(
                "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_status, "stopped");
            let run_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_count, 1, "startup must not create a successor run");
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                1,
                "startup classification is durable exactly once"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resources")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                0,
                "observation must not dispatch destructive retirement effects"
            );
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1").bind(ATTEMPT).fetch_one(db.pool()).await.unwrap(), 0);
            println!("\n{COMPLETE}");
        }
        "verify-reopen-idempotence" => {
            assert_eq!(db.list_running_close_runs().await.unwrap().len(), 0);
            let mut commits = 0;
            assert_eq!(observe_running_close_runs(&db, || commits += 1).await.unwrap(), 0);
            assert_eq!(commits, 0);
            let run_status: String = sqlx::query_scalar(
                "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_status, "stopped");
            let run_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_count, 1);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1").bind(ATTEMPT).fetch_one(db.pool()).await.unwrap(), 0);
            println!("\n{COMPLETE}");
        }
        unexpected => panic!("unknown child mode {unexpected}"),
    }
}
