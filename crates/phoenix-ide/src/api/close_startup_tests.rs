use super::{handlers::api_test_support::make_test_state_with_db, recover_interrupted_close_runs};
use crate::db::{Database, GenericCloseAdmission};
use crate::terminal::session::ActiveTerminalInsertError;
use crate::work_scope::ResourceScopeKey;
use phoenix_core::domain::close::{CloseAttemptId, CloseRunRef, TranscriptConversationId};
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
        assert!(
            Instant::now() < deadline,
            "admission child did not commit in time"
        );
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
        assert!(
            output.status.success(),
            "{mode}: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.lines().any(|line| line == COMPLETE),
            "{mode} did not complete: {stdout}"
        );
        assert!(
            stdout.contains("1 passed; 0 failed; 0 ignored"),
            "unexpected {mode} result: {stdout}"
        );
    }
}

#[allow(clippy::too_many_lines)]
async fn run_child(mode: &str) {
    let database_path = std::env::var(DATABASE_PATH).expect("database path must be supplied");
    let marker_path = std::env::var(MARKER_PATH).expect("marker path must be supplied");
    let db = if mode == "admit-and-wait" {
        Database::open_prefix_for_test(&database_path, 195)
            .await
            .unwrap()
    } else {
        let db = Database::open(&database_path).await.unwrap();
        crate::db::run_pending_migrations(db.pool()).await.unwrap();
        assert_migrated_stop_claim(&db, &marker_path).await;
        db
    };
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
            assert!(conversation.attached_work_scope_id.is_some());
            let product_id = conversation.product_conversation_id;
            let transcript_id = TranscriptConversationId::parse("startup-close").unwrap();
            let attempt = CloseAttemptId::parse(ATTEMPT).unwrap();
            accept_startup_turn(&db).await;
            assert!(matches!(
                db.begin_generic_close_settlement_classified(&product_id, &transcript_id, &attempt)
                    .await
                    .established()
                    .expect("classified fixture transaction")
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
            assert_eq!(
                running, 1,
                "generic admission must durably own the exact running run"
            );
            let captured: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT turn_id, expected_generation FROM close_attempt_direct_turn_settlements
                 WHERE attempt_id = ?1",
            )
            .bind(ATTEMPT)
            .fetch_all(db.pool())
            .await
            .unwrap();
            let [(turn, generation)] = captured.as_slice() else {
                panic!("expected one exact captured turn: {captured:?}");
            };
            sqlx::query(
                "INSERT INTO native_close_initial_settlement_stop_claims
                    (attempt_id, run_ordinal, turn_id, expected_generation)
                 VALUES (?1, 1, ?2, ?3)",
            )
            .bind(ATTEMPT)
            .bind(turn)
            .bind(generation)
            .execute(db.pool())
            .await
            .unwrap();
            let marker = std::path::Path::new(&marker_path);
            let pending_marker = marker.with_extension("pending");
            std::fs::write(&pending_marker, format!("{turn}:{generation}")).unwrap();
            std::fs::rename(pending_marker, marker).unwrap();
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }
        "reopen-and-classify" => {
            assert!(std::path::Path::new(&marker_path).exists());
            assert_eq!(db.list_running_close_runs().await.unwrap().len(), 1);
            let scope = captured_work_scope(&db).await;
            let state = make_test_state_with_db(db.clone());
            assert!(state.terminals.reserve_spawn(&scope).is_ok());
            state.terminals.release_spawn(&scope);
            let unrelated = ResourceScopeKey::Unattached("unrelated-startup-scope".into());
            let mut commits = 0;
            assert_eq!(
                recover_interrupted_close_runs(&state.runtime, || {
                    assert!(matches!(
                        state.terminals.reserve_spawn(&scope),
                        Err(ActiveTerminalInsertError::RetirementFenced)
                    ));
                    assert!(state.terminals.reserve_spawn(&unrelated).is_ok());
                    state.terminals.release_spawn(&unrelated);
                    commits += 1;
                })
                .await
                .unwrap(),
                1
            );
            assert_eq!(commits, 1);
            assert!(db.list_running_close_runs().await.unwrap().is_empty());

            assert_eq!(
                recover_interrupted_close_runs(&state.runtime, || commits += 1)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(
                commits, 1,
                "reopening an already classified run must be idempotent"
            );

            let run_status: String = sqlx::query_scalar(
                "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_status, "stopped");
            let run_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1")
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
            let scope = captured_work_scope(&db).await;
            let state = make_test_state_with_db(db.clone());
            let mut commits = 0;
            assert_eq!(
                recover_interrupted_close_runs(&state.runtime, || commits += 1)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(commits, 0);
            assert!(matches!(
                state.terminals.reserve_spawn(&scope),
                Err(ActiveTerminalInsertError::RetirementFenced)
            ));
            let run_status: String = sqlx::query_scalar(
                "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
            )
            .bind(ATTEMPT)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(run_status, "stopped");
            let run_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1")
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
            let run = CloseRunRef::initial(CloseAttemptId::parse(ATTEMPT).unwrap());
            let event: (String, String, String, String, String) = sqlx::query_as(
                "SELECT route_kind, source_occurrence_id, source_transcript_id,
                        terminal_kind, terminal_reason
                   FROM coordinator_watch_events
                  WHERE mandatory_failure_occurrence_id = ?1",
            )
            .bind(run.failure_occurrence_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(
                event,
                (
                    "mandatory_close_failure".into(),
                    run.failure_occurrence_id(),
                    "startup-close".into(),
                    "cleanup_failed".into(),
                    "interrupted".into(),
                ),
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events")
                    .fetch_one(db.pool())
                    .await
                    .unwrap(),
                1,
                "startup emits only the mandatory interruption failure outbox item",
            );
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1").bind(ATTEMPT).fetch_one(db.pool()).await.unwrap(), 0);
            println!("\n{COMPLETE}");
        }
        unexpected => panic!("unknown child mode {unexpected}"),
    }
}

async fn captured_work_scope(db: &Database) -> ResourceScopeKey {
    let target = db.get_conversation("startup-close").await.unwrap();
    let scope = target.attached_work_scope_id.expect("target WorkScope");
    assert_eq!(
        db.list_close_execution_fence_scopes().await.unwrap(),
        [scope.clone()]
    );
    ResourceScopeKey::Work(scope)
}

async fn accept_startup_turn(db: &Database) {
    use phoenix_core::domain::db_schema::InputOrigin;
    use phoenix_core::domain::sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    };
    use phoenix_workflow::{
        AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
        TurnOutcome,
    };

    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: "close-target-input".into(),
            origin: InputOrigin::UserApi,
            text: "target work".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "target work".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let accepted = db
        .workflow_repository()
        .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new("close-target-input").unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority("startup-close".to_string()),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .established()
        .unwrap()
        .unwrap();
    let TurnOutcome::Created { .. } = accepted.outcome else {
        panic!("expected an authoritative target turn");
    };
}

async fn assert_migrated_stop_claim(db: &Database, marker_path: &str) {
    let marker = std::fs::read_to_string(marker_path).unwrap();
    let (turn, generation) = marker.split_once(':').unwrap();
    let claim: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT run_ordinal, turn_id, expected_generation FROM close_settlement_stop_claims
         WHERE attempt_id = ?1",
    )
    .bind(ATTEMPT)
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(
        claim,
        vec![(
            1,
            turn.parse::<i64>().unwrap(),
            generation.parse::<i64>().unwrap()
        )]
    );
    let migration: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migrations WHERE version = 196")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(migration, 1);
    let old_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'native_close_initial_settlement_stop_claims'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(old_table, 0);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap()
        .is_empty());
}
