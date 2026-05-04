//! Probe a tmux server's reachability via `tmux -S <sock> ls`.
//!
//! REQ-TMUX-005 (live server reused on operation), REQ-TMUX-006 (stale
//! socket detection / system-reboot recovery). The probe is the single
//! decision point for the three lifecycle branches handled in
//! `registry.rs::ensure_live`.

use std::path::Path;
use std::process::Stdio;

/// Result of probing an existing socket path. Phoenix issues
/// `tmux -S <sock> ls` and inspects the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// Socket file exists and `tmux ls` succeeded.
    Live,
    /// Socket file exists but `tmux ls` failed (typical post-system-
    /// reboot state where the socket lingers but the server process is
    /// gone).
    DeadSocket,
    /// Socket file does not exist on disk.
    NoSocket,
}

/// Probe a socket path. The function is best-effort: an I/O failure
/// while invoking tmux is propagated as `Err` so the caller can decide
/// whether to retry or surface the error. A non-zero exit from `tmux
/// ls` is mapped to `DeadSocket`, never `Err`, because the process
/// running fine but reporting "no server" is the typical stale-socket
/// signal.
pub async fn probe(socket_path: &Path) -> std::io::Result<ProbeResult> {
    if !socket_path.exists() {
        return Ok(ProbeResult::NoSocket);
    }
    let status = tokio::process::Command::new("tmux")
        .args(["-S", &socket_path.to_string_lossy(), "ls"])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if status.success() {
        Ok(ProbeResult::Live)
    } else {
        Ok(ProbeResult::DeadSocket)
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

    #[tokio::test]
    async fn probe_returns_dead_socket_for_orphan_file() {
        // A regular file existing at the socket path that is not a real
        // tmux socket: `tmux ls` should fail, and the probe surfaces
        // `DeadSocket`.
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("orphan.sock");
        std::fs::write(&path, b"not a real tmux socket").unwrap();
        assert_eq!(probe(&path).await.unwrap(), ProbeResult::DeadSocket);
    }
}
