#![cfg(unix)]

use base64::Engine;
use phoenix_core::process_identity::{
    current_process_identity, process_identity_matches, ProcessIdentity,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt;
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
        assert!(response.get("browser_lifetime").is_some());
        assert!(response["result"].get("browser_lifetime").is_none());
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
        assert!(response.get("browser_lifetime").is_some());
        assert!(response["result"].get("browser_lifetime").is_none());
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

fn signal_owned(process: ProcessIdentity, signal: libc::c_int) {
    assert!(
        process_identity_matches(process),
        "owned process exited before signal"
    );
    let pid = libc::pid_t::try_from(process.pid).unwrap();
    // SAFETY: this PID is the exact process identity captured by this test.
    assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
}

#[track_caller]
fn wait_for_observation<T>(reason: &str, mut observe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = observe() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "observation never arrived: {reason}"
        );
        // test-timing-allow: poll an explicit external OS/file condition under a liveness deadline; elapsed time never establishes readiness.
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn await_process_exit(process: ProcessIdentity) {
    wait_for_observation("owned process exited", || {
        (!process_identity_matches(process)).then_some(())
    });
}

struct PendingLaunchCleanup {
    release: PathBuf,
    processes: Vec<ProcessIdentity>,
}

impl Drop for PendingLaunchCleanup {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.release, b"release");
        for process in &self.processes {
            if process_identity_matches(*process) {
                if let Ok(pid) = libc::pid_t::try_from(process.pid) {
                    // SAFETY: only an exact process created by this test is signaled.
                    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
    }
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
    let output = phoenix_core::git::command()
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
#[cfg(target_os = "linux")]
fn non_utf8_worktree_paths_share_sessions_without_lossy_collisions() {
    // REQ-BCLI-002.
    let cli = BrowserCli::new();
    let mut identities = Vec::new();
    for name in [b"repository-\xff".to_vec(), b"repository-\xfe".to_vec()] {
        let root = cli
            .directory
            .path()
            .join(std::ffi::OsString::from_vec(name));
        let nested = root.join("nested/deeper");
        std::fs::create_dir_all(&nested).unwrap();
        git(&root, &["init", "--quiet"]);
        let identity = selected_session(&cli, &root, None, None);
        assert_eq!(selected_session(&cli, &nested, None, None), identity);
        identities.push(identity);
    }
    assert_ne!(identities[0], identities[1]);
    let replacement = cli.directory.path().join("repository-\u{fffd}");
    std::fs::create_dir(&replacement).unwrap();
    let replacement_identity = selected_session(&cli, &replacement, None, None);
    assert!(!identities.contains(&replacement_identity));

    let outside = cli
        .directory
        .path()
        .join(std::ffi::OsString::from_vec(b"outside-\xff".to_vec()));
    std::fs::create_dir(&outside).unwrap();
    let alias = cli.directory.path().join("outside-alias");
    std::os::unix::fs::symlink(&outside, &alias).unwrap();
    assert_eq!(
        selected_session(&cli, &outside, None, None),
        selected_session(&cli, &alias, None, None)
    );
}

#[test]
fn invalid_input_and_permissive_runtime_directory_fail_without_startup() {
    // REQ-BCLI-003, REQ-BCLI-010.
    let cli = BrowserCli::new();
    for arguments in [
        vec!["--session", "", "status"],
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
fn parse_errors_preserve_selected_session_and_reject_invalid_budgets_before_startup() {
    let cli = BrowserCli::new();
    for arguments in [
        vec!["snapshot", "--input", "{"],
        vec!["unknown-command"],
        vec!["open", "about:blank", "--timeout", "nonsense"],
        vec![
            "open",
            "--input",
            r#"{"url":"about:blank","timeout":"1hour"}"#,
        ],
        vec!["open", "--input", r#"{"url":"about:blank","timeout":15}"#],
        vec!["open", "about:blank", "--timeout", "18446744073709551615m"],
        vec!["status", "--idle-timeout"],
    ] {
        let output = cli.command().args(&arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["session"], "named:integration", "{arguments:?}");
        assert_eq!(response["error"]["code"], "invalid_input");
        assert!(response["lifetime"].is_null());
        assert!(response["browser_lifetime"].is_null());
        assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
    }
    let output = cli
        .base_command()
        .env("PHOENIX_BROWSER_SESSION", "environment")
        .args(["snapshot", "--input", "{"])
        .output()
        .unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["session"], "named:environment");
    let cwd = tempfile::tempdir().unwrap();
    let output = cli
        .base_command()
        .current_dir(cwd.path())
        .args(["snapshot", "--input", "{"])
        .output()
        .unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["session"],
        format!("directory:{}", cwd.path().canonicalize().unwrap().display())
    );
}

#[test]
fn help_is_an_option_only_at_the_command_help_position() {
    let cli = BrowserCli::new();
    for arguments in [
        vec!["--help"],
        vec!["-h"],
        vec!["snapshot", "--help"],
        vec!["snapshot", "--help", "--json"],
    ] {
        let output = cli.command().args(arguments).output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: phoenix-browser"));
    }
    for payload in ["--help", "-h"] {
        let response = cli.invoke(&["type", "#name", payload], None, false);
        assert_eq!(response["error"]["code"], "state_lost");
    }
    let response = cli.invoke(&["type", "#name", "--", "--json"], None, false);
    assert_eq!(response["error"]["code"], "state_lost");
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
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
                    .set_read_timeout(Some(Duration::from_secs(60)))
                    .unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["command"], "eval");
                stream.write_all(response_bytes).unwrap();
                drop(stream);
                released.recv_timeout(Duration::from_secs(60)).unwrap();
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
fn oversized_stdin_is_rejected_before_starting_or_evaluating() {
    // REQ-BCLI-010, REQ-BCLI-011: truncated JavaScript must never execute.
    let cli = BrowserCli::new();
    let mut child = cli
        .base_command()
        .args(["eval", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = vec![b' '; 32 * 1024 * 1024 + 1];
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["code"], "invalid_input");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not evaluated"));
    assert!(response["lifetime"].is_null());
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
}

#[test]
fn encoded_request_budget_is_checked_before_session_access() {
    let cli = BrowserCli::new();
    for (character, length) in [
        ('"', 17 * 1024 * 1024),
        ('\\', 17 * 1024 * 1024),
        ('a', 32 * 1024 * 1024),
    ] {
        let input: String = std::iter::repeat_n(character, length).collect();
        let mut child = cli
            .command()
            .args(["eval", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["session"], "named:integration");
        assert_eq!(response["error"]["code"], "invalid_input");
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("serialized protocol message"));
        assert!(response["lifetime"].is_null());
        assert!(response["browser_lifetime"].is_null());
        assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
    }
    let accepted = cli.invoke(
        &["eval", "--stdin"],
        Some(&"a".repeat(17 * 1024 * 1024)),
        false,
    );
    assert_eq!(accepted["error"]["code"], "state_lost");
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
}

fn assert_semantic_rejections(cli: &BrowserCli) {
    for arguments in [
        vec!["click"],
        vec!["click", "--input", "{\"selector\":\"#x\",\"ref\":\"@x\"}"],
        vec!["profile", "run_scenario", "--input", "{\"steps\":[]}"],
        vec!["profile", "unknown-action"],
        vec!["profile", "throttle", "--input", "{\"rate\":0}"],
        vec!["profile", "cpu_summary"],
        vec!["profile", "cpu_summary", "--input", "{\"path\":42}"],
        vec!["profile", "help", "--input", "{\"runs\":\"bad\"}"],
    ] {
        let output = cli.command().args(&arguments).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["error"]["code"], "invalid_input");
    }
}

#[test]
fn semantic_input_errors_are_reported_before_looking_up_browser_state() {
    let cli = BrowserCli::new();
    assert_semantic_rejections(&cli);
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 0);
}

#[test]
fn invalid_idle_environment_does_not_block_browser_free_commands() {
    let cli = BrowserCli::new();
    let profile = cli.directory.path().join("saved.cpuprofile");
    std::fs::write(
        &profile,
        br#"{"nodes":[],"startTime":0,"endTime":1,"samples":[],"timeDeltas":[]}"#,
    )
    .unwrap();
    let input = json!({"path":profile}).to_string();
    for arguments in [
        vec!["status"],
        vec!["close"],
        vec!["schema"],
        vec!["profile", "help"],
        vec!["profile", "cpu_summary", "--input", input.as_str()],
    ] {
        let output = cli
            .command()
            .env("PHOENIX_BROWSER_IDLE_TIMEOUT", "not-a-duration")
            .args(&arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(response["lifetime"].is_null());
    }
    assert_eq!(std::fs::read_dir(cli.directory.path()).unwrap().count(), 1);
    let output = cli
        .command()
        .env("PHOENIX_BROWSER_IDLE_TIMEOUT", "not-a-duration")
        .args(["open", "about:blank"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"]["code"], "invalid_input");
    assert_eq!(cli.ok(&["status"])["result"]["active"], false);
}

#[test]
fn live_session_input_errors_and_close_ignore_startup_only_idle_config() {
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let opened = cli.ok(&["open", &fixture()]);
    assert_semantic_rejections(&cli);
    assert_eq!(cli.ok(&["status"])["lifetime"], opened["lifetime"]);
    let resources = live_resources(&cli);
    for command in ["status", "close"] {
        let output = cli
            .command()
            .env("PHOENIX_BROWSER_IDLE_TIMEOUT", "not-a-duration")
            .arg(command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    assert_resources_removed(&resources);
}

#[test]
fn close_interrupts_an_active_evaluation_and_cleans_up() {
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    cli.ok(&["open", &fixture()]);
    let resources = live_resources(&cli);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let expression = format!("(() => {{ void fetch('http://{}/active').catch(() => {{}}); return new Promise(() => {{}}); }})()", listener.local_addr().unwrap());
    std::thread::scope(|scope| {
        let evaluation =
            scope.spawn(|| cli.invoke(&["eval", &expression, "--timeout", "1m"], None, false));
        let mut stream = wait_for_observation("evaluation reached its HTTP boundary", || {
            match listener.accept() {
                Ok((stream, _)) => Some(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(error) => panic!("evaluation readiness listener failed: {error}"),
            }
        });
        stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        drop(stream);
        let close_started = Instant::now();
        cli.ok(&["close"]);
        assert!(
            close_started.elapsed() < Duration::from_secs(10),
            "close waited for the evaluation timeout"
        );
        let cancelled = evaluation.join().unwrap();
        assert_eq!(cancelled["error"]["code"], "operation_cancelled");
    });
    assert_resources_removed(&resources);
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
fn partial_clients_do_not_block_close_signals_or_idle_cleanup() {
    // REQ-BCLI-006.
    if !chrome_enabled() {
        return;
    }
    for shutdown in ["close", "signal", "idle"] {
        let cli = BrowserCli::new();
        cli.ok(&["--idle-timeout", "2", "open", &fixture()]);
        let resources = live_resources(&cli);
        let status = cli.ok(&["status"]);
        let daemon_pid = u32::try_from(status["result"]["daemon_pid"].as_u64().unwrap()).unwrap();
        let daemon = current_process_identity(daemon_pid).unwrap();
        let mut partial = UnixStream::connect(cli.socket()).unwrap();
        partial
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        partial.write_all(b"{\"command\":").unwrap();
        match shutdown {
            "close" => {
                cli.ok(&["close"]);
            }
            "signal" => signal_owned(daemon, libc::SIGTERM),
            "idle" => {}
            _ => unreachable!(),
        }
        let mut reply = Vec::new();
        match partial.read_to_end(&mut reply) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!("{shutdown}: partial client was not retired: {error}"),
        }
        assert!(
            reply.is_empty(),
            "{shutdown}: partial client outlived request-read timeout: {reply:?}"
        );
        await_process_exit(daemon);
        assert_resources_removed(&resources);
        assert!(!cli.socket().exists());
    }
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
    for payload in ["--help", "-h"] {
        cli.ok(&["type", "#name", payload, "--clear"]);
        assert_eq!(cli.eval("document.querySelector('#name').value"), payload);
    }
    cli.ok(&["type", "#name", "--clear", "--", "--json"]);
    assert_eq!(cli.eval("document.querySelector('#name').value"), "--json");
    let lifetime = opened["lifetime"].clone();
    let browser_lifetime = opened["browser_lifetime"].clone();
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
    assert_eq!(scenario["browser_lifetime"], browser_lifetime);
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
    assert_ne!(reopened["browser_lifetime"], browser_lifetime);
    let rejected = cli.invoke(&["click", &stale], None, false);
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("snapshot"));
    assert_eq!(cli.eval("window.clicks"), 0);
    cli.ok(&["close"]);
}

#[test]
fn failed_navigation_preserves_session_console_history() {
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let opened = cli.ok(&["open", &fixture()]);
    cli.eval("console.error('history-survives-failed-open'); true");
    let before = wait_for_observation("console captured the retained history entry", || {
        let response = cli.ok(&["console"]);
        response["result"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("history-survives-failed-open"))
            .then_some(response)
    });
    let failed = cli.invoke(&["open", "http://127.0.0.1:1/"], None, false);
    assert_eq!(failed["lifetime"], opened["lifetime"]);
    assert_eq!(failed["browser_lifetime"], opened["browser_lifetime"]);
    let after = cli.ok(&["console"]);
    assert_eq!(after["lifetime"], before["lifetime"]);
    assert_eq!(after["browser_lifetime"], before["browser_lifetime"]);
    let entries = |response: &Value| -> Vec<Value> {
        serde_json::from_str(response["result"]["text"].as_str().unwrap()).unwrap()
    };
    let before = entries(&before);
    let after = entries(&after);
    let retained = before
        .iter()
        .find(|entry| entry["text"] == "history-survives-failed-open")
        .unwrap();
    assert!(
        after.contains(retained),
        "failed open discarded history: {after:?}"
    );
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
    assert_eq!(first["browser_lifetime"], second["browser_lifetime"]);
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
    assert_ne!(first["browser_lifetime"], second["browser_lifetime"]);
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
fn blocked_profile_preserves_structured_measurement_failure() {
    // REQ-BCLI-008, REQ-BCLI-010: failed measurements retain their methodology data.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let opened = cli.ok(&["open", &fixture()]);
    let plan = json!({"runs":2,"warmup":0,"reset":"none","gc_per_run":false,
        "steps":[{"kind":"wait_selector","selector":"#never-present","timeout":"20ms"}]})
    .to_string();
    let blocked = cli.invoke(&["profile", "run_scenario", "--input", &plan], None, false);
    assert_eq!(blocked["error"]["code"], "operation_failed");
    assert_eq!(blocked["lifetime"], opened["lifetime"]);
    assert_eq!(blocked["browser_lifetime"], opened["browser_lifetime"]);
    let details = &blocked["error"]["details"];
    assert_eq!(details["outcome"], "blocked");
    assert!(details["blocked_step"]
        .as_str()
        .unwrap()
        .contains("#never-present"));
    assert_eq!(details["raw_samples"], json!([]));
    assert!(!details["methodology_warnings"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(blocked["error"]["artifacts"], json!([]));
    cli.ok(&["close"]);
}

#[test]
fn lost_chrome_reports_state_loss_and_open_replaces_only_browser_lifetime() {
    // REQ-BCLI-004, REQ-BCLI-005: the daemon can outlive its owned Chrome.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let url = fixture();
    let opened = cli.ok(&["open", &url]);
    let tree = &opened["result"]["snapshot"]["result"]["text"];
    let stale = reference(tree.as_str().unwrap(), "Increment");
    cli.ok(&["profile", "cpu_start"]);
    cli.eval("for(let i=0;i<150;i++){const b=document.createElement('button');b.textContent='Large snapshot control '+i;document.body.append(b)};true");
    let snapshot = cli.ok(&["snapshot"]);
    let artifact = PathBuf::from(snapshot["result"]["artifacts"][0].as_str().unwrap());
    assert!(artifact.is_file());
    let resources = live_resources(&cli);
    let sentinel = resources.0.join("old-lifetime-sentinel");
    std::fs::write(&sentinel, b"old browser state").unwrap();
    signal_owned(resources.1, libc::SIGKILL);
    let status = wait_for_observation("owned Chrome was reaped", || {
        let status = cli.ok(&["status"]);
        (status["result"]["active"] == false).then_some(status)
    });
    await_process_exit(resources.1);
    assert_eq!(status["lifetime"], opened["lifetime"]);
    assert_eq!(status["result"]["active"], false);
    assert!(status["browser_lifetime"].is_null());
    assert!(status["result"]["profile_path"].is_null());
    assert!(
        !artifact.exists(),
        "dead browser retained its snapshot artifact"
    );
    assert_resources_removed(&resources);
    let missing = cli.invoke(&["snapshot"], None, false);
    assert_eq!(missing["error"]["code"], "state_lost");
    assert_eq!(missing["lifetime"], opened["lifetime"]);
    let reopened = cli.ok(&["open", &url]);
    assert_eq!(reopened["lifetime"], opened["lifetime"]);
    assert_ne!(reopened["browser_lifetime"], opened["browser_lifetime"]);
    assert!(
        !sentinel.exists(),
        "old browser profile contents survived replacement"
    );
    assert!(!process_identity_matches(resources.1));
    let rejected = cli.invoke(&["click", &stale], None, false);
    assert_eq!(rejected["browser_lifetime"], reopened["browser_lifetime"]);
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("snapshot"));
    assert_eq!(cli.eval("window.clicks"), 0);
    let retired_profile = cli.invoke(&["profile", "cpu_stop"], None, false);
    assert_eq!(
        retired_profile["browser_lifetime"],
        reopened["browser_lifetime"]
    );
    assert!(retired_profile["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not active"));
    cli.ok(&["close"]);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One launch barrier proves shutdown waits for ownership publication"
)]
fn sigterm_during_chrome_startup_cleans_up_the_published_browser() {
    // REQ-BCLI-006: shutdown cannot abandon a browser initialization future.
    if !chrome_enabled() {
        return;
    }
    let cli = BrowserCli::new();
    let executable = std::env::var_os("PHOENIX_CHROME_EXECUTABLE")
        .map(PathBuf::from)
        .or_else(|| which::which("google-chrome").ok())
        .or_else(|| which::which("chromium").ok())
        .or_else(|| {
            let chrome =
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
            chrome.is_file().then_some(chrome)
        })
        .expect("dev.py supplies the Chrome executable for browser tests");
    let wrapper = cli.directory.path().join("chrome-launch-barrier.sh");
    let ready = cli.directory.path().join("launch-ready");
    let release = cli.directory.path().join("launch-release");
    std::fs::write(&wrapper, b"#!/bin/sh\nprintf '%s %s\\n' \"$$\" \"$PPID\" > \"$PHOENIX_BROWSER_TEST_READY\"\nwhile [ ! -e \"$PHOENIX_BROWSER_TEST_RELEASE\" ]; do sleep 0.02; done\nexec \"$PHOENIX_BROWSER_TEST_CHROME\" \"$@\"\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut cleanup = PendingLaunchCleanup {
        release: release.clone(),
        processes: Vec::new(),
    };
    let mut opening = cli
        .command()
        .args(["open", &fixture()])
        .env("PHOENIX_CHROME_EXECUTABLE", &wrapper)
        .env("PHOENIX_BROWSER_TEST_READY", &ready)
        .env("PHOENIX_BROWSER_TEST_RELEASE", &release)
        .env("PHOENIX_BROWSER_TEST_CHROME", &executable)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (chrome, daemon) = wait_for_observation("Chrome launch barrier was reached", || {
        if let Ok(contents) = std::fs::read_to_string(&ready) {
            let pids: Vec<_> = contents
                .split_whitespace()
                .filter_map(|pid| pid.parse::<u32>().ok())
                .collect();
            if pids.len() == 2 {
                return Some((
                    current_process_identity(pids[0]).unwrap(),
                    current_process_identity(pids[1]).unwrap(),
                ));
            }
        }
        assert!(
            opening.try_wait().unwrap().is_none(),
            "CLI exited before launch barrier"
        );
        None
    });
    cleanup.processes.extend([daemon, chrome]);
    let profiles: Vec<_> = std::fs::read_dir(cli.directory.path().join("scratch"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("phoenix-chrome-")
        })
        .collect();
    assert_eq!(profiles.len(), 1);
    signal_owned(daemon, libc::SIGTERM);
    std::fs::write(&release, b"release").unwrap();
    wait_for_observation("accepted startup operation completed", || {
        opening.try_wait().unwrap()
    });
    let output = opening.wait_with_output().unwrap();
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    if !output.status.success() {
        assert_eq!(response["error"]["code"], "operation_cancelled");
    }
    await_process_exit(daemon);
    await_process_exit(chrome);
    assert!(!profiles[0].exists(), "startup profile was orphaned");
    assert_eq!(cli.ok(&["status"])["result"]["active"], false);
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
        let closed = cli.ok(&["close"]);
        assert_eq!(closed["lifetime"], previous["lifetime"]);
        assert_eq!(closed["browser_lifetime"], previous["browser_lifetime"]);
        let opened = cli.ok(&["open", &url]);
        assert_ne!(opened["lifetime"], previous["lifetime"]);
        assert_ne!(opened["browser_lifetime"], previous["browser_lifetime"]);
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let opening = cli
        .command()
        .args(["--idle-timeout", "1", "open", &url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut page_request =
        wait_for_observation(
            "initial navigation reached HTTP boundary",
            || match listener.accept() {
                Ok((stream, _)) => Some(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(error) => panic!("navigation listener failed: {error}"),
            },
        );
    let queue = |request: Value| {
        let mut stream = UnixStream::connect(cli.socket()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        stream
    };
    let read_response = |stream: UnixStream| -> Value {
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    };
    let status = queue(json!({"command":"status", "input":{}}));
    let evaluation = queue(json!({"command":"eval", "input":{
        "expression":"new Promise(resolve => setTimeout(() => resolve('finished'), 2200))",
        "timeout":"10s"
    }}));
    page_request.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 55\r\nConnection: close\r\n\r\n<!doctype html><title>Idle lifetime</title><p>Ready</p>").unwrap();
    drop(page_request);
    let status = read_response(status);
    assert_eq!(status["status"], "success");
    assert_eq!(status["result"]["idle_timeout_seconds"], 1);
    let daemon_pid = u32::try_from(status["result"]["daemon_pid"].as_u64().unwrap()).unwrap();
    let daemon = current_process_identity(daemon_pid).expect("operation owns idle daemon");
    let chrome_pid = u32::try_from(status["result"]["chrome_pid"].as_u64().unwrap()).unwrap();
    let resources = (
        PathBuf::from(status["result"]["profile_path"].as_str().unwrap()),
        current_process_identity(chrome_pid).expect("operation owns Chrome"),
    );
    let output = opening.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let opened: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["lifetime"], opened["lifetime"]);
    let active = read_response(evaluation);
    assert_eq!(active["status"], "success");
    assert_eq!(active["lifetime"], opened["lifetime"]);
    assert_eq!(evaluation_value(&active), "finished");
    await_process_exit(daemon);
    assert_eq!(cli.ok(&["status"])["result"]["active"], false);
    assert_eq!(
        cli.invoke(&["snapshot"], None, false)["error"]["code"],
        "state_lost"
    );
    assert_resources_removed(&resources);
    let restarted = cli.ok(&["open", &fixture()]);
    assert_ne!(restarted["lifetime"], opened["lifetime"]);
    assert_ne!(restarted["browser_lifetime"], opened["browser_lifetime"]);
    cli.ok(&["close"]);
}
