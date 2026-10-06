//! External effects behind the tmux registry and `tmux_run`: tmux CLI
//! invocations, socket-endpoint observation, and exact process liveness.
//! [`SystemTmuxBackend`] is the production implementation; ADR-087 records why
//! the seam exists.

use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use phoenix_core::process_identity::{
    current_process_identity, process_identity_matches, ProcessIdentity,
};

use super::probe::{command_output, probe_until, ProbeResult};
use super::registry::{TmuxError, TMUX_DEFAULT_SESSION};

pub(crate) const SERVER_TOKEN_VAR: &str = "PHOENIX_TMUX_SERVER_TOKEN";

// Bound on the post-spawn pane-readiness poll: 50 * 100ms = 5s ceiling.
// Conservative — under normal load the pane is ready on the first probe.
const PANE_READY_MAX_ATTEMPTS: u32 = 50;
const PANE_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

const TMUX_RUN_SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(10);

/// The `phx`-companion setup version stamped into a tmux server's global
/// environment under [`COMPANION_VERSION_VAR`]. Bump it whenever the PTY env
/// injection or the terminal-features that `phx` / OSC-8 run-links depend on
/// change, so a server spawned under an older version is brought up to date on
/// reuse (see `refresh_companion_if_stale`). A server with the current stamp is
/// left untouched.
pub(crate) const COMPANION_ENV_VERSION: &str = "1";
pub(crate) const COMPANION_VERSION_VAR: &str = "PHOENIX_COMPANION_VERSION";

/// Device and inode of one socket-file incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketFileIdentity {
    pub device: u64,
    pub inode: u64,
}

/// What exists at a socket endpoint path, observed without following symlinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketEndpoint {
    Socket(SocketFileIdentity),
    Absent,
    NotSocket,
}

/// Exact-identity process observation. `DeadOrReused` is the only state that
/// proves the identified process is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactProcessState {
    Live,
    DeadOrReused,
    Unproven,
}

/// One read of a tmux server's global environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalEnvRead {
    Value(String),
    /// The command failed, or the variable is unset.
    Unreadable,
    DeadlineExceeded,
}

impl GlobalEnvRead {
    pub(crate) fn into_value(self) -> Option<String> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unreadable | Self::DeadlineExceeded => None,
        }
    }
}

/// Result of the token-bound `kill-server` used by exact retirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenBoundKill {
    Killed,
    TokenMismatch,
    DeadlineExceeded,
    Failed { reason: String },
}

#[async_trait]
pub trait TmuxBackend: Send + Sync + std::fmt::Debug {
    fn binary_available(&self) -> bool;

    /// `Ok(None)` means `expires` passed before the probe finished.
    async fn probe(
        &self,
        socket_path: &Path,
        expires: Option<Instant>,
    ) -> std::io::Result<Option<ProbeResult>>;

    /// # Errors
    /// Returns the metadata error when it is not `NotFound`.
    fn endpoint(&self, path: &Path) -> std::io::Result<SocketEndpoint>;

    async fn remove_endpoint(&self, path: &Path) -> std::io::Result<()>;

    async fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>>;

    /// Start a detached `main` session whose server publishes a fresh token.
    async fn spawn_session(
        &self,
        socket_path: &Path,
        config_path: &Path,
        cwd: &Path,
    ) -> Result<(), TmuxError>;

    async fn global_env(
        &self,
        socket_path: &Path,
        var: &str,
        expires: Option<Instant>,
    ) -> GlobalEnvRead;

    async fn set_global_env(&self, socket_path: &Path, var: &str, value: &str);

    async fn refresh_companion_if_stale(&self, socket_path: &Path);

    /// Exact identity of the server process, only when that server reports
    /// `expected_token`.
    async fn server_process(
        &self,
        socket_path: &Path,
        expected_token: &str,
        expires: Instant,
    ) -> Option<ProcessIdentity>;

    async fn kill_server_if_token(
        &self,
        socket_path: &Path,
        config_path: &Path,
        expected_token: &str,
        expires: Instant,
    ) -> TokenBoundKill;

    /// `Ok(None)` when tmux reports no such window.
    async fn capture_pane(
        &self,
        socket_path: &Path,
        window_id: &str,
    ) -> std::io::Result<Option<String>>;

    async fn kill_window(&self, socket_path: &Path, window_id: &str);

    fn process_state(&self, process: ProcessIdentity) -> ExactProcessState;

    async fn wait_process_exit(
        &self,
        process: ProcessIdentity,
        expires: Instant,
    ) -> ExactProcessState;

    /// Run one tmux CLI command for `tmux_run`, with Phoenix's `-f`/`-S` first.
    async fn run_cli(
        &self,
        config_path: &Path,
        socket_path: &Path,
        args: &[String],
    ) -> Result<Output, String>;
}

#[derive(Debug)]
pub struct SystemTmuxBackend {
    binary_available: bool,
}

impl SystemTmuxBackend {
    /// `which::which("tmux")` is called once here and cached for the
    /// backend's lifetime (REQ-TMUX-003 "Binary Availability Detection").
    #[must_use]
    pub fn detect() -> Self {
        Self {
            binary_available: which::which("tmux").is_ok(),
        }
    }
}

#[async_trait]
impl TmuxBackend for SystemTmuxBackend {
    fn binary_available(&self) -> bool {
        self.binary_available
    }

    async fn probe(
        &self,
        socket_path: &Path,
        expires: Option<Instant>,
    ) -> std::io::Result<Option<ProbeResult>> {
        match expires {
            Some(expires) => probe_until(socket_path, expires).await,
            None => super::probe::probe(socket_path).await.map(Some),
        }
    }

    fn endpoint(&self, path: &Path) -> std::io::Result<SocketEndpoint> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                Ok(SocketEndpoint::Socket(SocketFileIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                }))
            }
            Ok(_) => Ok(SocketEndpoint::NotSocket),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SocketEndpoint::Absent)
            }
            Err(error) => Err(error),
        }
    }

    async fn remove_endpoint(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::remove_file(path).await
    }

    async fn list_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut paths = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            paths.push(entry.path());
        }
        Ok(paths)
    }

    async fn spawn_session(
        &self,
        socket_path: &Path,
        config_path: &Path,
        cwd: &Path,
    ) -> Result<(), TmuxError> {
        spawn_session(socket_path, config_path, cwd).await
    }

    async fn global_env(
        &self,
        socket_path: &Path,
        var: &str,
        expires: Option<Instant>,
    ) -> GlobalEnvRead {
        match tmux_global_env_output(socket_path, var, expires).await {
            Ok(Some(output)) => parse_tmux_global_env(&output, var)
                .map_or(GlobalEnvRead::Unreadable, GlobalEnvRead::Value),
            Ok(None) => GlobalEnvRead::DeadlineExceeded,
            Err(_) => GlobalEnvRead::Unreadable,
        }
    }

    async fn set_global_env(&self, socket_path: &Path, var: &str, value: &str) {
        run_tmux_quiet(socket_path, &["set-environment", "-g", var, value]).await;
    }

    async fn refresh_companion_if_stale(&self, socket_path: &Path) {
        refresh_companion_if_stale(socket_path).await;
    }

    async fn server_process(
        &self,
        socket_path: &Path,
        expected_token: &str,
        expires: Instant,
    ) -> Option<ProcessIdentity> {
        exact_server_process_identity_until(socket_path, expected_token, expires).await
    }

    async fn kill_server_if_token(
        &self,
        socket_path: &Path,
        config_path: &Path,
        expected_token: &str,
        expires: Instant,
    ) -> TokenBoundKill {
        let token_test = format!("#{{==:#{{E:{SERVER_TOKEN_VAR}}},{expected_token}}}");
        let mut command = tokio::process::Command::new("tmux");
        command
            .arg("-f")
            .arg(config_path)
            .arg("-S")
            .arg(socket_path)
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
        match command_output(command, Some(expires)).await {
            Ok(None) => TokenBoundKill::DeadlineExceeded,
            Ok(Some(output)) if output.status.success() => {
                if String::from_utf8_lossy(&output.stdout).contains("PHOENIX_TOKEN_MISMATCH") {
                    TokenBoundKill::TokenMismatch
                } else {
                    TokenBoundKill::Killed
                }
            }
            Ok(Some(output)) => TokenBoundKill::Failed {
                reason: format!(
                    "exact token-bound kill-server failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            },
            Err(error) => TokenBoundKill::Failed {
                reason: error.to_string(),
            },
        }
    }

    async fn capture_pane(
        &self,
        socket_path: &Path,
        window_id: &str,
    ) -> std::io::Result<Option<String>> {
        let output = run_tmux_quiet_output(
            socket_path,
            &["capture-pane", "-p", "-t", window_id, "-S", "-2000"],
        )
        .await?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    async fn kill_window(&self, socket_path: &Path, window_id: &str) {
        run_tmux_quiet(socket_path, &["kill-window", "-t", window_id]).await;
    }

    fn process_state(&self, process: ProcessIdentity) -> ExactProcessState {
        exact_process_state(process)
    }

    async fn wait_process_exit(
        &self,
        process: ProcessIdentity,
        expires: Instant,
    ) -> ExactProcessState {
        exact_process_exit_until(process, expires).await
    }

    async fn run_cli(
        &self,
        config_path: &Path,
        socket_path: &Path,
        args: &[String],
    ) -> Result<Output, String> {
        let mut full_args = vec![
            "-f".to_string(),
            config_path.to_string_lossy().into_owned(),
            "-S".to_string(),
            socket_path.to_string_lossy().into_owned(),
        ];
        full_args.extend(args.iter().cloned());

        tokio::time::timeout(TMUX_RUN_SUBPROCESS_TIMEOUT, async move {
            let mut command = tokio::process::Command::new("tmux");
            command
                .args(&full_args)
                .env_remove("TMUX")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            command.output().await
        })
        .await
        .map_err(|_| "tmux subprocess timed out".to_string())?
        .map_err(|e| format!("failed to spawn tmux subprocess: {e}"))
    }
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

/// Set an explicit environment on a tmux command so the spawned server (and
/// thus its pane shells) match the direct-shell PTY contract: the fixed base
/// env plus the `PtyEnvInjection` (the `phx` shim on PATH, `PHOENIX_API_URL`,
/// `PHOENIX_SUGGEST_TOKEN`) and the safe-var allowlist — never a blind copy of
/// the Phoenix process environment, which would leak server secrets (LLM API
/// keys, gateway config) into every tmux-backed terminal. `build_env_for_tmux`
/// is the single source for that env (`specs/terminal` REQ-TERM-002).
fn tmux_server_env(server_token: &str) -> Vec<(String, String)> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());
    let launch_uuid = uuid::Uuid::new_v4().to_string();
    tmux_server_env_for(server_token, &shell, &launch_uuid)
}

fn tmux_server_env_for(
    server_token: &str,
    shell: &str,
    launch_uuid: &str,
) -> Vec<(String, String)> {
    let mut env = phoenix_terminal::spawn::build_env_for_tmux(shell, launch_uuid);
    env.push((
        COMPANION_VERSION_VAR.to_owned(),
        COMPANION_ENV_VERSION.to_owned(),
    ));
    env.push((SERVER_TOKEN_VAR.to_owned(), server_token.to_owned()));
    env
}

fn set_tmux_server_env(cmd: &mut tokio::process::Command, env: &[(String, String)]) {
    cmd.env_clear();
    cmd.envs(env.iter().cloned());
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

async fn run_tmux_quiet_output(socket_path: &Path, args: &[&str]) -> std::io::Result<Output> {
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
}

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
    let output = command_output(command, Some(expires)).await.ok()??;
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

async fn tmux_global_env_output(
    socket_path: &Path,
    var: &str,
    expires: Option<tokio::time::Instant>,
) -> std::io::Result<Option<Output>> {
    let mut command = tokio::process::Command::new("tmux");
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

/// `tmux show-environment -g VAR` prints `VAR=value` when set and `-VAR` when
/// not.
fn parse_tmux_global_env(out: &Output, var: &str) -> Option<String> {
    if !out.status.success() {
        return None;
    }
    let prefix = format!("{var}=");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
}

async fn tmux_global_env(socket_path: &Path, var: &str) -> Option<String> {
    let output = tmux_global_env_output(socket_path, var, None)
        .await
        .ok()??;
    parse_tmux_global_env(&output, var)
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
/// # Errors
/// Returns a [`TmuxError`] when the `tmux new-session` process fails to
/// spawn or exits non-zero.
pub async fn spawn_session(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
) -> Result<(), TmuxError> {
    let server_token = uuid::Uuid::new_v4().to_string();
    let server_env = tmux_server_env(&server_token);
    let tmux_args = tmux_new_session_args(socket_path, config_path, cwd, &server_token);
    let mut command = tokio::process::Command::new("tmux");
    command.args(&tmux_args);
    set_tmux_server_env(&mut command, &server_env);
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| TmuxError::SpawnFailed {
            socket_path: socket_path.to_path_buf(),
            reason: format!("failed to invoke tmux: {error}"),
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
    wait_for_spawned_pane(socket_path, config_path).await
}

fn tmux_new_session_args(
    socket_path: &Path,
    config_path: &Path,
    cwd: &Path,
    server_token: &str,
) -> Vec<String> {
    vec![
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
        ";".to_string(),
        "set-environment".to_string(),
        "-g".to_string(),
        SERVER_TOKEN_VAR.to_string(),
        server_token.to_owned(),
    ]
}

async fn wait_for_spawned_pane(socket_path: &Path, config_path: &Path) -> Result<(), TmuxError> {
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
            .output()
            .await
            .map_err(|error| TmuxError::SpawnFailed {
                socket_path: socket_path.to_path_buf(),
                reason: format!("failed to probe pane readiness: {error}"),
            })?;
        if panes.status.success() && !panes.stdout.is_empty() {
            return Ok(());
        }
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
        reason: format!("session spawned but pane never became ready after {PANE_READY_MAX_ATTEMPTS} probes (last: {last_diag})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn production_spawn_builds_exact_pane_environment() {
        let token = "test-server-token";
        let shell = "/bin/test-shell";
        let launch_id = "test-launch-id";
        let expected = tmux_server_env_for(token, shell, launch_id)
            .into_iter()
            .collect::<HashMap<_, _>>();
        let base = phoenix_terminal::spawn::build_env_for_tmux(shell, launch_id)
            .into_iter()
            .collect::<HashMap<_, _>>();
        for (key, value) in base {
            assert_eq!(
                expected.get(&key),
                Some(&value),
                "base environment key {key}"
            );
        }
        assert_eq!(
            expected.get(SERVER_TOKEN_VAR).map(String::as_str),
            Some(token)
        );
        assert_eq!(
            expected.get(COMPANION_VERSION_VAR).map(String::as_str),
            Some(COMPANION_ENV_VERSION)
        );
        assert_eq!(
            expected.len(),
            phoenix_terminal::spawn::build_env_for_tmux(shell, launch_id).len() + 2
        );
        assert!(!expected.contains_key("OPENAI_API_KEY"));
        assert!(!expected.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn new_session_publishes_token_in_the_same_tmux_command() {
        let args = tmux_new_session_args(
            Path::new("/sock"),
            Path::new("/conf"),
            Path::new("/cwd"),
            "token-1",
        );
        assert_eq!(
            args,
            [
                "-f",
                "/conf",
                "-S",
                "/sock",
                "new-session",
                "-d",
                "-c",
                "/cwd",
                "-s",
                TMUX_DEFAULT_SESSION,
                ";",
                "set-environment",
                "-g",
                SERVER_TOKEN_VAR,
                "token-1",
            ]
        );
    }

    #[test]
    fn global_env_parser_reads_only_the_requested_set_variable() {
        use std::os::unix::process::ExitStatusExt as _;
        let output = |code: i32, stdout: &str| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            parse_tmux_global_env(
                &output(0, "PHOENIX_TMUX_SERVER_TOKEN=abc\n"),
                SERVER_TOKEN_VAR
            ),
            Some("abc".to_string())
        );
        assert_eq!(
            parse_tmux_global_env(&output(0, "-PHOENIX_TMUX_SERVER_TOKEN\n"), SERVER_TOKEN_VAR),
            None
        );
        assert_eq!(
            parse_tmux_global_env(
                &output(1, "PHOENIX_TMUX_SERVER_TOKEN=abc\n"),
                SERVER_TOKEN_VAR
            ),
            None
        );
    }

    #[test]
    fn endpoint_distinguishes_absence_from_unknown_incarnation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let backend = SystemTmuxBackend {
            binary_available: false,
        };
        assert_eq!(
            backend.endpoint(&tmp.path().join("missing.sock")).unwrap(),
            SocketEndpoint::Absent
        );
        let unknown = tmp.path().join("not-a-socket");
        std::fs::write(&unknown, b"not an owned socket incarnation").unwrap();
        assert_eq!(
            backend.endpoint(&unknown).unwrap(),
            SocketEndpoint::NotSocket
        );
        let socket = tmp.path().join("bound.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(matches!(
            backend.endpoint(&socket).unwrap(),
            SocketEndpoint::Socket(_)
        ));
    }
}
