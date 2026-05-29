//! Per-`WorkScope` tmux server registry.
//!
//! REQ-TMUX-001 (socket isolation), REQ-TMUX-002 (lazy spawn),
//! REQ-TMUX-005 (Phoenix-restart probe re-use), REQ-TMUX-006
//! (stale-socket detection), REQ-TMUX-007 (hard-delete cascade),
//! REQ-TMUX-013 (`ToolContext` accessor shape),
//! REQ-TMUX-WS-001 (`WorkScope` ownership).
//!
//! Lifetime: registries live in process memory only. The tmux servers
//! themselves are owned by the OS and survive Phoenix restart; the in-
//! memory `TmuxServer` entry is rebuilt on the first operation after
//! restart by probing the socket.
//!
//! Registry + socket keying (task 03001, REQ-TMUX-WS-001): both the
//! `HashMap` entry and the socket path are keyed by `WorkScope` —
//! `WorkScope::Worktree(path)` for Work/Branch/Explore conversations and
//! `WorkScope::Conversation(id)` for Direct-mode conversations.
//! Continuations resolving to the same scope share the entry and the
//! socket, so session continuity across context-exhaustion continuations
//! is correct by construction — the worktree is the logical coding
//! environment, and the tmux session IS that environment's shell state.
//! The map key itself is `WorkScope::stable_key()`, which gives
//! `worktree:` and `conversation:` disjoint namespaces.
//!
//! Lock ordering for `ensure_live`: acquire the registry's
//! `RwLock<HashMap>` long enough to clone (or insert) the per-scope
//! `Arc<RwLock<TmuxServer>>`, then drop the outer lock and take that
//! entry's write lock for the probe + spawn sequence. The write lock
//! serialises concurrent `ensure_live` calls on the same `WorkScope`;
//! the second caller observes `Live` after the first one finishes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::work_scope::WorkScope;

use thiserror::Error;
use tokio::sync::{OnceCell, RwLock};

use super::probe::{probe, ProbeResult};

/// Default sub-directory under the Phoenix data dir for per-conversation
/// tmux sockets (REQ-TMUX-001 / `TMUX_SOCKET_DIR`).
const DEFAULT_SOCKET_SUBDIR: &str = "tmux-sockets";

/// Default session name created on lazy spawn (REQ-TMUX-002 /
/// `TMUX_DEFAULT_SESSION`).
pub const TMUX_DEFAULT_SESSION: &str = "main";

// Bound on the post-spawn pane-readiness poll: 50 * 100ms = 5s ceiling.
// Conservative — under normal load the pane is ready on the first probe.
const PANE_READY_MAX_ATTEMPTS: u32 = 50;
const PANE_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

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

/// Lifecycle state of a per-`WorkScope` tmux server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Gone` is the terminal-transition target; cascade
                    // drops entries rather than setting status=Gone today.
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

/// Per-`WorkScope` tmux server entity. One per scope that has ever
/// performed a tmux operation; scopes that never use tmux have no entry.
///
/// `socket_path` is computed once at entry creation and is stable for
/// the entry's lifetime (REQ-TMUX-001 / `SocketPathDeterministic`
/// invariant). For `WorkScope::Worktree(path)` the path is keyed to the
/// worktree; for `WorkScope::Conversation(id)` it falls back to the
/// conversation id (task 03001 / REQ-TMUX-WS-001).
#[derive(Debug)]
pub struct TmuxServer {
    /// The scope this server belongs to. Diagnostic field — the
    /// cleanup cascade derives the lookup key from its own `WorkScope`
    /// arg, not from this field. Replaces the prior `conversation_id:
    /// String` field; for `Worktree`-scoped servers, "one conversation"
    /// was misleading because the chain of continuation members all
    /// share the entry.
    #[allow(dead_code)]
    pub work_scope: WorkScope,
    pub socket_path: PathBuf,
    pub status: ServerStatus,
}

impl TmuxServer {
    fn new(work_scope: WorkScope, socket_path: PathBuf) -> Self {
        Self {
            work_scope,
            socket_path,
            status: ServerStatus::NotProbed,
        }
    }
}

/// Compute the deterministic socket path for a worktree-scoped session
/// (Work/Branch/Explore modes). The worktree path is hashed with SHA-256
/// (first 8 bytes → 16 hex chars) so the socket filename is filesystem-safe,
/// bounded in length, **and stable across Rust/Phoenix releases** —
/// `std::collections::hash_map::DefaultHasher` is explicitly not a persistent
/// hash and would re-key every existing tmux session on toolchain upgrade
/// (task 03001).
pub fn socket_path_for_worktree(socket_dir: &Path, worktree_path: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(worktree_path.as_os_str().as_encoded_bytes());
    let digest = h.finalize();
    let prefix = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 digest is 32 bytes; first 8 always fits a u64"),
    );
    socket_dir.join(format!("wt-{prefix:016x}.sock"))
}

/// Compute the deterministic socket path for a Direct-mode conversation
/// (no worktree). Retained for fallback and for Direct-mode conversations
/// (REQ-TMUX-001).
pub fn socket_path_for(socket_dir: &Path, conversation_id: &str) -> PathBuf {
    socket_dir.join(format!("conv-{conversation_id}.sock"))
}

/// Compute the deterministic socket path for the singleton Global scope
/// (REQ-TERM-WS-001). Only one global tmux server can exist per Phoenix
/// process; the filename is a constant so the same server is reused
/// across process restarts that find the socket already present.
pub fn socket_path_for_global(socket_dir: &Path) -> PathBuf {
    socket_dir.join("global.sock")
}

/// Top-level registry: maps `WorkScope::stable_key()` → per-scope tmux
/// server. One registry instance per Phoenix process.
#[derive(Debug)]
pub struct TmuxRegistry {
    /// Keyed by `WorkScope::stable_key()` so Worktree-scoped continuation
    /// members share an entry, and Worktree vs Conversation namespaces
    /// stay disjoint.
    inner: RwLock<HashMap<String, Arc<RwLock<TmuxServer>>>>,
    socket_dir: PathBuf,
    binary_available: bool,
    /// Bootstrap of the socket dir + 0700 perms + Phoenix server config
    /// file. Runs at most once per process — config-text bumps require a
    /// Phoenix restart anyway (existing tmux servers don't reload `-f`).
    /// `OnceCell::get_or_try_init` retries on failure so a transient
    /// disk error doesn't permanently brick the registry.
    runtime_assets: OnceCell<()>,
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
            runtime_assets: OnceCell::new(),
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
            runtime_assets: OnceCell::new(),
        }
    }

    /// Cached `which("tmux")` result (REQ-TMUX-003). Discovered once at
    /// registry init and not re-checked.
    pub fn binary_available(&self) -> bool {
        self.binary_available
    }

    /// Configured socket directory for this registry. Available to the
    /// cleanup cascade for orphan-socket fallback paths and to
    /// diagnostic surfaces.
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

    /// One-shot bootstrap of the socket dir (perms 0700) and the
    /// Phoenix server config file. Gated by [`Self::runtime_assets`]
    /// so it runs at most once per process — config-text bumps require
    /// a Phoenix restart to take effect anyway, since running tmux
    /// servers don't re-read `-f` on subsequent invocations.
    ///
    /// Always enforces 0700 on Unix even if the directory pre-existed
    /// (a pre-existing dir with broader perms would otherwise leak the
    /// socket-path security boundary). On a chmod failure, logs at WARN
    /// and continues — degraded security beats refusing to start.
    ///
    /// Uses `tokio::fs` so the bootstrap doesn't block the runtime.
    async fn ensure_runtime_assets(&self) -> Result<(), TmuxError> {
        self.runtime_assets
            .get_or_try_init(|| async { self.bootstrap_runtime_assets().await })
            .await?;
        Ok(())
    }

    async fn bootstrap_runtime_assets(&self) -> Result<(), TmuxError> {
        // Idempotent mkdir.
        tokio::fs::create_dir_all(&self.socket_dir)
            .await
            .map_err(|source| TmuxError::SocketDirCreate {
                path: self.socket_dir.clone(),
                source,
            })?;

        // Lock the directory down to the current user only — the socket
        // path is a security boundary (anyone who can read it can attach
        // to every conversation's tmux server). Always enforce on every
        // bootstrap, even if the dir pre-existed: a pre-existing dir
        // with broader perms left the boundary open before this fix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            if let Err(e) = tokio::fs::set_permissions(&self.socket_dir, perms).await {
                // Don't fail registry init on a chmod failure (e.g. an
                // unusual filesystem that doesn't honor mode bits) —
                // the directory is at least usable. Make the degraded
                // posture loud so the operator can investigate.
                tracing::warn!(
                    socket_dir = %self.socket_dir.display(),
                    error = %e,
                    "tmux: failed to enforce 0700 on socket dir; per-conversation sockets may be reachable by other users on this host"
                );
            }
        }

        // Write the Phoenix-shipped config file once.
        let config_path = self.config_path();
        tokio::fs::write(&config_path, SERVER_CONFIG_TEXT)
            .await
            .map_err(|source| TmuxError::SocketDirCreate {
                path: config_path,
                source,
            })?;

        Ok(())
    }

    /// Get-or-create the per-`WorkScope` `Arc<RwLock<TmuxServer>>` and
    /// drive the probe-and-act sequence (REQ-TMUX-002 / REQ-TMUX-005 /
    /// REQ-TMUX-006, REQ-TMUX-WS-001).
    ///
    /// `cwd` is the conversation's working directory; passed to tmux's
    /// `new-session -c` when a fresh server is spawned so the pane
    /// shell starts in the conversation's project. `cwd` is ignored
    /// when the probe sees `Live` — re-attaching to an existing server
    /// uses whatever start directory was set when it was first spawned.
    ///
    /// `work_scope` controls registry keying *and* socket keying (task
    /// 03001, REQ-TMUX-WS-001):
    /// - `WorkScope::Worktree(path)` — Work/Branch/Explore: registry
    ///   entry and socket keyed to the worktree path so continuations
    ///   resolving to the same scope automatically share the same session.
    /// - `WorkScope::Conversation(id)` — Direct mode: registry entry and
    ///   socket keyed to the conversation id.
    ///
    /// On `Live`: no spawn, status=Live.
    /// On `NoSocket`: spawn `main` session in `cwd`, status=Live.
    /// On `DeadSocket`: unlink stale file, spawn `main` session in
    /// `cwd`, status=Live.
    ///
    /// Concurrent calls on the same scope race for that entry's write
    /// lock; the loser observes the freshly-spawned server as `Live`
    /// and skips the spawn.
    pub async fn ensure_live(
        &self,
        work_scope: &WorkScope,
        cwd: &Path,
    ) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
        if !self.binary_available {
            return Err(TmuxError::BinaryUnavailable);
        }
        self.ensure_runtime_assets().await?;

        let socket_path = match work_scope {
            WorkScope::Worktree(path) => {
                socket_path_for_worktree(&self.socket_dir, Path::new(path))
            }
            WorkScope::Conversation(id) => socket_path_for(&self.socket_dir, id),
            WorkScope::Global => socket_path_for_global(&self.socket_dir),
        };

        let server_arc = self.get_or_insert(work_scope, socket_path).await;

        let mut server = server_arc.write().await;
        // Probe under the per-scope entry write lock — the only
        // authoritative point of decision. An earlier outer-lock probe
        // was an unsound shortcut: if the server died (or the outer
        // probe transiently lied) between probe and lock acquisition,
        // marking the entry Live would skip the spawn and leave a
        // dead-but-Live entry behind. Always probing under the lock
        // gives us the latest server state at the moment we decide.
        let probe_result =
            probe(&server.socket_path)
                .await
                .map_err(|source| TmuxError::ProbeFailed {
                    socket_path: server.socket_path.clone(),
                    source,
                })?;

        match probe_result {
            ProbeResult::Live => {
                server.status = ServerStatus::Live;
            }
            ProbeResult::NoSocket => {
                spawn_session(&server.socket_path, &self.config_path(), cwd).await?;
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
                spawn_session(&server.socket_path, &self.config_path(), cwd).await?;
                server.status = ServerStatus::Live;
            }
        }
        drop(server);
        Ok(server_arc)
    }

    /// Get-or-create the entry without driving probe/spawn. Internal
    /// helper for `ensure_live`; not exposed because callers should
    /// always go through the probe-and-act sequence.
    async fn get_or_insert(
        &self,
        work_scope: &WorkScope,
        socket_path: PathBuf,
    ) -> Arc<RwLock<TmuxServer>> {
        let key = work_scope.stable_key();
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(&key) {
                return entry.clone();
            }
        }
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get(&key) {
            return entry.clone();
        }
        let entry = Arc::new(RwLock::new(TmuxServer::new(
            work_scope.clone(),
            socket_path,
        )));
        map.insert(key, entry.clone());
        entry
    }

    /// Best-effort tear-down of a `WorkScope`'s tmux server, called from
    /// the unified `run_resource_cleanup_cascade` (REQ-BED-032 —
    /// archive / abandon / mark-merged / hard-delete all share this
    /// path).
    ///
    /// The registry is keyed by `WorkScope::stable_key()` (same lookup
    /// `ensure_live` uses for insertion). When the registry holds no
    /// entry — orphaned socket from a prior process, or a scope whose
    /// tools never reached `tmux_run` — the deterministic socket path
    /// is derived from the scope so we still attempt the unlink.
    ///
    /// `inheritor_scope`: the resolved `WorkScope` of the conversation
    /// (continuation) that this conversation transfers ownership to, if
    /// any. Preservation is purely scope equality — when the inheritor
    /// resolves to the *same* scope, the tmux session is still in active
    /// use and we skip the kill/unlink. When the inheritor resolves to a
    /// different scope — Direct conversations always do, since their
    /// continuations get a fresh `Conversation` scope — we fall through
    /// to kill+unlink. This makes preservation correct by construction:
    /// "are my resources still owned by someone live?" rather than
    /// case-analysis on scope kind plus an implicit invariant about
    /// continuation inheritance.
    ///
    /// Postcondition: registry has no entry for `work_scope`. If
    /// `inheritor_scope` is `None` (or differs from `work_scope`): socket
    /// file is gone and the tmux server process is gone. Failures of
    /// `kill-server` (server already dead) and `remove_file` (file already
    /// gone) are non-fatal.
    ///
    /// REQ-TMUX-007, REQ-TMUX-WS-001, REQ-TMUX-WS-002.
    pub async fn cascade_on_delete(
        &self,
        work_scope: &WorkScope,
        inheritor_scope: Option<&WorkScope>,
    ) -> CascadeReport {
        let key = work_scope.stable_key();
        let entry = {
            let mut map = self.inner.write().await;
            map.remove(&key)
        };

        let socket_path = if let Some(arc) = entry {
            let server = arc.read().await;
            server.socket_path.clone()
        } else {
            // No registry entry — fall back to the deterministic path so
            // we still attempt cleanup of any orphaned socket from a
            // prior process. Path derivation matches `ensure_live`.
            match work_scope {
                WorkScope::Worktree(path) => {
                    socket_path_for_worktree(&self.socket_dir, Path::new(path))
                }
                WorkScope::Conversation(id) => socket_path_for(&self.socket_dir, id),
                WorkScope::Global => socket_path_for_global(&self.socket_dir),
            }
        };

        // Preservation by scope equality: the inheritor (continuation) is
        // still driving the same tmux server iff it resolves to the same
        // WorkScope. Falls out structurally — Direct continuations
        // resolve to Conversation(<their own id>), which is never equal
        // to the parent's Conversation(<parent id>), so they take the
        // kill+unlink path automatically.
        if inheritor_scope == Some(work_scope) {
            tracing::debug!(
                work_scope = %work_scope,
                socket = %socket_path.display(),
                "tmux: skipping server kill — scope inherited by continuation"
            );
            return CascadeReport {
                socket_path,
                kill_server_error: None,
                unlink_error: None,
            };
        }

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
/// surfaced to the caller (the unified `run_resource_cleanup_cascade`
/// in handlers.rs) so partial failures can be logged. Neither field is
/// fatal.
#[derive(Debug, Clone, Default)]
pub struct CascadeReport {
    pub socket_path: PathBuf,
    pub kill_server_error: Option<String>,
    pub unlink_error: Option<String>,
}

/// Convenience function for the cleanup-cascade orchestrator. Equivalent
/// to `registry.cascade_on_delete(…).await` — kept as a free function
/// for symmetry with the bash registry's `remove_conversation` API and
/// the new `cascade_browser_on_delete`.
pub async fn cascade_tmux_on_delete(
    registry: &Arc<TmuxRegistry>,
    work_scope: &WorkScope,
    inheritor_scope: Option<&WorkScope>,
) -> CascadeReport {
    registry
        .cascade_on_delete(work_scope, inheritor_scope)
        .await
}

/// Spawn a fresh detached tmux session named `main` against
/// `socket_path` with `cwd` as the pane's start directory
/// (REQ-TMUX-002 / `tmux_default_session`). This is the only place
/// `new-session -d` is issued, and therefore the only place where
/// `-f <config_path>` actually loads the Phoenix-shipped config —
/// subsequent invocations against the same socket connect to the
/// already-running server and inherit its loaded config.
///
/// `-c <cwd>` is load-bearing: without it tmux would inherit Phoenix's
/// own working directory for the pane's shell, putting the agent (and
/// any in-app terminal that later attaches) in the Phoenix repo
/// instead of the conversation's project directory.
pub async fn spawn_session(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
) -> Result<(), TmuxError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "-f",
            &config_path.to_string_lossy(),
            "-S",
            &socket_path.to_string_lossy(),
            "new-session",
            "-d",
            "-c",
            &cwd.to_string_lossy(),
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

    // `new-session -d` returns once the server has accepted the session, but
    // the pane's shell is not necessarily in steady state — under load, a
    // format-string query (`display-message -p '#{pane_current_path}'`) issued
    // immediately after can come back empty with a 0 exit. Poll list-panes
    // until the pane exists so the postcondition "spawn_session returns =>
    // pane usable" holds for every caller, not just well-timed ones. Task 62006.
    for attempt in 0..PANE_READY_MAX_ATTEMPTS {
        let panes = tokio::process::Command::new("tmux")
            .args([
                "-f",
                &config_path.to_string_lossy(),
                "-S",
                &socket_path.to_string_lossy(),
                "list-panes",
                "-t",
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
                reason: format!("failed to probe pane readiness: {e}"),
            })?;
        if panes.status.success() && !panes.stdout.is_empty() {
            return Ok(());
        }
        if attempt + 1 < PANE_READY_MAX_ATTEMPTS {
            tokio::time::sleep(PANE_READY_POLL_INTERVAL).await;
        }
    }
    Err(TmuxError::SpawnFailed {
        socket_path: socket_path.to_path_buf(),
        reason: "session spawned but pane never became ready".to_string(),
    })
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
    fn socket_path_for_worktree_is_deterministic() {
        let dir = PathBuf::from("/x/y");
        let wt = PathBuf::from("/home/user/.phoenix-ide/worktrees/abc123");
        let a = socket_path_for_worktree(&dir, &wt);
        let b = socket_path_for_worktree(&dir, &wt);
        assert_eq!(a, b);
        // Socket name starts with "wt-" and ends with ".sock".
        let name = a.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("wt-"), "expected wt- prefix, got {name}");
        assert!(name.ends_with(".sock"), "expected .sock suffix, got {name}");
    }

    #[test]
    fn socket_path_for_worktree_differs_from_conv_path() {
        let dir = PathBuf::from("/x/y");
        let wt = PathBuf::from("/some/worktree");
        let wt_path = socket_path_for_worktree(&dir, &wt);
        let conv_path = socket_path_for(&dir, "some-conv-id");
        assert_ne!(wt_path, conv_path);
    }

    /// Pin the exact SHA-256 prefix for a fixed worktree path so a future
    /// hash-algorithm regression (e.g. someone reverting to `DefaultHasher`)
    /// fails loudly instead of silently re-keying every live tmux session.
    #[test]
    fn socket_path_for_worktree_uses_stable_sha256_prefix() {
        let dir = PathBuf::from("/x/y");
        let wt = PathBuf::from("/repo/.phoenix/worktrees/abc123");
        // First 8 bytes of SHA-256("/repo/.phoenix/worktrees/abc123") as a
        // big-endian u64. Computed once and pinned.
        let p = socket_path_for_worktree(&dir, &wt);
        assert_eq!(
            p,
            PathBuf::from("/x/y/wt-2e83c86fb0db24ce.sock"),
            "socket path drifted — hash algorithm changed?"
        );
    }

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
            reg.ensure_live(
                &crate::work_scope::WorkScope::Conversation("conv-x".to_string()),
                tmp.path()
            )
            .await,
            Err(TmuxError::BinaryUnavailable)
        ));
    }

    #[tokio::test]
    async fn ensure_runtime_assets_sets_0700_perms_and_writes_config_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("nested").join("tmux-sockets");
        let reg = TmuxRegistry::with_socket_dir_and_binary(dir.clone(), false);
        reg.ensure_runtime_assets()
            .await
            .expect("mkdir + config write");
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

    #[tokio::test]
    async fn ensure_runtime_assets_tightens_perms_on_pre_existing_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("preexisting");
        // Pre-create the dir with broader perms — simulates a manually
        // created socket dir or one inherited from an earlier
        // installation. Phoenix should tighten it on bootstrap.
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let reg = TmuxRegistry::with_socket_dir_and_binary(dir.clone(), false);
        reg.ensure_runtime_assets()
            .await
            .expect("bootstrap on pre-existing dir");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&dir).unwrap();
            assert_eq!(
                meta.permissions().mode() & 0o777,
                0o700,
                "pre-existing dir's perms should be tightened to 0700"
            );
        }
    }

    #[tokio::test]
    async fn ensure_runtime_assets_runs_at_most_once_per_registry() {
        // A second call to ensure_runtime_assets should not overwrite
        // the config file (the OnceCell guard prevents redundant work).
        // We verify this by hand-mutating the file between calls and
        // checking that the second call leaves our mutation intact.
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        reg.ensure_runtime_assets().await.expect("first call");
        let config_path = reg.config_path();

        // Hand-mutate the on-disk file.
        std::fs::write(&config_path, b"# clobbered for test\n").unwrap();

        reg.ensure_runtime_assets().await.expect("second call");
        let observed = std::fs::read(&config_path).unwrap();
        assert_eq!(
            observed, b"# clobbered for test\n",
            "second ensure_runtime_assets should not overwrite the config file"
        );
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
        let scope = WorkScope::Conversation("never-existed".to_string());
        let report = reg.cascade_on_delete(&scope, None).await;
        assert!(report.kill_server_error.is_none());
        assert!(report.unlink_error.is_none());
    }

    /// Regression for the original key-mismatch bug Copilot flagged on
    /// PR #136: `ensure_live` inserts the registry entry keyed by
    /// `work_scope.stable_key()`, and the cascade must use the same key
    /// for removal. The scope-equality refactor now passes the scope
    /// directly, so the bug is structurally precluded — this test
    /// pins the contract so it can't regress.
    #[tokio::test]
    async fn cascade_on_delete_removes_entry_inserted_by_ensure_live_paths() {
        let tmp = TempDir::new().unwrap();
        // binary_available = false so ensure_live short-circuits before
        // probing/spawning; we exercise only the registry insertion +
        // removal contract, not the tmux subprocess.
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);

        let conv_scope = WorkScope::Conversation("conv-direct".to_string());
        let conv_sock = socket_path_for(tmp.path(), "conv-direct");
        let _ = reg.get_or_insert(&conv_scope, conv_sock).await;

        let wt_path = std::path::PathBuf::from("/tmp/phoenix-tmux-cascade-regression-wt");
        let wt_scope = WorkScope::Worktree(wt_path.to_string_lossy().into_owned());
        let wt_sock = socket_path_for_worktree(tmp.path(), &wt_path);
        let _ = reg.get_or_insert(&wt_scope, wt_sock).await;

        assert_eq!(
            reg.conversation_count().await,
            2,
            "precondition: both entries present"
        );

        let _ = reg.cascade_on_delete(&conv_scope, None).await;
        assert_eq!(
            reg.conversation_count().await,
            1,
            "Conversation-scope cascade must remove the Conversation-keyed entry"
        );

        let _ = reg.cascade_on_delete(&wt_scope, None).await;
        assert_eq!(
            reg.conversation_count().await,
            0,
            "Worktree-scope cascade must remove the Worktree-keyed entry"
        );
    }

    /// Direct-mode (no worktree) continuations resolve to their own
    /// `Conversation(<child id>)` scope, which is never equal to the
    /// parent's `Conversation(<parent id>)` scope. Cascade must therefore
    /// tear the orphan server down via the scope-equality preservation
    /// rule — even when `inheritor_scope` is provided.
    #[tokio::test]
    async fn cascade_on_delete_direct_continuation_does_not_preserve() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        // Stage an orphaned socket file at the conv-{id} keyed path.
        let socket_path = socket_path_for(tmp.path(), "parent-direct");
        std::fs::write(&socket_path, b"stale").unwrap();
        assert!(socket_path.exists(), "precondition: socket file staged");

        // Direct conv (Conversation scope) being continued. The
        // continuation has a different scope (its own conversation id),
        // so preservation must NOT trigger — socket should be unlinked.
        let parent_scope = WorkScope::Conversation("parent-direct".to_string());
        let child_scope = WorkScope::Conversation("child-conv".to_string());
        let report = reg
            .cascade_on_delete(&parent_scope, Some(&child_scope))
            .await;
        assert!(report.kill_server_error.is_none());
        assert!(report.unlink_error.is_none());
        assert!(
            !socket_path.exists(),
            "Direct continuation must not preserve socket; got lingering {}",
            socket_path.display()
        );
    }

    /// Worktree-backed continuations resolve to the same `Worktree(<path>)`
    /// scope as the parent. Cascade must skip kill/unlink in this case
    /// via the scope-equality preservation rule.
    #[tokio::test]
    async fn cascade_on_delete_worktree_continuation_preserves_socket() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let worktree = std::path::PathBuf::from("/tmp/phoenix-test-worktree-preserve");
        let socket_path = socket_path_for_worktree(tmp.path(), &worktree);
        std::fs::write(&socket_path, b"live").unwrap();

        let parent_scope = WorkScope::Worktree(worktree.to_string_lossy().into_owned());
        let child_scope = parent_scope.clone();
        let report = reg
            .cascade_on_delete(&parent_scope, Some(&child_scope))
            .await;
        assert!(report.kill_server_error.is_none());
        assert!(report.unlink_error.is_none());
        assert!(
            socket_path.exists(),
            "worktree-backed continuation must preserve socket at {}",
            socket_path.display()
        );
        // Cleanup so the file doesn't leak into the next test run.
        let _ = std::fs::remove_file(&socket_path);
    }
}
