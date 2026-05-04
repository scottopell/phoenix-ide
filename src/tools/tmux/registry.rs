//! Per-conversation tmux server registry.
//!
//! REQ-TMUX-001 (per-conversation socket isolation), REQ-TMUX-002 (lazy
//! spawn), REQ-TMUX-005 (Phoenix-restart probe re-use), REQ-TMUX-006
//! (stale-socket detection), REQ-TMUX-007 (hard-delete cascade),
//! REQ-TMUX-013 (`ToolContext` accessor shape).
//!
//! Lifetime: registries live in process memory only. The tmux servers
//! themselves are owned by the OS and survive Phoenix restart; the in-
//! memory `TmuxServer` entry is rebuilt on the first operation after
//! restart by probing the socket.
//!
//! Lock ordering for `ensure_live`: acquire the registry's
//! `RwLock<HashMap>` long enough to clone (or insert) the per-conversation
//! `Arc<RwLock<TmuxServer>>`, then drop the outer lock and take the
//! conversation's write lock for the probe + spawn sequence. The write
//! lock serialises concurrent `ensure_live` calls on the same
//! conversation; the second caller observes `Live` after the first one
//! finishes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::RwLock;

use super::probe::{probe, ProbeResult};

/// Default sub-directory under the Phoenix data dir for per-conversation
/// tmux sockets (REQ-TMUX-001 / `TMUX_SOCKET_DIR`).
const DEFAULT_SOCKET_SUBDIR: &str = "tmux-sockets";

/// Default session name created on lazy spawn (REQ-TMUX-002 /
/// `TMUX_DEFAULT_SESSION`).
pub const TMUX_DEFAULT_SESSION: &str = "main";

/// Filename for the Phoenix-shipped tmux server config, written into
/// the socket directory and passed via `tmux -f` on every invocation.
/// The leading underscore avoids collision with the `conv-<id>.sock`
/// socket-file naming pattern.
const SERVER_CONFIG_FILENAME: &str = "_phoenix.tmux.conf";

/// Embedded Phoenix tmux server config. Source-of-truth lives in
/// `src/tools/tmux/server.conf`; the file is written into the socket
/// directory at registry-init time (see [`TmuxRegistry::ensure_runtime_assets`]).
pub const SERVER_CONFIG_TEXT: &str = include_str!("server.conf");

/// Errors surfaced by the tmux registry. The tmux tool translates these
/// into the stable error envelope on the agent's response.
#[derive(Debug, Error)]
pub enum TmuxError {
    #[error("the tmux binary is not installed on this host")]
    BinaryUnavailable,

    #[error("failed to create tmux socket directory at {path}: {source}")]
    SocketDirCreate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to spawn tmux server at {socket_path}: {reason}")]
    SpawnFailed {
        socket_path: PathBuf,
        reason: String,
    },

    #[error("failed to probe tmux server at {socket_path}: {source}")]
    ProbeFailed {
        socket_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Lifecycle state of a per-conversation tmux server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Gone is wired up in task 02696's cascade orchestrator.
pub enum ServerStatus {
    /// Initial state — the entry exists but no operation has touched the
    /// server yet. Promoted to `Live` on the first successful
    /// `ensure_live` call.
    NotProbed,
    /// `tmux ls` succeeded against the socket; the server is reachable.
    Live,
    /// The conversation was hard-deleted; the entry is in the process of
    /// being torn down. Entries in this state are dropped from the
    /// registry by `cascade_tmux_on_delete`.
    Gone,
}

/// Per-conversation tmux server entity. One per conversation that has
/// ever performed a tmux operation; conversations that never use tmux
/// have no entry.
///
/// `socket_path` is computed once at entry creation and is stable for
/// the entry's lifetime (REQ-TMUX-001 / `SocketPathDeterministic`
/// invariant).
#[derive(Debug)]
pub struct TmuxServer {
    /// The conversation this server belongs to. Read by the cascade
    /// orchestrator (task 02696) and by diagnostic surfaces.
    #[allow(dead_code)]
    pub conversation_id: String,
    pub socket_path: PathBuf,
    pub status: ServerStatus,
}

impl TmuxServer {
    fn new(conversation_id: &str, socket_dir: &Path) -> Self {
        Self {
            conversation_id: conversation_id.to_string(),
            socket_path: socket_path_for(socket_dir, conversation_id),
            status: ServerStatus::NotProbed,
        }
    }
}

/// Compute the deterministic socket path for a conversation
/// (REQ-TMUX-001).
pub fn socket_path_for(socket_dir: &Path, conversation_id: &str) -> PathBuf {
    socket_dir.join(format!("conv-{conversation_id}.sock"))
}

/// Top-level registry: maps `conversation_id` -> per-conversation tmux
/// server. One registry instance per Phoenix process.
#[derive(Debug)]
pub struct TmuxRegistry {
    inner: RwLock<HashMap<String, Arc<RwLock<TmuxServer>>>>,
    socket_dir: PathBuf,
    binary_available: bool,
}

impl TmuxRegistry {
    /// Construct a registry with the default socket directory rooted at
    /// `~/.phoenix-ide/tmux-sockets/` (or `$PHOENIX_DATA_DIR` if set).
    /// `which::which("tmux")` is called once here and cached for the
    /// process lifetime (REQ-TMUX-003 design / "Binary Availability
    /// Detection").
    pub fn new() -> Self {
        Self::with_socket_dir(default_socket_dir())
    }

    /// Construct a registry with a caller-supplied socket directory.
    /// Used by tests and integration scenarios that need an isolated
    /// `tempfile::TempDir`.
    pub fn with_socket_dir(socket_dir: PathBuf) -> Self {
        let binary_available = which::which("tmux").is_ok();
        Self {
            inner: RwLock::new(HashMap::new()),
            socket_dir,
            binary_available,
        }
    }

    /// Test-only constructor that lets the caller force
    /// `binary_available` to a chosen value, regardless of whether tmux
    /// is on PATH. Used to exercise the "tmux binary missing" branches
    /// of the tool dispatch and the terminal attach fallback without
    /// requiring a host without tmux.
    #[cfg(test)]
    pub fn with_socket_dir_and_binary(socket_dir: PathBuf, binary_available: bool) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            socket_dir,
            binary_available,
        }
    }

    /// Cached `which("tmux")` result (REQ-TMUX-003). Discovered once at
    /// registry init and not re-checked.
    pub fn binary_available(&self) -> bool {
        self.binary_available
    }

    /// Configured socket directory for this registry. Used by the
    /// cascade orchestrator (task 02696) to find sockets for entries
    /// already evicted from memory.
    #[allow(dead_code)]
    pub fn socket_dir(&self) -> &Path {
        &self.socket_dir
    }

    /// Path to the Phoenix-shipped tmux server config file. Always
    /// passed via `tmux -f <path>` so the per-conversation tmux servers
    /// run in a deterministic config independent of the user's own
    /// `~/.tmux.conf` / `~/.config/tmux/tmux.conf`.
    pub fn config_path(&self) -> PathBuf {
        self.socket_dir.join(SERVER_CONFIG_FILENAME)
    }

    /// Idempotent mkdir of the socket directory (perms 0700) AND write
    /// of the Phoenix server config file. Called lazily on first use.
    /// Re-writing the config file each time is safe and ensures bumps
    /// to [`SERVER_CONFIG_TEXT`] propagate without manual intervention.
    fn ensure_runtime_assets(&self) -> Result<(), TmuxError> {
        if !self.socket_dir.exists() {
            std::fs::create_dir_all(&self.socket_dir).map_err(|source| {
                TmuxError::SocketDirCreate {
                    path: self.socket_dir.clone(),
                    source,
                }
            })?;
            // Lock the directory down to the current user only. The socket
            // path is a security boundary — anyone who can read it can
            // attach to every conversation's tmux server.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o700);
                std::fs::set_permissions(&self.socket_dir, perms).map_err(|source| {
                    TmuxError::SocketDirCreate {
                        path: self.socket_dir.clone(),
                        source,
                    }
                })?;
            }
        }

        // Write the Phoenix-shipped config file. Overwrite each time so
        // a bump to SERVER_CONFIG_TEXT lands without operator action.
        let config_path = self.config_path();
        std::fs::write(&config_path, SERVER_CONFIG_TEXT).map_err(|source| {
            TmuxError::SocketDirCreate {
                path: config_path,
                source,
            }
        })?;

        Ok(())
    }

    /// Get-or-create the per-conversation `Arc<RwLock<TmuxServer>>` and
    /// drive the probe-and-act sequence (REQ-TMUX-002 / REQ-TMUX-005 /
    /// REQ-TMUX-006).
    ///
    /// On `Live`: no spawn, status=Live.
    /// On `NoSocket`: spawn `main` session, status=Live.
    /// On `DeadSocket`: unlink stale file, spawn `main` session,
    /// status=Live.
    ///
    /// Concurrent calls on the same conversation race for the per-
    /// conversation write lock; the loser observes the freshly-spawned
    /// server as `Live` and skips the spawn.
    pub async fn ensure_live(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
        if !self.binary_available {
            return Err(TmuxError::BinaryUnavailable);
        }
        self.ensure_runtime_assets()?;

        let server_arc = self.get_or_insert(conversation_id).await;

        let socket_path = server_arc.read().await.socket_path.clone();

        let probe_result = probe(&socket_path)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: socket_path.clone(),
                source,
            })?;

        let mut server = server_arc.write().await;
        // Re-probe under the write lock to absorb a concurrent peer that
        // spawned the server while we were waiting on the lock. The
        // outer probe is best-effort; the authoritative check is here.
        let probe_result = if matches!(probe_result, ProbeResult::Live) {
            ProbeResult::Live
        } else {
            probe(&server.socket_path)
                .await
                .map_err(|source| TmuxError::ProbeFailed {
                    socket_path: server.socket_path.clone(),
                    source,
                })?
        };

        match probe_result {
            ProbeResult::Live => {
                server.status = ServerStatus::Live;
            }
            ProbeResult::NoSocket => {
                spawn_session(&server.socket_path, &self.config_path()).await?;
                server.status = ServerStatus::Live;
            }
            ProbeResult::DeadSocket => {
                // Post-system-reboot: file present, server gone. Unlink
                // and recreate. No breadcrumb (see design.md §"No Stale-
                // Recovery Breadcrumb").
                tracing::debug!(
                    socket = %server.socket_path.display(),
                    "tmux: stale socket detected, unlinking and respawning"
                );
                let _ = tokio::fs::remove_file(&server.socket_path).await;
                spawn_session(&server.socket_path, &self.config_path()).await?;
                server.status = ServerStatus::Live;
            }
        }
        drop(server);
        Ok(server_arc)
    }

    /// Get-or-create the entry without driving probe/spawn. Internal
    /// helper for `ensure_live`; not exposed because callers should
    /// always go through the probe-and-act sequence.
    async fn get_or_insert(&self, conversation_id: &str) -> Arc<RwLock<TmuxServer>> {
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(conversation_id) {
                return entry.clone();
            }
        }
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get(conversation_id) {
            return entry.clone();
        }
        let entry = Arc::new(RwLock::new(TmuxServer::new(
            conversation_id,
            &self.socket_dir,
        )));
        map.insert(conversation_id.to_string(), entry.clone());
        entry
    }

    /// Best-effort tear-down of a conversation's tmux server, called
    /// from the bedrock hard-delete cascade (task 02696).
    ///
    /// Postcondition: registry has no entry for `conversation_id`,
    /// socket file is gone, and the tmux server process is gone.
    /// Failures of `kill-server` (server already dead) and `remove_file`
    /// (file already gone) are non-fatal; the cascade orchestrator
    /// surfaces them via the structured error.
    ///
    /// REQ-TMUX-007.
    #[allow(dead_code)] // Wired up in task 02696 (bedrock hard-delete cascade).
    pub async fn cascade_on_delete(&self, conversation_id: &str) -> CascadeReport {
        let entry = {
            let mut map = self.inner.write().await;
            map.remove(conversation_id)
        };

        let socket_path = if let Some(arc) = entry {
            let server = arc.read().await;
            server.socket_path.clone()
        } else {
            // No registry entry — fall back to the deterministic path so
            // we still attempt cleanup of any orphaned socket from a
            // prior process.
            socket_path_for(&self.socket_dir, conversation_id)
        };

        let mut report = CascadeReport {
            socket_path: socket_path.clone(),
            kill_server_error: None,
            unlink_error: None,
        };

        if self.binary_available {
            // `kill-server` connects to an existing server (which already
            // has its config loaded), so `-f` is functionally a no-op
            // here — included for symmetry with other Phoenix tmux
            // invocations and to harden against an unlikely auto-spawn
            // path on some tmux versions.
            let kill = tokio::process::Command::new("tmux")
                .args([
                    "-f",
                    &self.config_path().to_string_lossy(),
                    "-S",
                    &socket_path.to_string_lossy(),
                    "kill-server",
                ])
                .env_remove("TMUX")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            if let Err(e) = kill {
                report.kill_server_error = Some(e.to_string());
            }
        }

        match tokio::fs::remove_file(&socket_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.unlink_error = Some(e.to_string());
            }
        }

        report
    }

    /// Number of conversations currently tracked. Test/diagnostic only.
    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn conversation_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for TmuxRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Best-effort cascade outcome (REQ-TMUX-007). Both error fields are
/// surfaced to the caller (the hard-delete orchestrator in task 02696)
/// so partial failures can be logged. Neither field is fatal.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // Consumed by task 02696's cascade orchestrator.
pub struct CascadeReport {
    pub socket_path: PathBuf,
    pub kill_server_error: Option<String>,
    pub unlink_error: Option<String>,
}

/// Convenience function for the bedrock cascade orchestrator (task
/// 02696). Equivalent to `registry.cascade_on_delete(conv_id).await` —
/// kept as a free function for symmetry with the bash registry's
/// `remove_conversation` API.
#[allow(dead_code)] // Wired up in task 02696.
pub async fn cascade_tmux_on_delete(
    registry: &Arc<TmuxRegistry>,
    conversation_id: &str,
) -> CascadeReport {
    registry.cascade_on_delete(conversation_id).await
}

/// Spawn a fresh detached tmux session named `main` against
/// `socket_path` (REQ-TMUX-002 / `tmux_default_session`). This is the
/// only place `new-session -d` is issued, and therefore the only place
/// where `-f <config_path>` actually loads the Phoenix-shipped config —
/// subsequent invocations against the same socket connect to the
/// already-running server and inherit its loaded config.
pub async fn spawn_session(socket_path: &Path, config_path: &Path) -> Result<(), TmuxError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "-f",
            &config_path.to_string_lossy(),
            "-S",
            &socket_path.to_string_lossy(),
            "new-session",
            "-d",
            "-s",
            TMUX_DEFAULT_SESSION,
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!("failed to invoke tmux: {e}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!(
                "tmux new-session exited with {:?}: {}",
                output.status.code(),
                stderr.trim()
            ),
        });
    }
    Ok(())
}

/// Default socket directory: `$PHOENIX_DATA_DIR/tmux-sockets/` if set,
/// else `$HOME/.phoenix-ide/tmux-sockets/`, else
/// `/tmp/phoenix-ide/tmux-sockets/` as a last resort.
fn default_socket_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PHOENIX_DATA_DIR") {
        return PathBuf::from(dir).join(DEFAULT_SOCKET_SUBDIR);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".phoenix-ide")
            .join(DEFAULT_SOCKET_SUBDIR);
    }
    PathBuf::from("/tmp/phoenix-ide").join(DEFAULT_SOCKET_SUBDIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn socket_path_is_deterministic() {
        let dir = PathBuf::from("/x/y");
        let p = socket_path_for(&dir, "abc-123");
        assert_eq!(p, PathBuf::from("/x/y/conv-abc-123.sock"));
    }

    #[test]
    fn socket_path_is_stable_across_calls() {
        let dir = PathBuf::from("/x/y");
        let a = socket_path_for(&dir, "z");
        let b = socket_path_for(&dir, "z");
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn binary_unavailable_short_circuits_ensure_live() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        assert!(matches!(
            reg.ensure_live("conv-x").await,
            Err(TmuxError::BinaryUnavailable)
        ));
    }

    #[tokio::test]
    async fn ensure_runtime_assets_sets_0700_perms_and_writes_config_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("nested").join("tmux-sockets");
        let reg = TmuxRegistry::with_socket_dir_and_binary(dir.clone(), false);
        reg.ensure_runtime_assets().expect("mkdir + config write");
        let meta = std::fs::metadata(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
        let _ = meta;

        // Phoenix server config is materialized in the socket dir.
        let config_path = reg.config_path();
        assert!(config_path.exists(), "config file should exist");
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(written, SERVER_CONFIG_TEXT);
    }

    #[test]
    fn config_path_is_in_socket_dir() {
        let reg = TmuxRegistry::with_socket_dir_and_binary("/tmp/x".into(), false);
        assert_eq!(
            reg.config_path(),
            std::path::PathBuf::from("/tmp/x/_phoenix.tmux.conf")
        );
    }

    #[tokio::test]
    async fn cascade_on_delete_no_entry_attempts_socket_unlink() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        // No prior entry, no on-disk socket — cascade should be a no-op
        // that returns without errors.
        let report = reg.cascade_on_delete("never-existed").await;
        assert!(report.kill_server_error.is_none());
        assert!(report.unlink_error.is_none());
    }
}
