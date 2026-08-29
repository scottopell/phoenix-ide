//! Probe a tmux server's reachability via `tmux -S <sock> ls`.
//!
//! REQ-TMUX-005 (live server reused on operation), REQ-TMUX-006 (stale
//! socket detection / system-reboot recovery). The probe is the single
//! decision point for the three lifecycle branches handled in
//! `registry.rs::ensure_live`.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::Stdio;

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
    probe_with_binary(socket_path, Path::new("tmux")).await
}

pub(crate) async fn probe_with_binary(
    socket_path: &Path,
    binary: &Path,
) -> std::io::Result<ProbeResult> {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Ok(ProbeResult::DeadSocket);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProbeResult::NoSocket);
        }
        Err(error) => return Err(error),
    }
    let output = tokio::process::Command::new(binary)
        .args(["-S", &socket_path.to_string_lossy(), "ls"])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;
    Ok(classify_output(
        output.status.success(),
        &output.stderr,
        socket_path,
    ))
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

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn probe_sync(socket_path: &Path) -> ProbeResult {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return ProbeResult::DeadSocket;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProbeResult::NoSocket;
        }
        Err(_) => return ProbeResult::DeadSocket,
    }
    let output = std::process::Command::new("tmux")
        .args(["-S", &socket_path.to_string_lossy(), "ls"])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(output) => classify_output(output.status.success(), &output.stderr, socket_path),
        Err(_) => ProbeResult::DeadSocket,
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

    #[tokio::test]
    async fn probe_returns_ambiguous_for_orphan_file() {
        // A regular file existing at the socket path is not proof that the
        // token-bound endpoint is absent.
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("orphan.sock");
        std::fs::write(&path, b"not a real tmux socket").unwrap();
        assert_eq!(probe(&path).await.unwrap(), ProbeResult::DeadSocket);
    }

    #[tokio::test]
    async fn probe_returns_no_server_for_orphan_socket() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("orphan.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);
        assert_eq!(probe(&path).await.unwrap(), ProbeResult::NoServer);
    }
}
