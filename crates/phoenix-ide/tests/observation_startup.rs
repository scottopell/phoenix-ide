//! Synthetic fixture admission and normal-executable isolation regressions.

#[path = "../src/immutable_fixture.rs"]
mod immutable_fixture;

use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use phoenix_core::domain::{
    db_schema::{MessageContent, MessageType},
    llm_types::ContentBlock,
    sm_state::ConvState,
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
    coordinator_product: String,
    root_successor: String,
    coordinator_successor: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_opening(false).await
    }

    async fn with_opening(accepted: bool) -> Self {
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
        let root_successor = seed_history(&db, &root.id, "ordinary", accepted).await;
        let coordinator_successor = seed_history(&db, &coordinator.id, "global", accepted).await;
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
            coordinator_product: coordinator.product_conversation_id.to_string(),
            root_successor,
            coordinator_successor,
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

fn history_messages(prefix: &str) -> Vec<(String, MessageContent)> {
    vec![
        (
            format!("{prefix}-prompt"),
            MessageContent::user("# Persisted prompt\n\n**Inspect** `fixture` without execution."),
        ),
        (
            format!("{prefix}-agent"),
            MessageContent::agent(vec![
                ContentBlock::Text {
                    text: "Persisted **analysis** with a tool call.".into(),
                },
                ContentBlock::ToolUse {
                    id: format!("{prefix}-tool-use"),
                    name: "think".into(),
                    input: serde_json::json!({"thoughts": "synthetic persisted reasoning"}),
                },
            ]),
        ),
        (
            format!("{prefix}-tool"),
            MessageContent::tool(format!("{prefix}-tool-use"), "Persisted tool result", false),
        ),
        (
            format!("{prefix}-boundary"),
            MessageContent::continuation("persisted summary"),
        ),
    ]
}

async fn seed_history(db: &Database, root: &str, prefix: &str, accepted: bool) -> String {
    for (id, content) in history_messages(prefix) {
        db.add_message(&id, root, &content, None, None)
            .await
            .unwrap();
    }
    db.update_conversation_state(
        root,
        &ConvState::ContextExhausted {
            summary: "persisted summary".into(),
        },
    )
    .await
    .unwrap();
    let opening_id = format!("{prefix}-opening");
    let (outcome, _) = db
        .continue_conversation_with_intent(
            root,
            phoenix_db::NewContinuationDispatchIntent::generated_predecessor_context(
                phoenix_workflow::ClientTurnKey::try_from(opening_id.as_str()).unwrap(),
                "persisted summary".into(),
            ),
        )
        .await
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = outcome else {
        panic!("expected a newly reserved continuation")
    };
    if accepted {
        db.add_message(
            &opening_id,
            &successor.id,
            &MessageContent::continuation("persisted summary"),
            None,
            None,
        )
        .await
        .unwrap();
    }
    successor.id
}

fn require_fields(value: &Value, fields: &[&str]) {
    let object = value.as_object().expect("wire object");
    for field in fields {
        assert!(
            object.contains_key(*field),
            "missing required wire field {field}: {value}"
        );
    }
}

#[allow(clippy::too_many_lines)]
fn assert_history(
    history: &Value,
    product: &str,
    root: &str,
    successor: &str,
    prefix: &str,
    accepted: bool,
    global: bool,
) {
    // API views are private and Serialize-only; check their required JSON fields here.
    require_fields(
        history,
        &[
            "product_conversation_id",
            "canonical_route",
            "requested_transcript_row_id",
            "canonical_root",
            "latest_transcript_row_id",
            "writable_transcript_row_id",
            "latest_opening",
            "updated_at",
            "segments",
            "boundaries",
            "before",
            "has_older",
        ],
    );
    assert_eq!(history["product_conversation_id"], product);
    assert!(history["canonical_route"]
        .as_str()
        .unwrap()
        .starts_with(if global { "/global/" } else { "/c/" }));
    assert_eq!(history["canonical_root"]["transcript_row_id"], root);
    require_fields(
        &history["canonical_root"],
        &["transcript_row_id", "slug", "title"],
    );
    assert_eq!(history["latest_transcript_row_id"], successor);
    assert!(history["updated_at"].as_str().is_some());
    assert!(history["before"].is_null());
    assert_eq!(history["has_older"], false);
    if global {
        for field in [
            "ordinary_lifecycle",
            "close",
            "work_identity",
            "presentation",
            "source",
            "chain_qa_compatibility",
        ] {
            assert!(
                history.get(field).is_none(),
                "ordinary authority leaked into Global: {field}"
            );
        }
    } else {
        require_fields(
            history,
            &[
                "ordinary_lifecycle",
                "close",
                "presentation",
                "work_identity",
                "source",
                "chain_qa_compatibility",
            ],
        );
    }
    let opening = &history["latest_opening"];
    require_fields(opening, &["status", "edge", "opening_authority"]);
    assert_eq!(
        opening["status"],
        if accepted { "accepted" } else { "pending" }
    );
    assert_eq!(
        opening["opening_authority"],
        "generated_predecessor_context"
    );
    assert_eq!(
        opening["edge"],
        serde_json::json!({
            "product_conversation_id": product,
            "predecessor_transcript_row_id": root,
            "successor_transcript_row_id": successor,
            "continuation_message_id": format!("{prefix}-boundary"),
        })
    );
    if accepted {
        require_fields(opening, &["accepted_message_id", "presentation"]);
        assert_eq!(opening["accepted_message_id"], format!("{prefix}-opening"));
        assert_eq!(opening["presentation"], "boundary");
        assert_eq!(history["writable_transcript_row_id"], successor);
    } else {
        assert_eq!(opening["client_turn_key"], format!("{prefix}-opening"));
        assert!(history["writable_transcript_row_id"].is_null());
    }
    let segments = history["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2);
    let mut messages = Vec::new();
    for (segment, member) in segments.iter().zip([root, successor]) {
        require_fields(
            segment,
            &[
                "segment_ordinal",
                "transcript_row_id",
                "slug",
                "title",
                "messages",
            ],
        );
        assert_eq!(segment["transcript_row_id"], member);
        assert!(segment.get("handoff").is_none());
        messages.extend(segment["messages"].as_array().unwrap());
    }
    let expected = history_messages(prefix);
    assert_eq!(messages.len(), expected.len());
    for (index, (message, (id, content))) in messages.iter().zip(expected).enumerate() {
        require_fields(
            message,
            &[
                "message_id",
                "origin",
                "event_presentation",
                "conversation_id",
                "sequence_id",
                "message_type",
                "content",
                "display_data",
                "usage_data",
                "created_at",
            ],
        );
        assert_eq!(message["message_id"], id);
        assert_eq!(message["conversation_id"], root);
        assert_eq!(
            message["sequence_id"].as_u64().unwrap(),
            u64::try_from(index + 1).unwrap()
        );
        assert!(message["event_presentation"].is_null());
        let message_type: MessageType =
            serde_json::from_value(message["message_type"].clone()).unwrap();
        let decoded = MessageContent::from_json(message_type, message["content"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(content).unwrap()
        );
    }
    let boundaries = history["boundaries"].as_array().unwrap();
    if accepted {
        assert_eq!(boundaries.len(), 1);
        assert_eq!(
            boundaries[0],
            serde_json::json!({
                "message_id": format!("{prefix}-boundary"),
                "represented_successor_opening_message_id": format!("{prefix}-opening"),
                "predecessor_transcript_row_id": root,
                "successor_transcript_row_id": successor,
                "segment_ordinal": segments[0]["segment_ordinal"],
            })
        );
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Value {
    let response = client.get(url).send().await.unwrap();
    assert_eq!(response.status(), 200, "required persisted read: {url}");
    response.json().await.unwrap()
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
async fn ordinary_and_global_fixture_startup_are_loopback_immutable_and_inert() {
    for accepted in [false, true] {
        assert_fixture_startup(accepted).await;
    }
}

#[allow(clippy::too_many_lines)]
async fn assert_fixture_startup(accepted: bool) {
    let fixture = Fixture::with_opening(accepted).await;
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
        let global = target == fixture.coordinator;
        let (member, successor, product, prefix) = if global {
            (
                target,
                fixture.coordinator_successor.as_str(),
                fixture.coordinator_product.as_str(),
                "global",
            )
        } else {
            (
                "root",
                fixture.root_successor.as_str(),
                fixture.product.as_str(),
                "ordinary",
            )
        };
        // This member endpoint requires live conversation observations, not persisted history.
        assert_eq!(
            client
                .get(format!("{url}/api/conversations/{member}/observation"))
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
        let discovery = get_json(&client, &format!("{url}/api/observation/target")).await;
        assert_eq!(
            discovery["kind"],
            if global { "coordinator" } else { "ordinary" }
        );
        let discovered = &discovery["conversation"];
        require_fields(
            discovered,
            &[
                "product_conversation_id",
                "canonical_route",
                "canonical_root",
                "latest_transcript_row_id",
                "updated_at",
            ],
        );
        require_fields(
            &discovered["canonical_root"],
            &["transcript_row_id", "slug", "title"],
        );
        assert_eq!(discovered["product_conversation_id"], product);
        assert_eq!(discovered["canonical_root"]["transcript_row_id"], member);
        assert_eq!(discovered["latest_transcript_row_id"], successor);
        if global {
            assert_eq!(discovered.as_object().unwrap().len(), 5);
        } else {
            require_fields(discovered, &["lifecycle", "presentation"]);
            assert!(discovered["presentation"]["display_name"]
                .as_str()
                .is_some());
        }
        let history_path = if global {
            format!("/api/global/coordinator/history/{member}?message_limit=100")
        } else {
            format!("/api/product-conversations/{target}?boundary_representation=original_messages&message_limit=100")
        };
        let history = get_json(&client, &format!("{url}{history_path}")).await;
        assert_eq!(discovered["canonical_route"], history["canonical_route"]);
        assert_history(
            &history, product, member, successor, prefix, accepted, global,
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

#[cfg(feature = "portable-ui")]
#[tokio::test]
async fn portable_ui_serves_exact_built_entry_assets_from_private_cwd() {
    use sha2::{Digest, Sha256};

    let fixture = Fixture::new().await;
    assert!(!fixture.dir.path().join("ui/dist").exists());
    let sentinel = listener();
    let mut server = Server(
        fixture
            .command(&sentinel)
            .arg("--observe-fixture")
            .arg(&fixture.path)
            .args(["--target", &fixture.product, "--port", "0"])
            .spawn()
            .unwrap(),
    );
    let address = server.ready();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .unwrap();
    let response = client
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let html = response.bytes().await.unwrap();
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    let expected_html = std::fs::read(dist.join("index.html")).unwrap();
    assert_eq!(Sha256::digest(&html), Sha256::digest(&expected_html));
    let html = std::str::from_utf8(&html).unwrap();
    let entries: Vec<_> = html
        .split("<script")
        .skip(1)
        .filter_map(|tag| {
            let tag = tag.split('>').next()?;
            if !tag.contains("type=\"module\"") {
                return None;
            }
            let asset = tag.split("src=\"").nth(1)?.split('"').next()?;
            (asset.starts_with("/assets/") && asset.ends_with(".js")).then_some(asset)
        })
        .collect();
    assert!(
        !entries.is_empty(),
        "built HTML must reference a module entry"
    );
    let mut manifest = serde_json::json!({
        "index.html": format!("{:x}", Sha256::digest(&expected_html)),
    });
    for entry in entries {
        let filename = entry.strip_prefix("/assets/").unwrap();
        assert!(!filename.contains('/'));
        assert!(
            filename.strip_suffix(".js").unwrap().contains('-'),
            "entry must be content-hashed"
        );
        let response = client
            .get(format!("http://{address}{entry}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mime = response.headers()["content-type"].to_str().unwrap();
        assert!(mime.contains("javascript"), "entry MIME: {mime}");
        let bytes = response.bytes().await.unwrap();
        assert!(!bytes.is_empty());
        let expected = std::fs::read(dist.join(entry.trim_start_matches('/'))).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(digest, format!("{:x}", Sha256::digest(&expected)));
        manifest[entry] = serde_json::Value::String(digest);
    }
    println!("portable_ui_asset_manifest={manifest}");
    terminate(&server.0);
    server.wait(true);
    fixture.assert_isolated(&sentinel);
}
