//! Per-`ResourceScopeKey` tmux server registry.
//!
//! REQ-TMUX-001 (socket isolation), REQ-TMUX-002 (lazy spawn),
//! REQ-TMUX-005 (Phoenix-restart probe re-use), REQ-TMUX-006
//! (stale-socket detection), REQ-TMUX-007 (hard-delete cascade),
//! REQ-TMUX-013 (`ToolContext` accessor shape),
//! REQ-TMUX-WS-001 (`ResourceScopeKey` ownership).
//!
//! Lifetime: registries live in process memory only. The tmux servers
//! themselves are owned by the OS and survive Phoenix restart; the in-
//! memory `TmuxServer` entry is rebuilt on the first operation after
//! restart by probing the socket.
//!
//! Both the `HashMap` entry and socket path are keyed by `ResourceScopeKey`.
//! Conversation resources use their persisted durable work-scope ID, so every
//! continuation in that scope shares the same tmux session. The global terminal
//! occupies a structurally separate namespace.
//!
//! Lock ordering is outer registry map, then per-scope entry. Operations that
//! only need an entry clone drop the map lock before acquiring the entry lock.
//! Authority-changing operations that must prove map membership atomically hold
//! the map lock while acquiring the entry lock; no path may acquire the map
//! while holding an entry lock. The entry write lock serialises concurrent
//! `ensure_live` calls on the same `ResourceScopeKey`.

use base64::Engine;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use phoenix_core::process_identity::{
    current_process_identity, process_identity_matches, ProcessIdentity,
};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::ResourceScopeKey;

use thiserror::Error;
use tokio::sync::{OnceCell, RwLock};

use super::{
    parse_last_exit_marker,
    probe::{command_output, probe, probe_until, ProbeResult},
};

fn ambiguous_socket_probe(path: &Path) -> TmuxError {
    TmuxError::AmbiguousSocketIdentity {
        reason: format!(
            "endpoint {} exists but its server liveness probe failed",
            path.display()
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SocketFileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreTeardownSocket {
    Incarnation(SocketFileIdentity),
    Absent,
    NotSocket,
}

fn pre_teardown_socket(path: &Path) -> std::io::Result<PreTeardownSocket> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            Ok(PreTeardownSocket::Incarnation(SocketFileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }))
        }
        Ok(_) => Ok(PreTeardownSocket::NotSocket),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PreTeardownSocket::Absent),
        Err(error) => Err(error),
    }
}

fn socket_file_identity(path: &Path) -> std::io::Result<Option<SocketFileIdentity>> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => Ok(Some(SocketFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn endpoint_is_definitely_not_socket(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        std::fs::symlink_metadata(path)
            .map(|metadata| !metadata.file_type().is_socket())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn select_socket_from_probes(
    current: PathBuf,
    legacy: PathBuf,
    legacy_probe: ProbeResult,
) -> Result<PathBuf, TmuxError> {
    match legacy_probe {
        ProbeResult::Live => Ok(legacy),
        ProbeResult::NoSocket | ProbeResult::NoServer => Ok(current),
        ProbeResult::DeadSocket => Err(ambiguous_socket_probe(&legacy)),
    }
}

/// Default session name created on lazy spawn (REQ-TMUX-002 /
/// `TMUX_DEFAULT_SESSION`).
pub const TMUX_DEFAULT_SESSION: &str = "main";

// Bound on the post-spawn pane-readiness poll: 50 * 100ms = 5s ceiling.
// Conservative — under normal load the pane is ready on the first probe.
const PANE_READY_MAX_ATTEMPTS: u32 = 50;
const PANE_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

const TMUX_CLOSE_DEADLINE: Duration = Duration::from_secs(2);
const TMUX_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[must_use]
pub fn close_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + TMUX_CLOSE_DEADLINE
}

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

/// Errors surfaced by the tmux registry. The tmux tool translates these
/// into the stable error envelope on the agent's response.
#[derive(Debug, Error)]
pub enum TmuxError {
    #[error("the tmux binary is not installed on this host")]
    BinaryUnavailable,

    #[error("tmux retirement is in progress for {work_scope}; ensure_live is fenced until repair reopens admission")]
    RetirementFenced { work_scope: ResourceScopeKey },

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

    #[error("tmux socket identity is ambiguous: {reason}")]
    AmbiguousSocketIdentity { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxServerInstanceIdentity {
    pub socket_path: PathBuf,
    pub server_token: String,
}

impl TmuxServerInstanceIdentity {
    #[must_use]
    pub fn stable_identity(&self) -> String {
        #[cfg(unix)]
        let socket_path = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(self.socket_path.as_os_str().as_bytes());
        #[cfg(not(unix))]
        let socket_path = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(self.socket_path.to_string_lossy().as_bytes());
        format!("tmux-v1:{socket_path}:{}", self.server_token)
    }

    #[must_use]
    pub fn parse_stable_identity(value: &str) -> Option<Self> {
        let value = value.strip_prefix("tmux-v1:")?;
        let (encoded_path, server_token) = value.rsplit_once(':')?;
        let server_token = uuid::Uuid::parse_str(server_token).ok()?.to_string();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_path)
            .ok()?;
        #[cfg(unix)]
        let socket_path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
        #[cfg(not(unix))]
        let socket_path = PathBuf::from(String::from_utf8(bytes).ok()?);
        socket_path.is_absolute().then_some(Self {
            socket_path,
            server_token,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxRetirementGeneration(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TmuxRetirementAuthority {
    ExactServer,
    ServerAbsenceVerified,
    EndpointAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExactProcessState {
    Live,
    DeadOrReused,
    Unproven,
}

#[cfg(target_os = "linux")]
fn process_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rfind(')')
        .and_then(|close| stat.get(close + 1..))
        .and_then(|tail| tail.split_whitespace().next())
        == Some("Z")
}

#[cfg(target_os = "macos")]
fn process_is_zombie(pid: u32) -> bool {
    let Ok(pid) = libc::c_int::try_from(pid) else {
        return false;
    };
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let Ok(size) = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()) else {
        return false;
    };
    let rc = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast::<libc::c_void>(),
            size,
        )
    };
    rc == size && info.pbi_status == libc::SZOMB
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_is_zombie(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn wait_process_exit(expected: ProcessIdentity, deadline: std::time::Instant) -> ExactProcessState {
    let Ok(pid) = libc::pid_t::try_from(expected.pid) else {
        return ExactProcessState::Unproven;
    };
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return exact_process_state(expected);
    }
    let Ok(fd) = libc::c_int::try_from(fd) else {
        return ExactProcessState::Unproven;
    };
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = deadline.saturating_duration_since(std::time::Instant::now());
    let millis =
        libc::c_int::try_from(timeout.as_millis().min(i32::MAX as u128)).unwrap_or(i32::MAX);
    let ready = unsafe { libc::poll(std::ptr::addr_of_mut!(poll_fd), 1, millis) };
    unsafe { libc::close(fd) };
    if ready > 0 && poll_fd.revents & libc::POLLIN != 0 {
        ExactProcessState::DeadOrReused
    } else {
        exact_process_state(expected)
    }
}

#[cfg(target_os = "macos")]
fn wait_process_exit(expected: ProcessIdentity, deadline: std::time::Instant) -> ExactProcessState {
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return ExactProcessState::Unproven;
    }
    let mut event = libc::kevent {
        ident: expected.pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let timeout = deadline.saturating_duration_since(std::time::Instant::now());
    let deadline = libc::timespec {
        tv_sec: timeout.as_secs().min(i64::MAX as u64).cast_signed(),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    let ready = unsafe {
        libc::kevent(
            queue,
            std::ptr::addr_of_mut!(event),
            1,
            std::ptr::addr_of_mut!(event),
            1,
            std::ptr::addr_of!(deadline),
        )
    };
    unsafe { libc::close(queue) };
    if ready > 0 && event.fflags & libc::NOTE_EXIT != 0 {
        ExactProcessState::DeadOrReused
    } else {
        exact_process_state(expected)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn wait_process_exit(
    expected: ProcessIdentity,
    _deadline: std::time::Instant,
) -> ExactProcessState {
    exact_process_state(expected)
}

async fn exact_process_exit_until(
    expected: ProcessIdentity,
    expires: tokio::time::Instant,
) -> ExactProcessState {
    let remaining = expires.saturating_duration_since(tokio::time::Instant::now());
    let deadline = std::time::Instant::now() + remaining;
    match tokio::time::timeout_at(
        expires,
        tokio::task::spawn_blocking(move || wait_process_exit(expected, deadline)),
    )
    .await
    {
        Ok(Ok(state)) => state,
        _ => ExactProcessState::Unproven,
    }
}

fn exact_process_state(expected: ProcessIdentity) -> ExactProcessState {
    if process_identity_matches(expected) {
        return if process_is_zombie(expected.pid) {
            ExactProcessState::DeadOrReused
        } else {
            ExactProcessState::Live
        };
    }
    if current_process_identity(expected.pid).is_some() {
        return ExactProcessState::DeadOrReused;
    }
    let Ok(pid) = i32::try_from(expected.pid) else {
        return ExactProcessState::Unproven;
    };
    match unsafe { libc::kill(pid, 0) } {
        0 => ExactProcessState::Unproven,
        _ => match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => ExactProcessState::DeadOrReused,
            _ => ExactProcessState::Unproven,
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TmuxRetirementPermit {
    pub work_scope: ResourceScopeKey,
    pub instance: TmuxServerInstanceIdentity,
    generation: TmuxRetirementGeneration,
    authority: TmuxRetirementAuthority,
    exact_process: Option<ProcessIdentity>,
    had_entry: bool,
    expires: tokio::time::Instant,
}

impl TmuxRetirementPermit {
    #[must_use]
    pub fn generation(&self) -> TmuxRetirementGeneration {
        self.generation
    }

    #[must_use]
    pub fn had_entry(&self) -> bool {
        self.had_entry
    }
}

#[derive(Debug)]
pub struct TmuxRetirementCancellationError {
    reason: String,
    permit: TmuxRetirementPermit,
}

impl TmuxRetirementCancellationError {
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    #[must_use]
    pub fn into_permit(self) -> TmuxRetirementPermit {
        self.permit
    }
}

impl std::fmt::Display for TmuxRetirementCancellationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for TmuxRetirementCancellationError {}

#[derive(Debug)]
pub struct TmuxRetirementBatchCancellationError {
    reason: String,
    permits: Vec<TmuxRetirementPermit>,
}

impl TmuxRetirementBatchCancellationError {
    #[must_use]
    pub fn into_permits(self) -> Vec<TmuxRetirementPermit> {
        self.permits
    }
}

impl std::fmt::Display for TmuxRetirementBatchCancellationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for TmuxRetirementBatchCancellationError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "proof_kind", rename_all = "snake_case")]
pub enum TmuxRetirementOutcome {
    Retired,
    AbsenceVerified,
    IdentityNotProven { reason: String },
    RemovalFailed { reason: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum TmuxRetirementRehydration {
    Permit(TmuxRetirementPermit),
    AbsenceVerified,
    Residual { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExactTmuxIdentityState {
    Live,
    Absent,
    Ambiguous { reason: String },
}

#[derive(Debug, PartialEq, Eq)]
enum ExactShutdownObservation {
    Complete,
    Outstanding { reason: String },
    IdentityNotProven { reason: String },
}

/// Lifecycle state of a per-`ResourceScopeKey` tmux server.
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

/// Per-`ResourceScopeKey` tmux server entity. One per scope that has ever
/// performed a tmux operation; scopes that never use tmux have no entry.
///
/// `socket_path` is computed once at entry creation and is stable for
/// the entry's lifetime (REQ-TMUX-001 / `SocketPathDeterministic` invariant).
#[derive(Debug)]
pub struct TmuxServer {
    /// The scope this server belongs to. Diagnostic field — the
    /// cleanup cascade derives the lookup key from its own `ResourceScopeKey`
    /// arg, not from this field. Replaces the prior `conversation_id:
    /// String` field; for `Worktree`-scoped servers, "one conversation"
    /// was misleading because the chain of continuation members all
    /// share the entry.
    #[allow(dead_code)]
    pub work_scope: ResourceScopeKey,
    pub socket_path: PathBuf,
    pub server_token: String,
    pub status: ServerStatus,
    retirement_generation: u64,
    retirement_fenced: bool,
}

#[derive(Debug)]
struct TmuxScopeEntry {
    server: Arc<RwLock<TmuxServer>>,
}

#[derive(Clone, Copy, Debug)]
struct RetirementDeadline {
    expires: tokio::time::Instant,
}

impl RetirementDeadline {
    fn new(expires: tokio::time::Instant) -> Self {
        Self { expires }
    }

    async fn read_map<'a>(
        self,
        registry: &'a TmuxRegistry,
        phase: &'static str,
    ) -> Result<tokio::sync::RwLockReadGuard<'a, HashMap<String, Arc<TmuxScopeEntry>>>, String>
    {
        tokio::time::timeout_at(self.expires, registry.inner.read())
            .await
            .map_err(|_| format!("tmux {phase} registry read lock exceeded the Close deadline"))
    }

    async fn write_map<'a>(
        self,
        registry: &'a TmuxRegistry,
        phase: &'static str,
    ) -> Result<tokio::sync::RwLockWriteGuard<'a, HashMap<String, Arc<TmuxScopeEntry>>>, String>
    {
        tokio::time::timeout_at(self.expires, registry.inner.write())
            .await
            .map_err(|_| format!("tmux {phase} registry write lock exceeded the Close deadline"))
    }

    async fn get_existing(
        self,
        registry: &TmuxRegistry,
        work_scope: &ResourceScopeKey,
        phase: &'static str,
    ) -> Result<Option<Arc<TmuxScopeEntry>>, String> {
        let map = self.read_map(registry, phase).await?;
        Ok(map.get(&work_scope.stable_key()).cloned())
    }

    async fn get_or_insert(
        self,
        registry: &TmuxRegistry,
        work_scope: &ResourceScopeKey,
        server: TmuxServer,
        phase: &'static str,
    ) -> Result<(Arc<TmuxScopeEntry>, bool), String> {
        let key = work_scope.stable_key();
        {
            let map = self.read_map(registry, phase).await?;
            if let Some(entry) = map.get(&key) {
                return Ok((entry.clone(), false));
            }
        }
        let mut map = self.write_map(registry, phase).await?;
        if let Some(entry) = map.get(&key) {
            return Ok((entry.clone(), false));
        }
        let entry = Arc::new(TmuxScopeEntry::new(server));
        map.insert(key, entry.clone());
        Ok((entry, true))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedWindow {
    pub exit_code: Option<i32>,
    pub occurred_at: Option<std::time::SystemTime>,
    pub final_tail: Vec<String>,
}

impl TmuxServer {
    fn new(work_scope: ResourceScopeKey, socket_path: PathBuf) -> Self {
        Self {
            work_scope,
            socket_path,
            server_token: uuid::Uuid::new_v4().to_string(),
            status: ServerStatus::NotProbed,
            retirement_generation: 0,
            retirement_fenced: false,
        }
    }

    fn exact_identity(&self) -> TmuxServerInstanceIdentity {
        TmuxServerInstanceIdentity {
            socket_path: self.socket_path.clone(),
            server_token: self.server_token.clone(),
        }
    }
}

impl TmuxScopeEntry {
    fn new(server: TmuxServer) -> Self {
        Self {
            server: Arc::new(RwLock::new(server)),
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
fn socket_path_for_coordinator(socket_dir: &Path) -> PathBuf {
    socket_dir.join("coordinator.sock")
}

#[must_use]
pub fn socket_path_for_global(socket_dir: &Path) -> PathBuf {
    socket_dir.join("global.sock")
}

/// Signal published when a `TmuxServer` in a `ResourceScopeKey` changes state
/// (entry created, status transition `NotProbed`→`Live` / →`Gone`, or
/// removal during the hard-delete cascade). Mirrors the bash
/// `BashLifecycleEvent` shape: it carries only the affected `ResourceScopeKey`,
/// leaving inventory assembly and conversation routing to the runtime's
/// work-scope bridge. State transitions only — NOT per-probe noops
/// (REQ-WSUI-007).
#[derive(Debug, Clone)]
pub struct TmuxLifecycleEvent {
    pub work_scope: ResourceScopeKey,
}

/// Sink the registry publishes [`TmuxLifecycleEvent`]s into. A `mpsc`
/// keeps the registry decoupled from per-conversation routing (the runtime
/// owns that). `None` for tool-level tests that don't care about the push
/// path. Mirrors [`super::super::bash::registry::BashLifecycleSink`].
pub type TmuxLifecycleSink = tokio::sync::mpsc::UnboundedSender<TmuxLifecycleEvent>;

#[cfg(test)]
#[derive(Debug)]
struct EnsureLiveLockTestHook {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
#[derive(Debug)]
struct CompleteRetirementLockTestHook {
    entry_lock_reached: Arc<tokio::sync::Notify>,
    socket_identity_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    final_authority_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

#[cfg(test)]
#[derive(Debug)]
struct CascadeNotProbedTestHook {
    observed: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
#[derive(Debug)]
struct RehydrateExistingTestHook {
    before_authority: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
#[derive(Debug)]
struct CancelRetirementTestHook {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

/// Top-level registry: maps `ResourceScopeKey::stable_key()` → per-scope tmux
/// server. One registry instance per Phoenix process.
#[derive(Debug)]
pub struct TmuxRegistry {
    /// Keyed by `ResourceScopeKey::stable_key()` so Worktree-scoped continuation
    /// members share an entry, and Worktree vs Conversation namespaces
    /// stay disjoint.
    inner: RwLock<HashMap<String, Arc<TmuxScopeEntry>>>,
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
    contain_test_spawns: bool,
    #[cfg(test)]
    ensure_live_lock_test_hook: Option<Arc<EnsureLiveLockTestHook>>,
    #[cfg(test)]
    complete_retirement_lock_test_hook: Option<Arc<CompleteRetirementLockTestHook>>,
    #[cfg(test)]
    cascade_not_probed_test_hook: Option<Arc<CascadeNotProbedTestHook>>,
    #[cfg(test)]
    cancel_retirement_test_hook: Option<Arc<CancelRetirementTestHook>>,
    #[cfg(test)]
    rehydrate_existing_test_hook: Option<Arc<RehydrateExistingTestHook>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistentTmuxDiscovery {
    EndpointAbsent,
    ServerAbsent,
    Exact(TmuxServerInstanceIdentity),
    Ambiguous { reason: String },
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
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: None,
            contain_test_spawns: false,
            #[cfg(test)]
            ensure_live_lock_test_hook: None,
            #[cfg(test)]
            complete_retirement_lock_test_hook: None,
            #[cfg(test)]
            cascade_not_probed_test_hook: None,
            #[cfg(test)]
            cancel_retirement_test_hook: None,
            #[cfg(test)]
            rehydrate_existing_test_hook: None,
        }
    }

    /// Construct a registry (default socket dir) that publishes tmux
    /// state-transition signals into `sink`. The runtime wires this to the
    /// work-scope push bridge, which resolves the scope's conversation and
    /// broadcasts a `ResourceScopeKeyUpdate`. Mirrors
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
    fn emit_lifecycle(&self, work_scope: &ResourceScopeKey) {
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
            socket_dir,
            binary_available,
            runtime_assets: OnceCell::new(),
            lifecycle_sink: None,
            contain_test_spawns: false,
            #[cfg(test)]
            ensure_live_lock_test_hook: None,
            #[cfg(test)]
            complete_retirement_lock_test_hook: None,
            #[cfg(test)]
            cascade_not_probed_test_hook: None,
            #[cfg(test)]
            cancel_retirement_test_hook: None,
            #[cfg(test)]
            rehydrate_existing_test_hook: None,
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
            contain_test_spawns: false,
            #[cfg(test)]
            ensure_live_lock_test_hook: None,
            #[cfg(test)]
            complete_retirement_lock_test_hook: None,
            #[cfg(test)]
            cascade_not_probed_test_hook: None,
            #[cfg(test)]
            cancel_retirement_test_hook: None,
            #[cfg(test)]
            rehydrate_existing_test_hook: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_test_spawn_containment(mut self) -> Self {
        self.contain_test_spawns = true;
        self
    }

    #[cfg(test)]
    fn with_ensure_live_lock_test_hook(
        mut self,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.ensure_live_lock_test_hook =
            Some(Arc::new(EnsureLiveLockTestHook { entered, release }));
        self
    }

    #[cfg(test)]
    fn with_complete_retirement_lock_test_hook(
        mut self,
        before_entry_lock: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.complete_retirement_lock_test_hook = Some(Arc::new(CompleteRetirementLockTestHook {
            entry_lock_reached: before_entry_lock,
            socket_identity_gate: None,
            final_authority_gate: None,
        }));
        self
    }

    #[cfg(test)]
    fn with_socket_identity_test_hook(
        mut self,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.complete_retirement_lock_test_hook = Some(Arc::new(CompleteRetirementLockTestHook {
            entry_lock_reached: Arc::new(tokio::sync::Notify::new()),
            socket_identity_gate: Some((entered, release)),
            final_authority_gate: None,
        }));
        self
    }

    #[cfg(test)]
    fn with_final_authority_test_hook(
        mut self,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.complete_retirement_lock_test_hook = Some(Arc::new(CompleteRetirementLockTestHook {
            entry_lock_reached: Arc::new(tokio::sync::Notify::new()),
            socket_identity_gate: None,
            final_authority_gate: Some((entered, release)),
        }));
        self
    }

    #[cfg(test)]
    fn with_cancel_retirement_test_hook(
        mut self,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.cancel_retirement_test_hook =
            Some(Arc::new(CancelRetirementTestHook { entered, release }));
        self
    }

    #[cfg(test)]
    fn with_cascade_not_probed_test_hook(
        mut self,
        observed: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.cascade_not_probed_test_hook =
            Some(Arc::new(CascadeNotProbedTestHook { observed, release }));
        self
    }

    #[cfg(test)]
    fn with_rehydrate_existing_test_hook(
        mut self,
        before_authority: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.rehydrate_existing_test_hook = Some(Arc::new(RehydrateExistingTestHook {
            before_authority,
            release,
        }));
        self
    }

    async fn spawn_owned_session(&self, socket_path: &Path, cwd: &Path) -> Result<(), TmuxError> {
        spawn_session_owned(
            socket_path,
            &self.config_path(),
            cwd,
            self.contain_test_spawns,
        )
        .await
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

    async fn select_current_or_legacy_socket_until(
        &self,
        current: PathBuf,
        legacy: Option<PathBuf>,
        expires: tokio::time::Instant,
    ) -> Result<PathBuf, TmuxError> {
        let current_probe = probe_until(&current, expires)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: current.clone(),
                source,
            })?
            .ok_or_else(|| TmuxError::AmbiguousSocketIdentity {
                reason: format!(
                    "current tmux endpoint probe at {} exceeded the Close deadline",
                    current.display()
                ),
            })?;
        match current_probe {
            ProbeResult::Live => Ok(current),
            ProbeResult::DeadSocket if endpoint_is_definitely_not_socket(&current) => Ok(current),
            ProbeResult::DeadSocket => Err(ambiguous_socket_probe(&current)),
            ProbeResult::NoSocket | ProbeResult::NoServer => {
                let Some(legacy) = legacy.filter(|legacy| *legacy != current) else {
                    return Ok(current);
                };
                let legacy_probe = probe_until(&legacy, expires)
                    .await
                    .map_err(|source| TmuxError::ProbeFailed {
                        socket_path: legacy.clone(),
                        source,
                    })?
                    .ok_or_else(|| TmuxError::AmbiguousSocketIdentity {
                        reason: format!(
                            "legacy tmux endpoint probe at {} exceeded the Close deadline",
                            legacy.display()
                        ),
                    })?;
                select_socket_from_probes(current, legacy, legacy_probe)
            }
        }
    }

    /// Get-or-create the per-`ResourceScopeKey` `Arc<RwLock<TmuxServer>>` and
    /// drive the probe-and-act sequence (REQ-TMUX-002 / REQ-TMUX-005 /
    /// REQ-TMUX-006, REQ-TMUX-WS-001).
    ///
    /// `cwd` is the conversation's working directory; passed to tmux's
    /// `new-session -c` when a fresh server is spawned so the pane
    /// shell starts in the conversation's project. `cwd` is ignored
    /// when the probe sees `Live` — re-attaching to an existing server
    /// uses whatever start directory was set when it was first spawned.
    ///
    /// `work_scope` controls registry and socket keying. Conversation resources
    /// use the durable work-scope ID; the global terminal uses its separate key.
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
    /// Discovers a running persistent tmux server without creating, adopting, or
    /// mutating one. Close uses this only to seal a durable token/socket pair.
    /// # Errors
    ///
    /// Returns [`TmuxError`] when the server endpoint cannot be probed.
    pub async fn discover_persistent_identity(
        &self,
        work_scope: &ResourceScopeKey,
        legacy_worktree_path: Option<&Path>,
        legacy_conversation_id: Option<&str>,
        expires: tokio::time::Instant,
    ) -> Result<PersistentTmuxDiscovery, TmuxError> {
        let current_socket = match work_scope {
            ResourceScopeKey::Work(id) => {
                socket_path_for_worktree(&self.socket_dir, Path::new(id.as_str()))
            }
            ResourceScopeKey::Unattached(conversation_id) => {
                socket_path_for(&self.socket_dir, conversation_id)
            }
            ResourceScopeKey::Coordinator => socket_path_for_coordinator(&self.socket_dir),
            ResourceScopeKey::GlobalTerminal => socket_path_for_global(&self.socket_dir),
        };
        let legacy_socket = legacy_worktree_path
            .map(|path| socket_path_for_worktree(&self.socket_dir, path))
            .or_else(|| legacy_conversation_id.map(|id| socket_path_for(&self.socket_dir, id)));
        let socket_path = self
            .select_current_or_legacy_socket_until(current_socket, legacy_socket, expires)
            .await?;
        let Some(probe_result) =
            probe_until(&socket_path, expires)
                .await
                .map_err(|source| TmuxError::ProbeFailed {
                    socket_path: socket_path.clone(),
                    source,
                })?
        else {
            return Ok(PersistentTmuxDiscovery::Ambiguous {
                reason: format!(
                    "tmux identity probe at {} exceeded the Close deadline",
                    socket_path.display()
                ),
            });
        };
        match probe_result {
            ProbeResult::NoSocket => Ok(PersistentTmuxDiscovery::EndpointAbsent),
            ProbeResult::NoServer => Ok(PersistentTmuxDiscovery::ServerAbsent),
            ProbeResult::DeadSocket => Ok(PersistentTmuxDiscovery::Ambiguous {
                reason: format!(
                    "tmux endpoint {} exists but its server liveness probe failed",
                    socket_path.display()
                ),
            }),
            ProbeResult::Live => match read_server_token_until(&socket_path, expires).await {
                Some(Some(server_token)) => {
                    Ok(PersistentTmuxDiscovery::Exact(TmuxServerInstanceIdentity {
                        socket_path,
                        server_token,
                    }))
                }
                Some(None) => Ok(PersistentTmuxDiscovery::Ambiguous {
                    reason: "live tmux server has no readable Phoenix token".to_string(),
                }),
                None => Ok(PersistentTmuxDiscovery::Ambiguous {
                    reason: "tmux server token read exceeded the Close deadline".to_string(),
                }),
            },
        }
    }

    /// # Errors
    ///
    /// Returns [`TmuxError`] when tmux is unavailable or its server cannot be
    /// probed, created, or configured.
    #[allow(clippy::too_many_lines)]
    pub async fn ensure_live(
        &self,
        work_scope: &ResourceScopeKey,
        cwd: &Path,
        legacy_worktree_path: Option<&Path>,
        legacy_conversation_id: Option<&str>,
    ) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
        let existing_socket = if let Some(server) = self.get_existing(work_scope).await {
            let server = server.read().await;
            if server.retirement_fenced {
                return Err(TmuxError::RetirementFenced {
                    work_scope: work_scope.clone(),
                });
            }
            Some(server.socket_path.clone())
        } else {
            None
        };
        if !self.binary_available {
            return Err(TmuxError::BinaryUnavailable);
        }
        self.ensure_runtime_assets().await?;

        let socket_path = match work_scope {
            ResourceScopeKey::Work(id) => {
                socket_path_for_worktree(&self.socket_dir, Path::new(id.as_str()))
            }
            ResourceScopeKey::Unattached(conversation_id) => {
                socket_path_for(&self.socket_dir, conversation_id)
            }
            ResourceScopeKey::Coordinator => socket_path_for_coordinator(&self.socket_dir),
            ResourceScopeKey::GlobalTerminal => socket_path_for_global(&self.socket_dir),
        };

        let legacy_socket = legacy_worktree_path
            .map(|path| socket_path_for_worktree(&self.socket_dir, path))
            .or_else(|| legacy_conversation_id.map(|id| socket_path_for(&self.socket_dir, id)));
        let selected_socket = if let Some(existing_socket) = existing_socket {
            existing_socket
        } else {
            let current_probe =
                probe(&socket_path)
                    .await
                    .map_err(|source| TmuxError::ProbeFailed {
                        socket_path: socket_path.clone(),
                        source,
                    })?;
            match current_probe {
                ProbeResult::Live => socket_path.clone(),
                ProbeResult::DeadSocket if endpoint_is_definitely_not_socket(&socket_path) => {
                    socket_path.clone()
                }
                ProbeResult::DeadSocket => return Err(ambiguous_socket_probe(&socket_path)),
                ProbeResult::NoSocket | ProbeResult::NoServer => {
                    if let Some(legacy) = legacy_socket.filter(|legacy| *legacy != socket_path) {
                        let legacy_probe =
                            probe(&legacy)
                                .await
                                .map_err(|source| TmuxError::ProbeFailed {
                                    socket_path: legacy.clone(),
                                    source,
                                })?;
                        select_socket_from_probes(socket_path.clone(), legacy, legacy_probe)?
                    } else {
                        socket_path.clone()
                    }
                }
            }
        };
        if selected_socket != socket_path {
            tracing::info!(
                scope = %work_scope,
                socket = %selected_socket.display(),
                "tmux: adopting live pre-opaque-scope socket"
            );
        }
        let socket_path = selected_socket;

        if let Some(existing) = self.get_existing(work_scope).await {
            if existing.read().await.retirement_fenced {
                return Err(TmuxError::RetirementFenced {
                    work_scope: work_scope.clone(),
                });
            }
        }
        let (entry, created) = self.get_or_insert(work_scope, socket_path).await;
        let server_arc = entry.server.clone();

        let mut server = server_arc.write().await;
        #[cfg(test)]
        if let Some(hook) = &self.ensure_live_lock_test_hook {
            hook.entered.notify_one();
            hook.release.notified().await;
        }
        if server.retirement_fenced {
            return Err(TmuxError::RetirementFenced {
                work_scope: work_scope.clone(),
            });
        }
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
                match read_server_token(&server.socket_path).await {
                    Some(token) => server.server_token = token,
                    None => {
                        run_tmux_quiet(
                            &server.socket_path,
                            &[
                                "set-environment",
                                "-g",
                                SERVER_TOKEN_VAR,
                                &server.server_token,
                            ],
                        )
                        .await;
                    }
                }
                server.status = ServerStatus::Live;
                reused_live = true;
            }
            ProbeResult::NoSocket => {
                self.spawn_owned_session(&server.socket_path, cwd).await?;
                server.server_token =
                    read_server_token(&server.socket_path)
                        .await
                        .ok_or_else(|| TmuxError::SpawnFailed {
                            socket_path: server.socket_path.clone(),
                            reason: "spawned tmux server did not publish its identity token"
                                .to_string(),
                        })?;
                server.status = ServerStatus::Live;
            }
            ProbeResult::NoServer => {
                tracing::debug!(
                    socket = %server.socket_path.display(),
                    "tmux: server absence proven, unlinking stale socket and respawning"
                );
                let _ = tokio::fs::remove_file(&server.socket_path).await;
                self.spawn_owned_session(&server.socket_path, cwd).await?;
                server.server_token =
                    read_server_token(&server.socket_path)
                        .await
                        .ok_or_else(|| TmuxError::SpawnFailed {
                            socket_path: server.socket_path.clone(),
                            reason: "spawned tmux server did not publish its identity token"
                                .to_string(),
                        })?;
                server.status = ServerStatus::Live;
            }
            ProbeResult::DeadSocket if endpoint_is_definitely_not_socket(&server.socket_path) => {
                tracing::debug!(
                    socket = %server.socket_path.display(),
                    "tmux: non-socket endpoint detected, unlinking and respawning"
                );
                tokio::fs::remove_file(&server.socket_path)
                    .await
                    .map_err(|source| TmuxError::ProbeFailed {
                        socket_path: server.socket_path.clone(),
                        source,
                    })?;
                self.spawn_owned_session(&server.socket_path, cwd).await?;
                server.status = ServerStatus::Live;
            }
            ProbeResult::DeadSocket => return Err(ambiguous_socket_probe(&server.socket_path)),
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
        work_scope: &ResourceScopeKey,
        socket_path: PathBuf,
    ) -> (Arc<TmuxScopeEntry>, bool) {
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
        let entry = Arc::new(TmuxScopeEntry::new(TmuxServer::new(
            work_scope.clone(),
            socket_path,
        )));
        map.insert(key, entry.clone());
        (entry, true)
    }
    /// Look up a `ResourceScopeKey`'s tmux server entry **without creating one or
    /// probing the socket**.
    ///
    /// Read-only counterpart to the `get_or_insert` + probe path in
    /// [`Self::ensure_live`], for observability surfaces (the work-scope
    /// inventory endpoint) that must report the in-memory `status` as-is.
    /// It deliberately does NOT run `tmux ls`: a probe is a process spawn,
    /// and the inventory must not spawn one on every assembly.
    pub async fn get_existing(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<Arc<RwLock<TmuxServer>>> {
        let key = work_scope.stable_key();
        self.inner
            .read()
            .await
            .get(&key)
            .map(|entry| entry.server.clone())
    }

    async fn exact_identity_state(
        &self,
        identity: &TmuxServerInstanceIdentity,
        expires: tokio::time::Instant,
    ) -> Result<ExactTmuxIdentityState, TmuxError> {
        let Some(result) = probe_until(&identity.socket_path, expires)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: identity.socket_path.clone(),
                source,
            })?
        else {
            return Ok(ExactTmuxIdentityState::Ambiguous {
                reason: "tmux exact-identity probe exceeded the Close deadline".to_string(),
            });
        };
        Self::exact_identity_state_from_probe_with_binary(
            identity,
            result,
            expires,
            Path::new("tmux"),
        )
        .await
    }

    #[cfg(test)]
    async fn exact_identity_state_from_probe(
        identity: &TmuxServerInstanceIdentity,
        result: ProbeResult,
        expires: tokio::time::Instant,
    ) -> Result<ExactTmuxIdentityState, TmuxError> {
        Self::exact_identity_state_from_probe_with_binary(
            identity,
            result,
            expires,
            Path::new("tmux"),
        )
        .await
    }

    async fn exact_identity_state_from_probe_with_binary(
        identity: &TmuxServerInstanceIdentity,
        result: ProbeResult,
        expires: tokio::time::Instant,
        binary: &Path,
    ) -> Result<ExactTmuxIdentityState, TmuxError> {
        match result {
            ProbeResult::NoSocket | ProbeResult::NoServer => Ok(ExactTmuxIdentityState::Absent),
            ProbeResult::DeadSocket => Ok(ExactTmuxIdentityState::Ambiguous {
                reason: format!(
                    "tmux probe failed for existing socket {}; cannot prove whether {} is absent",
                    identity.socket_path.display(),
                    identity.stable_identity()
                ),
            }),
            ProbeResult::Live => match read_server_token_until_with_binary(
                &identity.socket_path,
                expires,
                binary,
            )
            .await
            {
                Some(Some(token)) if token == identity.server_token => {
                    Ok(ExactTmuxIdentityState::Live)
                }
                Some(Some(_)) => Ok(ExactTmuxIdentityState::Absent),
                Some(None) => Ok(ExactTmuxIdentityState::Ambiguous {
                    reason: format!(
                        "live tmux server at {} did not report {}; cannot prove whether {} is still current",
                        identity.socket_path.display(),
                        SERVER_TOKEN_VAR,
                        identity.stable_identity()
                    ),
                }),
                None => Ok(ExactTmuxIdentityState::Ambiguous {
                    reason: "tmux exact-identity token read exceeded the Close deadline".to_string(),
                }),
            },
        }
    }

    async fn find_socket_for_token(
        &self,
        expected_server_token: &str,
    ) -> Result<Option<PathBuf>, TmuxError> {
        let mut entries = match tokio::fs::read_dir(&self.socket_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(TmuxError::ProbeFailed {
                    socket_path: self.socket_dir.clone(),
                    source,
                });
            }
        };
        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|source| TmuxError::ProbeFailed {
                    socket_path: self.socket_dir.clone(),
                    source,
                })?
        {
            let candidate = entry.path();
            if candidate.extension().and_then(|ext| ext.to_str()) != Some("sock") {
                continue;
            }
            if matches!(probe(&candidate).await, Ok(ProbeResult::Live))
                && read_server_token(&candidate).await.as_deref() == Some(expected_server_token)
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Read-only exact inspection of one window on its owning tmux server.
    /// A server absent from the in-memory registry is rediscovered by its deterministic
    /// socket path and accepted only when its stamped token matches the persisted binding.
    /// Returns `None` when the token differs or the exact window no longer exists.
    /// Does not create registry entries or spawn servers.
    ///
    /// # Errors
    /// Returns [`TmuxError`] when invoking the read-only tmux inspection command fails.
    pub async fn inspect_existing_window(
        &self,
        work_scope: &ResourceScopeKey,
        expected_server_token: &str,
        window_id: &str,
    ) -> Result<Option<ObservedWindow>, TmuxError> {
        let socket_path = if let Some(entry) = self.get_existing(work_scope).await {
            let server = entry.read().await;
            if server.server_token != expected_server_token {
                return Ok(None);
            }
            server.socket_path.clone()
        } else {
            let Some(matched) = self.find_socket_for_token(expected_server_token).await? else {
                return Ok(None);
            };
            matched
        };

        let output = run_tmux_quiet_output(
            &socket_path,
            &["capture-pane", "-p", "-t", window_id, "-S", "-2000"],
        )
        .await?;
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let final_tail = stdout
            .lines()
            .map(std::string::ToString::to_string)
            .collect();
        let marker = parse_last_exit_marker(&stdout);
        Ok(Some(ObservedWindow {
            exit_code: marker.as_ref().map(|m| m.exit_code),
            occurred_at: marker.map(|m| m.occurred_at),
            final_tail,
        }))
    }

    /// Kill one exact window only when its persisted server token still owns
    /// the scoped tmux server. A token mismatch means the original server is
    /// already gone; the replacement server must not be touched.
    ///
    /// # Errors
    /// Returns [`TmuxError`] if reading the scoped tmux server fails.
    pub async fn kill_exact_window(
        &self,
        work_scope: &ResourceScopeKey,
        expected_server_token: &str,
        window_id: &str,
    ) -> Result<(), TmuxError> {
        let socket_path = if let Some(entry) = self.get_existing(work_scope).await {
            let server = entry.read().await;
            if server.server_token != expected_server_token {
                return Ok(());
            }
            server.socket_path.clone()
        } else {
            let Some(matched) = self.find_socket_for_token(expected_server_token).await? else {
                return Ok(());
            };
            matched
        };
        run_tmux_quiet(&socket_path, &["kill-window", "-t", window_id]).await;
        Ok(())
    }

    /// Deterministic socket path for a `ResourceScopeKey`, derived the same way
    /// `ensure_live` derives it on insertion. Used by the cascade when no
    /// registry entry is present (orphan-socket cleanup) and on the
    /// preserved path where the entry is intentionally left intact.
    fn derived_socket_path(&self, work_scope: &ResourceScopeKey) -> PathBuf {
        match work_scope {
            ResourceScopeKey::Work(id) => {
                socket_path_for_worktree(&self.socket_dir, Path::new(id.as_str()))
            }
            ResourceScopeKey::Unattached(conversation_id) => {
                socket_path_for(&self.socket_dir, conversation_id)
            }
            ResourceScopeKey::Coordinator => socket_path_for_coordinator(&self.socket_dir),
            ResourceScopeKey::GlobalTerminal => socket_path_for_global(&self.socket_dir),
        }
    }

    fn build_retirement_permit(
        work_scope: &ResourceScopeKey,
        server: &mut TmuxServer,
        had_entry: bool,
        authority: TmuxRetirementAuthority,
        exact_process: Option<ProcessIdentity>,
        expires: tokio::time::Instant,
    ) -> TmuxRetirementPermit {
        server.retirement_generation = server.retirement_generation.wrapping_add(1);
        server.retirement_fenced = true;
        server.status = ServerStatus::Gone;
        TmuxRetirementPermit {
            work_scope: work_scope.clone(),
            instance: server.exact_identity(),
            generation: TmuxRetirementGeneration(server.retirement_generation),
            authority,
            had_entry,
            exact_process,
            expires,
        }
    }

    /// # Errors
    /// Returns a typed residual when the per-scope retirement fence cannot be
    /// acquired before `expires`.
    pub async fn begin_retirement(
        &self,
        work_scope: &ResourceScopeKey,
        legacy_worktree_path: Option<&Path>,
        legacy_conversation_id: Option<&str>,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementPermit, TmuxRetirementOutcome> {
        self.begin_retirement_inner(
            work_scope,
            legacy_worktree_path,
            legacy_conversation_id,
            true,
            TmuxRetirementAuthority::ExactServer,
            expires,
        )
        .await
    }

    /// Fence retirement after read-only discovery proved that no tmux server
    /// owns the selected endpoint.
    ///
    /// # Errors
    /// Returns a typed residual when the per-scope retirement fence cannot be
    /// acquired before `expires`.
    pub async fn begin_retirement_after_discovery(
        &self,
        work_scope: &ResourceScopeKey,
        discovery: &PersistentTmuxDiscovery,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementPermit, TmuxRetirementOutcome> {
        let authority = match discovery {
            PersistentTmuxDiscovery::EndpointAbsent => TmuxRetirementAuthority::EndpointAbsent,
            PersistentTmuxDiscovery::ServerAbsent => TmuxRetirementAuthority::ServerAbsenceVerified,
            PersistentTmuxDiscovery::Exact(_) | PersistentTmuxDiscovery::Ambiguous { .. } => {
                return Err(TmuxRetirementOutcome::IdentityNotProven {
                    reason: "tmux retirement absence authority was not proven by discovery"
                        .to_string(),
                });
            }
        };
        self.begin_retirement_inner(work_scope, None, None, true, authority, expires)
            .await
    }

    /// Rehydrate exact retirement authority for a persisted tmux server identity
    /// after a Phoenix restart.
    ///
    /// The persisted identity is authoritative only when the same live server is
    /// still reachable at the same socket path and reports the same
    /// `PHOENIX_TMUX_SERVER_TOKEN`. A missing socket or replacement token proves
    /// exact absence. Any scope conflict or live server without a token remains a
    /// residual ambiguity so callers do not take destructive action on path or
    /// token evidence alone.
    ///
    /// # Errors
    /// Returns [`TmuxError`] when probing the persisted socket path fails.
    #[allow(clippy::too_many_lines)]
    pub async fn rehydrate_retirement(
        &self,
        work_scope: &ResourceScopeKey,
        persisted: &TmuxServerInstanceIdentity,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementRehydration, TmuxError> {
        let deadline = RetirementDeadline::new(expires);
        let current_entry = match deadline
            .get_existing(self, work_scope, "retirement rehydration")
            .await
        {
            Ok(entry) => entry,
            Err(reason) => return Ok(TmuxRetirementRehydration::Residual { reason }),
        };
        if let Some(entry) = current_entry {
            let Ok(current) = tokio::time::timeout_at(expires, entry.server.read()).await else {
                return Ok(TmuxRetirementRehydration::Residual {
                    reason: "tmux entry identity read lock exceeded the Close deadline".to_string(),
                });
            };
            let exact_current = current.socket_path == persisted.socket_path
                && current.server_token == persisted.server_token;
            let current_socket_matches = current.socket_path == persisted.socket_path;
            let current_identity = current.exact_identity();
            let current_fenced = current.retirement_fenced;
            drop(current);

            if exact_current {
                return match self.exact_identity_state(persisted, expires).await? {
                    ExactTmuxIdentityState::Absent => {
                        Ok(TmuxRetirementRehydration::AbsenceVerified)
                    }
                    ExactTmuxIdentityState::Ambiguous { reason } => {
                        Ok(TmuxRetirementRehydration::Residual { reason })
                    }
                    ExactTmuxIdentityState::Live => {
                        let exact_process = exact_server_process_identity_until(
                            &persisted.socket_path,
                            &persisted.server_token,
                            expires,
                        )
                        .await;
                        #[cfg(test)]
                        if let Some(hook) = &self.rehydrate_existing_test_hook {
                            hook.before_authority.notify_one();
                            hook.release.notified().await;
                        }
                        let map = match deadline
                            .write_map(self, "retirement rehydration authority")
                            .await
                        {
                            Ok(map) => map,
                            Err(reason) => {
                                return Ok(TmuxRetirementRehydration::Residual { reason });
                            }
                        };
                        let key = work_scope.stable_key();
                        let Some(authoritative) = map.get(&key) else {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} registry entry changed while rehydrating persisted {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        };
                        if !Arc::ptr_eq(authoritative, &entry) {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} registry entry was replaced while rehydrating persisted {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        let Ok(mut current) =
                            tokio::time::timeout_at(expires, authoritative.server.write()).await
                        else {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason:
                                    "tmux retirement fence write lock exceeded the Close deadline"
                                        .to_string(),
                            });
                        };
                        if current.socket_path != persisted.socket_path
                            || current.server_token != persisted.server_token
                        {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} identity changed while rehydrating persisted {}; replacement left untouched",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        if current.retirement_fenced {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "tmux retirement already fenced for {} at {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        current.status = ServerStatus::Live;
                        let permit = Self::build_retirement_permit(
                            work_scope,
                            &mut current,
                            true,
                            TmuxRetirementAuthority::ExactServer,
                            exact_process,
                            expires,
                        );
                        drop(current);
                        drop(map);
                        self.emit_lifecycle(work_scope);
                        Ok(TmuxRetirementRehydration::Permit(permit))
                    }
                };
            }

            if current_socket_matches {
                return Ok(TmuxRetirementRehydration::AbsenceVerified);
            }

            return match self.exact_identity_state(persisted, expires).await? {
                ExactTmuxIdentityState::Absent => Ok(TmuxRetirementRehydration::AbsenceVerified),
                ExactTmuxIdentityState::Live => Ok(TmuxRetirementRehydration::Residual {
                    reason: format!(
                        "work scope {} is already materialized as {}; persisted server {} also remains live",
                        work_scope,
                        current_identity.stable_identity(),
                        persisted.stable_identity()
                    ),
                }),
                ExactTmuxIdentityState::Ambiguous { reason: ambiguity } => {
                    let mut reason = format!(
                        "work scope {} is already materialized as {}",
                        work_scope,
                        current_identity.stable_identity()
                    );
                    if current_fenced {
                        reason.push_str(" with retirement already fenced");
                    }
                    write!(&mut reason, "; {ambiguity}").expect("writing into String cannot fail");
                    Ok(TmuxRetirementRehydration::Residual { reason })
                }
            };
        }

        self.rehydrate_missing_entry(work_scope, persisted, expires)
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn rehydrate_missing_entry(
        &self,
        work_scope: &ResourceScopeKey,
        persisted: &TmuxServerInstanceIdentity,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementRehydration, TmuxError> {
        match self.exact_identity_state(persisted, expires).await? {
            ExactTmuxIdentityState::Absent => Ok(TmuxRetirementRehydration::AbsenceVerified),
            ExactTmuxIdentityState::Ambiguous { reason } => {
                Ok(TmuxRetirementRehydration::Residual { reason })
            }
            ExactTmuxIdentityState::Live => {
                let mut rehydrated =
                    TmuxServer::new(work_scope.clone(), persisted.socket_path.clone());
                rehydrated.server_token.clone_from(&persisted.server_token);
                rehydrated.status = ServerStatus::Live;
                let deadline = RetirementDeadline::new(expires);
                let (entry, _) = match deadline
                    .get_or_insert(
                        self,
                        work_scope,
                        rehydrated,
                        "missing retirement rehydration",
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(reason) => {
                        return Ok(TmuxRetirementRehydration::Residual { reason });
                    }
                };

                let current_identity = {
                    let Ok(server) = tokio::time::timeout_at(expires, entry.server.read()).await
                    else {
                        return Ok(TmuxRetirementRehydration::Residual {
                            reason: "tmux rehydrated entry identity read lock exceeded the Close deadline"
                                .to_string(),
                        });
                    };
                    if server.socket_path == persisted.socket_path {
                        None
                    } else {
                        Some(server.exact_identity())
                    }
                };
                if let Some(current_identity) = current_identity {
                    return Ok(TmuxRetirementRehydration::Residual {
                        reason: format!(
                            "work scope {} raced to a different tmux server {}; refusing to rehydrate persisted {}",
                            work_scope,
                            current_identity.stable_identity(),
                            persisted.stable_identity()
                        ),
                    });
                }
                match self.exact_identity_state(persisted, expires).await? {
                    ExactTmuxIdentityState::Absent => {
                        Ok(TmuxRetirementRehydration::AbsenceVerified)
                    }
                    ExactTmuxIdentityState::Ambiguous { reason } => {
                        Ok(TmuxRetirementRehydration::Residual { reason })
                    }
                    ExactTmuxIdentityState::Live => {
                        let exact_process = exact_server_process_identity_until(
                            &persisted.socket_path,
                            &persisted.server_token,
                            expires,
                        )
                        .await;
                        let map = match deadline
                            .write_map(self, "rehydrated retirement authority")
                            .await
                        {
                            Ok(map) => map,
                            Err(reason) => {
                                return Ok(TmuxRetirementRehydration::Residual { reason });
                            }
                        };
                        let key = work_scope.stable_key();
                        let Some(authoritative) = map.get(&key) else {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} registry entry disappeared while rehydrating persisted {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        };
                        if !Arc::ptr_eq(authoritative, &entry) {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} registry entry was replaced while rehydrating persisted {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        let Ok(mut server) =
                            tokio::time::timeout_at(expires, authoritative.server.write()).await
                        else {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: "tmux rehydrated retirement fence write lock exceeded the Close deadline"
                                    .to_string(),
                            });
                        };
                        if server.retirement_fenced {
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "tmux retirement already fenced for {} at {}",
                                    work_scope,
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        if server.socket_path != persisted.socket_path
                            || server.server_token != persisted.server_token
                        {
                            let current_identity = server.exact_identity();
                            return Ok(TmuxRetirementRehydration::Residual {
                                reason: format!(
                                    "work scope {} raced to a different tmux server {}; refusing to rehydrate persisted {}",
                                    work_scope,
                                    current_identity.stable_identity(),
                                    persisted.stable_identity()
                                ),
                            });
                        }
                        server.status = ServerStatus::Live;
                        let permit = Self::build_retirement_permit(
                            work_scope,
                            &mut server,
                            true,
                            TmuxRetirementAuthority::ExactServer,
                            exact_process,
                            expires,
                        );
                        Ok(TmuxRetirementRehydration::Permit(permit))
                    }
                }
            }
        }
    }

    async fn begin_retirement_inner(
        &self,
        work_scope: &ResourceScopeKey,
        legacy_worktree_path: Option<&Path>,
        legacy_conversation_id: Option<&str>,
        emit_lifecycle: bool,
        authority: TmuxRetirementAuthority,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementPermit, TmuxRetirementOutcome> {
        let current = self.derived_socket_path(work_scope);
        let legacy = legacy_worktree_path
            .map(|path| socket_path_for_worktree(&self.socket_dir, path))
            .or_else(|| legacy_conversation_id.map(|id| socket_path_for(&self.socket_dir, id)));
        let socket_path = if let Some(legacy) = legacy {
            if legacy != current
                && matches!(
                    probe_until(&current, expires).await,
                    Ok(Some(
                        ProbeResult::NoSocket | ProbeResult::NoServer | ProbeResult::DeadSocket
                    ))
                )
                && matches!(
                    probe_until(&legacy, expires).await,
                    Ok(Some(ProbeResult::Live))
                )
            {
                legacy
            } else {
                current
            }
        } else {
            current
        };

        let deadline = RetirementDeadline::new(expires);
        let server = TmuxServer::new(work_scope.clone(), socket_path);
        let (entry, created) = deadline
            .get_or_insert(self, work_scope, server, "retirement begin")
            .await
            .map_err(|reason| TmuxRetirementOutcome::RemovalFailed { reason })?;
        let observed_entry = if created {
            None
        } else {
            let server = tokio::time::timeout_at(expires, entry.server.read())
                .await
                .map_err(|_| TmuxRetirementOutcome::RemovalFailed {
                    reason: "tmux retirement identity read lock exceeded the Close deadline"
                        .to_string(),
                })?;
            let identity = server.exact_identity();
            drop(server);
            let process = exact_server_process_identity_until(
                &identity.socket_path,
                &identity.server_token,
                expires,
            )
            .await;
            Some((identity, process))
        };
        let Ok(mut server) = tokio::time::timeout_at(expires, entry.server.write()).await else {
            return Err(TmuxRetirementOutcome::RemovalFailed {
                reason: "tmux retirement fence write lock exceeded the Close deadline".to_string(),
            });
        };
        let (authority, exact_process) = match observed_entry {
            None => (authority, None),
            Some((identity, _)) if server.exact_identity() != identity => {
                return Err(TmuxRetirementOutcome::IdentityNotProven {
                    reason: "tmux server identity changed after retirement discovery".to_string(),
                });
            }
            Some((_, Some(process))) => (TmuxRetirementAuthority::ExactServer, Some(process)),
            Some((_, None)) if authority == TmuxRetirementAuthority::ExactServer => {
                (authority, None)
            }
            Some((_, None)) if authority == TmuxRetirementAuthority::ServerAbsenceVerified => {
                (authority, None)
            }
            Some((_, None)) => {
                return Err(TmuxRetirementOutcome::IdentityNotProven {
                    reason:
                        "tmux server appeared after absence discovery without exact process authority"
                            .to_string(),
                });
            }
        };
        let permit = Self::build_retirement_permit(
            work_scope,
            &mut server,
            !created,
            authority,
            exact_process,
            expires,
        );
        drop(server);
        if !created && emit_lifecycle {
            self.emit_lifecycle(work_scope);
        }
        Ok(permit)
    }

    fn matches_exact_instance(server: &TmuxServer, permit: &TmuxRetirementPermit) -> bool {
        server.retirement_fenced
            && server.retirement_generation == permit.generation.0
            && server.socket_path == permit.instance.socket_path
            && server.server_token == permit.instance.server_token
    }

    async fn verify_exact_absence(
        &self,
        permit: &TmuxRetirementPermit,
    ) -> Result<TmuxRetirementOutcome, TmuxError> {
        let Some(result) = probe_until(&permit.instance.socket_path, permit.expires)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: permit.instance.socket_path.clone(),
                source,
            })?
        else {
            return Ok(TmuxRetirementOutcome::IdentityNotProven {
                reason: "tmux exact-absence probe exceeded the Close deadline".to_string(),
            });
        };
        Self::verify_exact_absence_from_probe_with_binary(
            permit,
            result,
            permit.expires,
            Path::new("tmux"),
        )
        .await
    }

    fn observe_dead_socket_shutdown(
        socket_path: &Path,
        killed_socket: SocketFileIdentity,
    ) -> Result<ExactShutdownObservation, TmuxError> {
        let observed =
            socket_file_identity(socket_path).map_err(|source| TmuxError::ProbeFailed {
                socket_path: socket_path.to_path_buf(),
                source,
            })?;
        if observed == Some(killed_socket) {
            // The pathname is deliberately not unlinked here. An identity check followed by
            // pathname removal can delete a replacement installed between those operations.
            // The tmux server/OS owns cleanup of its exact socket incarnation.
            Ok(ExactShutdownObservation::Outstanding {
                reason: "exact stale tmux socket remains after kill-server".to_string(),
            })
        } else {
            // A missing path or different inode proves that the killed incarnation is gone.
            // Whatever now occupies the pathname is a replacement and must remain untouched.
            Ok(ExactShutdownObservation::Complete)
        }
    }

    async fn observe_exact_shutdown(
        permit: &TmuxRetirementPermit,
        killed_socket: SocketFileIdentity,
        expires: tokio::time::Instant,
    ) -> Result<ExactShutdownObservation, TmuxError> {
        let Some(result) = probe_until(&permit.instance.socket_path, expires)
            .await
            .map_err(|source| TmuxError::ProbeFailed {
                socket_path: permit.instance.socket_path.clone(),
                source,
            })?
        else {
            return Ok(ExactShutdownObservation::Outstanding {
                reason: "tmux liveness probe exceeded the shutdown deadline".to_string(),
            });
        };
        match result {
            ProbeResult::NoSocket | ProbeResult::NoServer => Ok(ExactShutdownObservation::Complete),
            ProbeResult::DeadSocket => {
                Self::observe_dead_socket_shutdown(&permit.instance.socket_path, killed_socket)
            }
            ProbeResult::Live => {
                match read_server_token_until(&permit.instance.socket_path, expires).await {
                    Some(Some(token)) if token == permit.instance.server_token => {
                        Ok(ExactShutdownObservation::Outstanding {
                            reason: "exact tmux server instance remains live after kill-server"
                                .to_string(),
                        })
                    }
                    Some(Some(_)) => Ok(ExactShutdownObservation::Complete),
                    Some(None) => Ok(ExactShutdownObservation::IdentityNotProven {
                        reason: "live tmux server token is unreadable".to_string(),
                    }),
                    None => Ok(ExactShutdownObservation::Outstanding {
                        reason: "tmux server token reader exceeded the shutdown deadline"
                            .to_string(),
                    }),
                }
            }
        }
    }

    async fn wait_for_exact_shutdown_with<F, Fut>(
        expires: tokio::time::Instant,
        poll_interval: Duration,
        mut observe: F,
    ) -> Result<TmuxRetirementOutcome, TmuxError>
    where
        F: FnMut(tokio::time::Instant) -> Fut,
        Fut: std::future::Future<Output = Result<ExactShutdownObservation, TmuxError>>,
    {
        loop {
            let observation = match tokio::time::timeout_at(expires, observe(expires)).await {
                Ok(observation) => observation?,
                Err(_) => {
                    return Ok(TmuxRetirementOutcome::RemovalFailed {
                        reason:
                            "successful tmux kill-server observation exceeded its shutdown deadline"
                                .to_string(),
                    });
                }
            };
            let last_outstanding = match observation {
                ExactShutdownObservation::Complete => {
                    return Ok(TmuxRetirementOutcome::AbsenceVerified);
                }
                ExactShutdownObservation::IdentityNotProven { reason } => {
                    return Ok(TmuxRetirementOutcome::IdentityNotProven { reason });
                }
                ExactShutdownObservation::Outstanding { reason } => reason,
            };

            let now = tokio::time::Instant::now();
            if now >= expires {
                return Ok(TmuxRetirementOutcome::RemovalFailed {
                    reason: format!(
                        "successful tmux kill-server was not followed by exact absence before its shutdown deadline: {last_outstanding}"
                    ),
                });
            }
            tokio::time::sleep(poll_interval.min(expires - now)).await;
        }
    }

    async fn verify_exact_absence_after_successful_kill(
        &self,
        permit: &TmuxRetirementPermit,
        killed_socket: SocketFileIdentity,
    ) -> Result<TmuxRetirementOutcome, TmuxError> {
        Self::wait_for_exact_shutdown_with(permit.expires, TMUX_SHUTDOWN_POLL_INTERVAL, |expires| {
            Self::observe_exact_shutdown(permit, killed_socket, expires)
        })
        .await
    }

    #[cfg(test)]
    async fn verify_exact_absence_from_probe(
        permit: &TmuxRetirementPermit,
        result: ProbeResult,
        expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementOutcome, TmuxError> {
        Self::verify_exact_absence_from_probe_with_binary(
            permit,
            result,
            expires,
            Path::new("tmux"),
        )
        .await
    }

    async fn verify_exact_absence_from_probe_with_binary(
        permit: &TmuxRetirementPermit,
        result: ProbeResult,
        expires: tokio::time::Instant,
        binary: &Path,
    ) -> Result<TmuxRetirementOutcome, TmuxError> {
        match result {
            ProbeResult::NoSocket | ProbeResult::NoServer => {
                Ok(TmuxRetirementOutcome::AbsenceVerified)
            }
            ProbeResult::DeadSocket => Ok(TmuxRetirementOutcome::IdentityNotProven {
                reason: format!(
                    "tmux probe failed for existing socket {}; exact server absence is not proven",
                    permit.instance.socket_path.display()
                ),
            }),
            ProbeResult::Live => {
                match read_server_token_until_with_binary(
                    &permit.instance.socket_path,
                    expires,
                    binary,
                )
                .await
                {
                    Some(Some(token)) if token == permit.instance.server_token => {
                        Ok(TmuxRetirementOutcome::RemovalFailed {
                            reason: "exact tmux server instance remained live after teardown"
                                .to_string(),
                        })
                    }
                    Some(Some(_)) => Ok(TmuxRetirementOutcome::AbsenceVerified),
                    Some(None) => Ok(TmuxRetirementOutcome::IdentityNotProven {
                        reason: "live tmux server token is unreadable".to_string(),
                    }),
                    None => Ok(TmuxRetirementOutcome::IdentityNotProven {
                        reason: "tmux exact-absence token read exceeded the Close deadline"
                            .to_string(),
                    }),
                }
            }
        }
    }

    /// # Errors
    /// Returns [`TmuxError`] when exact-instance absence cannot be verified.
    #[allow(clippy::too_many_lines)]
    pub async fn complete_retirement(
        &self,
        permit: &TmuxRetirementPermit,
    ) -> Result<TmuxRetirementOutcome, TmuxError> {
        if !permit.had_entry {
            return Ok(TmuxRetirementOutcome::AbsenceVerified);
        }
        let deadline = RetirementDeadline::new(permit.expires);
        let current_entry = match deadline
            .get_existing(self, &permit.work_scope, "retirement complete initial")
            .await
        {
            Ok(entry) => entry,
            Err(reason) => return Ok(TmuxRetirementOutcome::RemovalFailed { reason }),
        };
        let Some(entry) = current_entry else {
            return self.verify_exact_absence(permit).await;
        };

        #[cfg(test)]
        if let Some(hook) = &self.complete_retirement_lock_test_hook {
            hook.entry_lock_reached.notify_one();
        }
        let Ok(server) = tokio::time::timeout_at(permit.expires, entry.server.write()).await else {
            return Ok(TmuxRetirementOutcome::RemovalFailed {
                reason: "tmux exact teardown write lock exceeded the Close deadline".to_string(),
            });
        };
        let exact_owned = Self::matches_exact_instance(&server, permit);
        if !exact_owned {
            return self.verify_exact_absence(permit).await;
        }
        #[cfg(test)]
        if let Some((entered, release)) = self
            .complete_retirement_lock_test_hook
            .as_ref()
            .and_then(|hook| hook.socket_identity_gate.as_ref())
        {
            entered.notify_one();
            release.notified().await;
        }
        let pre_teardown = if self.binary_available {
            match pre_teardown_socket(&permit.instance.socket_path) {
                Ok(observation) => Some(observation),
                Err(source) => {
                    return Err(TmuxError::ProbeFailed {
                        socket_path: permit.instance.socket_path.clone(),
                        source,
                    });
                }
            }
        } else {
            None
        };
        if matches!(pre_teardown, Some(PreTeardownSocket::NotSocket)) {
            return Ok(TmuxRetirementOutcome::IdentityNotProven {
                reason: "tmux socket incarnation was unavailable before exact teardown".to_string(),
            });
        }
        let kill_failure = if matches!(pre_teardown, Some(PreTeardownSocket::Incarnation(_))) {
            let token_test = format!(
                "#{{==:#{{E:{SERVER_TOKEN_VAR}}},{}}}",
                permit.instance.server_token
            );
            let mut command = tokio::process::Command::new("tmux");
            command
                .arg("-f")
                .arg(self.config_path())
                .arg("-S")
                .arg(&permit.instance.socket_path)
                .args([
                    "if-shell",
                    "-F",
                    &token_test,
                    "kill-server",
                    "display-message -p PHOENIX_TOKEN_MISMATCH",
                ])
                .env_remove("TMUX")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command.kill_on_drop(true);
            match command_output(command, Some(permit.expires)).await {
                Ok(None) => Some(TmuxRetirementOutcome::RemovalFailed {
                    reason: "tmux exact teardown command exceeded the Close deadline".to_string(),
                }),
                Ok(Some(output)) if output.status.success() => {
                    if String::from_utf8_lossy(&output.stdout).contains("PHOENIX_TOKEN_MISMATCH") {
                        Some(TmuxRetirementOutcome::IdentityNotProven {
                            reason: "tmux server token changed before exact teardown".to_string(),
                        })
                    } else {
                        None
                    }
                }
                Ok(Some(output)) => Some(TmuxRetirementOutcome::RemovalFailed {
                    reason: format!(
                        "exact token-bound kill-server failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                }),
                Err(error) => Some(TmuxRetirementOutcome::RemovalFailed {
                    reason: error.to_string(),
                }),
            }
        } else {
            None
        };
        drop(server);

        let exact_process_exit = if matches!(
            (pre_teardown, permit.authority),
            (
                Some(PreTeardownSocket::Absent),
                TmuxRetirementAuthority::ExactServer
            )
        ) {
            match permit.exact_process {
                Some(process) => exact_process_exit_until(process, permit.expires).await,
                None => ExactProcessState::Unproven,
            }
        } else {
            ExactProcessState::Unproven
        };
        let verified = match (kill_failure.as_ref(), pre_teardown, permit.authority) {
            (
                None,
                Some(PreTeardownSocket::Absent),
                TmuxRetirementAuthority::ServerAbsenceVerified,
            ) => TmuxRetirementOutcome::AbsenceVerified,
            (None, Some(PreTeardownSocket::Absent), TmuxRetirementAuthority::ExactServer)
                if exact_process_exit == ExactProcessState::DeadOrReused =>
            {
                TmuxRetirementOutcome::AbsenceVerified
            }
            (
                None,
                Some(PreTeardownSocket::Absent),
                TmuxRetirementAuthority::ExactServer | TmuxRetirementAuthority::EndpointAbsent,
            ) => TmuxRetirementOutcome::IdentityNotProven {
                reason: "tmux socket incarnation was unavailable before exact teardown".to_string(),
            },
            (None, Some(PreTeardownSocket::Incarnation(killed_socket)), _) => {
                self.verify_exact_absence_after_successful_kill(permit, killed_socket)
                    .await?
            }
            _ => self.verify_exact_absence(permit).await?,
        };
        if let Some(failure) = kill_failure {
            return Ok(failure);
        }

        match verified {
            TmuxRetirementOutcome::AbsenceVerified => {
                #[cfg(test)]
                if let Some((entered, release)) = self
                    .complete_retirement_lock_test_hook
                    .as_ref()
                    .and_then(|hook| hook.final_authority_gate.as_ref())
                {
                    entered.notify_one();
                    release.notified().await;
                }
                let mut map = match deadline
                    .write_map(self, "retirement complete final authority")
                    .await
                {
                    Ok(map) => map,
                    Err(reason) => return Ok(TmuxRetirementOutcome::RemovalFailed { reason }),
                };
                let key = permit.work_scope.stable_key();
                let exact_owned = if let Some(current) = map.get(&key) {
                    if Arc::ptr_eq(current, &entry) {
                        let Ok(server) =
                            tokio::time::timeout_at(permit.expires, current.server.read()).await
                        else {
                            return Ok(TmuxRetirementOutcome::RemovalFailed {
                                reason: "tmux final identity read lock exceeded the Close deadline"
                                    .to_string(),
                            });
                        };
                        Self::matches_exact_instance(&server, permit)
                    } else {
                        false
                    }
                } else {
                    false
                };
                let removed = exact_owned && map.remove(&key).is_some();
                drop(map);
                if removed && permit.had_entry {
                    self.emit_lifecycle(&permit.work_scope);
                }
                Ok(TmuxRetirementOutcome::Retired)
            }
            residual @ (TmuxRetirementOutcome::Retired
            | TmuxRetirementOutcome::IdentityNotProven { .. }
            | TmuxRetirementOutcome::RemovalFailed { .. }) => Ok(residual),
        }
    }

    /// Reopens an exact set of scope fences only after every current entry lock
    /// has been acquired, so deadline failure cannot partially reopen the set.
    ///
    /// # Errors
    /// Returns every exact permit with a fresh bounded retry deadline when map
    /// or entry authority cannot be acquired before the batch deadline.
    pub async fn cancel_retirement_batch(
        &self,
        mut permits: Vec<TmuxRetirementPermit>,
    ) -> Result<(), TmuxRetirementBatchCancellationError> {
        let expires = permits
            .iter()
            .map(|permit| permit.expires)
            .min()
            .unwrap_or_else(close_deadline);
        let deadline = RetirementDeadline::new(expires);
        let map = match deadline.read_map(self, "retirement cancellation").await {
            Ok(map) => map,
            Err(reason) => {
                let retry_deadline = close_deadline();
                for permit in &mut permits {
                    permit.expires = retry_deadline;
                }
                return Err(TmuxRetirementBatchCancellationError { reason, permits });
            }
        };
        let entries = permits
            .iter()
            .map(|permit| map.get(&permit.work_scope.stable_key()).cloned())
            .collect::<Vec<_>>();
        let mut guards = Vec::with_capacity(entries.len());
        for entry in &entries {
            let Some(entry) = entry else {
                guards.push(None);
                continue;
            };
            let Ok(server) = tokio::time::timeout_at(expires, entry.server.write()).await else {
                drop(guards);
                drop(map);
                let retry_deadline = close_deadline();
                for permit in &mut permits {
                    permit.expires = retry_deadline;
                }
                return Err(TmuxRetirementBatchCancellationError {
                    reason:
                        "tmux retirement cancellation entry write lock exceeded the Close deadline"
                            .to_string(),
                    permits,
                });
            };
            guards.push(Some(server));
        }
        let mut reopened = Vec::new();
        for (permit, server) in permits.iter().zip(&mut guards) {
            let Some(server) = server else {
                continue;
            };
            if Self::matches_exact_instance(server, permit) {
                server.retirement_fenced = false;
                if server.status == ServerStatus::Gone {
                    server.status = ServerStatus::NotProbed;
                }
                reopened.push(permit.work_scope.clone());
            }
        }
        drop(guards);
        drop(map);
        for work_scope in reopened {
            self.emit_lifecycle(&work_scope);
        }
        Ok(())
    }

    /// Consume exact Close retirement authority and reopen only its current
    /// generation and server identity. A stale permit is a no-op.
    ///
    /// # Errors
    /// Returns the same exact-generation permit when registry or entry authority
    /// cannot be acquired within the current attempt's absolute deadline. The
    /// returned permit carries a fresh bounded deadline for an explicit retry.
    pub async fn cancel_retirement(
        &self,
        mut permit: TmuxRetirementPermit,
    ) -> Result<(), TmuxRetirementCancellationError> {
        #[cfg(test)]
        if let Some(hook) = &self.cancel_retirement_test_hook {
            hook.entered.notify_one();
            hook.release.notified().await;
        }
        let deadline = RetirementDeadline::new(permit.expires);
        let map = match deadline.read_map(self, "retirement cancellation").await {
            Ok(map) => map,
            Err(reason) => {
                permit.expires = close_deadline();
                return Err(TmuxRetirementCancellationError { reason, permit });
            }
        };
        let key = permit.work_scope.stable_key();
        let Some(entry) = map.get(&key).cloned() else {
            return Ok(());
        };
        let Ok(mut server) = tokio::time::timeout_at(permit.expires, entry.server.write()).await
        else {
            permit.expires = close_deadline();
            return Err(TmuxRetirementCancellationError {
                reason: "tmux retirement cancellation entry write lock exceeded the Close deadline"
                    .to_string(),
                permit,
            });
        };
        if Self::matches_exact_instance(&server, &permit) {
            server.retirement_fenced = false;
            if server.status == ServerStatus::Gone {
                server.status = ServerStatus::NotProbed;
            }
            drop(server);
            drop(map);
            self.emit_lifecycle(&permit.work_scope);
        }
        Ok(())
    }

    pub async fn reopen_after_repair(&self, work_scope: &ResourceScopeKey) {
        let key = work_scope.stable_key();
        let Some(entry) = self.inner.read().await.get(&key).cloned() else {
            return;
        };
        let mut server = entry.server.write().await;
        if server.retirement_fenced {
            server.retirement_fenced = false;
            if server.status == ServerStatus::Gone {
                server.status = ServerStatus::NotProbed;
            }
            drop(server);
            self.emit_lifecycle(work_scope);
        }
    }

    async fn remove_not_probed_if_authoritative(
        &self,
        work_scope: &ResourceScopeKey,
        expected: &Arc<TmuxScopeEntry>,
        expires: tokio::time::Instant,
    ) -> Result<bool, String> {
        let deadline = RetirementDeadline::new(expires);
        let mut map = deadline
            .write_map(self, "cascade retirement authority")
            .await?;
        let key = work_scope.stable_key();
        let removable = if let Some(authoritative) = map.get(&key) {
            if Arc::ptr_eq(authoritative, expected) {
                let server = tokio::time::timeout_at(expires, authoritative.server.read())
                    .await
                    .map_err(|_| {
                        "tmux cascade authority entry read lock exceeded the Close deadline"
                            .to_string()
                    })?;
                server.status == ServerStatus::NotProbed
            } else {
                false
            }
        } else {
            false
        };
        Ok(removable && map.remove(&key).is_some())
    }

    /// Best-effort tear-down of a `ResourceScopeKey`'s tmux server, called from
    /// the unified `run_resource_cleanup_cascade` (REQ-BED-032 —
    /// archive / abandon / mark-merged / hard-delete all share this
    /// path).
    /// Best-effort tear-down of a `ResourceScopeKey`'s tmux server, called from
    /// the unified `run_resource_cleanup_cascade` (REQ-BED-032 —
    /// archive / abandon / mark-merged / hard-delete all share this
    /// path).
    ///
    /// The registry is keyed by `ResourceScopeKey::stable_key()` (same lookup
    /// `ensure_live` uses for insertion). When the registry holds no
    /// entry — orphaned socket from a prior process, or a scope whose
    /// tools never reached `tmux_run` — the deterministic socket path
    /// is derived from the scope so we still attempt the unlink.
    ///
    /// `inheritor_scope`: the resolved `ResourceScopeKey` of the conversation
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
    #[allow(clippy::too_many_lines)]
    pub async fn cascade_on_delete(
        &self,
        work_scope: &ResourceScopeKey,
        inheritor_scope: Option<&ResourceScopeKey>,
        legacy_worktree_path: Option<&Path>,
        legacy_conversation_id: Option<&str>,
    ) -> CascadeReport {
        // Preservation by scope equality: the inheritor (continuation) is
        // still driving the same tmux server iff it resolves to the same
        // ResourceScopeKey. Falls out structurally — Direct continuations
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
        let expires = close_deadline();

        let deadline = RetirementDeadline::new(expires);
        let existing = match deadline
            .get_existing(self, work_scope, "cascade retirement")
            .await
        {
            Ok(existing) => existing,
            Err(reason) => {
                return CascadeReport {
                    socket_path: self.derived_socket_path(work_scope),
                    kill_server_error: Some(reason),
                    unlink_error: None,
                };
            }
        };
        if existing.is_none() {
            let discovery = match self
                .discover_persistent_identity(
                    work_scope,
                    legacy_worktree_path,
                    legacy_conversation_id,
                    expires,
                )
                .await
            {
                Ok(discovery) => discovery,
                Err(error) => {
                    return CascadeReport {
                        socket_path: self.derived_socket_path(work_scope),
                        kill_server_error: Some(error.to_string()),
                        unlink_error: None,
                    };
                }
            };
            let identity = match discovery {
                PersistentTmuxDiscovery::EndpointAbsent | PersistentTmuxDiscovery::ServerAbsent => {
                    return CascadeReport {
                        socket_path: self.derived_socket_path(work_scope),
                        kill_server_error: None,
                        unlink_error: None,
                    };
                }
                PersistentTmuxDiscovery::Ambiguous { reason } => {
                    return CascadeReport {
                        socket_path: self.derived_socket_path(work_scope),
                        kill_server_error: Some(reason),
                        unlink_error: None,
                    };
                }
                PersistentTmuxDiscovery::Exact(identity) => identity,
            };
            let outcome = match self
                .rehydrate_retirement(work_scope, &identity, expires)
                .await
            {
                Ok(TmuxRetirementRehydration::Permit(permit)) => {
                    match self.complete_retirement(&permit).await {
                        Ok(outcome) => outcome,
                        Err(error) => TmuxRetirementOutcome::RemovalFailed {
                            reason: error.to_string(),
                        },
                    }
                }
                Ok(TmuxRetirementRehydration::AbsenceVerified) => {
                    TmuxRetirementOutcome::AbsenceVerified
                }
                Ok(TmuxRetirementRehydration::Residual { reason }) => {
                    TmuxRetirementOutcome::IdentityNotProven { reason }
                }
                Err(error) => TmuxRetirementOutcome::RemovalFailed {
                    reason: error.to_string(),
                },
            };
            return match outcome {
                TmuxRetirementOutcome::Retired | TmuxRetirementOutcome::AbsenceVerified => {
                    CascadeReport {
                        socket_path: identity.socket_path,
                        kill_server_error: None,
                        unlink_error: None,
                    }
                }
                TmuxRetirementOutcome::IdentityNotProven { reason }
                | TmuxRetirementOutcome::RemovalFailed { reason } => CascadeReport {
                    socket_path: identity.socket_path,
                    kill_server_error: Some(reason),
                    unlink_error: None,
                },
            };
        }
        if let Some(entry) = existing {
            let status = match tokio::time::timeout_at(expires, entry.server.read()).await {
                Ok(entry) => entry.status,
                Err(_) => {
                    return CascadeReport {
                        socket_path: self.derived_socket_path(work_scope),
                        kill_server_error: Some(
                            "tmux cascade entry read lock exceeded the Close deadline".to_string(),
                        ),
                        unlink_error: None,
                    };
                }
            };
            if status == ServerStatus::NotProbed {
                #[cfg(test)]
                if let Some(hook) = &self.cascade_not_probed_test_hook {
                    hook.observed.notify_one();
                    hook.release.notified().await;
                }
                let removed = match self
                    .remove_not_probed_if_authoritative(work_scope, &entry, expires)
                    .await
                {
                    Ok(removed) => removed,
                    Err(reason) => {
                        return CascadeReport {
                            socket_path: self.derived_socket_path(work_scope),
                            kill_server_error: Some(reason),
                            unlink_error: None,
                        };
                    }
                };
                if removed {
                    self.emit_lifecycle(work_scope);
                    return CascadeReport {
                        socket_path: self.derived_socket_path(work_scope),
                        kill_server_error: None,
                        unlink_error: None,
                    };
                }
                return CascadeReport {
                    socket_path: self.derived_socket_path(work_scope),
                    kill_server_error: Some(
                        "tmux cascade lost NotProbed registry authority; promoted or replacement entry left untouched"
                            .to_string(),
                    ),
                    unlink_error: None,
                };
            }
        }
        let permit = self
            .begin_retirement_inner(
                work_scope,
                legacy_worktree_path,
                legacy_conversation_id,
                false,
                TmuxRetirementAuthority::ExactServer,
                expires,
            )
            .await;
        let socket_path = permit.as_ref().map_or_else(
            |_| self.derived_socket_path(work_scope),
            |permit| permit.instance.socket_path.clone(),
        );
        let outcome = match permit {
            Ok(permit) => match self.complete_retirement(&permit).await {
                Ok(outcome) => outcome,
                Err(error) => TmuxRetirementOutcome::RemovalFailed {
                    reason: error.to_string(),
                },
            },
            Err(outcome) => outcome,
        };

        match outcome {
            TmuxRetirementOutcome::Retired | TmuxRetirementOutcome::AbsenceVerified => {
                CascadeReport {
                    socket_path,
                    kill_server_error: None,
                    unlink_error: None,
                }
            }
            TmuxRetirementOutcome::IdentityNotProven { reason }
            | TmuxRetirementOutcome::RemovalFailed { reason } => CascadeReport {
                socket_path,
                kill_server_error: Some(reason),
                unlink_error: None,
            },
        }
    }

    #[cfg(test)]
    async fn is_retirement_fenced(&self, work_scope: &ResourceScopeKey) -> bool {
        let Some(entry) = self
            .inner
            .read()
            .await
            .get(&work_scope.stable_key())
            .cloned()
        else {
            return false;
        };
        let fenced = entry.server.read().await.retirement_fenced;
        fenced
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

impl From<TmuxRetirementOutcome> for CascadeReport {
    fn from(outcome: TmuxRetirementOutcome) -> Self {
        match outcome {
            TmuxRetirementOutcome::Retired | TmuxRetirementOutcome::AbsenceVerified => Self {
                socket_path: PathBuf::new(),
                kill_server_error: None,
                unlink_error: None,
            },
            TmuxRetirementOutcome::IdentityNotProven { reason }
            | TmuxRetirementOutcome::RemovalFailed { reason } => Self {
                socket_path: PathBuf::new(),
                kill_server_error: Some(reason),
                unlink_error: None,
            },
        }
    }
}

/// Convenience function for the cleanup-cascade orchestrator. Equivalent
/// to `registry.cascade_on_delete(…).await` — kept as a free function
/// for symmetry with the bash registry's `cascade_bash_on_delete` API and
/// `cascade_browser_on_delete`.
pub async fn cascade_tmux_on_delete(
    registry: &Arc<TmuxRegistry>,
    work_scope: &ResourceScopeKey,
    inheritor_scope: Option<&ResourceScopeKey>,
    legacy_worktree_path: Option<&Path>,
    legacy_conversation_id: Option<&str>,
) -> CascadeReport {
    registry
        .cascade_on_delete(
            work_scope,
            inheritor_scope,
            legacy_worktree_path,
            legacy_conversation_id,
        )
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
    let launch_uuid = uuid::Uuid::new_v4().to_string();
    cmd.envs(phoenix_terminal::spawn::build_env_for_tmux(
        &shell,
        &launch_uuid,
    ));
    // Stamp the companion version so a later reuse can tell a current server
    // (no-op) from a pre-feature/older one that needs a refresh.
    cmd.env(COMPANION_VERSION_VAR, COMPANION_ENV_VERSION);
    cmd.env(SERVER_TOKEN_VAR, uuid::Uuid::new_v4().to_string());
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

async fn run_tmux_quiet_output(
    socket_path: &Path,
    args: &[&str],
) -> Result<std::process::Output, TmuxError> {
    let sock = socket_path.to_string_lossy().into_owned();
    let mut full: Vec<&str> = vec!["-S", &sock];
    full.extend_from_slice(args);
    tokio::process::Command::new("tmux")
        .args(&full)
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|source| TmuxError::ProbeFailed {
            socket_path: socket_path.to_path_buf(),
            source,
        })
}

const SERVER_TOKEN_VAR: &str = "PHOENIX_TMUX_SERVER_TOKEN";

async fn exact_server_process_identity_until(
    socket_path: &Path,
    expected_token: &str,
    expires: tokio::time::Instant,
) -> Option<ProcessIdentity> {
    let mut command = tokio::process::Command::new("tmux");
    command
        .arg("-S")
        .arg(socket_path)
        .args([
            "display-message",
            "-p",
            &format!("#{{pid}} #{{E:{SERVER_TOKEN_VAR}}}"),
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = crate::tmux::probe::command_output(command, Some(expires))
        .await
        .ok()??;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let (pid, token) = stdout.trim().split_once(' ')?;
    if token != expected_token {
        return None;
    }
    current_process_identity(pid.parse().ok()?)
}

async fn read_server_token(socket_path: &Path) -> Option<String> {
    tmux_global_env(socket_path, SERVER_TOKEN_VAR).await
}

async fn read_server_token_until(
    socket_path: &Path,
    expires: tokio::time::Instant,
) -> Option<Option<String>> {
    read_server_token_until_with_binary(socket_path, expires, Path::new("tmux")).await
}

async fn read_server_token_until_with_binary(
    socket_path: &Path,
    expires: tokio::time::Instant,
    binary: &Path,
) -> Option<Option<String>> {
    match tmux_global_env_output_with_binary(socket_path, SERVER_TOKEN_VAR, Some(expires), binary)
        .await
    {
        Ok(Some(output)) => Some(parse_tmux_global_env(&output, SERVER_TOKEN_VAR)),
        Ok(None) => None,
        Err(_) => Some(None),
    }
}

/// Read one variable from a server's global environment, or `None` if unset.
/// `tmux show-environment -g VAR` prints `VAR=value` when set and `-VAR` when
/// not.
async fn tmux_global_env(socket_path: &Path, var: &str) -> Option<String> {
    let output = tmux_global_env_output(socket_path, var, None)
        .await
        .ok()??;
    parse_tmux_global_env(&output, var)
}

async fn tmux_global_env_output(
    socket_path: &Path,
    var: &str,
    expires: Option<tokio::time::Instant>,
) -> std::io::Result<Option<std::process::Output>> {
    tmux_global_env_output_with_binary(socket_path, var, expires, Path::new("tmux")).await
}

async fn tmux_global_env_output_with_binary(
    socket_path: &Path,
    var: &str,
    expires: Option<tokio::time::Instant>,
    binary: &Path,
) -> std::io::Result<Option<std::process::Output>> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("-S")
        .arg(socket_path)
        .args(["show-environment", "-g", var])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command_output(command, expires).await
}

fn parse_tmux_global_env(out: &std::process::Output, var: &str) -> Option<String> {
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
    let launch_uuid = uuid::Uuid::new_v4().to_string();
    for (k, v) in phoenix_terminal::spawn::build_env_for_tmux(&shell, &launch_uuid) {
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
type TestCreatorHandoff = (PathBuf, PathBuf, PathBuf);

fn tmux_spawn_command(
    socket_path: &Path,
    tmux_args: &[String],
    contain_test_spawn: bool,
) -> (tokio::process::Command, Option<TestCreatorHandoff>) {
    let Some(root) = socket_path.parent().filter(|_| contain_test_spawn) else {
        let mut command = tokio::process::Command::new("tmux");
        command.args(tmux_args);
        return (command, None);
    };
    let marker = root.join(format!(".creating-{}", uuid::Uuid::new_v4()));
    let gate = root.join(format!(".creator-gate-{}", uuid::Uuid::new_v4()));
    let wrapper = r#"
import fcntl
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

parent = int(sys.argv[1])
gate = Path(sys.argv[2])
marker = Path(sys.argv[3])
locked = marker.with_suffix(".locked")
root = marker.parent
while (
    root.exists()
    and not (root / ".cleanup-request").exists()
    and not gate.exists()
    and os.getppid() == parent
):
    time.sleep(0.01)
if not gate.exists():
    sys.exit(1)
with marker.open("r+") as marker_file:
    fcntl.flock(marker_file, fcntl.LOCK_EX)
    locked.touch()
    child = subprocess.Popen(sys.argv[4:], start_new_session=True)
    try:
        sys.exit(child.wait(timeout=5))
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGKILL)
        child.wait()
        sys.exit(124)
    finally:
        locked.unlink(missing_ok=True)
        gate.unlink(missing_ok=True)
        marker.unlink(missing_ok=True)
"#;
    let mut command = tokio::process::Command::new("python3");
    command
        .arg("-c")
        .arg(wrapper)
        .arg(std::process::id().to_string())
        .arg(&gate)
        .arg(&marker)
        .arg("tmux");
    command.args(tmux_args);
    let locked = marker.with_extension("locked");
    (command, Some((marker, gate, locked)))
}

/// # Errors
/// Returns a [`TmuxError`] when the `tmux new-session` process fails to
/// spawn or exits non-zero.
async fn contained_spawn_failure(
    child: &mut tokio::process::Child,
    socket_path: &Path,
    reason: String,
) -> TmuxError {
    let _ = child.kill().await;
    let _ = child.wait().await;
    TmuxError::SpawnFailed {
        socket_path: socket_path.to_path_buf(),
        reason,
    }
}

async fn publish_creator_handoff(
    child: &mut tokio::process::Child,
    socket_path: &Path,
    handoff: TestCreatorHandoff,
) -> Result<(), TmuxError> {
    let (marker, gate, locked) = handoff;
    let pending = marker.with_extension("pending");
    if let Err(error) = std::fs::write(&pending, child.id().unwrap_or_default().to_string()) {
        return Err(contained_spawn_failure(
            child,
            socket_path,
            format!("failed to write creator identity: {error}"),
        )
        .await);
    }
    if let Err(error) = std::fs::rename(pending, marker) {
        return Err(contained_spawn_failure(
            child,
            socket_path,
            format!("failed to publish creator identity: {error}"),
        )
        .await);
    }
    if let Err(error) = std::fs::write(gate, []) {
        return Err(contained_spawn_failure(
            child,
            socket_path,
            format!("failed to release creator gate: {error}"),
        )
        .await);
    }
    if let Err(error) = tokio::time::timeout(Duration::from_secs(2), async {
        while !locked.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    {
        return Err(contained_spawn_failure(
            child,
            socket_path,
            format!("creator ownership lock was not acquired: {error}"),
        )
        .await);
    }
    Ok(())
}

/// # Errors
/// Returns a [`TmuxError`] when the `tmux new-session` process fails to
/// spawn or exits non-zero.
pub async fn spawn_session(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
) -> Result<(), TmuxError> {
    spawn_session_owned(socket_path, config_path, cwd, false).await
}

async fn spawn_session_owned(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
    contain_test_spawn: bool,
) -> Result<(), TmuxError> {
    let tmux_args = [
        "-f".to_string(),
        config_path.to_string_lossy().into_owned(),
        "-S".to_string(),
        socket_path.to_string_lossy().into_owned(),
        "new-session".to_string(),
        "-d".to_string(),
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        "-s".to_string(),
        TMUX_DEFAULT_SESSION.to_string(),
    ];
    let (mut cmd, creator_handoff) =
        tmux_spawn_command(socket_path, &tmux_args, contain_test_spawn);
    // A tmux pane shell inherits the tmux *server's* environment, captured here.
    // Build it explicitly (base + PtyEnvInjection + safe-var allowlist) rather
    // than inheriting Phoenix's env, which would leak server secrets into every
    // pane and diverge from the direct-shell path. env_clear also drops TMUX, so
    // an outer-tmux invocation does not trip tmux's nesting refusal.
    set_tmux_server_env(&mut cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!("failed to invoke tmux: {e}"),
        })?;
    if let Some(handoff) = creator_handoff {
        publish_creator_handoff(&mut child, socket_path, handoff).await?;
    }
    let output = child
        .wait_with_output()
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
    use crate::tmux::test_server::TestTmuxServerOwner;
    use tempfile::TempDir;

    fn scope(id: &str) -> ResourceScopeKey {
        ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse(id).unwrap())
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

        let scope = scope("conv-A");
        let sock = socket_path_for(tmp.path(), "conv-A");
        let (_arc, created) = reg.get_or_insert(&scope, sock).await;
        assert!(created, "first insert must report created");

        // Tearing down a held entry emits exactly one removal edge.
        let _ = reg.cascade_on_delete(&scope, None, None, None).await;
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
        let scope = scope("never-existed");
        let _ = reg.cascade_on_delete(&scope, None, None, None).await;
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
        let wt = scope("/tmp/phoenix-tmux-preserve-emit");
        let sock =
            socket_path_for_worktree(tmp.path(), Path::new("/tmp/phoenix-tmux-preserve-emit"));
        let _ = reg.get_or_insert(&wt, sock).await;
        let _ = reg.cascade_on_delete(&wt, Some(&wt), None, None).await;
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
        let wt = scope("/tmp/phoenix-tmux-preserve-entry");
        let sock =
            socket_path_for_worktree(tmp.path(), Path::new("/tmp/phoenix-tmux-preserve-entry"));
        let _ = reg.get_or_insert(&wt, sock).await;
        assert_eq!(
            reg.conversation_count().await,
            1,
            "precondition: entry held"
        );

        // Sibling continuation inherits the same scope → preserved path.
        let _ = reg.cascade_on_delete(&wt, Some(&wt), None, None).await;

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

    #[tokio::test]
    async fn cascade_not_probed_removal_loses_authority_to_same_entry_promotion() {
        let tmp = TempDir::new().unwrap();
        let observed = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(
            TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false)
                .with_cascade_not_probed_test_hook(Arc::clone(&observed), Arc::clone(&release)),
        );
        let work_scope = scope("cascade-promotion-authority");
        let socket_path = registry.derived_socket_path(&work_scope);
        let (entry, _) = registry.get_or_insert(&work_scope, socket_path).await;

        let cascade = {
            let registry = Arc::clone(&registry);
            let work_scope = work_scope.clone();
            tokio::spawn(async move {
                registry
                    .cascade_on_delete(&work_scope, None, None, None)
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), observed.notified())
            .await
            .expect("cascade must observe NotProbed before authority check");
        entry.server.write().await.status = ServerStatus::Live;
        release.notify_one();

        let report = cascade.await.unwrap();
        assert!(
            report.kill_server_error.is_some(),
            "lost NotProbed authority must remain a typed cascade residual"
        );
        let current = registry
            .inner
            .read()
            .await
            .get(&work_scope.stable_key())
            .cloned()
            .expect("promoted entry must remain registered");
        assert!(Arc::ptr_eq(&current, &entry));
        assert_eq!(current.server.read().await.status, ServerStatus::Live);
    }

    #[tokio::test]
    async fn existing_rehydration_rechecks_exact_identity_before_fencing() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let work_scope = scope("rehydrate-existing-authority");
        let registry = owner.registry();
        let live = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("bootstrap exact live server");
        let persisted = live.read().await.exact_identity();
        let before_authority = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut hooked = owner.registry();
        let existing = Arc::new(TmuxScopeEntry::new(TmuxServer {
            work_scope: work_scope.clone(),
            socket_path: persisted.socket_path.clone(),
            server_token: persisted.server_token.clone(),
            status: ServerStatus::Live,
            retirement_generation: 0,
            retirement_fenced: false,
        }));
        hooked
            .inner
            .get_mut()
            .insert(work_scope.stable_key(), Arc::clone(&existing));
        hooked = hooked
            .with_rehydrate_existing_test_hook(Arc::clone(&before_authority), Arc::clone(&release));
        let hooked = Arc::new(hooked);

        let rehydrate = {
            let registry = Arc::clone(&hooked);
            let work_scope = work_scope.clone();
            let persisted = persisted.clone();
            tokio::spawn(async move {
                registry
                    .rehydrate_retirement(&work_scope, &persisted, close_deadline())
                    .await
                    .unwrap()
            })
        };
        tokio::time::timeout(Duration::from_secs(1), before_authority.notified())
            .await
            .expect("rehydration must reach final authority check");
        let replacement_token = uuid::Uuid::new_v4().to_string();
        existing.server.write().await.server_token = replacement_token.clone();
        release.notify_one();

        assert!(matches!(
            rehydrate.await.unwrap(),
            TmuxRetirementRehydration::Residual { reason }
                if reason.contains("identity changed") && reason.contains("left untouched")
        ));
        let server = existing.server.read().await;
        assert_eq!(server.server_token, replacement_token);
        assert!(!server.retirement_fenced);
        assert_eq!(server.retirement_generation, 0);
        drop(server);
        owner.shutdown();
    }

    #[test]
    fn pre_teardown_socket_distinguishes_absence_from_unknown_incarnation() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing.sock");
        assert_eq!(
            pre_teardown_socket(&missing).unwrap(),
            PreTeardownSocket::Absent
        );

        let unknown = tmp.path().join("not-a-socket");
        std::fs::write(&unknown, b"not an owned socket incarnation").unwrap();
        assert_eq!(
            pre_teardown_socket(&unknown).unwrap(),
            PreTeardownSocket::NotSocket
        );
    }

    #[tokio::test]
    async fn unknown_pre_teardown_incarnation_remains_repair_required() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), true);
        let work_scope = scope("unknown-pre-teardown-incarnation");
        let socket_path = registry.derived_socket_path(&work_scope);
        let _ = registry
            .get_or_insert(&work_scope, socket_path.clone())
            .await;
        let permit = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("capture exact in-memory authority");
        std::fs::write(&socket_path, b"not an owned socket incarnation").unwrap();

        assert!(matches!(
            registry.complete_retirement(&permit).await.unwrap(),
            TmuxRetirementOutcome::IdentityNotProven { reason }
                if reason == "tmux socket incarnation was unavailable before exact teardown"
        ));
        assert!(registry.get_existing(&work_scope).await.is_some());
    }

    #[tokio::test]
    async fn unlinked_live_server_is_not_accepted_as_absent() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let registry = owner.registry();
        let work_scope = scope("unlinked-live-server");
        let live = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("create live server");
        let socket_path = live.read().await.socket_path.clone();
        let permit = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("capture exact live-server authority");
        std::fs::remove_file(&socket_path).expect("unlink live server socket");

        assert!(matches!(
            registry.complete_retirement(&permit).await.unwrap(),
            TmuxRetirementOutcome::IdentityNotProven { reason }
                if reason == "tmux socket incarnation was unavailable before exact teardown"
        ));
        assert!(registry.get_existing(&work_scope).await.is_some());
        owner.shutdown();
    }

    #[tokio::test]
    async fn exact_server_exit_after_permit_is_proven_from_process_birth() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let before_socket_identity = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(owner.registry().with_socket_identity_test_hook(
            Arc::clone(&before_socket_identity),
            Arc::clone(&release),
        ));
        let work_scope = scope("exact-exit-after-permit");
        let live = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("create exact server");
        let retired_identity = live.read().await.exact_identity();
        let permit = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("capture exact server permit");
        assert!(permit.exact_process.is_some());
        let completing = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.complete_retirement(&permit).await.unwrap() })
        };
        tokio::time::timeout(Duration::from_secs(1), before_socket_identity.notified())
            .await
            .expect("completion must prove exact authority before socket capture");
        kill_socket(&retired_identity.socket_path).await;
        match std::fs::remove_file(&retired_identity.socket_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove exact retired socket: {error}"),
        }
        release.notify_one();

        assert_eq!(completing.await.unwrap(), TmuxRetirementOutcome::Retired);
        assert!(registry.get_existing(&work_scope).await.is_none());
        owner.shutdown();
    }

    #[tokio::test]
    async fn verified_absent_server_is_idempotent_and_preserves_replacement() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let before_socket_identity = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(owner.registry().with_socket_identity_test_hook(
            Arc::clone(&before_socket_identity),
            Arc::clone(&release),
        ));
        let work_scope = scope("verified-absent-before-socket-capture");
        let live = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("create exact server");
        let retired_identity = live.read().await.exact_identity();
        kill_socket(&retired_identity.socket_path).await;
        let discovery = registry
            .discover_persistent_identity(&work_scope, None, None, close_deadline())
            .await
            .unwrap();
        assert_eq!(discovery, PersistentTmuxDiscovery::ServerAbsent);
        match std::fs::remove_file(&retired_identity.socket_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove exact retired socket: {error}"),
        }
        let permit = registry
            .begin_retirement_after_discovery(&work_scope, &discovery, close_deadline())
            .await
            .expect("carry verified absence into retirement");
        let completing = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move {
                let outcome = registry.complete_retirement(&permit).await.unwrap();
                (outcome, permit)
            })
        };
        tokio::time::timeout(Duration::from_secs(1), before_socket_identity.notified())
            .await
            .expect("completion must prove exact authority before socket capture");
        release.notify_one();

        let (outcome, permit) = completing.await.unwrap();
        assert_eq!(outcome, TmuxRetirementOutcome::Retired);
        assert!(registry.get_existing(&work_scope).await.is_none());
        assert_eq!(
            registry.complete_retirement(&permit).await.unwrap(),
            TmuxRetirementOutcome::AbsenceVerified
        );

        let replacement = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("create replacement server");
        let replacement_identity = replacement.read().await.exact_identity();
        assert_ne!(
            replacement_identity.server_token,
            retired_identity.server_token
        );
        assert_eq!(
            registry.complete_retirement(&permit).await.unwrap(),
            TmuxRetirementOutcome::AbsenceVerified
        );
        assert_eq!(
            read_server_token(&replacement_identity.socket_path).await,
            Some(replacement_identity.server_token.clone())
        );
        assert!(registry.get_existing(&work_scope).await.is_some());
        owner.shutdown();
    }

    #[tokio::test]
    async fn final_completion_cannot_remove_newer_registry_replacement() {
        let tmp = TempDir::new().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(
            TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false)
                .with_final_authority_test_hook(Arc::clone(&entered), Arc::clone(&release)),
        );
        let work_scope = scope("completion-replacement-authority");
        let socket_path = registry.derived_socket_path(&work_scope);
        let (retiring, _) = registry
            .get_or_insert(&work_scope, socket_path.clone())
            .await;
        let permit = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("retirement permit");

        let completing = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.complete_retirement(&permit).await.unwrap() })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("completion must reach final authority check");
        let replacement = Arc::new(TmuxScopeEntry::new(TmuxServer::new(
            work_scope.clone(),
            socket_path,
        )));
        registry
            .inner
            .write()
            .await
            .insert(work_scope.stable_key(), Arc::clone(&replacement));
        release.notify_one();

        assert_eq!(completing.await.unwrap(), TmuxRetirementOutcome::Retired);
        let current = registry
            .inner
            .read()
            .await
            .get(&work_scope.stable_key())
            .cloned()
            .expect("newer replacement must remain registered");
        assert!(Arc::ptr_eq(&current, &replacement));
        assert!(!Arc::ptr_eq(&current, &retiring));
    }

    #[tokio::test]
    async fn begin_retirement_fences_ensure_live_until_reopened() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("retirement-fence");
        let permit = reg
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("retirement fence should be acquired");

        assert_eq!(permit.work_scope, work_scope);
        assert!(reg.is_retirement_fenced(&work_scope).await);
        assert!(matches!(
            reg.ensure_live(&work_scope, tmp.path(), None, None).await,
            Err(TmuxError::RetirementFenced { work_scope: fenced }) if fenced == work_scope
        ));

        reg.reopen_after_repair(&work_scope).await;
        assert!(!reg.is_retirement_fenced(&work_scope).await);
        assert!(matches!(
            reg.ensure_live(&work_scope, tmp.path(), None, None).await,
            Err(TmuxError::BinaryUnavailable)
        ));
    }

    #[tokio::test]
    async fn stale_cancel_resuming_after_newer_fence_preserves_admission_rejection() {
        let tmp = TempDir::new().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(
            TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false)
                .with_cancel_retirement_test_hook(Arc::clone(&entered), Arc::clone(&release)),
        );
        let work_scope = scope("stale-cancel-newer-fence");
        let stale = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("first fence");
        let cancelling = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.cancel_retirement(stale).await })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("stale cancellation reached the deterministic barrier");

        let current = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("newer fence");
        release.notify_one();
        cancelling
            .await
            .expect("stale cancellation task")
            .expect("stale cancellation is a no-op");

        assert_eq!(current.generation().0, 2);
        assert!(registry.is_retirement_fenced(&work_scope).await);
        assert!(matches!(
            registry.ensure_live(&work_scope, tmp.path(), None, None).await,
            Err(TmuxError::RetirementFenced { work_scope: fenced }) if fenced == work_scope
        ));
    }

    #[tokio::test]
    async fn exact_tmux_cancel_reopens_admission_after_residual() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("exact-cancel-residual");
        let permit = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("retirement fence");

        registry
            .cancel_retirement(permit)
            .await
            .expect("exact cancellation");

        assert!(!registry.is_retirement_fenced(&work_scope).await);
        assert!(matches!(
            registry
                .ensure_live(&work_scope, tmp.path(), None, None)
                .await,
            Err(TmuxError::BinaryUnavailable)
        ));
    }

    #[tokio::test]
    async fn close_deadline_bounds_tmux_cancel_map_lock() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("cancel-deadline-map-lock");
        let permit = registry
            .begin_retirement(
                &work_scope,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_millis(50),
            )
            .await
            .expect("retirement fence");
        let guard = registry.inner.write().await;

        let error = registry
            .cancel_retirement(permit)
            .await
            .expect_err("cancellation must remain bounded");

        assert!(error.reason().contains("registry read lock"));
        assert!(error.reason().contains("Close deadline"));
        drop(guard);
        registry
            .cancel_retirement(error.into_permit())
            .await
            .expect("the exact permit remains retryable after the lock is released");
        assert!(!registry.is_retirement_fenced(&work_scope).await);
    }

    #[tokio::test]
    async fn batch_cancel_deadline_preserves_every_scope_fence_and_permit() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let scope_a = scope("batch-cancel-a");
        let scope_b = scope("batch-cancel-b");
        let permit_a = registry
            .begin_retirement(&scope_a, None, None, close_deadline())
            .await
            .expect("first scope fence");
        let permit_b = registry
            .begin_retirement(
                &scope_b,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_millis(50),
            )
            .await
            .expect("second scope fence");
        let entry_b = registry
            .inner
            .read()
            .await
            .get(&scope_b.stable_key())
            .cloned()
            .expect("second scope entry");
        let guard = entry_b.server.write().await;

        let error = registry
            .cancel_retirement_batch(vec![permit_a, permit_b])
            .await
            .expect_err("batch cancellation must remain bounded");
        drop(guard);

        assert!(registry.is_retirement_fenced(&scope_a).await);
        assert!(registry.is_retirement_fenced(&scope_b).await);
        registry
            .cancel_retirement_batch(error.into_permits())
            .await
            .expect("the complete exact permit set remains retryable");
        assert!(!registry.is_retirement_fenced(&scope_a).await);
        assert!(!registry.is_retirement_fenced(&scope_b).await);
    }

    #[tokio::test]
    async fn deadline_failed_cancel_n_cannot_reopen_newer_fence_n_plus_one() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("deadline-stale-cancel-newer-fence");
        let permit_n = registry
            .begin_retirement(
                &work_scope,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_millis(50),
            )
            .await
            .expect("first fence");
        let guard = registry.inner.write().await;
        let failed_n = registry
            .cancel_retirement(permit_n)
            .await
            .expect_err("first cancellation must remain bounded");
        drop(guard);

        let permit_n_plus_one = registry
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("newer fence");
        registry
            .cancel_retirement(failed_n.into_permit())
            .await
            .expect("stale exact cancellation is a no-op");

        assert_eq!(permit_n_plus_one.generation().0, 2);
        assert!(registry.is_retirement_fenced(&work_scope).await);
        registry
            .cancel_retirement(permit_n_plus_one)
            .await
            .expect("newer exact cancellation reopens admission");
        assert!(!registry.is_retirement_fenced(&work_scope).await);
    }

    #[tokio::test]
    async fn close_deadline_bounds_tmux_cancel_entry_lock() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("cancel-deadline-entry-lock");
        let permit = registry
            .begin_retirement(
                &work_scope,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_millis(50),
            )
            .await
            .expect("retirement fence");
        let entry = registry
            .inner
            .read()
            .await
            .get(&work_scope.stable_key())
            .cloned()
            .expect("scope entry");
        let guard = entry.server.write().await;

        let error = registry
            .cancel_retirement(permit)
            .await
            .expect_err("cancellation must remain bounded");

        assert!(error.reason().contains("entry write lock"));
        assert!(error.reason().contains("Close deadline"));
        drop(guard);
        registry
            .cancel_retirement(error.into_permit())
            .await
            .expect("the exact permit remains retryable after the lock is released");
        assert!(!registry.is_retirement_fenced(&work_scope).await);
    }

    #[tokio::test]
    async fn close_deadline_bounds_retirement_lock_held_by_wedged_ensure_live() {
        let tmp = TempDir::new().unwrap();
        let lock_held = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(
            TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), true)
                .with_ensure_live_lock_test_hook(Arc::clone(&lock_held), Arc::clone(&release)),
        );
        let work_scope = scope("deadline-ensure-live-lock");
        let socket_path =
            socket_path_for_worktree(tmp.path(), Path::new("deadline-ensure-live-lock"));
        let (entry, _) = registry
            .get_or_insert(&work_scope, socket_path.clone())
            .await;
        let original_token = {
            let server = entry.server.read().await;
            server.server_token.clone()
        };

        let ensure_live = {
            let registry = Arc::clone(&registry);
            let work_scope = work_scope.clone();
            let cwd = tmp.path().to_path_buf();
            tokio::spawn(async move { registry.ensure_live(&work_scope, &cwd, None, None).await })
        };
        tokio::time::timeout(Duration::from_secs(1), lock_held.notified())
            .await
            .expect("ensure_live must acquire its per-scope lock");

        let expires = tokio::time::Instant::now() + Duration::from_millis(100);
        let started = tokio::time::Instant::now();
        let outcome = registry
            .begin_retirement(&work_scope, None, None, expires)
            .await
            .expect_err("contended retirement must preserve a typed residual");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("lock") && reason.contains("Close deadline")
        ));

        let exact_server = registry
            .get_existing(&work_scope)
            .await
            .expect("contended server entry must remain exact");
        ensure_live.abort();
        assert!(ensure_live.await.unwrap_err().is_cancelled());
        let server = exact_server.read().await;
        assert_eq!(server.socket_path, socket_path);
        assert_eq!(server.server_token, original_token);
        assert!(!server.retirement_fenced);
        assert_eq!(server.retirement_generation, 0);
    }

    #[tokio::test]
    async fn close_deadline_bounds_begin_registry_map_lock_without_partial_fence() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir_and_binary(
            tmp.path().to_path_buf(),
            false,
        ));
        let work_scope = scope("deadline-begin-map");
        let map_guard = registry.inner.write().await;
        let expires = tokio::time::Instant::now() + Duration::from_millis(50);

        let outcome = registry
            .begin_retirement(&work_scope, None, None, expires)
            .await
            .expect_err("contended map must produce a typed residual");
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("retirement begin registry read lock")
                    && reason.contains("Close deadline")
        ));
        assert!(map_guard.get(&work_scope.stable_key()).is_none());
    }

    #[tokio::test]
    async fn close_deadline_bounds_missing_rehydration_registry_map_lock() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("deadline-rehydrate-map");
        let bootstrap = owner.registry();
        let live = bootstrap
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("bootstrap live server");
        let persisted = live.read().await.exact_identity();
        let restarted = owner.registry();
        let map_guard = restarted.inner.write().await;
        let expires = tokio::time::Instant::now() + Duration::from_millis(50);

        let result = restarted
            .rehydrate_missing_entry(&scope, &persisted, expires)
            .await
            .expect("map contention is a typed rehydration residual");
        assert!(matches!(
            result,
            TmuxRetirementRehydration::Residual { reason }
                if reason.contains("Close deadline")
        ));
        assert!(map_guard.get(&scope.stable_key()).is_none());
        drop(map_guard);
        owner.shutdown();
    }

    #[tokio::test]
    async fn close_deadline_bounds_complete_initial_registry_map_lock() {
        let tmp = TempDir::new().unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let work_scope = scope("deadline-complete-initial-map");
        let socket_path = registry.derived_socket_path(&work_scope);
        registry.get_or_insert(&work_scope, socket_path).await;
        let permit = registry
            .begin_retirement(
                &work_scope,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_secs(1),
            )
            .await
            .expect("retirement fence");
        let map_guard = registry.inner.write().await;

        let outcome = registry.complete_retirement(&permit).await.unwrap();
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("retirement complete initial registry read lock")
                    && reason.contains("Close deadline")
        ));
        assert!(map_guard.get(&work_scope.stable_key()).is_some());
    }

    #[tokio::test]
    async fn close_deadline_bounds_complete_final_registry_map_lock() {
        let tmp = TempDir::new().unwrap();
        let before_entry_lock = Arc::new(tokio::sync::Notify::new());
        let registry = Arc::new(
            TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false)
                .with_complete_retirement_lock_test_hook(Arc::clone(&before_entry_lock)),
        );
        let work_scope = scope("deadline-complete-final-map");
        let socket_path = registry.derived_socket_path(&work_scope);
        registry.get_or_insert(&work_scope, socket_path).await;
        let permit = registry
            .begin_retirement(
                &work_scope,
                None,
                None,
                tokio::time::Instant::now() + Duration::from_millis(100),
            )
            .await
            .expect("retirement fence");
        let entry = registry
            .inner
            .read()
            .await
            .get(&work_scope.stable_key())
            .cloned()
            .unwrap();
        let entry_guard = entry.server.write().await;
        let completing = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.complete_retirement(&permit).await.unwrap() })
        };
        tokio::time::timeout(Duration::from_secs(1), before_entry_lock.notified())
            .await
            .expect("completion must reach the entry lock");
        let map_guard = registry.inner.write().await;
        drop(entry_guard);

        let outcome = completing.await.unwrap();
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("retirement complete final authority registry write lock")
                    && reason.contains("Close deadline")
        ));
        assert!(map_guard.get(&work_scope.stable_key()).is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_probe_for_live_token_bound_endpoint_does_not_prove_absence() {
        use std::os::unix::fs::PermissionsExt;

        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let work_scope = scope("nonzero-live-token-bound");
        let registry = owner.registry();
        let live = registry
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("live token-bound server");
        let identity = live.read().await.exact_identity();

        let fake_tmux = owner.path().join("tmux-probe-fails");
        std::fs::write(&fake_tmux, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = std::fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_tmux, permissions).unwrap();
        let result = crate::tmux::probe::probe_with_binary(&identity.socket_path, &fake_tmux)
            .await
            .expect("probe process runs");
        assert_eq!(result, ProbeResult::DeadSocket);

        assert!(matches!(
            TmuxRegistry::exact_identity_state_from_probe(&identity, result, close_deadline())
                .await
                .unwrap(),
            ExactTmuxIdentityState::Ambiguous { .. }
        ));
        let permit = TmuxRetirementPermit {
            work_scope,
            instance: identity,
            generation: TmuxRetirementGeneration(1),
            authority: TmuxRetirementAuthority::ExactServer,
            exact_process: None,
            had_entry: true,
            expires: close_deadline(),
        };
        assert!(matches!(
            TmuxRegistry::verify_exact_absence_from_probe(&permit, result, close_deadline())
                .await
                .unwrap(),
            TmuxRetirementOutcome::IdentityNotProven { .. }
        ));
        owner.shutdown();
    }

    #[tokio::test]
    async fn missing_socket_still_proves_exact_absence() {
        let tmp = TempDir::new().unwrap();
        let work_scope = scope("missing-proves-absence");
        let identity = TmuxServerInstanceIdentity {
            socket_path: tmp.path().join("missing.sock"),
            server_token: "persisted-token".to_string(),
        };
        let result = probe(&identity.socket_path).await.unwrap();
        assert_eq!(result, ProbeResult::NoSocket);
        assert_eq!(
            TmuxRegistry::exact_identity_state_from_probe(&identity, result, close_deadline())
                .await
                .unwrap(),
            ExactTmuxIdentityState::Absent
        );
        let permit = TmuxRetirementPermit {
            work_scope,
            instance: identity,
            generation: TmuxRetirementGeneration(1),
            authority: TmuxRetirementAuthority::ExactServer,
            exact_process: None,
            had_entry: true,
            expires: close_deadline(),
        };
        assert_eq!(
            TmuxRegistry::verify_exact_absence_from_probe(&permit, result, close_deadline())
                .await
                .unwrap(),
            TmuxRetirementOutcome::AbsenceVerified
        );
    }

    #[cfg(unix)]
    #[test]
    fn dead_socket_shutdown_observation_never_unlinks_path_or_replacement() {
        use std::os::unix::net::UnixListener;

        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("shutdown.sock");
        let stale = UnixListener::bind(&socket_path).unwrap();
        let stale_identity = socket_file_identity(&socket_path).unwrap().unwrap();

        assert!(matches!(
            TmuxRegistry::observe_dead_socket_shutdown(&socket_path, stale_identity).unwrap(),
            ExactShutdownObservation::Outstanding { .. }
        ));
        assert_eq!(
            socket_file_identity(&socket_path).unwrap(),
            Some(stale_identity),
            "observing the exact stale incarnation must not unlink it by pathname"
        );

        std::fs::remove_file(&socket_path).unwrap();
        let replacement = UnixListener::bind(&socket_path).unwrap();
        let replacement_identity = socket_file_identity(&socket_path).unwrap().unwrap();
        assert_ne!(replacement_identity, stale_identity);

        assert_eq!(
            TmuxRegistry::observe_dead_socket_shutdown(&socket_path, stale_identity).unwrap(),
            ExactShutdownObservation::Complete
        );
        assert_eq!(
            socket_file_identity(&socket_path).unwrap(),
            Some(replacement_identity),
            "a replacement at the checked pathname must remain"
        );
        drop(replacement);
        drop(stale);
    }

    #[cfg(unix)]
    fn fake_unresponsive_tmux(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let binary = dir.join("unresponsive-tmux");
        std::fs::write(&binary, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        binary
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_deadline_bounds_current_discovery_probe() {
        let tmp = TempDir::new().unwrap();
        let work_scope = scope("deadline-current-discovery");
        let socket_path =
            socket_path_for_worktree(tmp.path(), Path::new("deadline-current-discovery"));
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), true);

        let error = registry
            .discover_persistent_identity(&work_scope, None, None, tokio::time::Instant::now())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            TmuxError::AmbiguousSocketIdentity { reason }
                if reason.contains("current") && reason.contains("Close deadline")
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_deadline_bounds_legacy_discovery_probe() {
        let tmp = TempDir::new().unwrap();
        let work_scope = scope("deadline-legacy-discovery");
        let legacy_path = Path::new("deadline-legacy-path");
        let legacy_socket = socket_path_for_worktree(tmp.path(), legacy_path);
        let _listener = std::os::unix::net::UnixListener::bind(&legacy_socket).unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), true);

        let error = registry
            .discover_persistent_identity(
                &work_scope,
                Some(legacy_path),
                None,
                tokio::time::Instant::now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            TmuxError::AmbiguousSocketIdentity { reason }
                if reason.contains("legacy") && reason.contains("Close deadline")
        ));
    }

    #[tokio::test]
    async fn close_deadline_bounds_rehydration_token_read() {
        let tmp = TempDir::new().unwrap();
        let binary = fake_unresponsive_tmux(tmp.path());
        let identity = TmuxServerInstanceIdentity {
            socket_path: tmp.path().join("rehydration-token.sock"),
            server_token: uuid::Uuid::new_v4().to_string(),
        };
        let state = TmuxRegistry::exact_identity_state_from_probe_with_binary(
            &identity,
            ProbeResult::Live,
            tokio::time::Instant::now() + Duration::from_millis(100),
            &binary,
        )
        .await
        .unwrap();
        assert!(matches!(
            state,
            ExactTmuxIdentityState::Ambiguous { reason }
                if reason.contains("token read") && reason.contains("Close deadline")
        ));
    }

    #[tokio::test]
    async fn close_deadline_bounds_stale_permit_verification() {
        let tmp = TempDir::new().unwrap();
        let binary = fake_unresponsive_tmux(tmp.path());
        let permit = TmuxRetirementPermit {
            work_scope: scope("deadline-stale-permit"),
            instance: TmuxServerInstanceIdentity {
                socket_path: tmp.path().join("stale-token.sock"),
                server_token: uuid::Uuid::new_v4().to_string(),
            },
            generation: TmuxRetirementGeneration(1),
            authority: TmuxRetirementAuthority::ExactServer,
            exact_process: None,
            had_entry: true,
            expires: tokio::time::Instant::now() + Duration::from_millis(100),
        };
        let outcome = TmuxRegistry::verify_exact_absence_from_probe_with_binary(
            &permit,
            ProbeResult::Live,
            permit.expires,
            &binary,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::IdentityNotProven { reason }
                if reason.contains("token read") && reason.contains("Close deadline")
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_deadline_bounds_teardown_command_and_leaves_residual_fence() {
        let tmp = TempDir::new().unwrap();
        let work_scope = scope("deadline-teardown");
        let socket_path = tmp.path().join("teardown.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let registry = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), true);
        let (entry, _) = registry.get_or_insert(&work_scope, socket_path).await;
        entry.server.write().await.status = ServerStatus::Live;
        let permit = registry
            .begin_retirement(&work_scope, None, None, tokio::time::Instant::now())
            .await
            .expect("uncontended retirement fence should be acquired");

        let outcome = registry.complete_retirement(&permit).await.unwrap();
        assert!(matches!(
            outcome,
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("teardown command") && reason.contains("Close deadline")
        ));
        assert!(registry.is_retirement_fenced(&work_scope).await);
    }

    #[tokio::test(start_paused = true)]
    async fn close_deadline_bounds_final_observer_without_resetting_budget() {
        let expires = tokio::time::Instant::now() + Duration::from_millis(250);
        tokio::time::advance(Duration::from_millis(200)).await;
        let waiter = tokio::spawn(async move {
            TmuxRegistry::wait_for_exact_shutdown_with(expires, Duration::from_millis(100), |_| {
                std::future::pending()
            })
            .await
            .unwrap()
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            waiter.await.unwrap(),
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("observation exceeded") && reason.contains("shutdown deadline")
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn exact_shutdown_waiter_allows_delayed_completion_before_deadline() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let observations = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&observations);
        let waiter = tokio::spawn(async move {
            TmuxRegistry::wait_for_exact_shutdown_with(
                tokio::time::Instant::now() + Duration::from_secs(1),
                Duration::from_millis(100),
                move |_| {
                    let attempt = observed.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok(if attempt < 4 {
                            ExactShutdownObservation::Outstanding {
                                reason: "shutdown still outstanding".to_string(),
                            }
                        } else {
                            ExactShutdownObservation::Complete
                        })
                    }
                },
            )
            .await
            .unwrap()
        });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        tokio::time::advance(Duration::from_millis(400)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            waiter.await.unwrap(),
            TmuxRetirementOutcome::AbsenceVerified
        );
        assert_eq!(observations.load(Ordering::SeqCst), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn exact_shutdown_waiter_deadline_routes_outstanding_shutdown_to_residual() {
        let waiter = tokio::spawn(async move {
            TmuxRegistry::wait_for_exact_shutdown_with(
                tokio::time::Instant::now() + Duration::from_millis(250),
                Duration::from_millis(100),
                |_| async {
                    Ok(ExactShutdownObservation::Outstanding {
                        reason: "exact server still live".to_string(),
                    })
                },
            )
            .await
            .unwrap()
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            waiter.await.unwrap(),
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("shutdown deadline") && reason.contains("exact server still live")
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn exact_shutdown_waiter_bounds_a_never_ready_observer() {
        let waiter = tokio::spawn(async move {
            TmuxRegistry::wait_for_exact_shutdown_with(
                tokio::time::Instant::now() + Duration::from_millis(250),
                Duration::from_millis(100),
                |_| std::future::pending(),
            )
            .await
            .unwrap()
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            waiter.await.unwrap(),
            TmuxRetirementOutcome::RemovalFailed { reason }
                if reason.contains("observation exceeded") && reason.contains("shutdown deadline")
        ));
    }

    #[tokio::test]
    async fn rehydrate_retirement_fresh_registry_reclaims_same_live_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("rehydrate-same-server");
        let bootstrap = owner.registry();
        let live = bootstrap
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("bootstrap live server");
        let persisted = {
            let live = live.read().await;
            TmuxServerInstanceIdentity {
                socket_path: live.socket_path.clone(),
                server_token: live.server_token.clone(),
            }
        };

        let restarted = owner.registry();
        let permit = match restarted
            .rehydrate_retirement(&scope, &persisted, close_deadline())
            .await
            .unwrap()
        {
            TmuxRetirementRehydration::Permit(permit) => permit,
            other @ (TmuxRetirementRehydration::AbsenceVerified
            | TmuxRetirementRehydration::Residual { .. }) => {
                panic!("expected exact rehydrated permit, got {other:?}")
            }
        };
        let outcome = restarted
            .complete_retirement(&permit)
            .await
            .expect("complete exact rehydrated retirement");
        assert_eq!(outcome, TmuxRetirementOutcome::Retired);
        assert!(matches!(
            probe(&persisted.socket_path).await.unwrap(),
            ProbeResult::NoSocket | ProbeResult::NoServer | ProbeResult::DeadSocket
        ));
        let _ = std::fs::remove_file(&persisted.socket_path);
        owner.shutdown();
    }

    #[tokio::test]
    async fn rehydrate_retirement_replacement_server_is_left_untouched() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("rehydrate-replacement");
        let bootstrap = owner.registry();
        let first = bootstrap
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("bootstrap live server");
        let persisted = {
            let first = first.read().await;
            TmuxServerInstanceIdentity {
                socket_path: first.socket_path.clone(),
                server_token: first.server_token.clone(),
            }
        };
        kill_socket(&persisted.socket_path).await;

        let replacement_registry = owner.registry();
        let replacement = replacement_registry
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("replacement live server");
        let replacement_token = replacement.read().await.server_token.clone();
        assert_ne!(replacement_token, persisted.server_token);

        let restarted = owner.registry();
        assert_eq!(
            restarted
                .rehydrate_retirement(&scope, &persisted, close_deadline())
                .await
                .unwrap(),
            TmuxRetirementRehydration::AbsenceVerified
        );
        let output = run_tmux_quiet_output(&persisted.socket_path, &["list-sessions"])
            .await
            .expect("replacement should still be reachable");
        assert!(
            output.status.success(),
            "replacement server must survive stale rehydration"
        );
        owner.shutdown();
    }

    #[tokio::test]
    async fn rehydrate_retirement_missing_server_proves_exact_absence() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("rehydrate-missing");
        let bootstrap = owner.registry();
        let live = bootstrap
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("bootstrap live server");
        let persisted = {
            let live = live.read().await;
            TmuxServerInstanceIdentity {
                socket_path: live.socket_path.clone(),
                server_token: live.server_token.clone(),
            }
        };
        kill_socket(&persisted.socket_path).await;

        let restarted = owner.registry();
        assert_eq!(
            restarted
                .rehydrate_retirement(&scope, &persisted, close_deadline())
                .await
                .unwrap(),
            TmuxRetirementRehydration::AbsenceVerified
        );
        let _ = std::fs::remove_file(&persisted.socket_path);
        owner.shutdown();
    }

    #[tokio::test]
    async fn retirement_absence_verification_does_not_kill_reopened_replacement() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let reg = owner.registry();
        let work_scope = scope("retirement-absence-replacement");

        let first = reg
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("first server");
        let stale_socket = first.read().await.socket_path.clone();
        let stale_token = first.read().await.server_token.clone();
        let permit = reg
            .begin_retirement(&work_scope, None, None, close_deadline())
            .await
            .expect("retirement fence should be acquired");
        assert_eq!(permit.instance.socket_path, stale_socket);
        assert_eq!(permit.instance.server_token, stale_token);

        reg.reopen_after_repair(&work_scope).await;
        kill_socket(&stale_socket).await;
        let replacement = reg
            .ensure_live(&work_scope, owner.path(), None, None)
            .await
            .expect("replacement server");
        let replacement_token = replacement.read().await.server_token.clone();
        assert_ne!(
            replacement_token, stale_token,
            "replacement must rotate token"
        );

        let outcome = reg
            .complete_retirement(&permit)
            .await
            .expect("retirement completion");
        assert_eq!(outcome, TmuxRetirementOutcome::AbsenceVerified);
        assert!(reg.get_existing(&work_scope).await.is_some());

        let output = run_tmux_quiet_output(&stale_socket, &["list-sessions"])
            .await
            .expect("replacement still reachable");
        assert!(
            output.status.success(),
            "replacement server must survive stale permit retirement"
        );
        owner.shutdown();
    }

    #[test]
    fn production_style_registry_ignores_owner_marker_for_spawn_dispatch() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".armed"), []).unwrap();
        let registry = TmuxRegistry::with_socket_dir(tmp.path().to_path_buf());
        assert!(!registry.contain_test_spawns);
        let owned = registry.with_test_spawn_containment();
        assert!(owned.contain_test_spawns);
    }

    #[tokio::test]
    async fn first_ensure_live_emits_settled_live_status() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = owner.registry_with_sink(Some(tx));

        let scope = scope("conv-first-ensure");
        let arc = reg
            .ensure_live(&scope, owner.path(), None, None)
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
        let _ = reg
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("noop");
        assert!(
            rx.try_recv().is_err(),
            "probe-noop on a live server must not re-emit"
        );

        owner.shutdown();
    }

    #[tokio::test]
    async fn respawn_rotates_server_token_to_fence_old_bindings() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let reg = owner.registry();
        let scope = scope("conv-rotate-server-token");

        let first = reg
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("first ensure_live should succeed");
        let socket_path = first.read().await.socket_path.clone();
        let first_token = first.read().await.server_token.clone();

        kill_socket(&socket_path).await;

        let second = reg
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("respawn ensure_live should succeed");
        let second_token = second.read().await.server_token.clone();

        assert_ne!(
            first_token, second_token,
            "respawning a missing/dead tmux server must rotate its token so stale wake bindings are fenced"
        );
        owner.shutdown();
    }

    #[tokio::test]
    async fn opaque_scope_adopts_live_legacy_worktree_socket() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let legacy_worktree = owner.path().join("legacy-worktree");
        std::fs::create_dir_all(&legacy_worktree).unwrap();
        let legacy_socket = socket_path_for_worktree(owner.path(), &legacy_worktree);
        spawn_session(
            &legacy_socket,
            &owner.path().join("missing.conf"),
            &legacy_worktree,
        )
        .await
        .unwrap();

        let reg = owner.registry();
        let opaque_scope = scope("opaque-after-migration");
        let server = reg
            .ensure_live(
                &opaque_scope,
                &legacy_worktree,
                Some(&legacy_worktree),
                None,
            )
            .await
            .unwrap();

        assert_eq!(server.read().await.socket_path, legacy_socket);
        owner.shutdown();
    }

    #[tokio::test]
    async fn persistent_discovery_finds_live_legacy_worktree_socket() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let legacy_worktree = owner.path().join("legacy-discovery-worktree");
        std::fs::create_dir_all(&legacy_worktree).unwrap();
        let legacy_socket = socket_path_for_worktree(owner.path(), &legacy_worktree);
        spawn_session(
            &legacy_socket,
            &owner.path().join("missing.conf"),
            &legacy_worktree,
        )
        .await
        .unwrap();

        let discovery = owner
            .registry()
            .discover_persistent_identity(
                &scope("opaque-discovery-after-migration"),
                Some(&legacy_worktree),
                None,
                close_deadline(),
            )
            .await
            .unwrap();
        assert!(matches!(
            discovery,
            PersistentTmuxDiscovery::Exact(identity) if identity.socket_path == legacy_socket
        ));
        owner.shutdown();
    }

    #[tokio::test]
    async fn stale_server_token_cannot_kill_window_on_replacement_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let reg = owner.registry();
        let scope = scope("conv-token-fenced-kill");
        let first = reg
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .unwrap();
        let socket_path = first.read().await.socket_path.clone();
        let stale_token = first.read().await.server_token.clone();
        kill_socket(&socket_path).await;

        let replacement = reg
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .unwrap();
        let replacement_token = replacement.read().await.server_token.clone();
        let output = run_tmux_quiet_output(
            &socket_path,
            &[
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{window_id}",
                "-n",
                "replacement",
            ],
        )
        .await
        .unwrap();
        let window_id = String::from_utf8(output.stdout).unwrap().trim().to_string();

        reg.kill_exact_window(&scope, &stale_token, &window_id)
            .await
            .unwrap();
        let still_live =
            run_tmux_quiet_output(&socket_path, &["list-windows", "-F", "#{window_id}"])
                .await
                .unwrap();
        assert!(
            String::from_utf8_lossy(&still_live.stdout)
                .lines()
                .any(|id| id == window_id),
            "a stale wake binding must not kill a reused window id on the replacement server"
        );

        reg.kill_exact_window(&scope, &replacement_token, &window_id)
            .await
            .unwrap();
        owner.shutdown();
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
        reg.emit_lifecycle(&scope("conv-X"));
    }

    #[test]
    fn current_and_legacy_probe_matrix_fails_closed() {
        let current = PathBuf::from("/current.sock");
        let legacy = PathBuf::from("/legacy.sock");
        for legacy_probe in [
            ProbeResult::NoSocket,
            ProbeResult::NoServer,
            ProbeResult::Live,
            ProbeResult::DeadSocket,
        ] {
            let selected = select_socket_from_probes(current.clone(), legacy.clone(), legacy_probe);
            match legacy_probe {
                ProbeResult::Live => assert_eq!(selected.unwrap(), legacy),
                ProbeResult::NoSocket | ProbeResult::NoServer => {
                    assert_eq!(selected.unwrap(), current);
                }
                ProbeResult::DeadSocket => {
                    assert!(matches!(
                        selected,
                        Err(TmuxError::AmbiguousSocketIdentity { .. })
                    ));
                }
            }
        }
        assert!(matches!(
            ambiguous_socket_probe(&current),
            TmuxError::AmbiguousSocketIdentity { .. }
        ));
    }

    #[tokio::test]
    async fn binary_unavailable_short_circuits_ensure_live() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        assert!(matches!(
            reg.ensure_live(&scope("conv-x"), tmp.path(), None, None)
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
    async fn cascade_on_delete_empty_registry_reclaims_persistent_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("cascade-persistent-empty-registry");
        let bootstrap = owner.registry();
        let live = bootstrap
            .ensure_live(&scope, owner.path(), None, None)
            .await
            .expect("bootstrap persistent server");
        let socket_path = live.read().await.socket_path.clone();

        let restarted = owner.registry();
        assert_eq!(restarted.conversation_count().await, 0);
        let report = restarted.cascade_on_delete(&scope, None, None, None).await;

        assert!(report.kill_server_error.is_none(), "{report:?}");
        assert!(matches!(
            probe(&socket_path).await.unwrap(),
            ProbeResult::NoSocket | ProbeResult::NoServer | ProbeResult::DeadSocket
        ));
        let _ = std::fs::remove_file(&socket_path);
        owner.shutdown();
    }

    #[tokio::test]
    async fn cascade_on_delete_empty_registry_reclaims_legacy_persistent_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = scope("cascade-persistent-legacy-empty-registry");
        let legacy_worktree = owner.path().join("legacy-worktree");
        std::fs::create_dir_all(&legacy_worktree).unwrap();
        let legacy_socket = socket_path_for_worktree(owner.path(), &legacy_worktree);
        spawn_session_owned(
            &legacy_socket,
            &owner.path().join(SERVER_CONFIG_FILENAME),
            &legacy_worktree,
            true,
        )
        .await
        .expect("bootstrap legacy persistent server");

        let restarted = owner.registry();
        let report = restarted
            .cascade_on_delete(&scope, None, Some(&legacy_worktree), None)
            .await;

        assert!(report.kill_server_error.is_none(), "{report:?}");
        assert_eq!(report.socket_path, legacy_socket);
        assert!(matches!(
            probe(&report.socket_path).await.unwrap(),
            ProbeResult::NoSocket | ProbeResult::NoServer | ProbeResult::DeadSocket
        ));
        let _ = std::fs::remove_file(&report.socket_path);
        owner.shutdown();
    }

    #[tokio::test]
    async fn cascade_on_delete_no_entry_attempts_socket_unlink() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        // No prior entry, no on-disk socket — cascade should be a no-op
        // that returns without errors.
        let scope = scope("never-existed");
        let report = reg.cascade_on_delete(&scope, None, None, None).await;
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

        let conv_scope = scope("conv-direct");
        let conv_sock = socket_path_for(tmp.path(), "conv-direct");
        let _ = reg.get_or_insert(&conv_scope, conv_sock).await;

        let wt_path = std::path::PathBuf::from("/tmp/phoenix-tmux-cascade-regression-wt");
        let wt_scope = scope("cascade-regression-wt");
        let wt_sock = socket_path_for_worktree(tmp.path(), &wt_path);
        let _ = reg.get_or_insert(&wt_scope, wt_sock).await;

        assert_eq!(
            reg.conversation_count().await,
            2,
            "precondition: both entries present"
        );

        let _ = reg.cascade_on_delete(&conv_scope, None, None, None).await;
        assert_eq!(
            reg.conversation_count().await,
            1,
            "Conversation-scope cascade must remove the Conversation-keyed entry"
        );

        let _ = reg.cascade_on_delete(&wt_scope, None, None, None).await;
        assert_eq!(
            reg.conversation_count().await,
            0,
            "Worktree-scope cascade must remove the Worktree-keyed entry"
        );
    }

    /// Every continuation retains its parent's durable `WorkScopeId`. Cascade
    /// must therefore skip kill/unlink when a successor inherits the scope.
    #[tokio::test]
    async fn cascade_on_delete_continuation_preserves_socket() {
        let tmp = TempDir::new().unwrap();
        let reg = TmuxRegistry::with_socket_dir_and_binary(tmp.path().to_path_buf(), false);
        let worktree = std::path::PathBuf::from("/tmp/phoenix-test-worktree-preserve");
        let socket_path = socket_path_for_worktree(tmp.path(), &worktree);
        std::fs::write(&socket_path, b"live").unwrap();

        let parent_scope = scope("worktree-preserve");
        let child_scope = parent_scope.clone();
        let report = reg
            .cascade_on_delete(&parent_scope, Some(&child_scope), None, None)
            .await;
        assert!(report.kill_server_error.is_none());
        assert!(report.unlink_error.is_none());
        assert!(
            socket_path.exists(),
            "continuation must preserve its WorkScope socket at {}",
            socket_path.display()
        );
        // Cleanup so the file doesn't leak into the next test run.
        let _ = std::fs::remove_file(&socket_path);
    }
}
