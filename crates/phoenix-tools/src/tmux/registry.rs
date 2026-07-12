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
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::WorkScope;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
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
const TERMINAL_EVIDENCE_DIR: &str = "_terminal-evidence";
const TERMINAL_EVIDENCE_VERSION: u8 = 2;
const SERVER_GENERATION_VAR: &str = "PHOENIX_TMUX_GENERATION";

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

    #[error("tmux terminal evidence I/O failed at {path}: {source}")]
    EvidenceIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("tmux terminal evidence at {path} is invalid: {reason}")]
    InvalidEvidence { path: PathBuf, reason: String },
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
    /// Identity of the tmux server process currently bound to `socket_path`.
    pub generation: Option<String>,
    pub status: ServerStatus,
}

impl TmuxServer {
    fn new(work_scope: WorkScope, socket_path: PathBuf) -> Self {
        Self {
            work_scope,
            socket_path,
            generation: None,
            status: ServerStatus::NotProbed,
        }
    }
}

const TMUX_RUN_EXIT_MARKER_PREFIX: &str = "[phoenix] process exited with code ";

fn parse_tmux_run_exit_marker(output: &str) -> Option<i32> {
    output.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix(TMUX_RUN_EXIT_MARKER_PREFIX)
            .and_then(|code| code.parse().ok())
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxTerminalStatus {
    Exited,
    Killed,
}

#[derive(Debug, Clone)]
pub struct TmuxTerminalEvidence {
    pub observed_at: SystemTime,
    pub exit_code: Option<i32>,
    pub status: TmuxTerminalStatus,
    pub duration_ms: u64,
    pub tail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DurableEvidenceStatus {
    Exited,
    KillPending,
    Killed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableTerminalEvidence {
    version: u8,
    socket_identity: String,
    generation: String,
    window_id: String,
    observed_at: DateTime<Utc>,
    status: DurableEvidenceStatus,
    exit_code: Option<i32>,
    duration_ms: u64,
    tail: String,
}

impl DurableTerminalEvidence {
    fn terminal_evidence(&self) -> Option<TmuxTerminalEvidence> {
        let status = match self.status {
            DurableEvidenceStatus::Exited => TmuxTerminalStatus::Exited,
            DurableEvidenceStatus::Killed => TmuxTerminalStatus::Killed,
            DurableEvidenceStatus::KillPending => return None,
        };
        Some(TmuxTerminalEvidence {
            observed_at: self.observed_at.into(),
            exit_code: self.exit_code,
            status,
            duration_ms: self.duration_ms,
            tail: self.tail.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum TmuxWindowInspection {
    Missing,
    Live,
    Terminal(TmuxTerminalEvidence),
}

/// Stable identity for one `tmux_run` invocation. It survives a `WorkScope` rekey.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxWindowRunId(String);

#[derive(Debug, Clone)]
struct TmuxWindowRun {
    scope: WorkScope,
    window_id: String,
    generation: String,
    close_after_completion: bool,
    observer_claimed: bool,
}

#[derive(Debug, Clone)]
struct TmuxWindowStart {
    started_at: Instant,
}

/// Top-level registry: maps `WorkScope::stable_key()` → per-scope tmux
/// server. One registry instance per Phoenix process.
#[derive(Debug)]
pub struct TmuxRegistry {
    /// Keyed by `WorkScope::stable_key()` so Worktree-scoped continuation
    /// members share an entry, and Worktree vs Conversation namespaces
    /// stay disjoint.
    inner: RwLock<HashMap<String, Arc<RwLock<TmuxServer>>>>,
    window_starts: RwLock<HashMap<(WorkScope, String), TmuxWindowStart>>,
    terminal_evidence: RwLock<HashMap<(WorkScope, String), TmuxTerminalEvidence>>,
    window_runs: std::sync::Mutex<HashMap<TmuxWindowRunId, TmuxWindowRun>>,
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
            window_starts: RwLock::new(HashMap::new()),
            terminal_evidence: RwLock::new(HashMap::new()),
            window_runs: std::sync::Mutex::new(HashMap::new()),
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
            window_starts: RwLock::new(HashMap::new()),
            terminal_evidence: RwLock::new(HashMap::new()),
            window_runs: std::sync::Mutex::new(HashMap::new()),
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
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: sink,
            window_starts: RwLock::new(HashMap::new()),
            terminal_evidence: RwLock::new(HashMap::new()),
            window_runs: std::sync::Mutex::new(HashMap::new()),
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
                server.generation = Some(recover_or_install_generation(&server.socket_path).await?);
                server.status = ServerStatus::Live;
                reused_live = true;
            }
            ProbeResult::NoSocket => {
                let generation = uuid::Uuid::new_v4().to_string();
                spawn_session(&server.socket_path, &self.config_path(), cwd, &generation).await?;
                server.generation = Some(generation);
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
                let generation = uuid::Uuid::new_v4().to_string();
                spawn_session(&server.socket_path, &self.config_path(), cwd, &generation).await?;
                server.generation = Some(generation);
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

    #[cfg(test)]
    pub async fn install_generation_for_test(&self, work_scope: &WorkScope, generation: &str) {
        let socket = self.derived_socket_path(work_scope);
        let (entry, _) = self.get_or_insert(work_scope, socket).await;
        entry.write().await.generation = Some(generation.to_string());
    }

    /// Register a newly-created window and clear stale evidence for a reused id.
    ///
    /// # Errors
    /// Returns an error if no generated live server owns the window or stale
    /// evidence cannot be removed.
    pub async fn register_window_start(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Result<TmuxWindowRunId, TmuxError> {
        let entry =
            self.get_existing(work_scope)
                .await
                .ok_or_else(|| TmuxError::InvalidEvidence {
                    path: self.evidence_path(work_scope, window_id),
                    reason: "window registered without a live tmux server".to_string(),
                })?;
        let generation =
            entry
                .read()
                .await
                .generation
                .clone()
                .ok_or_else(|| TmuxError::InvalidEvidence {
                    path: self.evidence_path(work_scope, window_id),
                    reason: "live tmux server has no generation".to_string(),
                })?;
        let key = (work_scope.clone(), window_id.to_string());
        self.window_starts.write().await.insert(
            key.clone(),
            TmuxWindowStart {
                started_at: Instant::now(),
            },
        );
        self.terminal_evidence.write().await.remove(&key);
        self.remove_evidence_file(work_scope, window_id).await?;
        let id = TmuxWindowRunId(uuid::Uuid::new_v4().to_string());
        self.window_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id.clone(),
                TmuxWindowRun {
                    scope: work_scope.clone(),
                    window_id: window_id.to_string(),
                    generation,
                    close_after_completion: false,
                    observer_claimed: false,
                },
            );
        Ok(id)
    }

    pub fn claim_terminal_observer(
        &self,
        id: &TmuxWindowRunId,
        close_after_completion: bool,
    ) -> bool {
        let mut runs = self
            .window_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(run) = runs.get_mut(id) else {
            return false;
        };
        if run.observer_claimed {
            return false;
        }
        run.observer_claimed = true;
        run.close_after_completion = close_after_completion;
        true
    }

    pub fn release_terminal_observer(&self, id: &TmuxWindowRunId) {
        self.window_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
    }

    fn resolve_window_run(&self, id: &TmuxWindowRunId) -> Option<TmuxWindowRun> {
        self.window_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Returns whether the window belongs to a `tmux_run` launched in this scope.
    pub async fn owns_window_run(&self, work_scope: &WorkScope, window_id: &str) -> bool {
        if self
            .window_starts
            .read()
            .await
            .contains_key(&(work_scope.clone(), window_id.to_string()))
        {
            return true;
        }
        self.load_durable_evidence(work_scope, window_id)
            .await
            .ok()
            .flatten()
            .is_some()
    }

    /// Persist terminal evidence using the run's current scope alias.
    ///
    /// # Errors
    /// Returns an error when the identity is unknown, its generation changed,
    /// or the durable write fails.
    pub async fn record_run_terminal(
        &self,
        id: &TmuxWindowRunId,
        exit_code: Option<i32>,
        tail: String,
    ) -> Result<TmuxTerminalEvidence, TmuxError> {
        let run = self
            .resolve_window_run(id)
            .ok_or_else(|| TmuxError::InvalidEvidence {
                path: self.socket_dir.clone(),
                reason: "unknown tmux window run identity".to_string(),
            })?;
        self.record_window_terminal_for_generation(
            &run.scope,
            &run.window_id,
            &run.generation,
            exit_code,
            TmuxTerminalStatus::Exited,
            tail,
        )
        .await
    }

    /// Kill the run's pane only while its original server generation is current.
    ///
    /// # Errors
    /// Returns an error when the identity/scope is unavailable or tmux cannot be
    /// invoked.
    pub async fn cleanup_run_window(&self, id: &TmuxWindowRunId) -> Result<(), TmuxError> {
        let run = self
            .resolve_window_run(id)
            .ok_or_else(|| TmuxError::InvalidEvidence {
                path: self.socket_dir.clone(),
                reason: "unknown tmux window run identity".to_string(),
            })?;
        let entry =
            self.get_existing(&run.scope)
                .await
                .ok_or_else(|| TmuxError::InvalidEvidence {
                    path: self.socket_dir.clone(),
                    reason: "tmux run scope retired without replacement".to_string(),
                })?;
        let server = entry.read().await;
        if server.generation.as_deref() != Some(&run.generation) {
            return Ok(());
        }
        let output = tokio::process::Command::new("tmux")
            .args([
                "-S",
                &server.socket_path.to_string_lossy(),
                "kill-window",
                "-t",
                &run.window_id,
            ])
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .output()
            .await;
        if let Err(source) = output {
            return Err(TmuxError::ProbeFailed {
                socket_path: server.socket_path.clone(),
                source,
            });
        }
        Ok(())
    }

    fn digest_prefix_hex(digest: &[u8]) -> String {
        use std::fmt::Write as _;
        digest[..16]
            .iter()
            .fold(String::with_capacity(32), |mut encoded, byte| {
                write!(encoded, "{byte:02x}").expect("writing to String is infallible");
                encoded
            })
    }

    fn evidence_scope_dir(&self, work_scope: &WorkScope) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(work_scope.stable_key().as_bytes());
        let digest = hash.finalize();
        self.socket_dir
            .join(TERMINAL_EVIDENCE_DIR)
            .join(format!("scope-{}", Self::digest_prefix_hex(&digest)))
    }

    fn evidence_path(&self, work_scope: &WorkScope, window_id: &str) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(window_id.as_bytes());
        let digest = hash.finalize();
        self.evidence_scope_dir(work_scope)
            .join(format!("window-{}.json", Self::digest_prefix_hex(&digest)))
    }

    async fn durable_record(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        generation: &str,
        evidence: &TmuxTerminalEvidence,
        status: DurableEvidenceStatus,
    ) -> DurableTerminalEvidence {
        let socket_path = if let Some(entry) = self.get_existing(work_scope).await {
            entry.read().await.socket_path.clone()
        } else {
            self.derived_socket_path(work_scope)
        };
        DurableTerminalEvidence {
            version: TERMINAL_EVIDENCE_VERSION,
            socket_identity: socket_path.to_string_lossy().into_owned(),
            generation: generation.to_string(),
            window_id: window_id.to_string(),
            observed_at: evidence.observed_at.into(),
            status,
            exit_code: evidence.exit_code,
            duration_ms: evidence.duration_ms,
            tail: evidence.tail.clone(),
        }
    }

    async fn write_durable_evidence(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        record: &DurableTerminalEvidence,
    ) -> Result<(), TmuxError> {
        self.ensure_runtime_assets().await?;
        let path = self.evidence_path(work_scope, window_id);
        let parent = path.parent().expect("evidence path always has a parent");
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| TmuxError::EvidenceIo {
                path: parent.to_path_buf(),
                source,
            })?;
        #[cfg(unix)]
        tokio::fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .await
            .map_err(|source| TmuxError::EvidenceIo {
                path: parent.to_path_buf(),
                source,
            })?;

        let bytes = serde_json::to_vec(record).map_err(|source| TmuxError::InvalidEvidence {
            path: path.clone(),
            reason: source.to_string(),
        })?;
        let temp_path = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp_path)
            .await
            .map_err(|source| TmuxError::EvidenceIo {
                path: temp_path.clone(),
                source,
            })?;
        let write_result = async {
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&temp_path, &path).await?;
            let parent = parent.to_path_buf();
            tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
                .await
                .map_err(std::io::Error::other)??;
            Ok(())
        }
        .await;
        if let Err(source) = write_result {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(TmuxError::EvidenceIo { path, source });
        }
        Ok(())
    }

    async fn load_durable_evidence(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Result<Option<DurableTerminalEvidence>, TmuxError> {
        let path = self.evidence_path(work_scope, window_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(TmuxError::EvidenceIo { path, source }),
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|source| TmuxError::InvalidEvidence {
                path: path.clone(),
                reason: source.to_string(),
            })?;
        if value.get("version").and_then(serde_json::Value::as_u64)
            != Some(u64::from(TERMINAL_EVIDENCE_VERSION))
        {
            self.remove_evidence_file(work_scope, window_id).await?;
            return Ok(None);
        }
        let record: DurableTerminalEvidence =
            serde_json::from_value(value).map_err(|source| TmuxError::InvalidEvidence {
                path: path.clone(),
                reason: source.to_string(),
            })?;
        if record.window_id != window_id {
            return Err(TmuxError::InvalidEvidence {
                path,
                reason: "window identity mismatch".to_string(),
            });
        }
        let mut current_generation = self.current_generation(work_scope).await;
        if current_generation.is_none() && self.binary_available {
            let recorded_socket = PathBuf::from(&record.socket_identity);
            if probe(&recorded_socket)
                .await
                .map_err(|source| TmuxError::ProbeFailed {
                    socket_path: recorded_socket.clone(),
                    source,
                })?
                == ProbeResult::Live
            {
                let recovered = recover_or_install_generation(&recorded_socket).await?;
                let (entry, _) = self.get_or_insert(work_scope, recorded_socket).await;
                let mut server = entry.write().await;
                server.generation = Some(recovered.clone());
                server.status = ServerStatus::Live;
                current_generation = Some(recovered);
            }
        }
        if current_generation.as_deref() != Some(&record.generation) {
            self.remove_evidence_file(work_scope, window_id).await?;
            return Ok(None);
        }
        Ok(Some(record))
    }

    async fn current_generation(&self, work_scope: &WorkScope) -> Option<String> {
        let entry = self.get_existing(work_scope).await?;
        let generation = entry.read().await.generation.clone();
        generation
    }

    async fn remove_evidence_file(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Result<(), TmuxError> {
        let path = self.evidence_path(work_scope, window_id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(TmuxError::EvidenceIo { path, source }),
        }
    }

    fn terminal_evidence(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        exit_code: Option<i32>,
        status: TmuxTerminalStatus,
        tail: String,
    ) -> TmuxTerminalEvidence {
        let key = (work_scope.clone(), window_id.to_string());
        let duration_ms = self
            .window_starts
            .try_read()
            .ok()
            .and_then(|starts| starts.get(&key).cloned())
            .map_or(0, |start| {
                u64::try_from(start.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
            });
        TmuxTerminalEvidence {
            observed_at: SystemTime::now(),
            exit_code,
            status,
            duration_ms,
            tail,
        }
    }

    /// Persist terminal evidence before publishing it to process memory.
    ///
    /// # Errors
    /// Returns an error when runtime assets, serialization, or the atomic
    /// sidecar write cannot complete.
    pub async fn record_window_terminal(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        exit_code: Option<i32>,
        status: TmuxTerminalStatus,
        tail: String,
    ) -> Result<TmuxTerminalEvidence, TmuxError> {
        let generation = self.current_generation(work_scope).await.ok_or_else(|| {
            TmuxError::InvalidEvidence {
                path: self.evidence_path(work_scope, window_id),
                reason: "terminal evidence has no current server generation".to_string(),
            }
        })?;
        self.record_window_terminal_for_generation(
            work_scope,
            window_id,
            &generation,
            exit_code,
            status,
            tail,
        )
        .await
    }

    async fn record_window_terminal_for_generation(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        generation: &str,
        exit_code: Option<i32>,
        status: TmuxTerminalStatus,
        tail: String,
    ) -> Result<TmuxTerminalEvidence, TmuxError> {
        if self.current_generation(work_scope).await.as_deref() != Some(generation) {
            return Err(TmuxError::InvalidEvidence {
                path: self.evidence_path(work_scope, window_id),
                reason: "tmux server generation changed before evidence commit".to_string(),
            });
        }
        let key = (work_scope.clone(), window_id.to_string());
        if status == TmuxTerminalStatus::Killed {
            if let Some(existing) = self.terminal_evidence.read().await.get(&key).cloned() {
                if existing.status == TmuxTerminalStatus::Exited {
                    return Ok(existing);
                }
            }
            if let Some(record) = self.load_durable_evidence(work_scope, window_id).await? {
                if record.status == DurableEvidenceStatus::Exited {
                    if let Some(existing) = record.terminal_evidence() {
                        self.terminal_evidence
                            .write()
                            .await
                            .insert(key, existing.clone());
                        return Ok(existing);
                    }
                }
            }
        }
        let duration_ms = self
            .window_starts
            .read()
            .await
            .get(&key)
            .map_or(0, |start| {
                u64::try_from(start.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
            });
        let evidence = TmuxTerminalEvidence {
            observed_at: SystemTime::now(),
            exit_code,
            status,
            duration_ms,
            tail,
        };
        let durable_status = match status {
            TmuxTerminalStatus::Exited => DurableEvidenceStatus::Exited,
            TmuxTerminalStatus::Killed => DurableEvidenceStatus::Killed,
        };
        let record = self
            .durable_record(work_scope, window_id, generation, &evidence, durable_status)
            .await;
        self.write_durable_evidence(work_scope, window_id, &record)
            .await?;
        self.terminal_evidence
            .write()
            .await
            .insert(key, evidence.clone());
        Ok(evidence)
    }

    /// Persist a non-terminal kill intent before invoking `kill-window`.
    ///
    /// # Errors
    /// Returns an error when the durable intent cannot be written atomically.
    pub async fn prepare_window_kill(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
        tail: String,
    ) -> Result<(), TmuxError> {
        let evidence = self.terminal_evidence(
            work_scope,
            window_id,
            None,
            TmuxTerminalStatus::Killed,
            tail,
        );
        let generation = self.current_generation(work_scope).await.ok_or_else(|| {
            TmuxError::InvalidEvidence {
                path: self.evidence_path(work_scope, window_id),
                reason: "kill intent has no current server generation".to_string(),
            }
        })?;
        let record = self
            .durable_record(
                work_scope,
                window_id,
                &generation,
                &evidence,
                DurableEvidenceStatus::KillPending,
            )
            .await;
        self.write_durable_evidence(work_scope, window_id, &record)
            .await
    }

    /// Clear a failed kill's pending intent, restoring prior terminal evidence.
    ///
    /// # Errors
    /// Returns an error when evidence cannot be read, restored, or removed.
    pub async fn abort_window_kill(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Result<(), TmuxError> {
        let Some(record) = self.load_durable_evidence(work_scope, window_id).await? else {
            return Ok(());
        };
        if record.status != DurableEvidenceStatus::KillPending {
            return Ok(());
        }
        let key = (work_scope.clone(), window_id.to_string());
        if let Some(evidence) = self.terminal_evidence.read().await.get(&key).cloned() {
            let status = match evidence.status {
                TmuxTerminalStatus::Exited => DurableEvidenceStatus::Exited,
                TmuxTerminalStatus::Killed => DurableEvidenceStatus::Killed,
            };
            let generation = self.current_generation(work_scope).await.ok_or_else(|| {
                TmuxError::InvalidEvidence {
                    path: self.evidence_path(work_scope, window_id),
                    reason: "kill abort has no current server generation".to_string(),
                }
            })?;
            let record = self
                .durable_record(work_scope, window_id, &generation, &evidence, status)
                .await;
            return self
                .write_durable_evidence(work_scope, window_id, &record)
                .await;
        }
        let path = self.evidence_path(work_scope, window_id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(TmuxError::EvidenceIo { path, source }),
        }
    }

    async fn recover_live_entry_for_inspection(
        &self,
        work_scope: &WorkScope,
    ) -> Result<(), TmuxError> {
        if self.get_existing(work_scope).await.is_some() || !self.binary_available {
            return Ok(());
        }
        let socket = self.derived_socket_path(work_scope);
        if probe(&socket)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: socket.clone(),
                source,
            })?
            != ProbeResult::Live
        {
            return Ok(());
        }
        let generation = recover_or_install_generation(&socket).await?;
        let (entry, _) = self.get_or_insert(work_scope, socket).await;
        let mut server = entry.write().await;
        server.generation = Some(generation);
        server.status = ServerStatus::Live;
        Ok(())
    }

    /// Inspect memory, then validated durable evidence, then the pane. A pending
    /// kill plus an absent pane is finalized as Killed, closing the crash window
    /// between a successful `kill-window` and its confirmation write.
    ///
    /// # Errors
    /// Returns an error when a sidecar is corrupt, mismatched, unreadable, or a
    /// newly discovered terminal marker cannot be persisted.
    pub async fn inspect_window(
        &self,
        work_scope: &WorkScope,
        window_id: &str,
    ) -> Result<TmuxWindowInspection, TmuxError> {
        self.recover_live_entry_for_inspection(work_scope).await?;
        let key = (work_scope.clone(), window_id.to_string());
        if let Some(evidence) = self.terminal_evidence.read().await.get(&key).cloned() {
            return Ok(TmuxWindowInspection::Terminal(evidence));
        }
        let durable = self.load_durable_evidence(work_scope, window_id).await?;
        if let Some(evidence) = durable
            .as_ref()
            .and_then(DurableTerminalEvidence::terminal_evidence)
        {
            self.terminal_evidence
                .write()
                .await
                .insert(key, evidence.clone());
            return Ok(TmuxWindowInspection::Terminal(evidence));
        }
        if !self.binary_available {
            return Ok(TmuxWindowInspection::Missing);
        }
        // A rekeyed entry retains the conversation-scoped socket on which the
        // server was created. Prefer that stored identity; deriving from the new
        // worktree scope would probe a different server and falsely report Missing.
        let socket_path = if let Some(entry) = self.get_existing(work_scope).await {
            entry.read().await.socket_path.clone()
        } else if let Some(record) = durable.as_ref() {
            PathBuf::from(&record.socket_identity)
        } else {
            self.derived_socket_path(work_scope)
        };
        let output = tokio::process::Command::new("tmux")
            .args([
                "-S",
                &socket_path.to_string_lossy(),
                "capture-pane",
                "-p",
                "-t",
                window_id,
                "-S",
                "-2000",
            ])
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .output()
            .await;
        let pane_missing = !matches!(&output, Ok(output) if output.status.success());
        if pane_missing {
            if let Some(pending) =
                durable.filter(|record| record.status == DurableEvidenceStatus::KillPending)
            {
                let evidence = self
                    .record_window_terminal(
                        work_scope,
                        window_id,
                        None,
                        TmuxTerminalStatus::Killed,
                        pending.tail,
                    )
                    .await?;
                return Ok(TmuxWindowInspection::Terminal(evidence));
            }
            return Ok(TmuxWindowInspection::Missing);
        }
        let Ok(output) = output else {
            unreachable!("pane_missing handled the subprocess error")
        };
        let tail = String::from_utf8_lossy(&output.stdout).into_owned();
        if let Some(exit_code) = parse_tmux_run_exit_marker(&tail) {
            let evidence = self
                .record_window_terminal(
                    work_scope,
                    window_id,
                    Some(exit_code),
                    TmuxTerminalStatus::Exited,
                    tail,
                )
                .await?;
            Ok(TmuxWindowInspection::Terminal(evidence))
        } else {
            Ok(TmuxWindowInspection::Live)
        }
    }

    /// Verify registry and durable-evidence destinations before approval starts
    /// publishing aliases. This method does not mutate either store.
    ///
    /// # Errors
    /// Returns an evidence I/O or destination-collision error.
    pub async fn preflight_rekey_scope(
        &self,
        old: &WorkScope,
        new: &WorkScope,
    ) -> Result<(), TmuxError> {
        if old == new {
            return Ok(());
        }
        {
            let map = self.inner.read().await;
            if let (Some(old_entry), Some(new_entry)) =
                (map.get(&old.stable_key()), map.get(&new.stable_key()))
            {
                if !Arc::ptr_eq(old_entry, new_entry) {
                    return Err(TmuxError::InvalidEvidence {
                        path: self.evidence_scope_dir(new),
                        reason: "destination tmux scope already occupied".to_string(),
                    });
                }
            }
        }
        let old_dir = self.evidence_scope_dir(old);
        let new_dir = self.evidence_scope_dir(new);
        let old_exists =
            tokio::fs::try_exists(&old_dir)
                .await
                .map_err(|source| TmuxError::EvidenceIo {
                    path: old_dir,
                    source,
                })?;
        let new_exists =
            tokio::fs::try_exists(&new_dir)
                .await
                .map_err(|source| TmuxError::EvidenceIo {
                    path: new_dir.clone(),
                    source,
                })?;
        if old_exists && new_exists {
            return Err(TmuxError::InvalidEvidence {
                path: new_dir,
                reason: "destination evidence scope already exists".to_string(),
            });
        }
        Ok(())
    }

    /// Remove a destination alias only when it still aliases `old`.
    pub async fn rollback_alias_scope(&self, old: &WorkScope, new: &WorkScope) {
        if old == new {
            return;
        }
        let mut map = self.inner.write().await;
        let old_key = old.stable_key();
        let new_key = new.stable_key();
        let should_remove = match (map.get(&old_key), map.get(&new_key)) {
            (Some(old_entry), Some(new_entry)) => Arc::ptr_eq(old_entry, new_entry),
            _ => false,
        };
        if should_remove {
            map.remove(&new_key);
        }
    }

    /// Add `new` as an alias of `old`, preserving the stored socket identity.
    /// Concurrent inspectors can therefore use either scope throughout the
    /// durable wake-contract update.
    pub async fn alias_scope(&self, old: &WorkScope, new: &WorkScope) -> bool {
        if old == new {
            return true;
        }
        let old_key = old.stable_key();
        let new_key = new.stable_key();
        let mut map = self.inner.write().await;
        let Some(entry) = map.get(&old_key).cloned() else {
            return true;
        };
        if let Some(destination) = map.get(&new_key) {
            return Arc::ptr_eq(destination, &entry);
        }
        map.insert(new_key, entry);
        true
    }

    /// Move durable and in-memory per-window identity to the destination alias.
    /// This runs while both server lookup aliases exist, so an observer using
    /// either contract scope still reaches the same socket.
    ///
    /// # Errors
    /// Returns an error when durable evidence cannot be moved or the destination
    /// already contains evidence for a different resource identity.
    pub async fn rekey_window_evidence(
        &self,
        old: &WorkScope,
        new: &WorkScope,
    ) -> Result<(), TmuxError> {
        if old == new {
            return Ok(());
        }
        let old_dir = self.evidence_scope_dir(old);
        let new_dir = self.evidence_scope_dir(new);
        if tokio::fs::try_exists(&old_dir)
            .await
            .map_err(|source| TmuxError::EvidenceIo {
                path: old_dir.clone(),
                source,
            })?
        {
            if tokio::fs::try_exists(&new_dir)
                .await
                .map_err(|source| TmuxError::EvidenceIo {
                    path: new_dir.clone(),
                    source,
                })?
            {
                return Err(TmuxError::InvalidEvidence {
                    path: new_dir,
                    reason: "destination evidence scope already exists".to_string(),
                });
            }
            tokio::fs::rename(&old_dir, &new_dir)
                .await
                .map_err(|source| TmuxError::EvidenceIo {
                    path: old_dir,
                    source,
                })?;
        }
        {
            let mut runs = self
                .window_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for run in runs.values_mut().filter(|run| &run.scope == old) {
                run.scope = new.clone();
            }
        }
        let mut evidence = self.terminal_evidence.write().await;
        let moved: Vec<_> = evidence
            .iter()
            .filter(|((scope, _), _)| scope == old)
            .map(|((_, window), value)| (window.clone(), value.clone()))
            .collect();
        for (window, value) in moved {
            evidence.remove(&(old.clone(), window.clone()));
            evidence.insert((new.clone(), window), value);
        }
        drop(evidence);
        let mut starts = self.window_starts.write().await;
        let windows: Vec<_> = starts
            .keys()
            .filter(|(scope, _)| scope == old)
            .map(|(_, window)| window.clone())
            .collect();
        for window in windows {
            if let Some(value) = starts.remove(&(old.clone(), window.clone())) {
                starts.insert((new.clone(), window), value);
            }
        }
        Ok(())
    }

    /// Reverse a completed evidence rekey. Used when the durable approval
    /// transaction fails after aliases and evidence have been published.
    ///
    /// # Errors
    /// Returns an error if the evidence cannot be restored to the old scope.
    pub async fn rollback_window_evidence(
        &self,
        old: &WorkScope,
        new: &WorkScope,
    ) -> Result<(), TmuxError> {
        self.rekey_window_evidence(new, old).await
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
        if let (Some(old_entry), Some(new_entry)) = (map.get(&old_key), map.get(&new_key)) {
            if Arc::ptr_eq(old_entry, new_entry) {
                let Some(entry) = map.remove(&old_key) else {
                    return false;
                };
                entry.write().await.work_scope = new.clone();
                return true;
            }
            tracing::warn!(
                old = %old,
                new = %new,
                "tmux: refusing to rekey server entry — destination scope already occupied; leaving both entries in place"
            );
            return false;
        }
        let Some(entry) = map.remove(&old_key) else {
            return false;
        };
        entry.write().await.work_scope = new.clone();
        map.insert(new_key, entry);
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

        let evidence_dir = self.evidence_scope_dir(work_scope);
        if let Err(error) = tokio::fs::remove_dir_all(&evidence_dir).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::error!(
                    work_scope = %work_scope,
                    path = %evidence_dir.display(),
                    %error,
                    "tmux: failed to remove durable terminal evidence during scope cleanup"
                );
            }
        }
        self.window_starts
            .write()
            .await
            .retain(|(scope, _), _| scope != work_scope);
        self.terminal_evidence
            .write()
            .await
            .retain(|(scope, _), _| scope != work_scope);

        self.window_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, run| &run.scope != work_scope);

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
fn set_tmux_server_env(cmd: &mut tokio::process::Command) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    cmd.env_clear();
    cmd.envs(phoenix_terminal::spawn::build_env_for_tmux(&shell));
    // Stamp the companion version so a later reuse can tell a current server
    // (no-op) from a pre-feature/older one that needs a refresh.
    cmd.env(COMPANION_VERSION_VAR, COMPANION_ENV_VERSION);
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

async fn recover_or_install_generation(socket_path: &Path) -> Result<String, TmuxError> {
    if let Some(generation) = tmux_global_env(socket_path, SERVER_GENERATION_VAR).await {
        return Ok(generation);
    }
    let generation = uuid::Uuid::new_v4().to_string();
    let output = tokio::process::Command::new("tmux")
        .args([
            "-S",
            &socket_path.to_string_lossy(),
            "set-environment",
            "-g",
            SERVER_GENERATION_VAR,
            &generation,
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|source| TmuxError::ProbeFailed {
            socket_path: socket_path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!(
                "failed to install server generation: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(generation)
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
    set_tmux_server_env(&mut cmd);
    cmd.env(SERVER_GENERATION_VAR, generation);
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

    async fn live_registry(tmp: &TempDir, scope: &WorkScope) -> TmuxRegistry {
        let registry = TmuxRegistry::with_socket_dir(tmp.path().to_path_buf());
        registry.ensure_live(scope, tmp.path()).await.unwrap();
        registry
    }

    async fn install_test_generation(registry: &TmuxRegistry, scope: &WorkScope, generation: &str) {
        registry
            .install_generation_for_test(scope, generation)
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
    async fn preflight_rejects_evidence_collision_without_publishing_alias() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("preflight-old".into());
        let new = WorkScope::Worktree("/tmp/preflight-new".into());
        let socket = socket_path_for(tmp.path(), "preflight-old");
        registry.get_or_insert(&old, socket).await;
        tokio::fs::create_dir_all(registry.evidence_scope_dir(&old))
            .await
            .unwrap();
        tokio::fs::create_dir_all(registry.evidence_scope_dir(&new))
            .await
            .unwrap();

        assert!(registry.preflight_rekey_scope(&old, &new).await.is_err());
        assert!(registry.get_existing(&old).await.is_some());
        assert!(registry.get_existing(&new).await.is_none());
        assert!(tokio::fs::try_exists(registry.evidence_scope_dir(&old))
            .await
            .unwrap());
        assert!(tokio::fs::try_exists(registry.evidence_scope_dir(&new))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn evidence_and_alias_rollback_restores_old_only_scope() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("rollback-old".into());
        let new = WorkScope::Worktree("/tmp/rollback-new".into());
        let socket = socket_path_for(tmp.path(), "rollback-old");
        registry.get_or_insert(&old, socket).await;
        tokio::fs::create_dir_all(registry.evidence_scope_dir(&old))
            .await
            .unwrap();

        assert!(registry.alias_scope(&old, &new).await);
        registry.rekey_window_evidence(&old, &new).await.unwrap();
        registry.rollback_window_evidence(&old, &new).await.unwrap();
        registry.rollback_alias_scope(&old, &new).await;

        assert!(registry.get_existing(&old).await.is_some());
        assert!(registry.get_existing(&new).await.is_none());
        assert!(tokio::fs::try_exists(registry.evidence_scope_dir(&old))
            .await
            .unwrap());
        assert!(!tokio::fs::try_exists(registry.evidence_scope_dir(&new))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn active_run_identity_follows_rekey_for_evidence_and_cleanup() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let old = WorkScope::Conversation("pre-approval".into());
        let new = WorkScope::Worktree("/tmp/approved".into());
        install_test_generation(&registry, &old, "generation-a").await;
        let run = registry.register_window_start(&old, "@9").await.unwrap();
        assert!(registry.claim_terminal_observer(&run, true));

        assert!(registry.alias_scope(&old, &new).await);
        registry.rekey_window_evidence(&old, &new).await.unwrap();
        assert!(registry.rekey_scope(&old, &new).await);
        registry
            .record_run_terminal(&run, Some(0), "after approval".into())
            .await
            .unwrap();

        assert!(
            matches!(registry.inspect_window(&new, "@9").await, Ok(TmuxWindowInspection::Terminal(ref evidence)) if evidence.status == TmuxTerminalStatus::Exited)
        );
        assert!(
            registry.get_existing(&old).await.is_none(),
            "old scope alias is retired"
        );
        registry.cleanup_run_window(&run).await.unwrap();
        assert!(
            matches!(
                registry.inspect_window(&new, "@9").await,
                Ok(TmuxWindowInspection::Terminal(_))
            ),
            "cleanup cannot erase durable new-scope evidence"
        );
        registry.release_terminal_observer(&run);
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
    async fn terminal_and_killed_evidence_survive_restart_without_tmux() {
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("durable-evidence".to_string());
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        install_test_generation(&registry, &scope, "test-generation").await;

        registry
            .record_window_terminal(
                &scope,
                "@../escaped",
                Some(7),
                TmuxTerminalStatus::Exited,
                "final tail".to_string(),
            )
            .await
            .unwrap();
        registry
            .record_window_terminal(
                &scope,
                "@killed",
                None,
                TmuxTerminalStatus::Killed,
                "killed tail".to_string(),
            )
            .await
            .unwrap();

        let restarted = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        install_test_generation(&restarted, &scope, "test-generation").await;
        let Ok(TmuxWindowInspection::Terminal(exited)) =
            restarted.inspect_window(&scope, "@../escaped").await
        else {
            panic!("exited evidence must survive restart with no pane capability");
        };
        assert_eq!(exited.status, TmuxTerminalStatus::Exited);
        assert_eq!(exited.exit_code, Some(7));
        assert_eq!(exited.tail, "final tail");
        let Ok(TmuxWindowInspection::Terminal(killed)) =
            restarted.inspect_window(&scope, "@killed").await
        else {
            panic!("killed evidence must survive restart with no pane capability");
        };
        assert_eq!(killed.status, TmuxTerminalStatus::Killed);

        let files: Vec<_> = std::fs::read_dir(restarted.evidence_scope_dir(&scope))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(files.iter().all(|name| {
            name.starts_with("window-")
                && Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        }));
        assert!(!tmp.path().join("escaped").exists());
        #[cfg(unix)]
        for file in files {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(restarted.evidence_scope_dir(&scope).join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn phoenix_restart_recovers_generation_and_accepts_evidence() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("generation-restart".into());
        let registry = live_registry(&tmp, &scope).await;
        let generation = registry.current_generation(&scope).await.unwrap();
        registry.register_window_start(&scope, "@77").await.unwrap();
        registry
            .record_window_terminal(
                &scope,
                "@77",
                Some(0),
                TmuxTerminalStatus::Exited,
                "done".into(),
            )
            .await
            .unwrap();

        let restarted = TmuxRegistry::with_socket_dir(tmp.path().to_path_buf());
        assert!(matches!(
            restarted.inspect_window(&scope, "@77").await,
            Ok(TmuxWindowInspection::Terminal(_))
        ));
        assert_eq!(
            restarted.current_generation(&scope).await.as_deref(),
            Some(generation.as_str())
        );
        kill_socket(&socket_path_for(tmp.path(), "generation-restart")).await;
    }

    #[tokio::test]
    async fn stale_v1_and_generation_mismatch_are_cache_misses() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("stale-generation".into());
        let registry = live_registry(&tmp, &scope).await;
        let path = registry.evidence_path(&scope, "@missing");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"version":1}"#).unwrap();
        assert!(matches!(
            registry.inspect_window(&scope, "@missing").await,
            Ok(TmuxWindowInspection::Missing)
        ));
        assert!(!path.exists());

        let evidence = registry.terminal_evidence(
            &scope,
            "@missing",
            None,
            TmuxTerminalStatus::Killed,
            "pending".into(),
        );
        let stale = registry
            .durable_record(
                &scope,
                "@missing",
                "different-generation",
                &evidence,
                DurableEvidenceStatus::KillPending,
            )
            .await;
        registry
            .write_durable_evidence(&scope, "@missing", &stale)
            .await
            .unwrap();
        assert!(matches!(
            registry.inspect_window(&scope, "@missing").await,
            Ok(TmuxWindowInspection::Missing)
        ));
        assert!(!path.exists(), "mismatched KillPending must not finalize");
        kill_socket(&socket_path_for(tmp.path(), "stale-generation")).await;
    }

    #[tokio::test]
    async fn register_reused_window_clears_current_generation_evidence() {
        if which::which("tmux").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("reuse-window".into());
        let registry = live_registry(&tmp, &scope).await;
        registry
            .record_window_terminal(
                &scope,
                "@1",
                Some(9),
                TmuxTerminalStatus::Exited,
                "old".into(),
            )
            .await
            .unwrap();
        assert!(registry.evidence_path(&scope, "@1").exists());
        registry.register_window_start(&scope, "@1").await.unwrap();
        assert!(!registry.evidence_path(&scope, "@1").exists());
        assert!(!registry
            .terminal_evidence
            .read()
            .await
            .contains_key(&(scope.clone(), "@1".into())));
        kill_socket(&socket_path_for(tmp.path(), "reuse-window")).await;
    }

    #[tokio::test]
    async fn exited_evidence_is_monotonic_over_cleanup_killed() {
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("monotonic-evidence".to_string());
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        install_test_generation(&registry, &scope, "test-generation").await;
        registry
            .record_window_terminal(
                &scope,
                "@1",
                Some(3),
                TmuxTerminalStatus::Exited,
                "authentic exit tail".to_string(),
            )
            .await
            .unwrap();
        let evidence = registry
            .record_window_terminal(
                &scope,
                "@1",
                None,
                TmuxTerminalStatus::Killed,
                "cleanup replacement".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(evidence.status, TmuxTerminalStatus::Exited);
        assert_eq!(evidence.exit_code, Some(3));
        assert_eq!(evidence.tail, "authentic exit tail");
    }

    #[tokio::test]
    async fn evidence_write_failure_is_surfaced_and_not_published_in_memory() {
        let tmp = TempDir::new().unwrap();
        let blocked = tmp.path().join("not-a-directory");
        std::fs::write(&blocked, b"blocked").unwrap();
        let scope = WorkScope::Conversation("write-failure".to_string());
        let registry = TmuxRegistry::with_socket_dir_and_binary(blocked, false);

        assert!(registry
            .record_window_terminal(
                &scope,
                "@1",
                Some(0),
                TmuxTerminalStatus::Exited,
                "must not publish".to_string(),
            )
            .await
            .is_err());
        assert!(registry.terminal_evidence.read().await.is_empty());
    }

    #[tokio::test]
    async fn corrupt_evidence_is_a_capability_error_not_missing() {
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Conversation("corrupt-evidence".to_string());
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        registry.ensure_runtime_assets().await.unwrap();
        let path = registry.evidence_path(&scope, "@1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not-json").unwrap();

        assert!(matches!(
            registry.inspect_window(&scope, "@1").await,
            Err(TmuxError::InvalidEvidence { .. })
        ));
    }

    #[tokio::test]
    async fn cleanup_deletes_evidence_but_same_scope_continuation_preserves_it() {
        let tmp = TempDir::new().unwrap();
        let scope = WorkScope::Worktree("/tmp/durable-scope".to_string());
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        install_test_generation(&registry, &scope, "test-generation").await;
        registry
            .record_window_terminal(
                &scope,
                "@1",
                Some(0),
                TmuxTerminalStatus::Exited,
                "done".to_string(),
            )
            .await
            .unwrap();
        let evidence_dir = registry.evidence_scope_dir(&scope);
        assert!(evidence_dir.exists());

        registry.cascade_on_delete(&scope, Some(&scope)).await;
        assert!(evidence_dir.exists(), "continuation must preserve evidence");
        assert!(matches!(
            registry.inspect_window(&scope, "@1").await,
            Ok(TmuxWindowInspection::Terminal(_))
        ));

        registry.cascade_on_delete(&scope, None).await;
        assert!(!evidence_dir.exists(), "hard cleanup must delete evidence");
        assert!(matches!(
            registry.inspect_window(&scope, "@1").await,
            Ok(TmuxWindowInspection::Missing)
        ));
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
