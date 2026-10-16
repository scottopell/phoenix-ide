#![cfg(unix)]

use base64::Engine;
use phoenix_core::process_identity::{
    current_process_identity, process_identity_matches, ProcessIdentity,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

struct BrowserCli {
    directory: tempfile::TempDir,
    artifacts: Mutex<Vec<PathBuf>>,
    extra_sessions: Mutex<Vec<String>>,
}

impl BrowserCli {
    fn new() -> Self {
        // Unix-domain sockets have a small path budget, including on macOS.
        let directory = tempfile::Builder::new()
            .prefix("pb-cli-")
            .tempdir_in("/tmp")
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            artifacts: Mutex::new(Vec::new()),
            extra_sessions: Mutex::new(Vec::new()),
        }
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-browser"));
        command
            .env("PHOENIX_BROWSER_RUNTIME_DIR", self.directory.path())
            .env("PHOENIX_TMP_DIR", self.directory.path().join("scratch"))
            .env_remove("PHOENIX_BROWSER_SESSION")
            .env_remove("PHOENIX_BROWSER_IDLE_TIMEOUT")
            .arg("--json")
            .stdin(Stdio::null());
        command
    }

    fn command(&self) -> Command {
        let mut command = self.base_command();
        command.args(["--session", "integration"]);
        command
    }

    fn socket(&self) -> PathBuf {
        let key = hex::encode(&Sha256::digest(b"named:integration")[..12]);
        self.directory.path().join(format!("{key}.sock"))
    }

    fn invoke(&self, arguments: &[&str], stdin: Option<&str>, success: bool) -> Value {
        let mut command = self.command();
        command
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            success,
            "CLI {arguments:?}: exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
            output.status.code()
        );
        let response: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("CLI {arguments:?}: {error}\n{stdout}\n{stderr}"));
        assert_eq!(response["session"], "named:integration");
        assert_eq!(
            response["status"],
            if success { "success" } else { "error" }
        );
        if let Some(paths) = response["result"]["artifacts"].as_array() {
            self.artifacts
                .lock()
                .unwrap()
                .extend(paths.iter().filter_map(Value::as_str).map(PathBuf::from));
        }
        response
    }

    fn ok(&self, arguments: &[&str]) -> Value {
        self.invoke(arguments, None, true)
    }

    fn named(&self, name: &str, arguments: &[&str]) -> Value {
        self.extra_sessions.lock().unwrap().push(name.to_string());
        let output = self
            .base_command()
            .args(["--session", name])
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["session"], format!("named:{name}"));
        response
    }

    fn eval(&self, expression: &str) -> Value {
        let response = self.invoke(&["eval", "--stdin"], Some(expression), true);
        evaluation_value(&response)
    }
}

impl Drop for BrowserCli {
    fn drop(&mut self) {
        let _ = self.command().arg("close").output();
        if let Ok(names) = self.extra_sessions.lock() {
            for name in names.iter() {
                let _ = self
                    .base_command()
                    .args(["--session", name, "close"])
                    .output();
            }
        }
        if let Ok(paths) = self.artifacts.lock() {
            for path in paths.iter() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn fixture() -> String {
    let html = r#"<!doctype html><title>CLI browser journey</title>
<label>Name <input id="name" aria-label="Name"></label>
<button id="increment" onclick="window.clicks += 1; console.log('clicked', window.clicks)">Increment</button>
<script>window.clicks = 0; console.error('navigation-marker');</script>"#;
    format!(
        "data:text/html;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(html)
    )
}

fn reference(tree: &str, name: &str) -> String {
    let line = tree
        .lines()
        .find(|line| line.contains(name))
        .unwrap_or_else(|| panic!("missing accessible name {name:?} in snapshot:\n{tree}"));
    let start = line.find('@').unwrap();
    line.get(start..)
        .unwrap()
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || matches!(character, '@' | ':'))
        .collect()
}

fn chrome_enabled() -> bool {
    !matches!(
        std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
        Ok("1" | "true")
    )
}

fn evaluation_value(response: &Value) -> Value {
    let text = response["result"]["text"].as_str().unwrap();
    let value = text
        .strip_prefix("<javascript_result>")
        .and_then(|text| text.strip_suffix("</javascript_result>"))
        .unwrap_or(text);
    serde_json::from_str(value).unwrap()
}

fn live_resources(cli: &BrowserCli) -> (PathBuf, ProcessIdentity) {
    let status = cli.ok(&["status"]);
    let profile = PathBuf::from(status["result"]["profile_path"].as_str().unwrap());
    assert!(
        profile.is_dir(),
        "live profile missing: {}",
        profile.display()
    );
    let pid = u32::try_from(status["result"]["chrome_pid"].as_u64().unwrap()).unwrap();
    let process = current_process_identity(pid).expect("owned Chrome is live");
    (profile, process)
}

fn assert_resources_removed(resources: &(PathBuf, ProcessIdentity)) {
    assert!(
        !resources.0.exists(),
        "profile retained: {}",
        resources.0.display()
    );
    assert!(
        !process_identity_matches(resources.1),
        "owned Chrome is still live: {:?}",
        resources.1
    );
}

fn selected_session(
    cli: &BrowserCli,
    cwd: &Path,
    environment: Option<&str>,
    explicit: Option<&str>,
) -> String {
    let mut command = cli.base_command();
    command.current_dir(cwd);
    if let Some(name) = environment {
        command.env("PHOENIX_BROWSER_SESSION", name);
    }
    if let Some(name) = explicit {
        command.args(["--session", name]);
    }
    let output = command.arg("status").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["active"], false);
    response["session"].as_str().unwrap().to_string()
}

fn git(cwd: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn session_selection_obeys_precedence_and_canonical_worktree_boundaries() {
    // REQ-BCLI-002.
    let cli = BrowserCli::new();
    let root = cli.directory.path().join("repository");
    let nested = root.join("nested/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    git(&root, &["init", "--quiet"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Browser CLI Test",
            "-c",
            "user.email=browser-cli@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Initialize browser CLI session fixture",
        ],
    );
    let linked = cli.directory.path().join("linked");
    git(
        &root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ],
    );
    let root_identity = format!("directory:{}", root.canonicalize().unwrap().display());
    assert_eq!(selected_session(&cli, &root, None, None), root_identity);
    assert_eq!(selected_session(&cli, &nested, None, None), root_identity);
    assert_eq!(
        selected_session(&cli, &nested, Some("harness"), None),
        "named:harness"
    );
    assert_eq!(
        selected_session(&cli, &nested, Some("harness"), Some("explicit")),
        "named:explicit"
    );
    let linked_identity = selected_session(&cli, &linked, None, None);
    assert_eq!(
        linked_identity,
        format!("directory:{}", linked.canonicalize().unwrap().display())
    );
    assert_ne!(linked_identity, root_identity);

    let outside = cli.directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let alias = cli.directory.path().join("outside-alias");
    std::os::unix::fs::symlink(&outside, &alias).unwrap();
    assert_eq!(
        selected_session(&cli, &alias, None, None),
        format!("directory:{}", outside.canonicalize().unwrap().display())
    );
    assert_ne!(
        selected_session(
            &cli,
            &outside,
            None,
            Some(&outside.canonicalize().unwrap().to_string_lossy())
        ),
        selected_session(&cli, &outside, None, None)
    );
}

#[test]
fn invalid_input_and_permissive_runtime_directory_fail_without_startup() {
    // REQ-BCLI-003, REQ-BCLI-010.
    let cli = BrowserCli::new();
    for arguments in [
        vec!["--session", "", "status"],
        vec!["--idle-timeout", "0", "status"],
        vec!["snapshot", "--input", "[1]"],
        vec!["snapshot", "--input", "{"],
        vec!["unknown-command"],
    ] {
        let output = cli.base_command().args(&arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["error"]["code"], "invalid_input");
        assert!(response["lifetime"].is_null());
    }
    std::fs::set_permissions(cli.directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let refused = cli.invoke(&["open", &fixture()], None, false);
    assert_eq!(refused["error"]["code"], "startup_failed");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("private directory"));
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
    std::fs::set_permissions(cli.directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn missing_or_malformed_response_reports_uncertainty_without_replay() {
    // REQ-BCLI-011.
    for response_bytes in [b"not-json\n".as_slice(), b"".as_slice()] {
        let cli = BrowserCli::new();
        let socket = cli.socket();
        let listener = UnixListener::bind(&socket).unwrap();
        let (release, released) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let listener = &listener;
            let server = scope.spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["command"], "eval");
                stream.write_all(response_bytes).unwrap();
                drop(stream);
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                listener.set_nonblocking(true).unwrap();
                assert_eq!(
                    listener.accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
            });
            let response = cli.invoke(&["eval", "window.sideEffects += 1"], None, false);
            assert_eq!(response["error"]["code"], "transport_uncertain");
            assert!(response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not replayed"));
            release.send(()).unwrap();
            server.join().unwrap();
        });
        drop(listener);
        std::fs::remove_file(socket).unwrap();
    }
}

#[test]
fn discovery_does_not_start_a_daemon_or_browser() {
    // REQ-BCLI-004, REQ-BCLI-009, REQ-BCLI-010.
    let cli = BrowserCli::new();
    assert!(
        cli.ok(&["schema"])["result"]["operations"]
            .as_array()
            .unwrap()
            .len()
            >= 12
    );
    assert!(cli.ok(&["profile", "help"])["lifetime"].is_null());
    assert_eq!(cli.ok(&["status"])["result"]["active"], false);
    assert_eq!(cli.ok(&["close"])["result"]["active"], false);
    let missing = cli.invoke(&["click", "#increment"], None, false);
    assert_eq!(missing["error"]["code"], "state_lost");
    assert!(missing["error"]["message"]
        .as_str()
        .unwrap()
        .contains("open"));
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One process-spanning user journey retains its causal ordering"
)]
fn executable_browser_journey_preserves_state_and_profiles() {
    // REQ-BCLI-001, REQ-BCLI-004, REQ-BCLI-005, REQ-BCLI-007..010.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let url = fixture();
    let opened = cli.ok(&["open", &url]);
    let lifetime = opened["lifetime"].clone();
    let browser_lifetime = opened["result"]["browser_lifetime"].clone();
    assert!(lifetime.is_string());
    assert!(browser_lifetime.is_string());
    let mut malformed = UnixStream::connect(cli.socket()).unwrap();
    malformed
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    malformed.write_all(b"{\"command\":\n").unwrap();
    let mut invalid_response = String::new();
    BufReader::new(&malformed)
        .read_line(&mut invalid_response)
        .unwrap();
    let invalid_response: Value = serde_json::from_str(&invalid_response).unwrap();
    assert_eq!(invalid_response["error"]["code"], "invalid_input");
    assert_eq!(invalid_response["lifetime"], lifetime);
    drop(malformed);
    assert_eq!(opened["result"]["url"], url);
    assert_eq!(opened["result"]["title"], "CLI browser journey");
    assert_eq!(opened["result"]["snapshot"]["status"], "success");
    assert!(opened["result"]["console_errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["text"]
                .as_str()
                .is_some_and(|text| text.contains("navigation-marker"))
        }));
    cli.eval("console.error('stale-navigation-marker'); true");
    let navigated = cli.ok(&["open", &url]);
    assert_eq!(navigated["lifetime"], lifetime);
    assert!(!navigated["result"]["console_errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["text"]
            .as_str()
            .is_some_and(|text| text.contains("stale-navigation-marker"))));

    let snapshot = cli.ok(&["snapshot", "--interactive"]);
    assert_eq!(snapshot["lifetime"], lifetime);
    let tree = snapshot["result"]["text"].as_str().unwrap();
    let button = reference(tree, "Increment");
    let input = reference(tree, "Name");
    cli.ok(&["type", &input, "Ada", "--clear"]);
    cli.ok(&["click", &button]);
    assert_eq!(
        cli.eval("({name: document.querySelector('#name').value, clicks: window.clicks})"),
        json!({"name":"Ada","clicks":1})
    );
    assert!(cli.ok(&["console"])["result"]["text"]
        .as_str()
        .unwrap()
        .contains("clicked"));
    let screenshot = cli.ok(&["screenshot"]);
    let screenshot_path = screenshot["result"]["artifacts"][0].as_str().unwrap();
    assert!(std::fs::read(screenshot_path)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));

    cli.ok(&["profile", "cpu_start"]);
    cli.eval("(() => {const end = performance.now() + 50; while (performance.now() < end) {} return true;})()");
    let stopped = cli.ok(&["profile", "cpu_stop"]);
    let profile_path = stopped["result"]["artifacts"][0]
        .as_str()
        .unwrap()
        .to_string();
    let cpu: Value = serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
    assert!(cpu["nodes"]
        .as_array()
        .is_some_and(|nodes| !nodes.is_empty()));

    let plan = json!({
        "runs":2,"warmup":1,"throttle_rate":1,"gc_per_run":false,
        "steps":[{"kind":"wait_selector","selector":"#increment"},
                 {"kind":"eval","expression":"window.clicks += 1"}]
    })
    .to_string();
    let scenario = cli.ok(&["profile", "run_scenario", "--input", &plan]);
    assert_eq!(scenario["lifetime"], lifetime);
    assert_eq!(scenario["result"]["browser_lifetime"], browser_lifetime);
    let data = &scenario["result"]["data"];
    assert_eq!(data["requested_runs"], 2);
    assert_eq!(data["warmup"], 1);
    let samples = data["raw_samples"].as_array().unwrap();
    assert_eq!(samples.len(), 2);
    assert!(samples
        .iter()
        .all(|sample| sample["js_heap_used"].is_null()));
    assert!(!data["methodology_warnings"].as_array().unwrap().is_empty());

    let fresh = cli.ok(&["snapshot", "--interactive"]);
    let stale = reference(fresh["result"]["text"].as_str().unwrap(), "Increment");
    let resources = live_resources(&cli);
    cli.ok(&["close"]);
    assert_resources_removed(&resources);
    let missing = cli.invoke(&["eval", "window.clicks"], None, false);
    assert_eq!(missing["error"]["code"], "state_lost");
    let summary = cli.ok(&[
        "profile",
        "cpu_summary",
        "--input",
        &json!({"path":profile_path}).to_string(),
    ]);
    assert!(summary["lifetime"].is_null());
    assert_eq!(cli.ok(&["status"])["result"]["active"], false);

    let reopened = cli.ok(&["open", &url]);
    assert_ne!(reopened["lifetime"], lifetime);
    assert_ne!(reopened["result"]["browser_lifetime"], browser_lifetime);
    let rejected = cli.invoke(&["click", &stale], None, false);
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("snapshot"));
    assert_eq!(cli.eval("window.clicks"), 0);
    cli.ok(&["close"]);
}

#[test]
fn concurrent_initial_open_has_one_owner_and_lifetime() {
    // REQ-BCLI-003.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let barrier = Barrier::new(2);
    let url = fixture();
    let (first, second) = std::thread::scope(|scope| {
        let open = || {
            barrier.wait();
            cli.ok(&["open", &url])
        };
        let first = scope.spawn(open);
        let second = scope.spawn(open);
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!(first["lifetime"], second["lifetime"]);
    assert_eq!(
        first["result"]["browser_lifetime"],
        second["result"]["browser_lifetime"]
    );
    assert_eq!(cli.ok(&["status"])["lifetime"], first["lifetime"]);
    cli.ok(&["close"]);
}

#[test]
fn separate_named_sessions_isolate_live_page_state() {
    // REQ-BCLI-002, REQ-BCLI-003, REQ-BCLI-005.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let url = fixture();
    let first = cli.ok(&["open", &url]);
    cli.eval("window.onlyFirstSession = 'first'; true");
    let second = cli.named("isolated", &["open", &url]);
    assert_ne!(first["lifetime"], second["lifetime"]);
    assert_ne!(
        first["result"]["browser_lifetime"],
        second["result"]["browser_lifetime"]
    );
    assert_eq!(
        evaluation_value(&cli.named("isolated", &["eval", "typeof window.onlyFirstSession"])),
        "undefined"
    );
    cli.ok(&["close"]);
    assert_eq!(cli.named("isolated", &["status"])["result"]["active"], true);
    cli.named("isolated", &["close"]);
}

#[test]
fn navigation_success_survives_snapshot_response_limit_failure() {
    // REQ-BCLI-007: the snapshot is a separate observation after navigation.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let html = "<!doctype html><title>Partial snapshot failure</title><button id='huge'>Control</button><script>document.querySelector('#huge').ariaLabel = 'x'.repeat(9 * 1024 * 1024);</script>";
    let url = format!(
        "data:text/html;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(html)
    );
    let opened = cli.ok(&["open", &url]);
    assert_eq!(opened["result"]["navigation"], "succeeded");
    assert_eq!(opened["result"]["url"], url);
    assert_eq!(opened["result"]["title"], "Partial snapshot failure");
    assert_eq!(opened["result"]["snapshot"]["status"], "error");
    let error = opened["result"]["snapshot"]["error"]["message"]
        .as_str()
        .unwrap();
    assert!(error.len() < 4096, "snapshot error should remain bounded");
    assert_eq!(cli.eval("document.querySelectorAll('button').length"), 1);
    assert_eq!(cli.ok(&["status"])["lifetime"], opened["lifetime"]);
    cli.ok(&["close"]);
}

#[test]
fn consecutive_close_and_immediate_open_replace_owned_resources() {
    // REQ-BCLI-003, REQ-BCLI-005, REQ-BCLI-006.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let url = fixture();
    let mut previous = cli.ok(&["open", &url]);
    for _ in 0..3 {
        let resources = live_resources(&cli);
        cli.ok(&["close"]);
        let opened = cli.ok(&["open", &url]);
        assert_ne!(opened["lifetime"], previous["lifetime"]);
        assert_ne!(
            opened["result"]["browser_lifetime"],
            previous["result"]["browser_lifetime"]
        );
        assert_resources_removed(&resources);
        previous = opened;
    }
    let resources = live_resources(&cli);
    cli.ok(&["close"]);
    assert_resources_removed(&resources);
}

#[test]
fn daemon_releases_descriptors_inherited_from_its_invoking_harness() {
    // REQ-BCLI-001, REQ-BCLI-003: a live daemon must not hold harness pipes open.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let (mut reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let descriptor = writer.as_raw_fd();
    let mut command = cli.command();
    command.args(["open", &fixture()]);
    // SAFETY: this child-only callback performs one async-signal-safe fcntl
    // operation on a descriptor retained until the spawned child exits.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(descriptor, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "success");
    drop(writer);
    let mut byte = [0];
    assert_eq!(
        reader
            .read(&mut byte)
            .expect("live daemon retained a harness descriptor"),
        0
    );
    assert_eq!(cli.ok(&["status"])["result"]["active"], true);
    cli.ok(&["close"]);
}

#[test]
fn active_work_outlives_idle_budget_then_session_expires() {
    // REQ-BCLI-004, REQ-BCLI-005, REQ-BCLI-006.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let url = fixture();
    let opened = cli.ok(&["--idle-timeout", "1", "open", &url]);
    let resources = live_resources(&cli);
    let active = cli.invoke(
        &["eval", "--stdin"],
        Some("new Promise(resolve => setTimeout(() => resolve('finished'), 2200))"),
        true,
    );
    assert_eq!(active["lifetime"], opened["lifetime"]);
    assert_eq!(evaluation_value(&active), "finished");
    let status = cli.ok(&["status"]);
    assert_eq!(status["lifetime"], opened["lifetime"]);
    assert_eq!(status["result"]["idle_timeout_seconds"], 1);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        // Poll only after a full quiet interval: status itself is activity.
        std::thread::sleep(Duration::from_millis(1300));
        let status = cli.ok(&["status"]);
        if status["result"]["active"] == false {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "session did not expire: {status}"
        );
    }
    assert_eq!(
        cli.invoke(&["snapshot"], None, false)["error"]["code"],
        "state_lost"
    );
    assert_resources_removed(&resources);
    let restarted = cli.ok(&["open", &url]);
    assert_ne!(restarted["lifetime"], opened["lifetime"]);
    assert_ne!(
        restarted["result"]["browser_lifetime"],
        opened["result"]["browser_lifetime"]
    );
    cli.ok(&["close"]);
}
