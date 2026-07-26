use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::probe::{probe_sync, ProbeResult};
use super::registry::TmuxRegistry;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);

/// Owns real tmux servers created by tests, including after abrupt runner death.
pub struct TestTmuxServerOwner {
    root: Option<TempDir>,
    watchdog: Option<Child>,
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
    /// Panics when the temporary root, watchdog pipe, or watchdog process cannot
    /// be created. Tests cannot safely continue without containment.
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

        let script = r#"
root=$1
parent=$2
[ "$(ps -o ppid= -p $$ | tr -d ' ')" = "$parent" ] || exit 1
command -v tmux >/dev/null 2>&1 || exit 1
while [ ! -f "$root/.cleanup-request" ] && [ "$(ps -o ppid= -p $$ | tr -d ' ')" = "$parent" ]; do
  sleep 0.05
done
: > "$root/.cleanup-ack"
attempt=0
quiet=0
while [ "$attempt" -lt 50 ]; do
  unconfirmed=0
  for socket in "$root"/*.sock; do
    [ -S "$socket" ] || continue
    if tmux -S "$socket" list-sessions >/dev/null 2>&1; then
      if tmux -S "$socket" kill-server >/dev/null 2>&1; then
        rm -f "$socket"
      else
        unconfirmed=1
      fi
    else
      unconfirmed=1
    fi
  done
  sockets=0
  for socket in "$root"/*.sock; do
    [ -S "$socket" ] && sockets=1
  done
  if [ "$unconfirmed" -eq 0 ] && [ "$sockets" -eq 0 ]; then
    quiet=$((quiet + 1))
  else
    quiet=0
  fi
  if [ "$quiet" -ge 5 ]; then
    rm -rf "$root"
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
exit 1
"#;
        let parent_pid = std::process::id().to_string();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "phoenix-tmux-test-watchdog"])
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
        let watchdog = command.spawn().expect("spawn tmux test watchdog");

        Self {
            root: Some(root),
            watchdog: Some(watchdog),
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
        TmuxRegistry::with_socket_dir(self.socket_dir().to_path_buf())
    }

    #[cfg(test)]
    pub(crate) fn registry_with_sink(
        &self,
        sink: Option<super::registry::TmuxLifecycleSink>,
    ) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir_binary_and_sink(self.socket_dir().to_path_buf(), true, sink)
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

fn request_cleanup(root: &Path, graceful: bool) -> io::Result<()> {
    let request = root.join(".cleanup-request");
    let pending = root.join(".cleanup-request.pending");
    let reason: &[u8] = if graceful { b"graceful" } else { b"drop" };
    fs::write(&pending, reason)?;
    fs::rename(pending, request)?;
    let ack = root.join(".cleanup-ack");
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    while !ack.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not acknowledge cleanup request",
            ));
        }
        // test-timing-allow: the watchdog acknowledgment file is the completion signal; the deadline only bounds a failed handoff
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
        let panic = std::panic::catch_unwind(|| owner.shutdown());
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
