use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
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

pub(crate) struct TestTmuxServerOwner {
    root: Option<TempDir>,
    keepalive: Option<OwnedFd>,
    watchdog: Option<Child>,
}

impl TestTmuxServerOwner {
    pub(crate) fn new() -> Self {
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

        let (read_fd, write_fd) = cloexec_pipe().expect("create tmux watchdog pipe");
        let script = r#"
root=$1
while IFS= read -r _; do :; done
mode=abrupt
[ -f "$root/.graceful-shutdown" ] && mode=graceful
status=0
attempt=0
while :; do
  status=0
  for socket in "$root"/*.sock; do
    [ -S "$socket" ] || continue
    tmux -S "$socket" kill-server >/dev/null 2>&1 || true
    if tmux -S "$socket" list-sessions >/dev/null 2>&1; then
      status=1
    fi
  done
  [ "$mode" = graceful ] && break
  attempt=$((attempt + 1))
  [ "$attempt" -ge 50 ] && break
  sleep 0.1
done
if [ "$status" -eq 0 ]; then
  for socket in "$root"/*.sock; do
    [ -S "$socket" ] || continue
    if tmux -S "$socket" list-sessions >/dev/null 2>&1; then
      status=1
      break
    fi
  done
fi
[ "$status" -eq 0 ] && rm -rf "$root"
exit "$status"
"#;
        let read_file = unsafe { std::fs::File::from_raw_fd(read_fd.into_raw_fd()) };
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "phoenix-tmux-test-watchdog"])
            .arg(&canonical_root)
            .stdin(Stdio::from(read_file))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
            keepalive: Some(write_fd),
            watchdog: Some(watchdog),
        }
    }

    pub(crate) fn socket_dir(&self) -> &Path {
        self.root.as_ref().expect("owner is live").path()
    }

    pub(crate) fn path(&self) -> &Path {
        self.socket_dir()
    }

    pub(crate) fn registry(&self) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir(self.socket_dir().to_path_buf())
    }

    pub(crate) fn registry_with_sink(
        &self,
        sink: Option<super::registry::TmuxLifecycleSink>,
    ) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir_binary_and_sink(self.socket_dir().to_path_buf(), true, sink)
    }

    pub(crate) fn shutdown(mut self) {
        self.shutdown_inner()
            .expect("tmux test cleanup must succeed");
    }

    fn shutdown_inner(&mut self) -> io::Result<()> {
        let result = self.try_shutdown();
        if result.is_err() {
            self.preserve_root();
        }
        result
    }

    fn try_shutdown(&mut self) -> io::Result<()> {
        fs::write(self.socket_dir().join(".graceful-shutdown"), [])?;
        self.keepalive.take();
        let mut watchdog = self.watchdog.take().expect("watchdog is live");
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        let status = loop {
            if let Some(status) = watchdog.try_wait()? {
                break status;
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
        };
        if !status.success() {
            return Err(io::Error::other(format!(
                "tmux test watchdog reported cleanup failure: {status}"
            )));
        }
        if self.socket_dir().exists() {
            verify_no_live_servers(self.socket_dir())?;
        }
        self.root.take();
        Ok(())
    }

    fn preserve_root(&mut self) {
        if let Some(root) = self.root.take() {
            let _ = root.keep();
        }
    }
}

impl Drop for TestTmuxServerOwner {
    fn drop(&mut self) {
        if self.watchdog.is_none() {
            return;
        }
        if let Err(error) = self.shutdown_inner() {
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

fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    for fd in [&read_fd, &write_fd] {
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read_fd, write_fd))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
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
