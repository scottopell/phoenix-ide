use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::probe::{probe_sync, ProbeResult};
use super::registry::TmuxRegistry;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);

const WATCHDOG_PROGRAM: &str = r#"
import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

root = Path(sys.argv[1])
parent = int(sys.argv[2])
(root / ".armed").touch()
heartbeat = root / ".parent-heartbeat"
while not (root / ".cleanup-request").exists():
    if parent == 1:
        try:
            if time.time() - heartbeat.stat().st_mtime > 0.5:
                break
        except FileNotFoundError:
            break
    elif os.getppid() != parent:
        break
    time.sleep(0.05)
(root / ".cleanup-ack").touch()
quiet = 0
for _ in range(50):
    unconfirmed = False
    for socket in root.glob("*.sock"):
        if socket.is_symlink():
            socket.unlink(missing_ok=True)
            continue
        if not socket.is_socket():
            continue
        try:
            probe = subprocess.run(
                ["tmux", "-S", str(socket), "list-sessions"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=0.5,
            )
            if probe.returncode == 0:
                killed = subprocess.run(
                    ["tmux", "-S", str(socket), "kill-server"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                    timeout=0.5,
                )
                if killed.returncode == 0:
                    socket.unlink(missing_ok=True)
                else:
                    unconfirmed = True
            else:
                unconfirmed = True
        except (OSError, subprocess.TimeoutExpired):
            unconfirmed = True
    sockets = any(path.is_socket() for path in root.glob("*.sock"))
    creators = False
    for marker in root.glob(".creating-*"):
        try:
            with marker.open("r+") as marker_file:
                try:
                    fcntl.flock(marker_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    marker.unlink(missing_ok=True)
                    marker.with_suffix(".locked").unlink(missing_ok=True)
                except BlockingIOError:
                    creators = True
        except OSError:
            creators = True
    quiet = quiet + 1 if not unconfirmed and not sockets and not creators else 0
    if quiet >= 5:
        shutil.rmtree(root)
        sys.exit(0)
    time.sleep(0.1)
sys.exit(1)
"#;

/// Owns real tmux servers created by tests, including after abrupt runner death.
pub struct TestTmuxServerOwner {
    root: Option<TempDir>,
    watchdog: Option<Child>,
    heartbeat_stop: Arc<AtomicBool>,
    heartbeat: Option<thread::JoinHandle<()>>,
}

impl Default for TestTmuxServerOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl TestTmuxServerOwner {
    /// Creates an isolated short socket root and its detached cleanup watchdog.
    ///
    /// # Panics
    ///
    /// Panics when the temporary root or watchdog process cannot be created or
    /// armed. Tests cannot safely continue without containment.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_watchdog_path(None)
    }

    fn new_with_watchdog_path(watchdog_path: Option<&Path>) -> Self {
        let root = tempfile::Builder::new()
            .prefix("ptt-")
            .tempdir_in("/private/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("ptt-").tempdir_in("/tmp"))
            .expect("create short isolated tmux test root");
        let canonical_root = root.path().canonicalize().expect("canonicalize test root");
        let private_tmp = Path::new("/private/tmp")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("/private/tmp"));
        let tmp = Path::new("/tmp")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        assert!(
            canonical_root.starts_with(private_tmp) || canonical_root.starts_with(tmp),
            "tmux test root must be under a short system temporary directory"
        );

        fs::write(canonical_root.join(".parent-heartbeat"), [])
            .expect("initialize tmux test owner heartbeat");
        let parent_pid = std::process::id().to_string();
        let mut command = Command::new("python3");
        command
            .args(["-c", WATCHDOG_PROGRAM])
            .arg(&canonical_root)
            .arg(parent_pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = watchdog_path {
            let inherited_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![path.to_path_buf()];
            paths.extend(std::env::split_paths(&inherited_path));
            command.env(
                "PATH",
                std::env::join_paths(paths).expect("join watchdog PATH"),
            );
        }
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut watchdog = command.spawn().expect("spawn tmux test watchdog");
        wait_for_watchdog_arm(&mut watchdog, &canonical_root)
            .expect("tmux test watchdog must arm before owner is exposed");
        let heartbeat_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&heartbeat_stop);
        let heartbeat_path = canonical_root.join(".parent-heartbeat");
        let heartbeat = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let _ = fs::write(&heartbeat_path, []);
                // test-timing-allow: heartbeat cadence detects PID-1 runner death; cleanup uses explicit markers and watchdog exit
                thread::sleep(Duration::from_millis(100));
            }
        });

        Self {
            root: Some(root),
            watchdog: Some(watchdog),
            heartbeat_stop,
            heartbeat: Some(heartbeat),
        }
    }

    pub(crate) fn socket_dir(&self) -> &Path {
        self.root.as_ref().expect("owner is live").path()
    }

    /// Returns the unique socket root owned by this test fixture.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.socket_dir()
    }

    /// Creates a registry whose servers are confined to this owner's root.
    #[must_use]
    pub fn registry(&self) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir(self.socket_dir().to_path_buf()).with_test_spawn_containment()
    }

    #[cfg(test)]
    pub(crate) fn registry_with_sink(
        &self,
        sink: Option<super::registry::TmuxLifecycleSink>,
    ) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir_binary_and_sink(self.socket_dir().to_path_buf(), true, sink)
            .with_test_spawn_containment()
    }

    /// Kills and verifies all exact servers under the owned root.
    ///
    /// # Panics
    ///
    /// Panics when the watchdog cannot complete cleanup or any exact server
    /// remains live. The root is preserved for recovery in that case.
    pub fn shutdown(mut self) {
        self.finish(true).expect("tmux test cleanup must succeed");
    }

    fn finish(&mut self, graceful: bool) -> io::Result<()> {
        let root = self.root.take().expect("owner root is live");
        let mut watchdog = self.watchdog.take().expect("watchdog is live");
        let root_path = root.path().to_path_buf();
        self.heartbeat_stop.store(true, Ordering::Release);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        let handoff_error = request_cleanup(&root_path, graceful).err();

        let result = wait_for_watchdog(&mut watchdog).and_then(|status| {
            if !status.success() {
                return Err(io::Error::other(format!(
                    "tmux test watchdog reported cleanup failure: {status}"
                )));
            }
            if root_path.exists() {
                verify_no_live_servers(&root_path)?;
            }
            if let Some(error) = handoff_error {
                return Err(error);
            }
            Ok(())
        });
        if result.is_err() {
            let _ = root.keep();
        }
        result
    }
}

impl Drop for TestTmuxServerOwner {
    fn drop(&mut self) {
        if self.watchdog.is_none() {
            return;
        }
        if let Err(error) = self.finish(false) {
            if thread::panicking() {
                eprintln!("tmux test cleanup failed while unwinding: {error}");
            } else {
                panic!("tmux test cleanup failed: {error}");
            }
        }
    }
}

fn verify_no_live_servers(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_socket() && probe_sync(&entry.path()) == ProbeResult::Live {
            return Err(io::Error::other(format!(
                "tmux server remains live at {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn wait_for_watchdog_arm(watchdog: &mut Child, root: &Path) -> io::Result<()> {
    let armed = root.join(".armed");
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    while !armed.exists() {
        if let Some(status) = watchdog.try_wait()? {
            return Err(io::Error::other(format!(
                "tmux test watchdog exited before arming: {status}"
            )));
        }
        if Instant::now() >= deadline {
            let _ = watchdog.kill();
            let _ = watchdog.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not arm",
            ));
        }
        // test-timing-allow: the armed marker is the completion signal; the deadline only bounds failed startup
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn request_cleanup(root: &Path, graceful: bool) -> io::Result<()> {
    let request = root.join(".cleanup-request");
    let pending = root.join(".cleanup-request.pending");
    let reason: &[u8] = if graceful { b"graceful" } else { b"drop" };
    fs::write(&pending, reason)?;
    fs::rename(pending, request)?;
    let ack = root.join(".cleanup-ack");
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    while !ack.exists() && root.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not acknowledge cleanup request",
            ));
        }
        // test-timing-allow: acknowledgment or completed root removal is the signal; the deadline only bounds a failed handoff
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn wait_for_watchdog(watchdog: &mut Child) -> io::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if let Some(status) = watchdog.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            if let Ok(pid) = i32::try_from(watchdog.id()) {
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            let _ = watchdog.kill();
            let _ = watchdog.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not finish cleanup",
            ));
        }
        // test-timing-allow: watchdog exit is the completion signal; the deadline only bounds failed cleanup
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    use super::*;

    fn spawn_server(owner: &TestTmuxServerOwner, name: &str) -> PathBuf {
        let socket = owner.path().join(format!("{name}.sock"));
        let status = Command::new("tmux")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "new-session",
                "-d",
                "-s",
                "main",
                "sleep 300",
            ])
            .env_remove("TMUX")
            .status()
            .expect("launch disposable tmux test server");
        assert!(status.success());
        assert_eq!(probe_sync(&socket), ProbeResult::Live);
        socket
    }

    fn wait_until(mut condition: impl FnMut() -> bool, description: &str) {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            // test-timing-allow: cross-process marker/root state is the completion signal; the deadline only bounds a wedged fixture
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn normal_shutdown_kills_exact_server_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let socket = spawn_server(&owner, "normal");
        owner.shutdown();
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert!(!root.exists());
    }

    #[test]
    fn idle_owner_does_not_require_tmux() {
        let fake_bin = TempDir::new().unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        owner.shutdown();
        assert!(!root.exists());
    }

    #[test]
    fn socket_symlink_is_unlinked_without_invoking_tmux() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        let invoked = fake_bin.path().join("invoked");
        fs::write(
            &fake_tmux,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", invoked.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();

        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        std::os::unix::fs::symlink("/tmp/not-a-test-socket", root.join("escape.sock")).unwrap();
        owner.shutdown();
        assert!(!invoked.exists(), "watchdog must not pass symlinks to tmux");
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_failure_preserves_root_without_reentrant_drop() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        fs::write(&fake_tmux, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();

        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let socket = root.join("unconfirmed.sock");
        std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err(), "cleanup failure must fail the test");
        assert!(root.exists(), "failed cleanup must preserve its exact root");
        assert!(
            socket.exists(),
            "unconfirmed socket must remain recoverable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn panic_unwind_kills_exact_server_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let (root, socket) = std::panic::catch_unwind(|| {
            let owner = TestTmuxServerOwner::new();
            let root = owner.path().to_path_buf();
            let socket = spawn_server(&owner, "panic");
            std::panic::panic_any((root, socket));
        })
        .expect_err("fixture must panic")
        .downcast::<(PathBuf, PathBuf)>()
        .map(|paths| *paths)
        .expect("panic payload");
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn task_cancellation_kills_exact_server_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let owner = TestTmuxServerOwner::new();
            let root = owner.path().to_path_buf();
            let socket = spawn_server(&owner, "cancel");
            tx.send((root, socket)).unwrap();
            std::future::pending::<()>().await;
        });
        let (root, socket) = rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_waits_for_in_flight_late_server_creation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let socket = root.join("late.sock");
        let marker = root.join(".creating-late");
        let script = r#"
import os
from pathlib import Path
import subprocess
import sys
import time

marker = Path(sys.argv[1])
marker.write_text(str(os.getpid()))
time.sleep(0.25)
try:
    subprocess.run(
        ["tmux", "-S", sys.argv[2], "new-session", "-d", "-s", "main", "sleep 300"],
        check=True,
    )
finally:
    marker.unlink(missing_ok=True)
"#;
        let mut creator = Command::new("python3")
            .args(["-c", script])
            .arg(&marker)
            .arg(&socket)
            .spawn()
            .unwrap();
        wait_until(|| marker.exists(), "in-flight creator marker");
        owner.shutdown();
        assert!(creator.wait().unwrap().success());
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert!(!root.exists());
    }

    #[test]
    fn forced_test_runner_termination_kills_exact_server() {
        run_forced_termination_case(false);
    }

    #[test]
    fn forced_test_runner_process_group_termination_kills_exact_server() {
        run_forced_termination_case(true);
    }

    #[test]
    #[ignore = "subprocess fixture; parent test terminates it"]
    fn forced_termination_fixture() {
        let Some(marker) = std::env::var_os("PHOENIX_TMUX_TEST_DEATH_MARKER") else {
            return;
        };
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let socket = spawn_server(&owner, "forced-death");
        let marker = PathBuf::from(marker);
        let pending_marker = marker.with_extension("pending");
        let mut file = fs::File::create(&pending_marker).unwrap();
        writeln!(file, "{}", root.display()).unwrap();
        writeln!(file, "{}", socket.display()).unwrap();
        file.sync_all().unwrap();
        fs::rename(pending_marker, marker).unwrap();
        let mut parent_pipe = String::new();
        std::io::stdin().read_line(&mut parent_pipe).unwrap();
        drop(owner);
    }

    fn run_forced_termination_case(kill_group: bool) {
        if which::which("tmux").is_err() {
            return;
        }
        let marker_dir = TempDir::new().unwrap();
        let marker = marker_dir.path().join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tmux::test_server::tests::forced_termination_fixture",
                "--nocapture",
                "--ignored",
            ])
            .env("PHOENIX_TMUX_TEST_DEATH_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if kill_group {
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().unwrap();
        wait_until(|| marker.exists(), "forced-death fixture readiness");
        let paths = fs::read_to_string(&marker).unwrap();
        let mut paths = paths.lines();
        let root = PathBuf::from(paths.next().unwrap());
        let socket = PathBuf::from(paths.next().unwrap());
        assert_eq!(probe_sync(&socket), ProbeResult::Live);

        let child_pid = i32::try_from(child.id()).expect("child pid fits pid_t");
        let result = if kill_group {
            unsafe { libc::killpg(child_pid, libc::SIGKILL) }
        } else {
            unsafe { libc::kill(child_pid, libc::SIGKILL) }
        };
        assert_eq!(result, 0, "failed to kill disposable test runner");
        let status = child.wait().unwrap();
        assert_killed(status);
        wait_until(|| !root.exists(), "watchdog cleanup after forced death");
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
    }

    #[cfg(unix)]
    fn assert_killed(status: ExitStatus) {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
