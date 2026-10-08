//! Probe a tmux server's reachability via `tmux -S <sock> ls`.
//!
//! REQ-TMUX-005 (live server reused on operation), REQ-TMUX-006 (stale
//! socket detection / system-reboot recovery). The probe is the single
//! decision point for the three lifecycle branches handled in
//! `registry.rs::ensure_live`.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::{Output, Stdio};

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::Instant;

/// Result of probing an existing socket path. Phoenix issues
/// `tmux -S <sock> ls` and inspects the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// Socket file exists and `tmux ls` succeeded.
    Live,
    /// Socket file exists and tmux explicitly reported that no server is running.
    NoServer,
    /// Socket file exists but `tmux ls` failed without proving server absence.
    DeadSocket,
    /// Socket file does not exist on disk.
    NoSocket,
}

/// Probe a socket path. The function is best-effort: an I/O failure
/// while invoking tmux is propagated as `Err` so the caller can decide
/// whether to retry or surface the error. A non-zero exit is classified
/// as [`ProbeResult::NoServer`] only when tmux explicitly reports that
/// no server is running; every other non-zero result remains ambiguous as
/// [`ProbeResult::DeadSocket`].
///
/// # Errors
/// Returns an [`std::io::Error`] when invoking the `tmux` process itself
/// fails (spawn/IO error). A non-zero exit from `tmux ls` returns a typed
/// non-live probe result, not an I/O error.
pub async fn probe(socket_path: &Path) -> std::io::Result<ProbeResult> {
    probe_inner(socket_path, None)
        .await?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "tmux probe timed out"))
}

pub(crate) async fn probe_until(
    socket_path: &Path,
    expires: Instant,
) -> std::io::Result<Option<ProbeResult>> {
    probe_inner(socket_path, Some(expires)).await
}

async fn probe_inner(
    socket_path: &Path,
    expires: Option<Instant>,
) -> std::io::Result<Option<ProbeResult>> {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Ok(Some(ProbeResult::DeadSocket));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(ProbeResult::NoSocket));
        }
        Err(error) => return Err(error),
    }
    let mut command = Command::new("tmux");
    command
        .args(["-S", &socket_path.to_string_lossy(), "ls"])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let Some(output) = command_output(command, expires).await? else {
        return Ok(None);
    };
    Ok(Some(classify_output(
        output.status.success(),
        &output.stderr,
        socket_path,
    )))
}

pub(crate) async fn command_output(
    command: Command,
    expires: Option<Instant>,
) -> std::io::Result<Option<Output>> {
    let (mut tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = run_command_output(command, expires, tx.closed()).await;
        let _ = tx.send(result);
    });
    rx.await.map_err(std::io::Error::other)?
}

async fn run_command_output(
    mut command: Command,
    expires: Option<Instant>,
    cancelled: impl std::future::Future<Output = ()>,
) -> std::io::Result<Option<Output>> {
    command.kill_on_drop(true);
    let child = command.spawn()?;
    child_output(child, expires, cancelled).await
}

async fn child_output(
    mut child: tokio::process::Child,
    expires: Option<Instant>,
    cancelled: impl std::future::Future<Output = ()>,
) -> std::io::Result<Option<Output>> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut stdout_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(mut stdout) = stdout {
            stdout.read_to_end(&mut bytes).await?;
        }
        Ok::<_, std::io::Error>(bytes)
    });
    let mut stderr_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_end(&mut bytes).await?;
        }
        Ok::<_, std::io::Error>(bytes)
    });

    tokio::pin!(cancelled);
    let status = if let Some(expires) = expires {
        tokio::select! {
            status = child.wait() => Some(status?),
            () = tokio::time::sleep_until(expires) => None,
            () = &mut cancelled => None,
        }
    } else {
        tokio::select! {
            status = child.wait() => Some(status?),
            () = &mut cancelled => None,
        }
    };
    let Some(status) = status else {
        let _ = child.start_kill();
        child.wait().await?;
        stdout_reader.abort();
        stderr_reader.abort();
        let _ = stdout_reader.await;
        let _ = stderr_reader.await;
        return Ok(None);
    };
    let output = {
        let readers = async {
            let stdout = (&mut stdout_reader)
                .await
                .map_err(std::io::Error::other)??;
            let stderr = (&mut stderr_reader)
                .await
                .map_err(std::io::Error::other)??;
            Ok::<_, std::io::Error>((stdout, stderr))
        };
        tokio::pin!(readers);
        if let Some(expires) = expires {
            tokio::select! {
                output = &mut readers => Some(output?),
                () = tokio::time::sleep_until(expires) => None,
                () = &mut cancelled => None,
            }
        } else {
            tokio::select! {
                output = &mut readers => Some(output?),
                () = &mut cancelled => None,
            }
        }
    };
    let Some((stdout, stderr)) = output else {
        stdout_reader.abort();
        stderr_reader.abort();
        let _ = stdout_reader.await;
        let _ = stderr_reader.await;
        return Ok(None);
    };
    Ok(Some(Output {
        status,
        stdout,
        stderr,
    }))
}

fn classify_output(success: bool, stderr: &[u8], socket_path: &Path) -> ProbeResult {
    if success {
        return ProbeResult::Live;
    }

    let mut expected = b"no server running on ".to_vec();
    expected.extend_from_slice(socket_path.as_os_str().as_bytes());
    expected.push(b'\n');
    if stderr == expected || stderr == &expected[..expected.len() - 1] {
        ProbeResult::NoServer
    } else {
        ProbeResult::DeadSocket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn probe_returns_no_socket_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.sock");
        assert_eq!(probe(&path).await.unwrap(), ProbeResult::NoSocket);
    }

    #[test]
    fn nonzero_without_exact_absence_for_probed_path_is_ambiguous() {
        let path = Path::new("/tmp/example.sock");
        assert_eq!(
            classify_output(false, b"permission denied", path),
            ProbeResult::DeadSocket
        );
        assert_eq!(
            classify_output(false, b"no server running on /tmp/other.sock\n", path),
            ProbeResult::DeadSocket
        );
    }

    #[test]
    fn exact_no_server_response_for_probed_path_proves_absence() {
        let path = Path::new("/tmp/example.sock");
        assert_eq!(
            classify_output(false, b"no server running on /tmp/example.sock\n", path),
            ProbeResult::NoServer
        );
    }

    #[test]
    fn successful_listing_proves_a_live_server() {
        assert_eq!(
            classify_output(true, b"", Path::new("/tmp/example.sock")),
            ProbeResult::Live
        );
    }

    /// An expired deadline is the only ready branch, so the outcome is causal:
    /// the child must already be killed and reaped when `child_output` returns.
    #[tokio::test]
    async fn expired_deadline_kills_and_reaps_the_child() {
        let mut command = Command::new("sleep");
        command
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = command.spawn().unwrap();
        let identity =
            phoenix_core::process_identity::current_process_identity(child.id().unwrap())
                .expect("capture the child's exact identity while it runs");

        let result = child_output(child, Some(Instant::now()), std::future::pending())
            .await
            .unwrap();

        assert_eq!(result, None);
        assert!(
            !phoenix_core::process_identity::process_identity_matches(identity),
            "timed-out child {identity:?} survived"
        );
    }

    #[tokio::test]
    async fn probe_returns_ambiguous_for_orphan_file() {
        // A regular file at the socket path is not proof that the token-bound
        // endpoint is absent. The metadata check answers before any tmux call.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("orphan.sock");
        std::fs::write(&path, b"not a real tmux socket").unwrap();
        assert_eq!(probe(&path).await.unwrap(), ProbeResult::DeadSocket);
    }
}
