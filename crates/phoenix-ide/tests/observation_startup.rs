//! Executable isolation tests use only synthetic, disposable databases and credentials.

use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use phoenix_db::{ContinueOutcome, ConvState, Database};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    original: Vec<u8>,
    product: String,
    successor: String,
    successor_slug: String,
    coordinator: String,
}

impl Fixture {
    async fn new(unsupported_schema: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let (root, successor, coordinator) = Self::populate(&db).await;
        sqlx::query("DROP TABLE _migrations")
            .execute(db.pool())
            .await
            .unwrap();
        if unsupported_schema {
            sqlx::query("DROP VIEW work_scope_environments")
                .execute(db.pool())
                .await
                .unwrap();
        }
        let (busy, frames, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(frames, checkpointed);
        db.pool().close().await;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let original = std::fs::read(&path).unwrap();
        Self {
            dir,
            path,
            original,
            product: root.product_conversation_id.to_string(),
            successor: successor.id,
            successor_slug: successor.slug.unwrap(),
            coordinator: coordinator.id,
        }
    }

    async fn populate(
        db: &Database,
    ) -> (
        phoenix_db::Conversation,
        phoenix_db::Conversation,
        phoenix_db::Conversation,
    ) {
        let root = db
            .create_conversation("a-root", "root-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "synthetic handoff".into(),
            },
        )
        .await
        .unwrap();
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(row) => row,
            ContinueOutcome::AlreadyContinued(row) => panic!("already continued: {}", row.id),
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                panic!("parent state: {state_variant}")
            }
        };
        db.create_conversation("other", "other-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "child",
            "child-slug",
            "/fixture",
            false,
            Some(&root.id),
            None,
        )
        .await
        .unwrap();
        db.create_conversation(
            "0-disconnected",
            "disconnected-slug",
            "/fixture",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        // Deliberately corrupt this disposable fixture, not a representable production state.
        sqlx::query("DROP TRIGGER conversations_reject_product_continuation_cycle_on_update")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = id WHERE id = '0-disconnected'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        for statement in [
            "DROP TRIGGER conversations_product_membership_is_immutable",
            "DROP TRIGGER conversations_require_single_product_root_on_update",
        ] {
            sqlx::query(statement).execute(db.pool()).await.unwrap();
        }
        sqlx::query(
            "UPDATE conversations SET product_conversation_id = ?, work_scope_id = (SELECT work_scope_id FROM conversations WHERE id = ?) WHERE id = '0-disconnected'",
        )
        .bind(root.product_conversation_id.as_str())
        .bind(&root.id)
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.resolve_ordinary_product_conversation(root.product_conversation_id.as_str())
                .await
                .unwrap()
                .requested_transcript_row_id,
            "0-disconnected"
        );
        assert_eq!(
            db.get_ordinary_product_conversation(&root.product_conversation_id)
                .await
                .unwrap()
                .root
                .conversation
                .id,
            root.id
        );
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        (root, successor, coordinator)
    }

    fn command(&self, sentinel: &TcpListener) -> Command {
        let helper = self.dir.path().join("helper.sh");
        std::fs::write(
            &helper,
            format!(
                "printf touched > '{}'; printf synthetic-token",
                self.dir.path().join("credential-marker").display()
            ),
        )
        .unwrap();
        let endpoint = format!("http://{}", sentinel.local_addr().unwrap());
        let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix_ide"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.path())
            .env("PHOENIX_DATA_DIR", self.dir.path().join("live-data"))
            .env("PHOENIX_STATE_DIR", self.dir.path().join("live-state"))
            .env("PHOENIX_TMP_DIR", self.dir.path().join("live-tmp"))
            .env("PHOENIX_DB_PATH", self.dir.path().join("live.db"))
            .env(
                "PHOENIX_LOG_FILE",
                self.dir.path().join("structured-marker"),
            )
            .env(
                "PHOENIX_FATAL_LOG_FILE",
                self.dir.path().join("fatal-marker"),
            )
            .env("PHOENIX_LOG_STDOUT", "true")
            .env("PHOENIX_DISCOVERY_ENABLED", "true")
            .env("PHOENIX_TRACE_EXPORTER", "none")
            .env(
                "PHOENIX_PORT",
                sentinel.local_addr().unwrap().port().to_string(),
            )
            .env("PHOENIX_BIND_ADDR", "192.0.2.1")
            .env("PHOENIX_PASSWORD", "synthetic-password")
            .env("LLM_API_KEY_HELPER", format!("sh '{}'", helper.display()))
            .env("ANTHROPIC_API_KEY", "synthetic-token")
            .env("OPENAI_API_KEY", "synthetic-token")
            .env("ANTHROPIC_BASE_URL", &endpoint)
            .env("OPENAI_BASE_URL", &endpoint)
            .env("OPENAI_RESPONSES_BASE_URL", &endpoint)
            .env("OPENAI_CHAT_COMPLETIONS_BASE_URL", &endpoint)
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for marker in ["structured-marker", "fatal-marker"] {
            std::fs::write(self.dir.path().join(marker), "untouched").unwrap();
        }
        command
    }

    fn assert_isolated(&self, sentinel: &TcpListener) {
        assert_eq!(std::fs::read(&self.path).unwrap(), self.original);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!Path::new(&format!("{}{suffix}", self.path.display())).exists());
        }
        for path in [
            "credential-marker",
            "live.db",
            "live-data",
            "live-state",
            "live-tmp",
            ".phoenix-ide",
        ] {
            assert!(
                !self.dir.path().join(path).exists(),
                "live side effect: {path}"
            );
        }
        for marker in ["structured-marker", "fatal-marker"] {
            assert_eq!(
                std::fs::read_to_string(self.dir.path().join(marker)).unwrap(),
                "untouched"
            );
        }
        assert_eq!(
            sentinel.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "provider or host discovery connected to sentinel"
        );
    }
}

fn sentinel() -> TcpListener {
    // These ports are in DiscoveryConfig's normal probe range.
    let listener = (9000..=9099)
        .find_map(|port| TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).ok())
        .expect("disposable discovery sentinel port");
    listener.set_nonblocking(true).unwrap();
    listener
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Server {
    fn start(fixture: &Fixture, sentinel: &TcpListener, target: &str) -> (Self, String) {
        let mut child = fixture
            .command(sentinel)
            .arg("--observe-fixture")
            .arg(&fixture.path)
            .args(["--target", target, "--port", "0"])
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let server = Self(child);
        let (result, line) = receive
            .recv_timeout(Duration::from_secs(15))
            .expect("bounded observation readiness");
        result.unwrap();
        let url = line
            .trim()
            .strip_prefix("Observation listening on ")
            .expect("observation readiness, not live logging")
            .to_owned();
        let address: SocketAddr = url.strip_prefix("http://").unwrap().parse().unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);
        assert_ne!(address.port(), sentinel.local_addr().unwrap().port());
        (server, url)
    }

    fn stop(&mut self, signal: Signal) {
        let pid = i32::try_from(self.0.id()).unwrap();
        kill(Pid::from_raw(pid), signal).unwrap();
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "observation signal shutdown: {status:?}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "observation shutdown was not bounded"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn normal_executable_observation_scopes_reads_and_never_constructs_live_services() {
    let fixture = Fixture::new(false).await;
    let sentinel = sentinel();
    let (mut server, url) = Server::start(&fixture, &sentinel, &fixture.product);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for (path, expected) in [
        (
            "/api/auth/status".to_owned(),
            json!({"auth_required": false, "authenticated": true}),
        ),
        (
            "/api/conversations/a-root/route".to_owned(),
            json!({"id": "a-root", "slug": "root-slug"}),
        ),
        (
            format!("/api/conversations/{}/route", fixture.successor),
            json!({"id": fixture.successor, "slug": fixture.successor_slug}),
        ),
        (
            format!(
                "/api/conversations/by-slug/{}/route",
                fixture.successor_slug
            ),
            json!({"id": fixture.successor, "slug": fixture.successor_slug}),
        ),
        (
            format!("/api/product-conversations/{}/route", fixture.product),
            json!({"transcript_row_id": "a-root"}),
        ),
        (
            format!("/api/global/coordinator/route/{}", fixture.product),
            json!({"coordinator_id": null}),
        ),
        (
            "/api/global/coordinator/route/root-slug".to_owned(),
            json!({"coordinator_id": null}),
        ),
    ] {
        let response = client.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.json::<Value>().await.unwrap(), expected, "{path}");
    }
    for reference in [
        "other",
        "other-slug",
        "child",
        "child-slug",
        "0-disconnected",
        "disconnected-slug",
        &fixture.coordinator,
    ] {
        for path in [
            format!("/api/product-conversations/{reference}/route"),
            format!("/api/global/coordinator/route/{reference}"),
        ] {
            assert_eq!(
                client
                    .get(format!("{url}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404,
                "{path}"
            );
        }
    }
    for path in [
        "/api/conversations/other/route",
        "/api/conversations/by-slug/other-slug/route",
        "/api/conversations/child/route",
        "/api/conversations/by-slug/child-slug/route",
        "/api/conversations/0-disconnected/route",
        "/api/conversations/by-slug/disconnected-slug/route",
        "/api/conversations/root-slug/route",
        "/api/conversations/by-slug/a-root/route",
    ] {
        assert_eq!(
            client
                .get(format!("{url}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            404,
            "{path}"
        );
    }
    for path in [
        "/api/models",
        "/api/conversations",
        "/api/product-conversations",
        "/api/conversations/a-root/stream",
        "/api/product-conversations/a-root",
    ] {
        let response = client.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["error_type"],
            "capability_unavailable"
        );
    }
    for (method, path) in [
        (reqwest::Method::GET, "/api/credential-helper/run"),
        (reqwest::Method::POST, "/api/conversations/a-root/chat"),
    ] {
        assert_eq!(
            client
                .request(method, format!("{url}{path}"))
                .header("content-type", "application/json")
                .body("malformed")
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
    }
    for path in [
        "/api/auth/status",
        "/",
        "/assets/index.js",
        "/api/conversations/a-root/route",
    ] {
        for header in ["forwarded", "x-forwarded-for"] {
            assert_eq!(
                client
                    .get(format!("{url}{path}"))
                    .header(header, "127.0.0.1")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                403,
                "{header} {path}"
            );
        }
    }
    fixture.assert_isolated(&sentinel);
    server.stop(Signal::SIGTERM);
    fixture.assert_isolated(&sentinel);
    for target in ["root-slug", fixture.successor_slug.as_str()] {
        let (mut server, _) = Server::start(&fixture, &sentinel, target);
        server.stop(Signal::SIGINT);
        fixture.assert_isolated(&sentinel);
    }
}

#[tokio::test]
async fn observation_startup_errors_do_not_log_migrate_or_fall_back_to_live() {
    let sentinel = sentinel();
    for unsupported in [false, true] {
        let fixture = Fixture::new(unsupported).await;
        let targets = if unsupported {
            vec!["a-root"]
        } else {
            vec![
                "missing",
                "child",
                "child-slug",
                "0-disconnected",
                "disconnected-slug",
                &fixture.coordinator,
            ]
        };
        for target in targets {
            let mut child = Server(
                fixture
                    .command(&sentinel)
                    .arg("--observe-fixture")
                    .arg(&fixture.path)
                    .args(["--target", target, "--port", "0"])
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(!status.success(), "inadmissible target/schema accepted");
                    break;
                }
                assert!(Instant::now() < deadline, "failed startup did not exit");
                std::thread::sleep(Duration::from_millis(20));
            }
            fixture.assert_isolated(&sentinel);
        }
        let output = fixture
            .command(&sentinel)
            .args([
                "--observe-fixture",
                "/absent",
                "--target",
                "a-root",
                "--port",
                "invalid",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        fixture.assert_isolated(&sentinel);
    }
}
