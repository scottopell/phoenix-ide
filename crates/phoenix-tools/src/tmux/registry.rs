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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::WorkScope;

use thiserror::Error;
use tokio::sync::{OnceCell, RwLock};

use super::probe::{probe, ProbeResult};

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

/// The `phx`-companion setup version stamped into a tmux server's global
/// environment under [`COMPANION_VERSION_VAR`]. Bump it whenever the PTY env
/// injection or the terminal-features that `phx` / OSC-8 run-links depend on
/// change, so a server spawned under an older version is brought up to date on
/// reuse (see `refresh_companion_if_stale`). A server with the current stamp is
/// left untouched.
const COMPANION_ENV_VERSION: &str = "1";
const COMPANION_VERSION_VAR: &str = "PHOENIX_COMPANION_VERSION";
const SERVER_GENERATION_VAR: &str = "PHOENIX_TMUX_SERVER_GENERATION";
const KILLED_WINDOW_ENV_PREFIX: &str = "PHOENIX_TMUX_KILLED_";
const TERMINAL_CAPTURE_START: &str = "-2000";

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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxWindowIdentity {
    pub work_scope: WorkScope,
    pub server_generation: String,
    pub window_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RegisteredWindowState {
    Live {
        wait_targetable: bool,
    },
    Terminal(TmuxTerminalInspection),
    Killed {
        occurred_at: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxTerminalInspection {
    Live,
    WindowKilled {
        occurred_at: chrono::DateTime<chrono::Utc>,
    },
    Terminal {
        exit_code: i32,
        occurred_at: Option<chrono::DateTime<chrono::Utc>>,
        duration_ms: Option<u64>,
        final_tail: Vec<String>,
    },
    Missing,
    Unavailable,
}

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
    pub server_generation: Option<String>,
}

impl TmuxServer {
    fn new(work_scope: WorkScope, socket_path: PathBuf) -> Self {
        Self {
            work_scope,
            socket_path,
            status: ServerStatus::NotProbed,
            server_generation: None,
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
///
/// # Panics
/// Never in practice: it slices the first 8 bytes of a SHA-256 digest, which
/// is always 32 bytes, so the `try_into` to a `[u8; 8]` is infallible.
#[must_use]
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
#[must_use]
pub fn socket_path_for(socket_dir: &Path, conversation_id: &str) -> PathBuf {
    socket_dir.join(format!("conv-{conversation_id}.sock"))
}

/// Compute the deterministic socket path for the singleton Global scope
/// (REQ-TERM-WS-001). Only one global tmux server can exist per Phoenix
/// process; the filename is a constant so the same server is reused
/// across process restarts that find the socket already present.
#[must_use]
pub fn socket_path_for_global(socket_dir: &Path) -> PathBuf {
    socket_dir.join("global.sock")
}

/// Signal published when a `TmuxServer` in a `WorkScope` changes state
/// (entry created, status transition `NotProbed`→`Live` / →`Gone`, or
/// removal during the hard-delete cascade). Mirrors the bash
/// `BashLifecycleEvent` shape: it carries only the affected `WorkScope`,
/// leaving inventory assembly and conversation routing to the runtime's
/// work-scope bridge. State transitions only — NOT per-probe noops
/// (REQ-WSUI-007).
#[derive(Debug, Clone)]
pub struct TmuxLifecycleEvent {
    pub work_scope: WorkScope,
}

/// Sink the registry publishes [`TmuxLifecycleEvent`]s into. A `mpsc`
/// keeps the registry decoupled from per-conversation routing (the runtime
/// owns that). `None` for tool-level tests that don't care about the push
/// path. Mirrors [`super::super::bash::registry::BashLifecycleSink`].
pub type TmuxLifecycleSink = tokio::sync::mpsc::UnboundedSender<TmuxLifecycleEvent>;

/// Top-level registry: maps `WorkScope::stable_key()` → per-scope tmux
/// server. One registry instance per Phoenix process.
#[derive(Debug)]
pub struct TmuxRegistry {
    /// Keyed by `WorkScope::stable_key()` so Worktree-scoped continuation
    /// members share an entry, and Worktree vs Conversation namespaces
    /// stay disjoint.
    inner: RwLock<HashMap<String, Arc<RwLock<TmuxServer>>>>,
    registered_windows: RwLock<HashMap<TmuxWindowIdentity, RegisteredWindowState>>,
    socket_dir: PathBuf,
    binary_available: bool,
    /// Bootstrap of the socket dir + 0700 perms + Phoenix server config
    /// file. Runs at most once per process — config-text bumps require a
    /// Phoenix restart anyway (existing tmux servers don't reload `-f`).
    /// `OnceCell::get_or_try_init` retries on failure so a transient
    /// disk error doesn't permanently brick the registry.
    runtime_assets: OnceCell<()>,
    /// Optional sink for tmux state-transition signals (entry created /
    /// status change / cascade removal). Populated by `RuntimeManager::new`
    /// so transitions flow into the work-scope push bridge; `None` for
    /// tool-level tests. Mirrors `BashHandleRegistry::lifecycle_sink`.
    lifecycle_sink: Option<TmuxLifecycleSink>,
}

impl TmuxRegistry {
    /// Construct a registry with the default socket directory rooted at
    /// `~/.phoenix-ide/tmux-sockets/` (or `$PHOENIX_DATA_DIR` if set).
    /// `which::which("tmux")` is called once here and cached for the
    /// process lifetime (REQ-TMUX-003 design / "Binary Availability
    /// Detection").
    #[must_use]
    pub fn new() -> Self {
        Self::with_socket_dir(default_socket_dir())
    }

    /// Construct a registry with a caller-supplied socket directory.
    /// Used by tests and integration scenarios that need an isolated
    /// `tempfile::TempDir`.
    #[must_use]
    pub fn with_socket_dir(socket_dir: PathBuf) -> Self {
        let binary_available = which::which("tmux").is_ok();
        Self {
            inner: RwLock::new(HashMap::new()),
            registered_windows: RwLock::new(HashMap::new()),
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: None,
        }
    }

    /// Construct a registry (default socket dir) that publishes tmux
    /// state-transition signals into `sink`. The runtime wires this to the
    /// work-scope push bridge, which resolves the scope's conversation and
    /// broadcasts a `WorkScopeUpdate`. Mirrors
    /// `BashHandleRegistry::with_lifecycle_sink`.
    #[must_use]
    pub fn with_lifecycle_sink(sink: Option<TmuxLifecycleSink>) -> Self {
        let mut reg = Self::with_socket_dir(default_socket_dir());
        reg.lifecycle_sink = sink;
        reg
    }

    /// Publish a tmux state-transition signal for `work_scope` if a sink is
    /// wired. Best-effort: a dropped receiver / closed channel is logged at
    /// `debug` (capability gap) and does not affect registry correctness.
    /// Mirrors `BashHandleRegistry::emit_lifecycle`.
    fn emit_lifecycle(&self, work_scope: &WorkScope) {
        let Some(sink) = self.lifecycle_sink.as_ref() else {
            return;
        };
        let event = TmuxLifecycleEvent {
            work_scope: work_scope.clone(),
        };
        if let Err(e) = sink.send(event) {
            tracing::debug!(
                work_scope = %work_scope,
                error = %e,
                "dropping tmux lifecycle event — sink closed"
            );
        }
    }

    /// Test-only constructor that lets the caller force
    /// `binary_available` to a chosen value, regardless of whether tmux
    /// is on PATH. Used to exercise the "tmux binary missing" branches
    /// of the tool dispatch and the terminal attach fallback without
    /// requiring a host without tmux.
    #[cfg(test)]
    #[must_use]
    pub fn with_socket_dir_and_binary(socket_dir: PathBuf, binary_available: bool) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            registered_windows: RwLock::new(HashMap::new()),
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: None,
        }
    }

    /// Test-only constructor combining a caller-supplied socket directory,
    /// forced `binary_available`, and a lifecycle sink — used to assert that
    /// status transitions and cascade removal round-trip through the sink
    /// without requiring a real tmux server.
    #[cfg(test)]
    #[must_use]
    pub fn with_socket_dir_binary_and_sink(
        socket_dir: PathBuf,
        binary_available: bool,
        sink: Option<TmuxLifecycleSink>,
    ) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            registered_windows: RwLock::new(HashMap::new()),
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: sink,
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
    ///
    /// # Errors
    /// - [`TmuxError::BinaryUnavailable`] when tmux was not found on PATH at
    ///   registry init.
    /// - Other [`TmuxError`] variants when the probe / unlink / spawn / mkdir
    ///   steps fail.
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

        let (server_arc, created) = self.get_or_insert(work_scope, socket_path).await;

        let mut server = server_arc.write().await;
        let prev_status = server.status;
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

        let mut reused_live = false;
        match probe_result {
            ProbeResult::Live => {
                let generation = ensure_server_generation(&server.socket_path).await?;
                server.server_generation = Some(generation);
                server.status = ServerStatus::Live;
                reused_live = true;
            }
            ProbeResult::NoSocket => {
                let generation = new_server_generation();
                spawn_session_with_generation(
                    &server.socket_path,
                    &self.config_path(),
                    cwd,
                    &generation,
                )
                .await?;
                verify_server_generation(&server.socket_path, &generation).await?;
                server.server_generation = Some(generation);
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
                let generation = new_server_generation();
                spawn_session_with_generation(
                    &server.socket_path,
                    &self.config_path(),
                    cwd,
                    &generation,
                )
                .await?;
                verify_server_generation(&server.socket_path, &generation).await?;
                server.server_generation = Some(generation);
                server.status = ServerStatus::Live;
            }
        }
        let status_changed = server.status != prev_status;
        let socket_path = server.socket_path.clone();
        drop(server);

        // A reused live server may predate the current companion setup (a
        // Phoenix upgrade, or a server created before `phx` existed). Bring it
        // up to date non-destructively — restore OSC-8 forwarding and inject the
        // `phx` env for new windows. Gated on the version stamp, so a current
        // server costs only one `show-environment` probe. Freshly spawned
        // servers (NoSocket / DeadSocket) already carry the current setup.
        if reused_live {
            refresh_companion_if_stale(&socket_path).await;
        }
        // Emit on the work-scope inventory edge once the status has
        // SETTLED. Two cases collapse to the same rule:
        //   - First materialization (`created`): the freshly inserted entry
        //     starts `not_probed` and the probe/spawn above resolves it to
        //     its real status (`live`/`gone`). Emit so the panel reflects
        //     the settled status — never the transient `not_probed`.
        //   - Later status transition on an existing entry (`status_changed`):
        //     a real edge the inventory must learn about.
        // A probe-noop on an already-`live` entry (neither created nor
        // changed) does not re-emit (REQ-WSUI-007: state transitions only).
        if created || status_changed {
            self.emit_lifecycle(work_scope);
        }
        Ok(server_arc)
    }

    /// Get-or-create the entry without driving probe/spawn. Internal
    /// helper for `ensure_live`; not exposed because callers should
    /// always go through the probe-and-act sequence.
    ///
    /// Returns `(entry, created)` where `created` is `true` iff this call
    /// inserted a fresh entry — so the caller can emit the entry-CREATED
    /// lifecycle edge exactly once, even under a concurrent-creator race
    /// (the loser observes the existing entry and gets `false`).
    async fn get_or_insert(
        &self,
        work_scope: &WorkScope,
        socket_path: PathBuf,
    ) -> (Arc<RwLock<TmuxServer>>, bool) {
        let key = work_scope.stable_key();
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(&key) {
                return (entry.clone(), false);
            }
        }
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get(&key) {
            return (entry.clone(), false);
        }
        let entry = Arc::new(RwLock::new(TmuxServer::new(
            work_scope.clone(),
            socket_path,
        )));
        map.insert(key, entry.clone());
        (entry, true)
    }

    /// Move a `WorkScope`'s tmux server entry from `old` to `new`.
    ///
    /// Used at an Explore→Work approval, where the conversation's scope flips
    /// from `WorkScope::Conversation(id)` to `WorkScope::Worktree(path)`: a tmux
    /// server spawned pre-approval is stored under `old` and must follow the
    /// scope so the inventory and cleanup cascade resolve it under `new`.
    ///
    /// Only the registry key and the entry's `work_scope` diagnostic field
    /// move. The `socket_path` is left untouched — the running tmux server
    /// lives on the socket derived from `old`, and the OS process is not
    /// re-socketed. The cascade reads the stored `socket_path` (it does not
    /// re-derive from the scope when an entry is present), so subsequent
    /// kill/unlink still targets the correct socket.
    ///
    /// Returns `true` if an entry was moved. No-ops (returns `false`) when:
    /// - `old == new` (nothing to do), or
    /// - there is no entry under `old`, or
    /// - `new` is already occupied — the pre-existing `new` entry is preserved
    ///   and `old` is left in place (NOT clobbered), with a WARN. At approval
    ///   `new` is a freshly created worktree scope, so occupancy is not
    ///   expected.
    pub async fn rekey_scope(&self, old: &WorkScope, new: &WorkScope) -> bool {
        if old == new {
            return false;
        }
        let old_key = old.stable_key();
        let new_key = new.stable_key();
        let mut map = self.inner.write().await;
        if map.contains_key(&new_key) {
            if map.contains_key(&old_key) {
                tracing::warn!(
                    old = %old,
                    new = %new,
                    "tmux: refusing to rekey server entry — destination scope already occupied; leaving both entries in place"
                );
            }
            return false;
        }
        let Some(entry) = map.remove(&old_key) else {
            return false;
        };
        entry.write().await.work_scope = new.clone();
        map.insert(new_key, entry);
        drop(map);

        let mut windows = self.registered_windows.write().await;
        let rekeyed = windows
            .iter()
            .filter(|(identity, _)| identity.work_scope == *old)
            .map(|(identity, state)| {
                let mut identity = identity.clone();
                identity.work_scope = new.clone();
                (identity, state.clone())
            })
            .collect::<Vec<_>>();
        windows.retain(|identity, _| identity.work_scope != *old);
        windows.extend(rekeyed);
        true
    }

    /// Look up a `WorkScope`'s tmux server entry **without creating one or
    /// probing the socket**.
    ///
    /// Read-only counterpart to the `get_or_insert` + probe path in
    /// [`Self::ensure_live`], for observability surfaces (the work-scope
    /// inventory endpoint) that must report the in-memory `status` as-is.
    /// It deliberately does NOT run `tmux ls`: a probe is a process spawn,
    /// and the inventory must not spawn one on every assembly.
    pub async fn get_existing(&self, work_scope: &WorkScope) -> Option<Arc<RwLock<TmuxServer>>> {
        let key = work_scope.stable_key();
        self.inner.read().await.get(&key).cloned()
    }

    pub async fn register_window(&self, identity: TmuxWindowIdentity, wait_targetable: bool) {
        self.registered_windows
            .write()
            .await
            .insert(identity, RegisteredWindowState::Live { wait_targetable });
    }

    pub async fn has_registered_window(&self, identity: &TmuxWindowIdentity) -> bool {
        self.registered_windows.read().await.contains_key(identity)
    }

    pub async fn is_wait_targetable_window(&self, identity: &TmuxWindowIdentity) -> bool {
        matches!(
            self.registered_windows.read().await.get(identity),
            Some(
                RegisteredWindowState::Live {
                    wait_targetable: true,
                } | RegisteredWindowState::Terminal(_)
            )
        )
    }

    pub async fn registered_window_identities(
        &self,
        work_scope: &WorkScope,
        server_generation: &str,
    ) -> Vec<TmuxWindowIdentity> {
        self.registered_windows
            .read()
            .await
            .keys()
            .filter(|identity| {
                identity.work_scope == *work_scope
                    && identity.server_generation == server_generation
            })
            .cloned()
            .collect()
    }

    pub async fn preserve_terminal_before_kill(&self, identity: &TmuxWindowIdentity) {
        let socket_path = match self.get_existing(&identity.work_scope).await {
            Some(entry) => entry.read().await.socket_path.clone(),
            None => self.derived_socket_path(&identity.work_scope),
        };
        let inspection = inspect_tmux_window(&socket_path, &identity.window_id).await;
        if matches!(inspection, TmuxTerminalInspection::Terminal { .. }) {
            if let Some(state) = self.registered_windows.write().await.get_mut(identity) {
                *state = RegisteredWindowState::Terminal(inspection);
            }
        }
    }

    pub async fn mark_window_killed(&self, identity: &TmuxWindowIdentity) -> bool {
        let occurred_at = chrono::Utc::now();
        let mut windows = self.registered_windows.write().await;
        let Some(state) = windows.get_mut(identity) else {
            return false;
        };
        if matches!(state, RegisteredWindowState::Terminal(_)) {
            return true;
        }
        *state = RegisteredWindowState::Killed { occurred_at };
        drop(windows);

        let socket_path = match self.get_existing(&identity.work_scope).await {
            Some(entry) => entry.read().await.socket_path.clone(),
            None => self.derived_socket_path(&identity.work_scope),
        };
        if !persist_killed_window(&socket_path, identity, occurred_at).await {
            tracing::warn!(window_id = %identity.window_id, "tmux: failed to persist killed-window tombstone");
        }
        true
    }

    pub async fn clear_window_killed(&self, identity: &TmuxWindowIdentity) {
        if let Some(state) = self.registered_windows.write().await.get_mut(identity) {
            *state = RegisteredWindowState::Live {
                wait_targetable: true,
            };
        }
        let socket_path = match self.get_existing(&identity.work_scope).await {
            Some(entry) => entry.read().await.socket_path.clone(),
            None => self.derived_socket_path(&identity.work_scope),
        };
        clear_killed_window(&socket_path, identity).await;
    }

    pub async fn recover_wait_target(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Option<TmuxWindowIdentity> {
        if !self.binary_available || self.ensure_runtime_assets().await.is_err() {
            return None;
        }
        let socket_path = self.derived_socket_path(work_scope);
        if !matches!(probe(&socket_path).await, Ok(ProbeResult::Live)) {
            return None;
        }
        if tmux_window_option(&socket_path, window_id, "@phoenix_wait_targetable").await
            != Some("1".to_owned())
        {
            return None;
        }
        let generation = tmux_global_env(&socket_path, SERVER_GENERATION_VAR).await?;
        let identity = TmuxWindowIdentity {
            work_scope: work_scope.clone(),
            server_generation: generation,
            window_id: window_id.to_owned(),
        };
        if matches!(
            inspect_tmux_window(&socket_path, window_id).await,
            TmuxTerminalInspection::Missing | TmuxTerminalInspection::Unavailable
        ) {
            return None;
        }
        Some(identity)
    }

    pub async fn inspect_window(&self, identity: &TmuxWindowIdentity) -> TmuxTerminalInspection {
        match self.registered_windows.read().await.get(identity) {
            Some(RegisteredWindowState::Terminal(inspection)) => return inspection.clone(),
            Some(RegisteredWindowState::Killed { occurred_at }) => {
                return TmuxTerminalInspection::WindowKilled {
                    occurred_at: *occurred_at,
                };
            }
            Some(RegisteredWindowState::Live { .. }) | None => {}
        }
        if !self.binary_available || self.ensure_runtime_assets().await.is_err() {
            return TmuxTerminalInspection::Unavailable;
        }

        // A scope rekey moves the registry entry but deliberately preserves its
        // running server's original socket. Prefer that durable in-memory path;
        // only derive when recovering after a Phoenix restart. This lookup is
        // read-only and never materializes a missing inventory entry.
        let socket_path = match self.get_existing(&identity.work_scope).await {
            Some(entry) => entry.read().await.socket_path.clone(),
            None => match self
                .socket_path_for_generation(&identity.server_generation)
                .await
            {
                Some(path) => path,
                None => self.derived_socket_path(&identity.work_scope),
            },
        };
        match probe(&socket_path).await {
            Ok(ProbeResult::Live) => {}
            Ok(ProbeResult::NoSocket | ProbeResult::DeadSocket) => {
                return TmuxTerminalInspection::Missing;
            }
            Err(_) => return TmuxTerminalInspection::Unavailable,
        }
        let Some(observed_generation) = tmux_global_env(&socket_path, SERVER_GENERATION_VAR).await
        else {
            return TmuxTerminalInspection::Missing;
        };

        if observed_generation != identity.server_generation {
            return TmuxTerminalInspection::Missing;
        }

        if let Some(occurred_at) = load_killed_window(&socket_path, identity).await {
            return TmuxTerminalInspection::WindowKilled { occurred_at };
        }

        inspect_tmux_window(&socket_path, &identity.window_id).await
    }

    async fn socket_path_for_generation(&self, generation: &str) -> Option<PathBuf> {
        let mut entries = tokio::fs::read_dir(&self.socket_dir).await.ok()?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("sock") {
                continue;
            }
            if matches!(probe(&path).await, Ok(ProbeResult::Live))
                && tmux_global_env(&path, SERVER_GENERATION_VAR)
                    .await
                    .as_deref()
                    == Some(generation)
            {
                return Some(path);
            }
        }
        None
    }

    /// Deterministic socket path for a `WorkScope`, derived the same way
    /// `ensure_live` derives it on insertion. Used by the cascade when no
    /// registry entry is present (orphan-socket cleanup) and on the
    /// preserved path where the entry is intentionally left intact.
    fn derived_socket_path(&self, work_scope: &WorkScope) -> PathBuf {
        match work_scope {
            WorkScope::Worktree(path) => {
                socket_path_for_worktree(&self.socket_dir, Path::new(path))
            }
            WorkScope::Conversation(id) => socket_path_for(&self.socket_dir, id),
            WorkScope::Global => socket_path_for_global(&self.socket_dir),
        }
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
        // Preservation by scope equality: the inheritor (continuation) is
        // still driving the same tmux server iff it resolves to the same
        // WorkScope. Falls out structurally — Direct continuations
        // resolve to Conversation(<their own id>), which is never equal
        // to the parent's Conversation(<parent id>), so they take the
        // kill+unlink path automatically.
        //
        // This check precedes the registry removal: when the scope is
        // preserved the tmux server keeps running, so its registry entry
        // must survive too — otherwise the inventory would report "no tmux
        // server" for a still-live session until a later `ensure_live`
        // recreated the entry. (Mirrors `cascade_bash_on_delete`, which
        // also short-circuits before its `registry.remove`.)
        if inheritor_scope == Some(work_scope) {
            let socket_path = self.derived_socket_path(work_scope);
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

        let key = work_scope.stable_key();
        let entry = {
            let mut map = self.inner.write().await;
            map.remove(&key)
        };
        let had_entry = entry.is_some();

        let socket_path = if let Some(arc) = entry {
            let server = arc.read().await;
            server.socket_path.clone()
        } else {
            // No registry entry — fall back to the deterministic path so
            // we still attempt cleanup of any orphaned socket from a
            // prior process.
            self.derived_socket_path(work_scope)
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

        // REMOVAL edge: an entry existed and was torn down, so the server
        // disappears from the work-scope inventory. Emit only when the
        // registry actually held an entry — an orphan-socket-only cleanup
        // (no in-memory entry) does not change the inventory.
        if had_entry {
            self.emit_lifecycle(work_scope);
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
/// for symmetry with the bash registry's `cascade_bash_on_delete` API and
/// `cascade_browser_on_delete`.
pub async fn cascade_tmux_on_delete(
    registry: &Arc<TmuxRegistry>,
    work_scope: &WorkScope,
    inheritor_scope: Option<&WorkScope>,
) -> CascadeReport {
    registry
        .cascade_on_delete(work_scope, inheritor_scope)
        .await
}

/// Set an explicit environment on a tmux command so the spawned server (and
/// thus its pane shells) match the direct-shell PTY contract: the fixed base
/// env plus the `PtyEnvInjection` (the `phx` shim on PATH, `PHOENIX_API_URL`,
/// `PHOENIX_SUGGEST_TOKEN`) and the safe-var allowlist — never a blind copy of
/// the Phoenix process environment, which would leak server secrets (LLM API
/// keys, gateway config) into every tmux-backed terminal. `build_env_for_tmux`
/// is the single source for that env (`specs/terminal` REQ-TERM-002).
fn set_tmux_server_env(cmd: &mut tokio::process::Command, generation: &str) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    cmd.env_clear();
    cmd.envs(phoenix_terminal::spawn::build_env_for_tmux(&shell));
    // Stamp the companion version so a later reuse can tell a current server
    // (no-op) from a pre-feature/older one that needs a refresh.
    cmd.env(COMPANION_VERSION_VAR, COMPANION_ENV_VERSION);
    cmd.env(SERVER_GENERATION_VAR, generation);
}

/// Run a tmux command against an existing server, discarding output.
/// Best-effort: errors are ignored, because a companion refresh must never
/// block or fail a terminal attach.
async fn run_tmux_quiet(socket_path: &Path, args: &[&str]) {
    let sock = socket_path.to_string_lossy().into_owned();
    let mut full: Vec<&str> = vec!["-S", &sock];
    full.extend_from_slice(args);
    let _ = tokio::process::Command::new("tmux")
        .args(&full)
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Read one variable from a server's global environment, or `None` if unset.
/// `tmux show-environment -g VAR` prints `VAR=value` when set and `-VAR` when
/// not.
async fn tmux_window_option(socket_path: &Path, window_id: &str, option: &str) -> Option<String> {
    let sock = socket_path.to_string_lossy().into_owned();
    let out = tokio::process::Command::new("tmux")
        .args(["-S", &sock, "show-option", "-wqv", "-t", window_id, option])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

async fn tmux_global_env(socket_path: &Path, var: &str) -> Option<String> {
    let sock = socket_path.to_string_lossy().into_owned();
    let out = tokio::process::Command::new("tmux")
        .args(["-S", &sock, "show-environment", "-g", var])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let prefix = format!("{var}=");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
}

/// Bring a reused tmux server up to the current companion version,
/// non-destructively. Gated on the version stamp, so a current server is a
/// no-op. A pre-feature/older live server otherwise reuses panes whose
/// environment and loaded config predate `phx`, so:
///
/// - `set -ag terminal-features ",*:hyperlinks"` restores OSC-8 hyperlink
///   forwarding for the next fresh attach (the relay re-attaches on every panel
///   open, so the user gets it then);
/// - `set-environment -g` injects the `phx` env (PATH prefix, API URL, token)
///   for new windows/panes;
/// - a one-time status hint tells the user how to reach `phx` in the *current*
///   pane, whose shell already exported its PATH and cannot be changed from
///   outside.
///
/// Recreating the server would fix the current pane too but destroy the user's
/// running panes and jobs — rejected. Best-effort throughout.
async fn refresh_companion_if_stale(socket_path: &Path) {
    if tmux_global_env(socket_path, COMPANION_VERSION_VAR)
        .await
        .as_deref()
        == Some(COMPANION_ENV_VERSION)
    {
        return;
    }

    run_tmux_quiet(
        socket_path,
        &["set", "-ag", "terminal-features", ",*:hyperlinks"],
    )
    .await;

    // Put `phx` on new panes' PATH. A `set-environment -g PATH` is silently
    // ignored by tmux for new panes (they take PATH from the server process),
    // so instead wrap the pane shell via default-command to prepend the bin dir
    // before exec — honored for every new window/pane, non-destructive to
    // existing ones.
    if let Some(bin) = phoenix_terminal::spawn::phx_bin_dir() {
        let wrapper = format!(
            r#"PATH="{}:$PATH"; export PATH; exec "${{SHELL:-/bin/sh}}""#,
            bin.display()
        );
        run_tmux_quiet(socket_path, &["set", "-g", "default-command", &wrapper]).await;
    }

    // The suggest token and API URL DO propagate to new panes via the global
    // environment (unlike PATH), so set them there.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    for (k, v) in phoenix_terminal::spawn::build_env_for_tmux(&shell) {
        if k == "PHOENIX_API_URL" || k == "PHOENIX_SUGGEST_TOKEN" {
            run_tmux_quiet(
                socket_path,
                &["set-environment", "-g", k.as_str(), v.as_str()],
            )
            .await;
        }
    }

    run_tmux_quiet(
        socket_path,
        &[
            "set-environment",
            "-g",
            COMPANION_VERSION_VAR,
            COMPANION_ENV_VERSION,
        ],
    )
    .await;

    // The current pane's shell already exported its PATH and can't pick up
    // `phx` retroactively; a new window (which the wrapper above dresses) can.
    run_tmux_quiet(
        socket_path,
        &[
            "display-message",
            "-d",
            "5000",
            "phx now available in new windows — open one (prefix + c) to use it",
        ],
    )
    .await;
}

fn new_server_generation() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("srv-{nanos}-{}", uuid::Uuid::new_v4())
}

async fn ensure_server_generation(socket_path: &Path) -> Result<String, TmuxError> {
    if let Some(generation) = tmux_global_env(socket_path, SERVER_GENERATION_VAR).await {
        return Ok(generation);
    }
    let generation = new_server_generation();
    let sock = socket_path.to_string_lossy().into_owned();
    let output = tokio::process::Command::new("tmux")
        .args([
            "-S",
            &sock,
            "set-environment",
            "-g",
            SERVER_GENERATION_VAR,
            generation.as_str(),
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!("failed to install server generation: {error}"),
        })?;
    if !output.status.success() {
        return Err(TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!(
                "tmux set-environment for server generation exited with {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    verify_server_generation(socket_path, &generation).await?;
    Ok(generation)
}

async fn verify_server_generation(socket_path: &Path, expected: &str) -> Result<(), TmuxError> {
    let observed = tmux_global_env(socket_path, SERVER_GENERATION_VAR).await;
    if observed.as_deref() == Some(expected) {
        return Ok(());
    }
    Err(TmuxError::SpawnFailed {
        socket_path: socket_path.to_path_buf(),
        reason: format!(
            "tmux server generation readback mismatch: expected {expected:?}, observed {observed:?}"
        ),
    })
}

fn killed_window_env_name(window_id: &str) -> String {
    use std::fmt::Write as _;

    window_id
        .as_bytes()
        .iter()
        .fold(KILLED_WINDOW_ENV_PREFIX.to_owned(), |mut encoded, byte| {
            write!(encoded, "{byte:02X}").expect("writing to String cannot fail");
            encoded
        })
}

async fn persist_killed_window(
    socket_path: &Path,
    identity: &TmuxWindowIdentity,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    let value = format!(
        "{}|{}",
        identity.server_generation,
        occurred_at.to_rfc3339()
    );
    let sock = socket_path.to_string_lossy().into_owned();
    tokio::process::Command::new("tmux")
        .args([
            "-S",
            &sock,
            "set-environment",
            "-g",
            &killed_window_env_name(&identity.window_id),
            &value,
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn clear_killed_window(socket_path: &Path, identity: &TmuxWindowIdentity) {
    let sock = socket_path.to_string_lossy().into_owned();
    let _ = tokio::process::Command::new("tmux")
        .args([
            "-S",
            &sock,
            "set-environment",
            "-gu",
            &killed_window_env_name(&identity.window_id),
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

async fn load_killed_window(
    socket_path: &Path,
    identity: &TmuxWindowIdentity,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let value = tmux_global_env(socket_path, &killed_window_env_name(&identity.window_id)).await?;
    let (generation, occurred_at) = value.split_once('|')?;
    if generation != identity.server_generation {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(occurred_at)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

async fn inspect_tmux_window(socket_path: &Path, window_id: &str) -> TmuxTerminalInspection {
    let sock = socket_path.to_string_lossy().into_owned();
    let output = tokio::process::Command::new("tmux")
        .args([
            "-S",
            &sock,
            "capture-pane",
            "-p",
            "-t",
            window_id,
            "-S",
            TERMINAL_CAPTURE_START,
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        return TmuxTerminalInspection::Unavailable;
    };
    if !output.status.success() {
        return TmuxTerminalInspection::Missing;
    }
    let pane_state = tokio::process::Command::new("tmux")
        .args([
            "-S",
            &sock,
            "display-message",
            "-p",
            "-t",
            window_id,
            "#{pane_dead}|#{pane_pid}",
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(pane_state) = pane_state else {
        return TmuxTerminalInspection::Unavailable;
    };
    if !pane_state.status.success() {
        return TmuxTerminalInspection::Missing;
    }
    let pane_state = String::from_utf8_lossy(&pane_state.stdout);
    let pane_is_dead = pane_state
        .trim()
        .split_once('|')
        .is_some_and(|(dead, pid)| dead == "1" && !pid.is_empty());
    let captured = String::from_utf8_lossy(&output.stdout);
    if pane_is_dead {
        if let Some(exit_code) = parse_exit_marker(&captured) {
            return TmuxTerminalInspection::Terminal {
                exit_code,
                occurred_at: parse_occurred_at_marker(&captured),
                duration_ms: parse_duration_ms(&captured),
                final_tail: terminal_tail(&captured),
            };
        }
    }
    TmuxTerminalInspection::Live
}

fn terminal_tail(output: &str) -> Vec<String> {
    let lines: Vec<_> = output.lines().collect();
    lines[lines.len().saturating_sub(20)..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

fn parse_occurred_at_marker(output: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    parse_time_marker(output, "[phoenix] process exited at unix seconds ")
}

fn parse_duration_ms(output: &str) -> Option<u64> {
    let started = parse_time_marker(output, "[phoenix] process started at unix seconds ")?;
    let finished = parse_occurred_at_marker(output)?;
    (finished - started).num_milliseconds().try_into().ok()
}

fn parse_time_marker(output: &str, prefix: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    output.lines().rev().find_map(|line| {
        let value = line.trim().strip_prefix(prefix)?;
        let (seconds, fraction) = value.split_once('.').unwrap_or((value, "0"));
        let seconds = seconds.parse::<i64>().ok()?;
        let nanos = format!("{fraction:0<9}").get(..9)?.parse::<u32>().ok()?;
        chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos)
    })
}

fn parse_exit_marker(output: &str) -> Option<i32> {
    const PREFIX: &str = "[phoenix] process exited with code ";
    output.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix(PREFIX)
            .and_then(|code| code.parse::<i32>().ok())
    })
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
///
/// # Errors
/// Returns a [`TmuxError`] when the `tmux new-session` process fails to
/// spawn or exits non-zero.
pub async fn spawn_session(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
) -> Result<(), TmuxError> {
    let generation = new_server_generation();
    spawn_session_with_generation(socket_path, config_path, cwd, &generation).await
}

async fn spawn_session_with_generation(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
    generation: &str,
) -> Result<(), TmuxError> {
    let mut cmd = tokio::process::Command::new("tmux");
    cmd.args([
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
    ]);
    // A tmux pane shell inherits the tmux *server's* environment, captured here.
    // Build it explicitly (base + PtyEnvInjection + safe-var allowlist) rather
    // than inheriting Phoenix's env, which would leak server secrets into every
    // pane and diverge from the direct-shell path. env_clear also drops TMUX, so
    // an outer-tmux invocation does not trip tmux's nesting refusal.
    set_tmux_server_env(&mut cmd, generation);
    let output = cmd
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
    let mut last_diag = String::from("no probe ran");
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
        // Retain the last probe's exit/stderr so an exhausted poll says WHY
        // (no session vs socket/auth error vs empty pane list), not just "never
        // became ready".
        last_diag = format!(
            "exit {:?}, stderr: {}",
            panes.status.code(),
            String::from_utf8_lossy(&panes.stderr).trim()
        );
        if attempt + 1 < PANE_READY_MAX_ATTEMPTS {
            tokio::time::sleep(PANE_READY_POLL_INTERVAL).await;
        }
    }
    Err(TmuxError::SpawnFailed {
        socket_path: socket_path.to_path_buf(),
        reason: format!(
            "session spawned but pane never became ready after {PANE_READY_MAX_ATTEMPTS} probes (last: {last_diag})"
        ),
    })
}

/// Default socket directory, resolved through [`PhoenixRuntimeEnvironment`]:
/// `$PHOENIX_DATA_DIR/tmux-sockets/` if set, else
/// `$HOME/.phoenix-ide/tmux-sockets/`, falling back to the system temp dir
/// when no home is resolvable.
fn default_socket_dir() -> PathBuf {
    PhoenixRuntimeEnvironment::detect().tmux_socket_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn killed_window_environment_key_is_exact_and_tmux_safe() {
        assert_eq!(killed_window_env_name("@12"), "PHOENIX_TMUX_KILLED_403132");
        assert_ne!(killed_window_env_name("@12"), killed_window_env_name("@21"));
    }

    #[tokio::test]
    async fn killed_window_tombstone_is_exact_and_survives_for_worker_inspection() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let identity = TmuxWindowIdentity {
            work_scope: WorkScope::Conversation("conv".to_owned()),
            server_generation: "generation-1".to_owned(),
            window_id: "@1".to_owned(),
        };
        registry.register_window(identity.clone(), true).await;
        assert!(registry.mark_window_killed(&identity).await);
        assert!(matches!(
            registry.inspect_window(&identity).await,
            TmuxTerminalInspection::WindowKilled { .. }
        ));
        let wrong_generation = TmuxWindowIdentity {
            server_generation: "generation-2".to_owned(),
            ..identity.clone()
        };
        let wrong_window = TmuxWindowIdentity {
            window_id: "@2".to_owned(),
            ..identity
        };
        assert_eq!(
            registry.inspect_window(&wrong_generation).await,
            TmuxTerminalInspection::Unavailable
        );
        assert_eq!(
            registry.inspect_window(&wrong_window).await,
            TmuxTerminalInspection::Unavailable
        );
    }

    #[tokio::test]
    async fn ephemeral_windows_are_not_wait_targetable() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let identity = TmuxWindowIdentity {
            work_scope: WorkScope::Conversation("conv".to_owned()),
            server_generation: "generation-1".to_owned(),
            window_id: "@1".to_owned(),
        };
        registry.register_window(identity.clone(), false).await;
        assert!(registry.has_registered_window(&identity).await);
        assert!(!registry.is_wait_targetable_window(&identity).await);
    }

    #[tokio::test]
    async fn terminal_marker_state_outranks_later_kill_tombstone() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let identity = TmuxWindowIdentity {
            work_scope: WorkScope::Conversation("conv".to_owned()),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let terminal = TmuxTerminalInspection::Terminal {
            exit_code: 0,
            occurred_at: Some(chrono::Utc::now()),
            duration_ms: None,
            final_tail: vec!["done".to_owned()],
        };
        registry.registered_windows.write().await.insert(
            identity.clone(),
            RegisteredWindowState::Terminal(terminal.clone()),
        );
        assert!(registry.mark_window_killed(&identity).await);
        assert_eq!(registry.inspect_window(&identity).await, terminal);
    }

    #[test]
    fn terminal_tail_preserves_three_line_chronology() {
        assert_eq!(
            terminal_tail("first\nsecond\nthird\n"),
            ["first", "second", "third"]
        );
    }

    #[test]
    fn terminal_markers_parse_subsecond_precision_and_exact_duration() {
        let output = "[phoenix] process started at unix seconds 1783936800.123456789\n\
                      [phoenix] process exited with code 0\n\
                      [phoenix] process exited at unix seconds 1783936801.357456789\n";
        assert_eq!(
            parse_occurred_at_marker(output),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_783_936_801, 357_456_789)
        );
        assert_eq!(parse_duration_ms(output), Some(1_234));
    }

    #[test]
    fn terminal_duration_requires_both_durable_markers() {
        assert_eq!(
            parse_duration_ms("[phoenix] process exited at unix seconds 1783936801.357456789\n"),
            None
        );
    }

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

    /// REQ-WSUI-007: cascade removal of a registry-held entry publishes a
    /// `TmuxLifecycleEvent` carrying the affected scope when a sink is wired.
    /// Mirrors the bash registry's `emit_lifecycle_round_trips_through_sink`.
    /// `binary_available = false` so no real tmux process is touched.
    #[tokio::test]
    async fn cascade_removal_round_trips_through_sink() {
        let tmp = TempDir::new().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = TmuxRegistry::with_socket_dir_binary_and_sink(
            tmp.path().to_path_buf(),
            false,
            Some(tx),
        );

        let scope = WorkScope::Conversation("conv-A".to_string());
        let sock = socket_path_for(tmp.path(), "conv-A");
        let (_arc, created) = reg.get_or_insert(&scope, sock).await;
        assert!(created, "first insert must report created");

        // Tearing down a held entry emits exactly one removal edge.
        let _ = reg.cascade_on_delete(&scope, None).await;
        let e = rx.try_recv().expect("removal event missing");
        assert_eq!(e.work_scope, scope);
        assert!(rx.try_recv().is_err(), "no more events expected");
    }

    /// Cascade on a scope with no registry entry (orphan-socket-only path)
    /// does not change the inventory, so it must not emit.
    #[tokio::test]
    async fn cascade_no_entry_does_not_emit() {
        let tmp = TempDir::new().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = TmuxRegistry::with_socket_dir_binary_and_sink(
            tmp.path().to_path_buf(),
            false,
            Some(tx),
        );
        let scope = WorkScope::Conversation("never-existed".to_string());
        let _ = reg.cascade_on_delete(&scope, None).await;
        assert!(rx.try_recv().is_err(), "no entry → no removal event");
    }

    /// Preservation (continuation inherits the same scope) leaves the entry
    /// in place, so it must not emit a removal edge.
    #[tokio::test]
    async fn cascade_preserve_does_not_emit() {
        let tmp = TempDir::new().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = TmuxRegistry::with_socket_dir_binary_and_sink(
            tmp.path().to_path_buf(),
            false,
            Some(tx),
        );
        let wt = WorkScope::Worktree("/tmp/phoenix-tmux-preserve-emit".to_string());
        let sock =
            socket_path_for_worktree(tmp.path(), Path::new("/tmp/phoenix-tmux-preserve-emit"));
        let _ = reg.get_or_insert(&wt, sock).await;
        let _ = reg.cascade_on_delete(&wt, Some(&wt)).await;
        assert!(
            rx.try_recv().is_err(),
            "preserved entry must not emit a removal edge"
        );
    }

    /// Preservation must leave the registry entry intact: when a sibling
    /// continuation still owns the scope, the tmux server keeps running, so
    /// the inventory must continue to see its entry. (Regression for the
    /// remove-before-preserve-check ordering bug, where the preserved path
    /// dropped the map entry and the inventory reported "no tmux server"
    /// for a live session until a later `ensure_live` recreated it.)
    #[tokio::test]
    async fn cascade_preserve_leaves_registry_entry_intact() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let wt = WorkScope::Worktree("/tmp/phoenix-tmux-preserve-entry".to_string());
        let sock =
            socket_path_for_worktree(tmp.path(), Path::new("/tmp/phoenix-tmux-preserve-entry"));
        let _ = reg.get_or_insert(&wt, sock).await;
        assert_eq!(
            reg.conversation_count().await,
            1,
            "precondition: entry held"
        );

        // Sibling continuation inherits the same scope → preserved path.
        let _ = reg.cascade_on_delete(&wt, Some(&wt)).await;

        assert_eq!(
            reg.conversation_count().await,
            1,
            "preserved cascade must leave the registry entry in place"
        );
        assert!(
            reg.get_existing(&wt).await.is_some(),
            "preserved scope must still resolve to its entry"
        );
    }

    /// First `ensure_live` for a scope must emit a work-scope update whose
    /// status reflects the SETTLED state after probe/spawn (`live`), never
    /// the transient `not_probed` seen at insertion. (Regression for the
    /// emit-on-create ordering, where the create emit fired at `not_probed`
    /// and the later create→live transition was suppressed, stranding the
    /// inventory at `not_probed` until a manual refresh.) Requires a real
    /// tmux binary to drive the spawn; skipped otherwise.
    #[tokio::test]
    async fn first_ensure_live_emits_settled_live_status() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg =
            TmuxRegistry::with_socket_dir_binary_and_sink(tmp.path().to_path_buf(), true, Some(tx));

        let scope = WorkScope::Conversation("conv-first-ensure".to_string());
        let arc = reg
            .ensure_live(&scope, tmp.path())
            .await
            .expect("first ensure_live should materialize a live server");

        // Exactly one emit on first materialization, carrying the settled
        // status — Live, not the transient NotProbed insert state.
        let e = rx.try_recv().expect("first ensure_live must emit");
        assert_eq!(e.work_scope, scope);
        assert!(
            rx.try_recv().is_err(),
            "first ensure_live must emit exactly once (no not_probed + live double-emit)"
        );
        assert_eq!(
            arc.read().await.status,
            ServerStatus::Live,
            "settled status must be Live before/at the emit"
        );

        // A second ensure_live on an already-live server is a probe-noop:
        // no status change → no spurious re-emit.
        let _ = reg.ensure_live(&scope, tmp.path()).await.expect("noop");
        assert!(
            rx.try_recv().is_err(),
            "probe-noop on a live server must not re-emit"
        );

        kill_socket(&socket_path_for(tmp.path(), "conv-first-ensure")).await;
    }

    async fn kill_socket(socket_path: &Path) {
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &socket_path.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    /// `emit_lifecycle` with no sink wired is a no-op (no panic). Mirrors the
    /// bash registry's `emit_lifecycle_without_sink_is_no_op`.
    #[tokio::test]
    async fn emit_lifecycle_without_sink_is_no_op() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        reg.emit_lifecycle(&WorkScope::Conversation("conv-X".to_string()));
    }

    #[tokio::test]
    async fn generation_installation_failure_is_fatal() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let missing_socket = tmp.path().join("missing.sock");
        assert!(matches!(
            ensure_server_generation(&missing_socket).await,
            Err(TmuxError::SpawnFailed { .. })
        ));
    }

    #[tokio::test]
    async fn binary_unavailable_short_circuits_ensure_live() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        assert!(matches!(
            reg.ensure_live(
                &phoenix_core::work_scope::WorkScope::Conversation("conv-x".to_string()),
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

    /// Approval scope-flip: a server entry created under the conversation
    /// scope is reachable under the worktree scope after a rekey, the old key
    /// is gone, and the stored `socket_path` is PRESERVED (the running tmux
    /// server lives on the old socket — re-deriving from the new scope would
    /// orphan it). The `work_scope` diagnostic field follows the new scope.
    #[tokio::test]
    async fn rekey_scope_moves_entry_and_preserves_socket_path() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("conv-explore".to_string());
        let new = WorkScope::Worktree("/tmp/wt-approved".to_string());
        let conv_sock = socket_path_for(tmp.path(), "conv-explore");
        let (before, _) = reg.get_or_insert(&old, conv_sock.clone()).await;

        assert!(
            reg.rekey_scope(&old, &new).await,
            "rekey must report a move"
        );

        assert!(
            reg.get_existing(&old).await.is_none(),
            "old key must be gone"
        );
        let after = reg
            .get_existing(&new)
            .await
            .expect("entry must be reachable under the new scope");
        assert!(
            Arc::ptr_eq(&before, &after),
            "rekey must move the Arc, not clone"
        );
        let server = after.read().await;
        assert_eq!(
            server.socket_path, conv_sock,
            "socket_path must be preserved — the running server is on the old socket"
        );
        assert_eq!(
            server.work_scope, new,
            "work_scope diagnostic must follow the new scope"
        );
    }

    #[tokio::test]
    async fn rekey_scope_moves_registered_window_identity() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("conv-explore".to_owned());
        let new = WorkScope::Worktree("/tmp/wt-approved".to_owned());
        reg.get_or_insert(&old, socket_path_for(tmp.path(), "conv-explore"))
            .await;
        let old_identity = TmuxWindowIdentity {
            work_scope: old.clone(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        reg.register_window(old_identity.clone(), true).await;

        assert!(reg.rekey_scope(&old, &new).await);
        let new_identity = TmuxWindowIdentity {
            work_scope: new,
            ..old_identity.clone()
        };
        assert!(!reg.has_registered_window(&old_identity).await);
        assert!(reg.has_registered_window(&new_identity).await);
        assert!(reg.mark_window_killed(&new_identity).await);
    }

    #[tokio::test]
    async fn rekey_scope_no_entry_is_noop() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("never".to_string());
        let new = WorkScope::Worktree("/tmp/wt".to_string());
        assert!(!reg.rekey_scope(&old, &new).await);
        assert!(reg.get_existing(&new).await.is_none());
    }

    #[tokio::test]
    async fn rekey_scope_occupied_destination_does_not_clobber() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("conv".to_string());
        let new = WorkScope::Worktree("/tmp/wt".to_string());
        let (old_arc, _) = reg
            .get_or_insert(&old, socket_path_for(tmp.path(), "conv"))
            .await;
        let (new_arc, _) = reg
            .get_or_insert(
                &new,
                socket_path_for_worktree(tmp.path(), Path::new("/tmp/wt")),
            )
            .await;

        assert!(
            !reg.rekey_scope(&old, &new).await,
            "occupied dest must not move"
        );
        let old_after = reg.get_existing(&old).await.expect("old entry preserved");
        let new_after = reg.get_existing(&new).await.expect("new entry preserved");
        assert!(Arc::ptr_eq(&old_arc, &old_after));
        assert!(Arc::ptr_eq(&new_arc, &new_after));
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
