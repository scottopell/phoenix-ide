//! Synthetic fixture admission and normal-executable isolation regressions.

#[path = "../src/immutable_fixture.rs"]
mod immutable_fixture;

use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use phoenix_db::Database;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn readonly(path: &Path) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn admission_requires_absolute_standalone_readonly_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.db");
    std::fs::write(&path, "synthetic").unwrap();
    assert!(immutable_fixture::standalone_path(Path::new("fixture.db")).is_err());
    assert!(immutable_fixture::standalone_path(&dir.path().join("absent.db")).is_err());
    assert!(immutable_fixture::standalone_path(dir.path()).is_err());
    assert!(immutable_fixture::standalone_path(&path).is_err());
    readonly(&path);
    assert_eq!(
        immutable_fixture::standalone_path(&path).unwrap(),
        path.canonicalize().unwrap()
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        std::fs::write(&sidecar, "synthetic").unwrap();
        assert!(immutable_fixture::standalone_path(&path).is_err());
        std::fs::remove_file(sidecar).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn admission_checks_canonical_production_aliases_hardlinks_and_both_sidecar_names() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.db");
    let alias = dir.path().join("alias.db");
    std::fs::write(&path, "synthetic").unwrap();
    readonly(&path);
    symlink(&path, &alias).unwrap();
    assert_eq!(
        immutable_fixture::standalone_path(&alias).unwrap(),
        path.canonicalize().unwrap()
    );
    for base in [&path, &alias] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", base.display()));
            symlink(dir.path().join("absent"), &sidecar).unwrap();
            assert!(immutable_fixture::standalone_path(&alias).is_err());
            std::fs::remove_file(sidecar).unwrap();
        }
    }
    std::fs::remove_file(&alias).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(immutable_fixture::standalone_path(&alias).is_err());
    assert!(immutable_fixture::standalone_path(&path).is_err());
    std::fs::remove_file(&alias).unwrap();
    for name in ["prod", "production", ".phoenix-ide"] {
        let production = dir.path().join(name);
        std::fs::create_dir(&production).unwrap();
        let source = production.join("source.db");
        std::fs::write(&source, "synthetic").unwrap();
        readonly(&source);
        symlink(&source, &alias).unwrap();
        assert!(immutable_fixture::standalone_path(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
    }
    let source = dir.path().join("prod.db");
    std::fs::write(&source, "synthetic").unwrap();
    readonly(&source);
    symlink(&source, &alias).unwrap();
    assert!(immutable_fixture::standalone_path(&alias).is_err());
}

struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    original: Vec<u8>,
    product: String,
    coordinator: String,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        phoenix_db::run_pending_migrations(db.pool()).await.unwrap();
        let root = db
            .create_conversation("root", "root-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        db.create_conversation("other", "other-slug", "/fixture", true, None, None)
            .await
            .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let (busy, frames, checkpointed): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(busy, 0);
        assert_eq!(frames, checkpointed);
        db.pool().close().await;
        readonly(&path);
        let original = std::fs::read(&path).unwrap();
        Self {
            dir,
            path,
            original,
            product: root.product_conversation_id.to_string(),
            coordinator: coordinator.id,
        }
    }

    fn command(&self, sentinel: &TcpListener) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix_ide"));
        let endpoint = format!("http://{}", sentinel.local_addr().unwrap());
        for marker in ["structured-marker", "fatal-marker"] {
            std::fs::write(self.dir.path().join(marker), "untouched").unwrap();
        }
        let helper = self.dir.path().join("helper.sh");
        std::fs::write(
            &helper,
            format!(
                "printf touched > '{}'; printf synthetic-token",
                self.dir.path().join("credential-marker").display()
            ),
        )
        .unwrap();
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
            .env("PHOENIX_BIND_ADDR", "192.0.2.1")
            .env(
                "PHOENIX_PORT",
                sentinel.local_addr().unwrap().port().to_string(),
            )
            .env("PHOENIX_PASSWORD", "synthetic-password")
            .env("PHOENIX_DISCOVERY_ENABLED", "true")
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
            .stderr(std::fs::File::create(self.dir.path().join("child-stderr")).unwrap());
        command
    }

    fn assert_isolated(&self, sentinel: &TcpListener) {
        assert_eq!(std::fs::read(&self.path).unwrap(), self.original);
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!Path::new(&format!("{}{suffix}", self.path.display())).exists());
        }
        for name in [
            "live.db",
            "live-data",
            "live-state",
            "live-tmp",
            "credential-marker",
            ".phoenix-ide",
        ] {
            assert!(
                !self.dir.path().join(name).exists(),
                "live side effect: {name}"
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
            std::io::ErrorKind::WouldBlock
        );
    }
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Server {
    fn wait(&mut self, success: bool) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                return;
            }
            assert!(Instant::now() < deadline, "bounded child termination");
            // test-timing-allow: Child has no exit notification; status is the oracle, deadline only bounds liveness.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn sentinel() -> TcpListener {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    listener
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn ordinary_and_global_fixture_startup_are_loopback_immutable_and_inert() {
    let fixture = Fixture::new().await;
    let sentinel = sentinel();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for target in [
        fixture.product.as_str(),
        "root-slug",
        fixture.coordinator.as_str(),
    ] {
        let mut server = Server(
            fixture
                .command(&sentinel)
                .arg("--observe-fixture")
                .arg(&fixture.path)
                .args(["--target", target, "--port", "0"])
                .spawn()
                .unwrap(),
        );
        let stdout = server.0.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line);
            let _ = send.send((result, line));
        });
        let (result, line) = receive.recv_timeout(Duration::from_secs(15)).unwrap();
        result.unwrap();
        let url = line
            .trim()
            .strip_prefix("Observation listening on ")
            .unwrap();
        let address: SocketAddr = url.strip_prefix("http://").unwrap().parse().unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), sentinel.local_addr().unwrap().port());
        let route = if target == fixture.coordinator {
            format!("/api/global/coordinator/route/{target}")
        } else {
            format!("/api/product-conversations/{target}/route")
        };
        assert_eq!(
            client
                .get(format!("{url}{route}"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let member = if target == fixture.coordinator {
            target
        } else {
            "root"
        };
        assert_eq!(
            client
                .get(format!("{url}/api/conversations/{member}/observation"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert_eq!(
            client
                .get(format!("{url}/api/conversations/other/route"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        let capabilities: Value = client
            .get(format!("{url}/api/capabilities"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(capabilities["live_stream"], "unavailable");
        assert_eq!(capabilities["user_input"], "unavailable");
        for path in ["/api/models", "/api/conversations/root/stream"] {
            assert_eq!(
                client
                    .get(format!("{url}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                503
            );
        }
        for header in ["forwarded", "x-forwarded-for"] {
            assert_eq!(
                client
                    .get(format!("{url}{route}"))
                    .header(header, "127.0.0.1")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                403
            );
        }
        fixture.assert_isolated(&sentinel);
        kill(
            Pid::from_raw(i32::try_from(server.0.id()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        server.wait(true);
        fixture.assert_isolated(&sentinel);
    }
}

#[tokio::test]
async fn invalid_fixture_or_target_never_falls_back_to_live_startup() {
    let fixture = Fixture::new().await;
    let sentinel = sentinel();
    for (path, target) in [
        (fixture.path.clone(), "missing"),
        (fixture.dir.path().join("absent.db"), "root"),
    ] {
        let mut server = Server(
            fixture
                .command(&sentinel)
                .arg("--observe-fixture")
                .arg(path)
                .args(["--target", target, "--port", "0"])
                .spawn()
                .unwrap(),
        );
        server.wait(false);
        fixture.assert_isolated(&sentinel);
    }
}
