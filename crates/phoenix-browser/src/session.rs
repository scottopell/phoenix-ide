//! Browser session management
//!
//! REQ-BT-010: Implicit Session Model
//! REQ-BT-011: State Persistence
//! REQ-BROWSER-WS-001: Sessions keyed by `ResourceScopeKey` so continuations share Chrome.

use chromiumoxide::{
    browser::{Browser, BrowserConfig},
    cdp::js_protocol::runtime::{ConsoleApiCalledType, EventConsoleApiCalled, RemoteObject},
    fetcher::{BrowserFetcher, BrowserFetcherOptions, BrowserKind},
    Page,
};
use futures::StreamExt;
use serde::{Serialize, Serializer};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;

use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::{EffectiveResourceAccess, ResourceAuthority, ResourceScopeKey};

/// Derive a Chrome user data dir from a browser session key.
///
/// Hash the key to a bounded filesystem-safe component while preserving
/// deterministic profile reuse for the same browser session identity.
///
/// SHA-256 is used (rather than `DefaultHasher`) so the derivation is
/// stable across Rust/Phoenix releases — a toolchain upgrade must not
/// orphan an existing on-disk Chrome profile by re-keying.
fn user_data_dir_for_key(scope_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(scope_key.as_bytes());
    let digest = h.finalize();
    let prefix = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 digest is 32 bytes; first 8 always fits a u64"),
    );
    format!("{USER_DATA_DIR_PREFIX}{prefix:016x}")
}

/// Filesystem prefix shared by every per-scope Chrome user-data directory.
/// Profiles are created at `{USER_DATA_DIR_PREFIX}{16-hex}` by
/// [`user_data_dir_for_key`].
const USER_DATA_DIR_PREFIX: &str = "/tmp/phoenix-chrome-";

/// Glob matching every per-scope Chrome user-data directory. Exposed so the
/// About-this-deployment endpoint can report this (known-large, unsized)
/// ephemeral location without enumerating individual profiles.
#[must_use]
pub fn user_data_dir_glob() -> String {
    format!("{USER_DATA_DIR_PREFIX}*")
}

/// Maximum console log entries to keep per session
const MAX_CONSOLE_LOGS: usize = 1000;

/// Idle timeout before session cleanup (30 minutes)
const IDLE_TIMEOUT: Duration = Duration::from_mins(30);

/// Cleanup check interval (60 seconds)
const CLEANUP_INTERVAL: Duration = Duration::from_mins(1);

// Per-phase ceiling on the individual cold-start CDP steps — browser launch,
// first-page open, helper inject, and each listener setup. chromiumoxide's
// awaits are otherwise unbounded, so a stalled subprocess or wedged CDP socket
// hangs the caller forever. Applied per step (not around the whole of
// BrowserSession::new) so the legitimate first-run chromium DOWNLOAD via
// BrowserFetcher::fetch() is NOT bounded by it. Task 45001.
const SESSION_INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default viewport dimensions
const DEFAULT_VIEWPORT_WIDTH: u32 = 1024;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 768;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("Browser operation failed: {0}")]
    OperationFailed(String),

    #[error("Session not found for conversation: {0}")]
    SessionNotFound(String),

    #[error("Chrome not available: {0}")]
    ChromeNotAvailable(String),

    #[error("Browser session init timed out after {0:?}")]
    InitTimeout(Duration),

    #[error("browser session access denied for this actor")]
    AccessDenied,
}

impl From<chromiumoxide::error::CdpError> for BrowserError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        BrowserError::OperationFailed(e.to_string())
    }
}

/// Browser console message level.
///
/// Phoenix-owned enum mirroring the CDP `Runtime.consoleAPICalled.type` value
/// set (see chromiumoxide's `ConsoleApiCalledType`). Stored on
/// [`ConsoleEntry`] as a typed value rather than a string so a level the LLM
/// or UI filters on (e.g. `"error"`) cannot drift onto an arbitrary string.
/// The [`From<ConsoleApiCalledType>`] impl is a total match: if chromiumoxide
/// adds a CDP variant, conversion stops compiling rather than silently
/// flattening to an unknown level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleLevel {
    Log,
    Debug,
    Info,
    Warning,
    Error,
    Dir,
    Dirxml,
    Table,
    Trace,
    Clear,
    StartGroup,
    StartGroupCollapsed,
    EndGroup,
    Assert,
    Profile,
    ProfileEnd,
    Count,
    TimeEnd,
}

impl ConsoleLevel {
    /// Canonical CDP wire identifier (`"log"`, `"warning"`, `"startGroup"`, …).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Dir => "dir",
            Self::Dirxml => "dirxml",
            Self::Table => "table",
            Self::Trace => "trace",
            Self::Clear => "clear",
            Self::StartGroup => "startGroup",
            Self::StartGroupCollapsed => "startGroupCollapsed",
            Self::EndGroup => "endGroup",
            Self::Assert => "assert",
            Self::Profile => "profile",
            Self::ProfileEnd => "profileEnd",
            Self::Count => "count",
            Self::TimeEnd => "timeEnd",
        }
    }
}

impl std::fmt::Display for ConsoleLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ConsoleLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl From<ConsoleApiCalledType> for ConsoleLevel {
    fn from(t: ConsoleApiCalledType) -> Self {
        match t {
            ConsoleApiCalledType::Log => Self::Log,
            ConsoleApiCalledType::Debug => Self::Debug,
            ConsoleApiCalledType::Info => Self::Info,
            ConsoleApiCalledType::Warning => Self::Warning,
            ConsoleApiCalledType::Error => Self::Error,
            ConsoleApiCalledType::Dir => Self::Dir,
            ConsoleApiCalledType::Dirxml => Self::Dirxml,
            ConsoleApiCalledType::Table => Self::Table,
            ConsoleApiCalledType::Trace => Self::Trace,
            ConsoleApiCalledType::Clear => Self::Clear,
            ConsoleApiCalledType::StartGroup => Self::StartGroup,
            ConsoleApiCalledType::StartGroupCollapsed => Self::StartGroupCollapsed,
            ConsoleApiCalledType::EndGroup => Self::EndGroup,
            ConsoleApiCalledType::Assert => Self::Assert,
            ConsoleApiCalledType::Profile => Self::Profile,
            ConsoleApiCalledType::ProfileEnd => Self::ProfileEnd,
            ConsoleApiCalledType::Count => Self::Count,
            ConsoleApiCalledType::TimeEnd => Self::TimeEnd,
        }
    }
}

/// Console log entry captured from the browser
#[derive(Debug, Clone)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub text: String,
    pub timestamp: Instant,
}

/// Conversation-scoped performance-profiling state (REQ-BT-019).
///
/// Models the three independent capture sub-machines plus the CPU-throttle
/// override and trace buffer from `browser-profiling.allium`. It hangs off
/// [`BrowserSession`] so the precondition gates are enforced per conversation
/// (invariant `ProfilingConversationScoped`) and reset structurally when the
/// session dies (rule `SessionDeathResetsAllCaptures`) — dropping the session
/// drops this state, no stop edge is issued to a dead browser.
#[derive(Debug, Default)]
pub struct ProfilingState {
    /// CPU sampling sub-machine (REQ-BT-019.7). idle when false.
    pub cpu_active: bool,
    /// Tracing sub-machine (REQ-BT-019.9). idle when false.
    pub tracing_active: bool,
    /// JS coverage sub-machine (REQ-BT-019.11). idle when false.
    pub coverage_active: bool,
    /// CPU-throttle override (REQ-BT-019.2). `None` = browser default; a
    /// `Some(rate)` always carries `rate >= 1` (invariant
    /// `ThrottleRateWellFormed`), set only via the gated throttle action.
    pub throttle_rate: Option<f64>,
    /// Buffered `Tracing.dataCollected` events. Non-empty only while
    /// `tracing_active` (invariant `TraceBufferOnlyWhileActive`): cleared
    /// before arming on start, drained-then-cleared on stop.
    pub trace_events: Vec<serde_json::Value>,
}

/// Per-conversation browser instance
pub struct BrowserSession {
    #[allow(dead_code)] // Browser must stay alive
    browser: Browser,
    chrome_pid: Option<u32>,
    #[allow(dead_code)] // Task must stay alive
    handler_task: JoinHandle<()>,
    #[allow(dead_code)] // Task must stay alive
    console_task: Option<JoinHandle<()>>,
    /// Background tasks for the profiling listener (trace dataCollected /
    /// tracingComplete). Aborted on drop alongside `console_task`.
    profiling_tasks: Vec<JoinHandle<()>>,
    /// The current page (public for tool access)
    pub page: Page,
    /// Conversation-scoped profiling state (REQ-BT-019). The
    /// `browser_profile` tool reads/mutates this through the session guard;
    /// the trace-listener task appends events when `tracing_active`.
    pub profiling: Arc<StdMutex<ProfilingState>>,
    /// Notified by the profiling listener when `Tracing.tracingComplete`
    /// fires, so `trace_stop` can drain the buffer only once all
    /// asynchronously-delivered events have arrived.
    pub trace_complete: Arc<tokio::sync::Notify>,
    /// Console logs captured from the page (separate lock to avoid contention)
    pub console_logs: Arc<StdMutex<VecDeque<ConsoleEntry>>>,
    /// Signals capture of a console event so consumers can await evidence rather
    /// than sleeping for background delivery.
    console_event: Arc<tokio::sync::Notify>,
    /// Last activity timestamp (for idle timeout)
    pub last_activity: Instant,
    /// Lazily-created live-view broker (REQ-BT-018). The slot holds a
    /// `Weak` so the broker is kept alive only by attached viewers; when
    /// the last viewer drops, the broker drops, and `Page.stopScreencast`
    /// fires automatically.
    screencast: Arc<tokio::sync::Mutex<std::sync::Weak<crate::screencast::ScreencastBroker>>>,
}

/// Maximum bytes stored per console arg in the capture buffer.
/// This is a memory-protection cap only — display truncation happens
/// at retrieval time in `browser_recent_console_logs`, not here.
const MAX_CAPTURE_ARG_BYTES: usize = 10_000;

/// Extract a human-readable string from a CDP `RemoteObject` console arg.
///
/// Priority:
/// 1. `value` field — present for primitives; strings unwrapped, others JSON-serialized
/// 2. `preview` field — for objects/arrays, reconstructs a `{k: v}` or `[v]` representation
/// 3. `description` field — fallback string representation (e.g. "Object", "Array(3)")
/// 4. `unserializable_value` — for `undefined`, `NaN`, `Infinity`, etc.
///
/// Output is truncated to `MAX_ARG_TEXT_LEN` characters.
pub(crate) fn extract_console_arg_text(arg: &RemoteObject) -> String {
    // 1. JSON value present (primitives and some serializable objects)
    if let Some(value) = &arg.value {
        let raw = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => value.to_string(),
        };
        return cap_for_memory(raw);
    }

    // 2. Preview: reconstruct a readable representation for objects/arrays
    if let Some(preview) = &arg.preview {
        use chromiumoxide::cdp::js_protocol::runtime::ObjectPreviewSubtype;
        let is_array = preview
            .subtype
            .as_ref()
            .is_some_and(|s| matches!(s, ObjectPreviewSubtype::Array));

        let props: Vec<String> = preview
            .properties
            .iter()
            .map(|p| {
                let val = p.value.as_deref().unwrap_or("…");
                if is_array {
                    val.to_string()
                } else {
                    format!("{}: {}", p.name, val)
                }
            })
            .collect();

        let overflow = if preview.overflow { ", …" } else { "" };
        let raw = if is_array {
            format!("[{}{}]", props.join(", "), overflow)
        } else {
            format!("{{{}{}}}", props.join(", "), overflow)
        };
        return cap_for_memory(raw);
    }

    // 3. Description fallback ("Object", "Array(3)", function source, etc.)
    if let Some(desc) = &arg.description {
        return cap_for_memory(desc.clone());
    }

    // 4. Unserializable values (undefined, NaN, Infinity, -Infinity, etc.)
    if let Some(unser) = &arg.unserializable_value {
        return cap_for_memory(unser.inner().clone());
    }

    cap_for_memory(String::from("[unknown]"))
}

/// Cap a captured arg string at `MAX_CAPTURE_ARG_BYTES` to protect memory.
/// This is NOT the display truncation — see `truncate_for_display`.
fn cap_for_memory(s: String) -> String {
    truncate_unicode_safe(s, MAX_CAPTURE_ARG_BYTES)
}

/// Truncate a string to at most `max_bytes` bytes at a valid UTF-8 char boundary,
/// appending `…` if truncation occurred.
#[must_use]
pub fn truncate_unicode_safe(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    // Map each char to its *end* byte position; keep those that fit within max_bytes.
    let boundary = s
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| end <= max_bytes)
        .last()
        .unwrap_or(0);
    // Safety: `boundary` is computed from `char_indices()` on `s`
    #[allow(clippy::string_slice)]
    let prefix = &s[..boundary];
    format!("{prefix}…")
}

/// Directory where the fetcher caches downloaded Chrome binaries. Resolved
/// through [`PhoenixRuntimeEnvironment`] so it agrees with the path the
/// deployment-info page reports.
#[must_use]
pub fn fetcher_cache_dir() -> PathBuf {
    PhoenixRuntimeEnvironment::detect().chromium_cache_dir()
}

#[derive(Debug)]
enum BrowserTerminationError {
    KillFailed(String),
    KillTimedOut,
}

impl std::fmt::Display for BrowserTerminationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KillFailed(error) => write!(formatter, "browser kill failed: {error}"),
            Self::KillTimedOut => formatter.write_str("browser kill timed out"),
        }
    }
}

impl BrowserSession {
    /// Directory where the fetcher caches downloaded Chrome binaries
    pub(crate) fn fetcher_cache_dir() -> PathBuf {
        fetcher_cache_dir()
    }

    /// Build a `BrowserConfig` with optional explicit Chrome executable path.
    /// The Chrome user data dir is derived from the browser session key.
    fn browser_config(
        session_key: &str,
        executable: Option<&Path>,
    ) -> Result<BrowserConfig, BrowserError> {
        let user_data_dir = user_data_dir_for_key(session_key);

        // Remove stale user data directory to avoid Chrome SingletonLock conflicts
        // (e.g. from a previous crash or test run that didn't clean up)
        let _ = std::fs::remove_dir_all(&user_data_dir);

        let mut builder = BrowserConfig::builder()
            .new_headless_mode()
            .no_sandbox()
            .arg("--disable-gpu")
            .arg("--disable-software-rasterizer")
            .user_data_dir(&user_data_dir)
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
                device_scale_factor: Some(1.0),
                emulating_mobile: false,
                is_landscape: true,
                has_touch: false,
            });

        if let Some(path) = executable {
            builder = builder.chrome_executable(path);
        }

        builder
            .build()
            .map_err(|e| BrowserError::LaunchFailed(e.clone()))
    }

    /// Launch browser and create a session
    async fn launch_and_init(
        scope_key: &str,
        executable: Option<&Path>,
    ) -> Result<Self, BrowserError> {
        let config = Self::browser_config(scope_key, executable)?;

        // Browser::launch can hang on a wedged chromium subprocess. Bound it.
        // If launch itself times out there is no browser handle to clean up.
        let (mut browser, mut handler) =
            match tokio::time::timeout(SESSION_INIT_TIMEOUT, Browser::launch(config)).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => return Err(BrowserError::LaunchFailed(e.to_string())),
                Err(_) => return Err(BrowserError::InitTimeout(SESSION_INIT_TIMEOUT)),
            };
        let chrome_pid = browser
            .get_mut_child()
            .and_then(|child| child.as_mut_inner().id());

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    tracing::warn!("CDP handler error: {e}");
                }
            }
        });

        // new_page can hang on a wedged CDP socket after launch. Bound it, and
        // on timeout/error kill the chromium we already launched so the failure
        // path doesn't orphan a process behind the returned error.
        let page = match tokio::time::timeout(SESSION_INIT_TIMEOUT, browser.new_page("about:blank"))
            .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(e)) => {
                handler_task.abort();
                let _ = browser.kill().await;
                return Err(BrowserError::LaunchFailed(e.to_string()));
            }
            Err(_) => {
                handler_task.abort();
                let _ = browser.kill().await;
                return Err(BrowserError::InitTimeout(SESSION_INIT_TIMEOUT));
            }
        };

        // Auto-inject the __phoenix React helper into every future document.
        // Runs before page JS, so React registers its fiber roots into our hook
        // at startup. Harmless on non-React pages. See react.rs for full docs.
        // Best-effort and bounded: a wedged socket here must not hang init, and
        // the browser is already usable without the helper.
        let inject_params =
            chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams::new(
                crate::react::PHOENIX_REACT_HELPER_SCRIPT.to_string(),
            );
        match tokio::time::timeout(SESSION_INIT_TIMEOUT, page.execute(inject_params)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("Failed to auto-inject React helper: {e}"),
            Err(_) => tracing::warn!("Auto-inject React helper timed out"),
        }

        Ok(Self {
            browser,
            chrome_pid,
            handler_task,
            console_task: None,
            profiling_tasks: Vec::new(),
            page,
            profiling: Arc::new(StdMutex::new(ProfilingState::default())),
            trace_complete: Arc::new(tokio::sync::Notify::new()),
            console_logs: Arc::new(StdMutex::new(VecDeque::with_capacity(MAX_CONSOLE_LOGS))),
            console_event: Arc::new(tokio::sync::Notify::new()),
            last_activity: Instant::now(),
            screencast: Arc::new(tokio::sync::Mutex::new(std::sync::Weak::new())),
        })
    }

    /// Create a new browser session.
    ///
    /// Order of attempts:
    ///   1. `PHOENIX_CHROME_EXECUTABLE` env var — explicit override. Set by
    ///      `./dev.py check` when it finds a Chromium binary in a cache
    ///      directory (Playwright `/opt/pw-browsers/`, Puppeteer `~/.cache/`,
    ///      etc.) so the tests don't have to download. Production users
    ///      can set this manually to point at any Chrome they trust.
    ///   2. System Chrome via chromiumoxide's lookup (PATH + standard
    ///      install paths).
    ///   3. `BrowserFetcher` downloads a compatible Chromium and caches it.
    async fn new(scope_key: &str) -> Result<Self, BrowserError> {
        // 1. Explicit env-var override — used by the test harness in
        //    sandboxes where Chrome lives at a non-standard path that
        //    chromiumoxide's lookup doesn't probe.
        if let Ok(explicit) = std::env::var("PHOENIX_CHROME_EXECUTABLE") {
            let explicit_path = PathBuf::from(&explicit);
            if explicit_path.exists() {
                tracing::info!(
                    "Using PHOENIX_CHROME_EXECUTABLE={}",
                    explicit_path.display()
                );
                match Self::launch_and_init(scope_key, Some(&explicit_path)).await {
                    Ok(session) => return Ok(session),
                    Err(e) => {
                        tracing::warn!(
                            "PHOENIX_CHROME_EXECUTABLE set but launch failed ({e}); falling through"
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "PHOENIX_CHROME_EXECUTABLE={} does not exist; falling through",
                    explicit_path.display()
                );
            }
        }

        // 2. System Chrome (no explicit executable — chromiumoxide finds it)
        match Self::launch_and_init(scope_key, None).await {
            Ok(session) => return Ok(session),
            Err(e) => {
                tracing::info!("System Chrome not available ({e}), trying fetcher...");
            }
        }

        // 2. Download / use cached Chrome via fetcher
        let cache_dir = Self::fetcher_cache_dir();
        tracing::info!("Downloading Chrome to {cache_dir:?} (first run only)...");

        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            BrowserError::LaunchFailed(format!(
                "Failed to create cache dir {}: {e}",
                cache_dir.display()
            ))
        })?;

        let fetcher_opts = BrowserFetcherOptions::builder()
            .with_path(&cache_dir)
            .with_kind(BrowserKind::ChromeHeadlessShell)
            .build()
            .map_err(|e| BrowserError::LaunchFailed(format!("Fetcher config error: {e}")))?;

        let fetcher = BrowserFetcher::new(fetcher_opts);
        let info = fetcher
            .fetch()
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Chrome download failed: {e:#}")))?;

        tracing::info!("Using Chrome at {:?}", info.executable_path);

        Self::launch_and_init(scope_key, Some(&info.executable_path)).await
    }

    /// Attach a new live-view viewer (REQ-BT-018).
    ///
    /// Returns an `Arc<ScreencastBroker>` keeping the broker alive plus a
    /// fresh broadcast receiver and the URL the page is currently on (if
    /// known). The broker is created lazily on first attach and dropped
    /// automatically when the last `Arc` returned from this method is
    /// dropped — at which point `Page.stopScreencast` fires.
    ///
    /// # Errors
    /// Returns [`BrowserError`] when the screencast cannot be started.
    pub async fn attach_viewer(
        &self,
    ) -> Result<
        (
            Arc<crate::screencast::ScreencastBroker>,
            tokio::sync::broadcast::Receiver<crate::screencast::ScreencastEvent>,
            Option<String>,
        ),
        BrowserError,
    > {
        let mut slot = self.screencast.lock().await;
        if let Some(broker) = slot.upgrade() {
            let (rx, url) = broker.subscribe().await;
            return Ok((broker, rx, url));
        }
        // No live broker — create one. The first attach pays the screencast
        // start-up cost; subsequent attaches share the same broker.
        let broker = crate::screencast::ScreencastBroker::start(self.page.clone()).await?;
        *slot = Arc::downgrade(&broker);
        let (rx, url) = broker.subscribe().await;
        Ok((broker, rx, url))
    }

    /// Set up console log listener (called after session is wrapped in Arc<RwLock>)
    ///
    /// # Errors
    /// Returns [`BrowserError`] when the CDP console event listener cannot be
    /// installed.
    pub async fn setup_console_listener(session: Arc<RwLock<Self>>) -> Result<(), BrowserError> {
        // Get the page event listener and console_logs handle
        let (mut console_events, console_logs, console_event) = {
            let guard = session.read().await;
            let events = guard.page.event_listener::<EventConsoleApiCalled>().await?;
            let logs = guard.console_logs.clone();
            let event = guard.console_event.clone();
            (events, logs, event)
        };

        // Spawn task to capture console events (uses separate lock, no contention)
        let task = tokio::spawn(async move {
            while let Some(event) = console_events.next().await {
                // Convert the chromiumoxide CDP enum into Phoenix's owned
                // `ConsoleLevel` via a total `From` match; downgrading to a
                // free-form string here would lose the closed-set invariant.
                let level: ConsoleLevel = event.r#type.clone().into();
                let text = event
                    .args
                    .iter()
                    .map(extract_console_arg_text)
                    .collect::<Vec<_>>()
                    .join(" ");

                // Add to console logs using separate lock (won't block tool execution)
                tracing::debug!(level = %level, text = %text, "Console event captured");
                if let Ok(mut logs) = console_logs.lock() {
                    if logs.len() >= MAX_CONSOLE_LOGS {
                        logs.pop_front();
                    }
                    logs.push_back(ConsoleEntry {
                        level,
                        text,
                        timestamp: Instant::now(),
                    });
                    console_event.notify_one();
                }
            }
        });

        // Store the task handle
        {
            let mut guard = session.write().await;
            guard.console_task = Some(task);
        }

        Ok(())
    }

    /// Wait until the capture buffer contains at least `count` console events.
    pub async fn wait_for_console_log_count(&self, count: usize) {
        loop {
            let notified = self.console_event.notified();
            if self
                .console_logs
                .lock()
                .is_ok_and(|logs| logs.len() >= count)
            {
                return;
            }
            notified.await;
        }
    }

    /// Set up the profiling trace listener (REQ-BT-019.9 / .12).
    ///
    /// Mirrors [`Self::setup_console_listener`]: subscribes to
    /// `Tracing.dataCollected` and `Tracing.tracingComplete` once and spawns
    /// long-lived tasks. `dataCollected` events are appended to the trace
    /// buffer **only while `tracing_active`** so a stray late event after a
    /// stop cannot resurrect the buffer (invariant `TraceBufferOnlyWhileActive`).
    /// `tracingComplete` signals the notifier so `trace_stop` can drain only
    /// after all asynchronously-delivered events have arrived.
    ///
    /// # Errors
    /// Returns [`BrowserError`] when the CDP tracing event listener cannot be
    /// installed.
    pub async fn setup_profiling_listener(session: Arc<RwLock<Self>>) -> Result<(), BrowserError> {
        use chromiumoxide::cdp::browser_protocol::tracing::{
            EventDataCollected, EventTracingComplete,
        };

        let (mut data_events, mut complete_events, profiling, notify) = {
            let guard = session.read().await;
            let data = guard.page.event_listener::<EventDataCollected>().await?;
            let complete = guard.page.event_listener::<EventTracingComplete>().await?;
            (
                data,
                complete,
                guard.profiling.clone(),
                guard.trace_complete.clone(),
            )
        };

        let profiling_for_data = profiling.clone();
        let data_task = tokio::spawn(async move {
            while let Some(event) = data_events.next().await {
                if let Ok(mut state) = profiling_for_data.lock() {
                    if state.tracing_active {
                        state.trace_events.extend(event.value.iter().cloned());
                    } else {
                        // Capability gap: a dataCollected event arrived while
                        // tracing is idle (late flush after stop). Dropped on
                        // purpose to keep the idle-buffer-empty invariant.
                        tracing::debug!(
                            n = event.value.len(),
                            "dropping Tracing.dataCollected events — tracing not active"
                        );
                    }
                }
            }
        });

        let complete_task = tokio::spawn(async move {
            while let Some(event) = complete_events.next().await {
                if event.data_loss_occurred {
                    tracing::debug!("Tracing.tracingComplete reported data loss");
                }
                notify.notify_waiters();
            }
        });

        {
            let mut guard = session.write().await;
            guard.profiling_tasks.push(data_task);
            guard.profiling_tasks.push(complete_task);
        }

        Ok(())
    }

    /// Force-close the underlying Chrome process and abort all background
    /// tasks. Called by [`BrowserSessionManager::kill_session`] so the
    /// cleanup cascade is authoritative — Chrome dies even if other
    /// holders of the session `Arc` (e.g. a live `browser-view`
    /// WebSocket viewer) keep the `BrowserSession` itself from being
    /// dropped.
    ///
    /// Without this, `kill_session` would only remove the map entry; the
    /// `Arc` clones held by other components keep `BrowserSession` (and
    /// therefore `Browser` and the OS chromium process) alive until the
    /// last clone drops. Cascade then lies: cleanup completed but the
    /// process is still running.
    ///
    /// Falls back to `Browser::kill` when graceful close does not complete.
    async fn terminate(&mut self) -> Result<(), BrowserTerminationError> {
        if let Some(t) = &self.console_task {
            t.abort();
        }
        for t in &self.profiling_tasks {
            t.abort();
        }

        let graceful = tokio::time::timeout(SESSION_INIT_TIMEOUT, async {
            self.browser
                .close()
                .await
                .map_err(|error| error.to_string())?;
            self.browser
                .wait()
                .await
                .map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        })
        .await;

        if !matches!(graceful, Ok(Ok(()))) {
            match graceful {
                Ok(Err(error)) => {
                    tracing::debug!(%error, "browser graceful close failed; falling back to kill");
                }
                Err(_) => {
                    tracing::warn!("browser graceful close timed out; falling back to kill");
                }
                Ok(Ok(())) => unreachable!(),
            }
            match tokio::time::timeout(SESSION_INIT_TIMEOUT, self.browser.kill()).await {
                Ok(Some(Err(error))) => {
                    return Err(BrowserTerminationError::KillFailed(error.to_string()));
                }
                Err(_) => return Err(BrowserTerminationError::KillTimedOut),
                Ok(Some(Ok(())) | None) => {}
            }
        }

        self.handler_task.abort();
        Ok(())
    }
}

/// RAII guard for browser session access
/// Updates `last_activity` timestamp on drop
pub struct BrowserSessionGuard<'a> {
    session: tokio::sync::RwLockWriteGuard<'a, BrowserSession>,
}

impl BrowserSessionGuard<'_> {
    #[must_use]
    pub fn page(&self) -> &Page {
        &self.session.page
    }

    pub fn page_mut(&mut self) -> &mut Page {
        &mut self.session.page
    }

    #[must_use]
    pub fn console_logs(&self) -> &Arc<StdMutex<VecDeque<ConsoleEntry>>> {
        &self.session.console_logs
    }
}

impl Drop for BrowserSessionGuard<'_> {
    fn drop(&mut self) {
        self.session.last_activity = Instant::now();
    }
}

/// Best-effort tear-down of the browser session belonging to `work_scope`,
/// mirroring the tmux registry's `cascade_tmux_on_delete` so
/// archive / abandon / mark-merged / delete drop the Chrome process the same
/// way they drop bash and tmux (REQ-BROWSER-WS-003).
///
/// `inheritor_scope`: the resolved `ResourceScopeKey` of the continuation, if any.
/// Preservation is scope equality — when the inheritor resolves to the
/// *same* scope, the Chrome window is still in use and we skip the kill.
/// Different-scope or no inheritor falls through to teardown. The
/// equality rule subsumes the per-kind case-analysis (Worktree vs
/// Conversation): Direct continuations resolve to their own
/// `Conversation` scope, never equal to the parent's, so they take the
/// kill path automatically.
///
/// REQ-BROWSER-WS-003, REQ-BROWSER-WS-002.
///
/// # Errors
/// Returns [`BrowserError`] when an authoritative browser teardown does not
/// confirm process termination.
pub async fn cascade_browser_on_delete(
    manager: &Arc<BrowserSessionManager>,
    work_scope: &ResourceScopeKey,
    actor: &EffectiveResourceAccess,
    inheritor_scope: Option<&ResourceScopeKey>,
) -> Result<(), BrowserError> {
    if inheritor_scope == Some(work_scope) {
        if actor.authority() == ResourceAuthority::Restricted {
            manager.kill_session_for_actor(work_scope, actor).await?;
        }
        return Ok(());
    }
    manager.kill_session(work_scope).await
}

/// Lifecycle event published by [`BrowserSessionManager`] when a session is
/// created or destroyed. The runtime bridges these into per-conversation SSE
/// streams so the UI can show "browser session live" state without inferring
/// it from message history.
///
/// Carries the `ResourceScopeKey` rather than a single `conversation_id` because a
/// `Worktree`-scoped session may be shared across continuation members; the
/// bridge fans out to every live runtime handle whose conversation resolves
/// to this scope (REQ-BROWSER-WS-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserSessionAudience {
    Scope,
    Conversation(String),
}

impl BrowserSessionAudience {
    #[must_use]
    pub fn matches_actor(&self, actor: &EffectiveResourceAccess) -> bool {
        match self {
            Self::Scope => actor.authority() == ResourceAuthority::Work,
            Self::Conversation(target) => target == actor.conversation_id(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSessionLifecycleKind {
    Active,
    TeardownPending,
    TeardownRetryPending,
    Inactive,
    TeardownFailed,
}

impl BrowserSessionLifecycleKind {
    #[must_use]
    pub const fn viewer_active(self) -> bool {
        matches!(self, Self::Active | Self::TeardownPending)
    }
}

#[derive(Debug, Clone)]
pub struct BrowserSessionLifecycleEvent {
    pub work_scope: ResourceScopeKey,
    pub audience: BrowserSessionAudience,
    pub kind: BrowserSessionLifecycleKind,
}

/// Sink the manager publishes lifecycle events into. A bounded `mpsc` keeps
/// the manager decoupled from any per-conversation routing (the runtime owns
/// that). `None` for tests / contexts that don't care about lifecycle.
pub type BrowserSessionLifecycleSink =
    tokio::sync::mpsc::UnboundedSender<BrowserSessionLifecycleEvent>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserInventoryState {
    Live,
    TeardownPending,
    TeardownFailed,
}

#[derive(Clone, Copy, Debug)]
pub struct BrowserInventoryMetadata {
    pub state: BrowserInventoryState,
    pub idle: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserCleanupIdentifiers {
    pub chrome_pid: Option<u32>,
    pub profile_path: String,
}

/// Predicate answering "does this `ResourceScopeKey` still own a live (non-terminal)
/// conversation?". Injected by the runtime after construction (the manager is
/// built inside `RuntimeManager::new`, before the runtime `Arc` exists, so the
/// hook cannot be a constructor argument). Async because the only authoritative
/// answer requires resolving live runtime handles against the conversation
/// store — the same enumeration the work-scope / browser lifecycle bridges use.
///
/// When unset, idle cleanup reaps purely on `last_activity` age (the historical
/// behavior), so tool-level tests and any caller that never wires a runtime are
/// unaffected.
pub type ScopeLivenessHook = Arc<
    dyn Fn(ResourceScopeKey, Option<String>) -> futures::future::BoxFuture<'static, bool>
        + Send
        + Sync,
>;

/// Map entry: the `ResourceScopeKey` (carried for idle-cleanup lifecycle emission)
/// plus the live session arc.
struct KillAttempt {
    done: Notify,
    result: std::sync::Mutex<Option<Result<(), String>>>,
}

impl KillAttempt {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Notify::new(),
            result: std::sync::Mutex::new(None),
        })
    }

    fn complete(&self, result: Result<(), String>) -> bool {
        let mut slot = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return false;
        }
        *slot = Some(result);
        drop(slot);
        self.done.notify_waiters();
        true
    }

    fn result(&self) -> Option<Result<(), String>> {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct ScopedSession {
    scope: ResourceScopeKey,
    creator_conversation_id: String,
    authority: ResourceAuthority,
    session: Arc<RwLock<BrowserSession>>,
    user_data_key: String,
    current_kill: std::sync::Mutex<Option<Arc<KillAttempt>>>,
    teardown_failed: Arc<AtomicBool>,
}

enum KillSessionOutcome {
    Absent,
    Started {
        key: String,
        handle: JoinHandle<Result<(), BrowserError>>,
        attempt: Arc<KillAttempt>,
    },
    AlreadyRequested {
        attempt: Arc<KillAttempt>,
    },
}

fn session_key(work_scope: &ResourceScopeKey, actor: &EffectiveResourceAccess) -> String {
    actor.restricted_private_key().map_or_else(
        || work_scope.stable_key(),
        |private_key| format!("{}:restricted:{private_key}", work_scope.stable_key()),
    )
}

/// Global manager for all browser sessions
pub struct BrowserSessionManager {
    /// Keyed by the browser session identity. Storing the `ResourceScopeKey` alongside
    /// the session lets idle cleanup emit lifecycle events with the original
    /// scope rather than parsing the string key back into a `ResourceScopeKey`.
    sessions: RwLock<HashMap<String, ScopedSession>>,
    /// Optional lifecycle event sink. Populated by [`RuntimeManager::new`]
    /// so session create/destroy edges flow into per-conversation SSE
    /// streams. Stays `None` for tool-level tests.
    lifecycle_sink: Option<BrowserSessionLifecycleSink>,
    /// Set-once predicate gating idle reaping on `ResourceScopeKey` liveness. The
    /// runtime installs it via [`Self::set_scope_liveness_hook`] when it wires
    /// the lifecycle bridges. Unset (the test/default case) means idle cleanup
    /// reaps on age alone.
    scope_liveness_hook: std::sync::OnceLock<ScopeLivenessHook>,
    shutting_down: AtomicBool,
}

impl BrowserSessionManager {
    /// Create a new session manager and start cleanup task
    #[must_use]
    pub fn new() -> Arc<Self> {
        Self::with_lifecycle_sink(None)
    }

    /// Construct a manager that publishes session-create / session-destroy
    /// edges into `sink`. The runtime wires this to a bridge task that
    /// resolves `conversation_id` to the matching `SseBroadcaster` and
    /// emits `SseEvent::BrowserSessionState`.
    #[must_use]
    pub fn with_lifecycle_sink(sink: Option<BrowserSessionLifecycleSink>) -> Arc<Self> {
        let manager = Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            lifecycle_sink: sink,
            scope_liveness_hook: std::sync::OnceLock::new(),
            shutting_down: AtomicBool::new(false),
        });

        // Start background cleanup task with weak reference to avoid reference cycle
        let manager_weak = Arc::downgrade(&manager);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CLEANUP_INTERVAL).await;
                // Try to upgrade weak reference - if manager is dropped, exit loop
                if let Some(manager) = manager_weak.upgrade() {
                    manager.cleanup_idle_sessions().await;
                } else {
                    tracing::debug!("BrowserSessionManager dropped, cleanup task exiting");
                    break;
                }
            }
        });

        manager
    }

    /// Install the predicate that gates idle reaping on `ResourceScopeKey` liveness.
    ///
    /// Set-once: a second call is a no-op (returns the supplied hook unused via
    /// the `OnceLock` semantics — the first writer wins). The runtime calls
    /// this exactly once, alongside starting the lifecycle bridges, with a
    /// `Weak`-backed closure so the predicate does not keep the runtime alive.
    pub fn set_scope_liveness_hook(&self, hook: ScopeLivenessHook) {
        if self.scope_liveness_hook.set(hook).is_err() {
            tracing::debug!("scope liveness hook already set; ignoring duplicate install");
        }
    }

    /// Whether a live session currently exists for `work_scope`.
    /// The `HashMap` is the single source of truth for session liveness —
    /// callers must not maintain a parallel bool.
    pub async fn is_active(&self, work_scope: &ResourceScopeKey) -> bool {
        self.sessions
            .read()
            .await
            .values()
            .any(|entry| entry.scope == *work_scope)
    }

    pub async fn is_active_for_actor(
        &self,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> bool {
        self.sessions
            .read()
            .await
            .get(&session_key(work_scope, actor))
            .is_some_and(|entry| !entry.teardown_failed.load(Ordering::SeqCst))
    }

    /// Publish a lifecycle edge if a sink is wired. Best-effort: dropped
    /// receivers / closed channels are logged at `debug` (capability gap)
    /// and do not affect session correctness.
    fn emit_lifecycle(
        &self,
        work_scope: &ResourceScopeKey,
        audience: BrowserSessionAudience,
        kind: BrowserSessionLifecycleKind,
    ) {
        let Some(sink) = self.lifecycle_sink.as_ref() else {
            return;
        };
        let event = BrowserSessionLifecycleEvent {
            work_scope: work_scope.clone(),
            audience,
            kind,
        };
        if let Err(e) = sink.send(event) {
            tracing::debug!(
                work_scope = %work_scope,
                ?kind,
                error = %e,
                "dropping browser session lifecycle event — sink closed"
            );
        }
    }

    async fn complete_kill_failure(&self, key: &str, attempt: &Arc<KillAttempt>, error: String) {
        let lifecycle = {
            let sessions = self.sessions.read().await;
            sessions.get(key).and_then(|entry| {
                let is_current = entry
                    .current_kill
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, attempt));
                if !is_current {
                    return None;
                }
                entry.teardown_failed.store(true, Ordering::SeqCst);
                let audience = match entry.authority {
                    ResourceAuthority::Work => BrowserSessionAudience::Scope,
                    ResourceAuthority::Restricted => {
                        BrowserSessionAudience::Conversation(entry.creator_conversation_id.clone())
                    }
                };
                Some((entry.scope.clone(), audience))
            })
        };
        if let Some((scope, audience)) = lifecycle {
            self.emit_lifecycle(
                &scope,
                audience,
                BrowserSessionLifecycleKind::TeardownFailed,
            );
        }
        attempt.complete(Err(error));
    }

    async fn wait_for_kill_completion(
        &self,
        attempt: Arc<KillAttempt>,
    ) -> Result<(), BrowserError> {
        loop {
            let notified = attempt.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = attempt.result() {
                return result.map_err(BrowserError::OperationFailed);
            }
            notified.await;
        }
    }

    /// Actor-authorized session access. A restricted actor cannot attach to a
    /// Work session merely because it shares the scope. Work actors retain the
    /// one-session-per-scope semantics and may control either authority kind.
    /// # Errors
    /// Returns [`BrowserError::AccessDenied`] when the actor cannot control an
    /// existing session, or a browser launch/initialization error when creating one.
    pub async fn get_session_for_actor(
        &self,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(BrowserError::OperationFailed(
                "browser session manager is shutting down".to_string(),
            ));
        }
        self.get_session_with_creator(work_scope, actor).await
    }

    /// Get a session for a `work_scope` (creates if needed).
    /// Returns Arc to the session - caller manages locking.
    ///
    /// Session identity starts with `ResourceScopeKey::stable_key()`: continuations
    /// of a worktree-backed conversation resolve to the same scope and therefore
    /// inherit the same Chrome window (REQ-BROWSER-WS-001), while Direct
    /// conversations fall back to per-conversation scoping (no shared owner
    /// exists for them to inherit).
    ///
    /// # Errors
    /// Returns [`BrowserError`] when Chrome cannot be launched for a
    /// not-yet-existing session.
    pub async fn get_session(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        let system = EffectiveResourceAccess::new("browser-manager", ResourceAuthority::Work);
        self.get_session_with_creator(work_scope, &system).await
    }

    fn ensure_accepting_sessions(&self) -> Result<(), BrowserError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            Err(BrowserError::OperationFailed(
                "browser session manager is shutting down".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    async fn setup_session_listeners(session: Arc<RwLock<BrowserSession>>) {
        match tokio::time::timeout(
            SESSION_INIT_TIMEOUT,
            BrowserSession::setup_console_listener(session.clone()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "Failed to set up console listener"),
            Err(_) => tracing::warn!("console listener setup timed out"),
        }
        match tokio::time::timeout(
            SESSION_INIT_TIMEOUT,
            BrowserSession::setup_profiling_listener(session),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, "Failed to set up profiling listener"),
            Err(_) => tracing::warn!("profiling listener setup timed out"),
        }
    }

    async fn get_session_with_creator(
        &self,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        let key = session_key(work_scope, actor);
        self.ensure_accepting_sessions()?;

        let mut sessions = loop {
            {
                let sessions = self.sessions.read().await;
                if let Some(entry) = sessions.get(&key) {
                    if entry.teardown_failed.load(Ordering::SeqCst) {
                        return Err(BrowserError::OperationFailed(
                            "browser session teardown failed; retry cleanup before reuse"
                                .to_string(),
                        ));
                    }
                    let pending_kill = entry
                        .current_kill
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                        .filter(|attempt| attempt.result().is_none());
                    if let Some(attempt) = pending_kill {
                        drop(sessions);
                        self.wait_for_kill_completion(attempt).await?;
                        continue;
                    }
                    if !actor.can_control(&entry.creator_conversation_id, entry.authority) {
                        return Err(BrowserError::AccessDenied);
                    }
                    return Ok(entry.session.clone());
                }
            }

            let sessions = self.sessions.write().await;
            self.ensure_accepting_sessions()?;
            if let Some(entry) = sessions.get(&key) {
                if entry.teardown_failed.load(Ordering::SeqCst) {
                    return Err(BrowserError::OperationFailed(
                        "browser session teardown failed; retry cleanup before reuse".to_string(),
                    ));
                }
                let pending_kill = entry
                    .current_kill
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|attempt| attempt.result().is_none());
                if let Some(attempt) = pending_kill {
                    drop(sessions);
                    self.wait_for_kill_completion(attempt).await?;
                    continue;
                }
                if !actor.can_control(&entry.creator_conversation_id, entry.authority) {
                    return Err(BrowserError::AccessDenied);
                }
                return Ok(entry.session.clone());
            }
            break sessions;
        };

        tracing::info!(work_scope = %work_scope, "Creating new browser session");
        // BrowserSession::new bounds its own CDP launch (and may legitimately
        // run a multi-minute first-run chromium download, which is NOT bounded).
        let session = BrowserSession::new(&key).await?;
        let session_arc = Arc::new(RwLock::new(session));

        Self::setup_session_listeners(session_arc.clone()).await;

        sessions.insert(
            key.clone(),
            ScopedSession {
                scope: work_scope.clone(),
                creator_conversation_id: actor.conversation_id().to_string(),
                authority: actor.authority(),
                session: session_arc.clone(),
                user_data_key: key.clone(),
                current_kill: std::sync::Mutex::new(None),
                teardown_failed: Arc::new(AtomicBool::new(false)),
            },
        );
        // Drop the write lock before emitting — the receiver may grab the
        // sessions read lock to confirm state, and we don't want to hold
        // the write lock across that.
        drop(sessions);
        let audience = match actor.authority() {
            ResourceAuthority::Work => BrowserSessionAudience::Scope,
            ResourceAuthority::Restricted => {
                BrowserSessionAudience::Conversation(actor.conversation_id().to_string())
            }
        };
        self.emit_lifecycle(work_scope, audience, BrowserSessionLifecycleKind::Active);

        Ok(session_arc)
    }

    pub async fn cleanup_identifiers(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Vec<BrowserCleanupIdentifiers> {
        let sessions: Vec<_> = self
            .sessions
            .read()
            .await
            .values()
            .filter(|entry| entry.scope == *work_scope)
            .map(|entry| (entry.session.clone(), entry.user_data_key.clone()))
            .collect();
        let mut identifiers = Vec::with_capacity(sessions.len());
        for (session, user_data_key) in sessions {
            identifiers.push(BrowserCleanupIdentifiers {
                chrome_pid: session.read().await.chrome_pid,
                profile_path: user_data_dir_for_key(&user_data_key),
            });
        }
        identifiers
    }

    /// Return control-plane idle metadata without exposing the session for reuse.
    #[must_use]
    pub async fn inventory_metadata(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<BrowserInventoryMetadata> {
        let (session, state) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.values().find(|entry| entry.scope == *work_scope)?;
            let state = if entry
                .current_kill
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|attempt| attempt.result().is_none())
            {
                BrowserInventoryState::TeardownPending
            } else if entry.teardown_failed.load(Ordering::SeqCst) {
                BrowserInventoryState::TeardownFailed
            } else {
                BrowserInventoryState::Live
            };
            (entry.session.clone(), state)
        };
        if state != BrowserInventoryState::Live {
            return Some(BrowserInventoryMetadata { state, idle: None });
        }
        let idle = session.read().await.last_activity.elapsed();
        Some(BrowserInventoryMetadata {
            state,
            idle: Some(idle),
        })
    }

    /// Return actor-authorized control-plane idle metadata without exposing the
    /// session for reuse.
    ///
    /// # Errors
    /// Returns [`BrowserError::AccessDenied`] when the actor cannot control the
    /// retained session.
    pub async fn inventory_metadata_for_actor(
        &self,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> Result<Option<BrowserInventoryMetadata>, BrowserError> {
        let (session, state) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(&session_key(work_scope, actor)) else {
                return Ok(None);
            };
            if !actor.can_control(&entry.creator_conversation_id, entry.authority) {
                return Err(BrowserError::AccessDenied);
            }
            let state = if entry
                .current_kill
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|attempt| attempt.result().is_none())
            {
                BrowserInventoryState::TeardownPending
            } else if entry.teardown_failed.load(Ordering::SeqCst) {
                BrowserInventoryState::TeardownFailed
            } else {
                BrowserInventoryState::Live
            };
            (entry.session.clone(), state)
        };
        if state != BrowserInventoryState::Live {
            return Ok(Some(BrowserInventoryMetadata { state, idle: None }));
        }
        let idle = session.read().await.last_activity.elapsed();
        Ok(Some(BrowserInventoryMetadata {
            state,
            idle: Some(idle),
        }))
    }

    /// Return the actor's reusable browser session.
    ///
    /// # Errors
    /// Returns [`BrowserError::AccessDenied`] when an existing session belongs
    /// to an actor with stronger or different restricted authority, or
    /// [`BrowserError::OperationFailed`] when retained teardown failure makes
    /// the session unusable.
    pub async fn get_existing_for_actor(
        &self,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> Result<Option<Arc<RwLock<BrowserSession>>>, BrowserError> {
        let key = session_key(work_scope, actor);
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(&key) else {
            return Ok(None);
        };
        if !actor.can_control(&entry.creator_conversation_id, entry.authority) {
            return Err(BrowserError::AccessDenied);
        }
        if entry.teardown_failed.load(Ordering::SeqCst) {
            return Err(BrowserError::OperationFailed(
                "browser session teardown failed; retry cleanup before reuse".to_string(),
            ));
        }
        Ok(Some(entry.session.clone()))
    }

    /// Get the session for a `work_scope` **without creating one**.
    ///
    /// Used by the live-view WS endpoint, which deliberately must not spawn
    /// a chromium just because someone opened the panel — the panel reflects
    /// the agent's existing browser, not a new one.
    ///
    /// Logs at `debug` when the lookup misses so a "viewer opened but no
    /// session existed" silent failure becomes auditable (capability-gap
    /// logging per REQ-BROWSER-WS-004).
    pub async fn get_existing(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<Arc<RwLock<BrowserSession>>> {
        let sessions = self.sessions.read().await;
        let hit = sessions
            .values()
            .find(|entry| {
                entry.scope == *work_scope && !entry.teardown_failed.load(Ordering::SeqCst)
            })
            .map(|entry| entry.session.clone());
        if hit.is_none() {
            tracing::debug!(
                work_scope = %work_scope,
                "browser session lookup miss — no session for scope"
            );
        }
        hit
    }

    async fn remove_profile_for_attempt(
        &self,
        key: &str,
        attempt: &Arc<KillAttempt>,
        user_data_dir: &str,
    ) -> Result<(), BrowserError> {
        let failure = match tokio::time::timeout(
            SESSION_INIT_TIMEOUT,
            tokio::fs::remove_dir_all(user_data_dir),
        )
        .await
        {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(Err(error)) => format!("failed to remove browser profile {user_data_dir}: {error}"),
            Err(_) => format!("timed out removing browser profile {user_data_dir}"),
        };
        self.complete_kill_failure(key, attempt, failure.clone())
            .await;
        Err(BrowserError::OperationFailed(failure))
    }

    async fn remove_session_if_current(
        &self,
        key: &str,
        session: &Arc<RwLock<BrowserSession>>,
    ) -> Option<ScopedSession> {
        let mut sessions = self.sessions.write().await;
        sessions
            .get(key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.session, session))
            .then(|| sessions.remove(key))
            .flatten()
    }

    /// Kill the session belonging to `work_scope` (called from the cleanup
    /// cascade on archive/abandon/mark-merged/delete of the chain leaf).
    ///
    /// Force-closes the underlying Chrome process via
    /// [`BrowserSession::terminate`] BEFORE dropping the map's `Arc`.
    /// Other holders of the session `Arc` (notably the `browser-view`
    /// WebSocket viewer, which keeps an `Arc` for the duration of its
    /// connection) would otherwise prevent `Drop for BrowserSession`
    /// from running, leaving the OS chromium process alive after the
    /// cascade claimed to have killed it.
    ///
    /// The sessions write lock is released as soon as the entry is
    /// removed; `terminate` and the awaited `remove_dir_all` run
    /// lock-free so concurrent `get_session` / `get_existing` /
    /// `is_active` calls on unrelated scopes are not blocked for the
    /// duration of fs deletion + Chrome shutdown.
    async fn spawn_kill_session_by_key(
        self: &Arc<Self>,
        key: String,
        work_scope: ResourceScopeKey,
    ) -> KillSessionOutcome {
        let (session, attempt, audience, lifecycle_kind) = {
            let sessions = self.sessions.write().await;
            let Some(entry) = sessions.get(&key) else {
                return KillSessionOutcome::Absent;
            };
            let mut current = entry
                .current_kill
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(attempt) = current
                .as_ref()
                .filter(|attempt| attempt.result().is_none())
            {
                tracing::debug!(work_scope = %work_scope, "browser kill already requested");
                return KillSessionOutcome::AlreadyRequested {
                    attempt: attempt.clone(),
                };
            }
            let attempt = KillAttempt::new();
            *current = Some(attempt.clone());
            let lifecycle_kind = if entry.teardown_failed.load(Ordering::SeqCst) {
                BrowserSessionLifecycleKind::TeardownRetryPending
            } else {
                BrowserSessionLifecycleKind::TeardownPending
            };
            let audience = match entry.authority {
                ResourceAuthority::Work => BrowserSessionAudience::Scope,
                ResourceAuthority::Restricted => {
                    BrowserSessionAudience::Conversation(entry.creator_conversation_id.clone())
                }
            };
            (entry.session.clone(), attempt, audience, lifecycle_kind)
        };
        self.emit_lifecycle(&work_scope, audience, lifecycle_kind);

        let requested_scope = work_scope;
        let manager = Arc::clone(self);
        let task_attempt = attempt.clone();
        KillSessionOutcome::Started {
            key: key.clone(),
            attempt,
            handle: tokio::spawn(async move {
                tracing::info!(work_scope = %requested_scope, "Killing browser session");

                let termination = {
                    let mut session_guard = session.write().await;
                    session_guard.terminate().await
                };
                if let Err(error) = termination {
                    tracing::warn!(work_scope = %requested_scope, %error, "browser termination failed; retaining tracked session");
                    manager
                        .complete_kill_failure(&key, &task_attempt, error.to_string())
                        .await;
                    return Err(BrowserError::OperationFailed(error.to_string()));
                }

                let Some(user_data_key) = ({
                    let sessions = manager.sessions.read().await;
                    sessions.get(&key).and_then(|entry| {
                        Arc::ptr_eq(&entry.session, &session).then(|| entry.user_data_key.clone())
                    })
                }) else {
                    tracing::debug!(work_scope = %requested_scope, "browser kill completed after session was removed or replaced");
                    task_attempt.complete(Ok(()));
                    return Ok(());
                };

                let user_data_dir = user_data_dir_for_key(&user_data_key);
                manager
                    .remove_profile_for_attempt(&key, &task_attempt, &user_data_dir)
                    .await?;

                let removed = manager.remove_session_if_current(&key, &session).await;
                let Some(entry) = removed else {
                    tracing::debug!(work_scope = %requested_scope, "browser kill cleaned profile after session was removed or replaced");
                    task_attempt.complete(Ok(()));
                    return Ok(());
                };
                let removed_scope = entry.scope.clone();
                let audience = match entry.authority {
                    ResourceAuthority::Work => BrowserSessionAudience::Scope,
                    ResourceAuthority::Restricted => {
                        BrowserSessionAudience::Conversation(entry.creator_conversation_id.clone())
                    }
                };
                drop(entry);
                manager.emit_lifecycle(
                    &removed_scope,
                    audience,
                    BrowserSessionLifecycleKind::Inactive,
                );
                task_attempt.complete(Ok(()));
                Ok(())
            }),
        }
    }

    fn observe_requested_kill(
        self: &Arc<Self>,
        outcome: KillSessionOutcome,
        work_scope: ResourceScopeKey,
        audience: BrowserSessionAudience,
    ) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let result = match outcome {
                KillSessionOutcome::Absent => Ok(()),
                KillSessionOutcome::Started {
                    key,
                    handle,
                    attempt,
                } => match handle.await {
                    Ok(result) => result,
                    Err(error) => {
                        manager
                            .complete_kill_failure(
                                &key,
                                &attempt,
                                format!("browser kill task failed: {error}"),
                            )
                            .await;
                        Err(BrowserError::OperationFailed(format!(
                            "browser kill task failed: {error}"
                        )))
                    }
                },
                KillSessionOutcome::AlreadyRequested { attempt } => {
                    manager.wait_for_kill_completion(attempt).await
                }
            };
            if let Err(error) = result {
                tracing::warn!(%work_scope, ?audience, %error, "requested browser teardown failed");
            }
            Ok::<(), BrowserError>(())
        });
    }

    pub async fn request_kill_session_for_actor(
        self: &Arc<Self>,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) {
        let key = session_key(work_scope, actor);
        let outcome = self
            .spawn_kill_session_by_key(key, work_scope.clone())
            .await;
        let audience = match actor.authority() {
            ResourceAuthority::Work => BrowserSessionAudience::Scope,
            ResourceAuthority::Restricted => {
                BrowserSessionAudience::Conversation(actor.conversation_id().to_string())
            }
        };
        self.observe_requested_kill(outcome, work_scope.clone(), audience);
    }

    /// Request session kill and return as soon as teardown has been queued.
    /// The session remains tracked as live until the spawned teardown task has
    /// actually terminated Chrome, removed the manager entry, and emitted the
    /// lifecycle false edge.
    pub async fn request_kill_session(self: &Arc<Self>, work_scope: &ResourceScopeKey) {
        let keys: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.scope == *work_scope)
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let outcome = self
                .spawn_kill_session_by_key(key, work_scope.clone())
                .await;
            self.observe_requested_kill(outcome, work_scope.clone(), BrowserSessionAudience::Scope);
        }
    }

    /// Kill the actor-specific session and wait for confirmed teardown.
    ///
    /// # Errors
    /// Returns [`BrowserError`] when the kill task fails or Chrome termination
    /// cannot be confirmed.
    pub async fn kill_session_for_actor(
        self: &Arc<Self>,
        work_scope: &ResourceScopeKey,
        actor: &EffectiveResourceAccess,
    ) -> Result<(), BrowserError> {
        let key = session_key(work_scope, actor);
        match self
            .spawn_kill_session_by_key(key, work_scope.clone())
            .await
        {
            KillSessionOutcome::Absent => Ok(()),
            KillSessionOutcome::Started {
                key,
                handle,
                attempt,
            } => match handle.await {
                Ok(result) => result,
                Err(error) => {
                    self.complete_kill_failure(
                        &key,
                        &attempt,
                        format!("browser kill task failed: {error}"),
                    )
                    .await;
                    Err(BrowserError::OperationFailed(format!(
                        "browser kill task failed: {error}"
                    )))
                }
            },
            KillSessionOutcome::AlreadyRequested { attempt } => {
                self.wait_for_kill_completion(attempt).await
            }
        }
    }

    /// Kill a session and wait for teardown to complete.
    ///
    /// Delete cascades use this stronger operation because the conversation is
    /// going away; user-facing Stop browser endpoints use
    /// [`Self::request_kill_session`] so they do not block behind an in-flight
    /// browser tool guard.
    /// # Errors
    /// Returns [`BrowserError`] when any matching session's kill task fails or
    /// Chrome termination cannot be confirmed.
    pub async fn kill_session(
        self: &Arc<Self>,
        work_scope: &ResourceScopeKey,
    ) -> Result<(), BrowserError> {
        let keys: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.scope == *work_scope)
            .map(|(key, _)| key.clone())
            .collect();
        let mut started = Vec::new();
        let mut already_requested = Vec::new();
        for key in keys {
            match self
                .spawn_kill_session_by_key(key, work_scope.clone())
                .await
            {
                KillSessionOutcome::Absent => {}
                KillSessionOutcome::Started {
                    key,
                    handle,
                    attempt,
                } => started.push((key, handle, attempt)),
                KillSessionOutcome::AlreadyRequested { attempt } => {
                    already_requested.push(attempt);
                }
            }
        }

        let mut failures = Vec::new();
        for (key, attempt, result) in futures::future::join_all(
            started
                .into_iter()
                .map(|(key, handle, attempt)| async move { (key, attempt, handle.await) }),
        )
        .await
        {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(error) => {
                    self.complete_kill_failure(
                        &key,
                        &attempt,
                        format!("browser kill task failed: {error}"),
                    )
                    .await;
                    failures.push(format!("browser kill task failed: {error}"));
                }
            }
        }
        for result in futures::future::join_all(
            already_requested
                .into_iter()
                .map(|attempt| self.wait_for_kill_completion(attempt)),
        )
        .await
        {
            if let Err(error) = result {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(BrowserError::OperationFailed(format!(
                "browser teardown failures for {work_scope}: {}",
                failures.join("; ")
            )))
        }
    }

    /// Kill all sessions and wait for their Chrome processes and profiles to be released.
    ///
    /// # Errors
    /// Returns [`BrowserError`] when any session cannot confirm authoritative teardown.
    pub async fn shutdown_all(self: &Arc<Self>) -> Result<(), BrowserError> {
        self.shutting_down.store(true, Ordering::SeqCst);
        let mut failures = Vec::new();
        loop {
            let scopes: HashSet<ResourceScopeKey> = self
                .sessions
                .read()
                .await
                .values()
                .map(|entry| entry.scope.clone())
                .collect();
            if scopes.is_empty() {
                break;
            }
            tracing::info!(count = scopes.len(), "Shutting down all browser sessions");
            failures.extend(
                futures::future::join_all(
                    scopes.into_iter().map(|scope| async move {
                        (scope.clone(), self.kill_session(&scope).await)
                    }),
                )
                .await
                .into_iter()
                .filter_map(|(scope, result)| {
                    result.err().map(|error| format!("{scope}: {error}"))
                }),
            );
            if !failures.is_empty() {
                break;
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(BrowserError::OperationFailed(format!(
                "browser shutdown failures: {}",
                failures.join("; ")
            )))
        }
    }

    /// Of the idle `(key, scope)` candidates, return the keys that may be
    /// reaped. A candidate is preserved (dropped from the result) when a
    /// scope-liveness hook is installed and reports the scope live. With no
    /// hook every idle candidate is reapable — the historical age-only
    /// behavior. Factored out of [`Self::cleanup_idle_sessions`] so the
    /// liveness gate is unit-testable without a live chromium process.
    async fn filter_reapable(
        &self,
        idle_candidates: Vec<(String, ResourceScopeKey, Option<String>)>,
    ) -> Vec<String> {
        let mut to_remove = Vec::new();
        for (key, scope, restricted_creator) in idle_candidates {
            if let Some(hook) = self.scope_liveness_hook.get() {
                if hook(scope.clone(), restricted_creator).await {
                    tracing::debug!(
                        work_scope = %scope,
                        "browser: skipping idle reap — scope still owns a live conversation"
                    );
                    continue;
                }
            }
            to_remove.push(key);
        }
        to_remove
    }

    /// Clean up sessions that have been idle too long.
    ///
    /// Idle age (`last_activity` older than [`IDLE_TIMEOUT`]) is necessary but
    /// not sufficient to reap: when a scope-liveness hook is installed, a scope
    /// whose `ResourceScopeKey` still owns a non-terminal conversation is preserved
    /// even past the timeout. `last_activity` only resets on a browser
    /// tool-call guard drop, so a conversation that is alive in the UI but has
    /// not issued a browser tool call in 30 minutes would otherwise lose its
    /// live page state / open tabs / live-view stream. The timer remains the
    /// backstop for scopes with no live conversation (or when no hook is
    /// wired). Re-checked every [`CLEANUP_INTERVAL`], so a scope that goes
    /// terminal is reaped on the next pass.
    async fn cleanup_idle_sessions(self: &Arc<Self>) {
        let now = Instant::now();
        let mut idle_candidates: Vec<(String, ResourceScopeKey, Option<String>)> = Vec::new();

        // Find idle sessions
        {
            let sessions = self.sessions.read().await;
            for (key, entry) in sessions.iter() {
                if let Ok(guard) = entry.session.try_read() {
                    if now.duration_since(guard.last_activity) > IDLE_TIMEOUT {
                        let restricted_creator =
                            matches!(entry.authority, ResourceAuthority::Restricted)
                                .then(|| entry.creator_conversation_id.clone());
                        idle_candidates.push((
                            key.clone(),
                            entry.scope.clone(),
                            restricted_creator,
                        ));
                    }
                }
            }
        }

        // Gate each idle candidate on scope liveness. A live scope (its
        // `ResourceScopeKey` still owns a non-terminal conversation) is skipped — the
        // timer re-checks it next interval. No hook means reap on age alone.
        let to_remove = self.filter_reapable(idle_candidates).await;

        let mut started = Vec::new();
        let mut already_requested = Vec::new();
        for key in to_remove {
            let scope = {
                let sessions = self.sessions.read().await;
                sessions.get(&key).map(|entry| entry.scope.clone())
            };
            if let Some(scope) = scope {
                tracing::info!(scope_key = %key, "Cleaning up idle browser session");
                match self.spawn_kill_session_by_key(key, scope).await {
                    KillSessionOutcome::Absent => {}
                    KillSessionOutcome::Started {
                        key,
                        handle,
                        attempt,
                    } => started.push((key, handle, attempt)),
                    KillSessionOutcome::AlreadyRequested { attempt } => {
                        already_requested.push(attempt);
                    }
                }
            }
        }

        for (key, attempt, result) in futures::future::join_all(
            started
                .into_iter()
                .map(|(key, handle, attempt)| async move { (key, attempt, handle.await) }),
        )
        .await
        {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "idle browser termination failed"),
                Err(error) => {
                    self.complete_kill_failure(
                        &key,
                        &attempt,
                        format!("browser kill task failed: {error}"),
                    )
                    .await;
                    tracing::warn!(%error, "idle browser kill task failed");
                }
            }
        }
        for result in futures::future::join_all(
            already_requested
                .into_iter()
                .map(|attempt| self.wait_for_kill_completion(attempt)),
        )
        .await
        {
            if let Err(error) = result {
                tracing::warn!(%error, "idle browser termination did not complete");
            }
        }
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        // Abort background tasks so their I/O registrations are released immediately.
        // Dropping a JoinHandle does NOT abort the task — explicit abort is required.
        self.handler_task.abort();
        if let Some(task) = &self.console_task {
            task.abort();
        }
        for task in &self.profiling_tasks {
            task.abort();
        }
    }
}

impl Default for BrowserSessionManager {
    fn default() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            lifecycle_sink: None,
            scope_liveness_hook: std::sync::OnceLock::new(),
            shutting_down: AtomicBool::new(false),
        }
    }
}

#[cfg(test)]
mod lifecycle_hook_tests {
    //! Tests for the lifecycle-event sink — exercises only the manager
    //! plumbing that doesn't require a real chromium process. The "create
    //! emits exactly once" path is covered by the chrome-gated integration
    //! tests in `super::tests`.

    use super::{
        BrowserSession, BrowserSessionAudience, BrowserSessionLifecycleEvent,
        BrowserSessionLifecycleKind, BrowserSessionLifecycleSink, BrowserSessionManager,
    };
    use phoenix_core::work_scope::{
        EffectiveResourceAccess, ResourceAuthority, ResourceScopeKey, WorkScopeId,
    };
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::RwLock;

    fn install_sink() -> (
        Arc<BrowserSessionManager>,
        tokio::sync::mpsc::UnboundedReceiver<BrowserSessionLifecycleEvent>,
    ) {
        let (tx, rx): (
            BrowserSessionLifecycleSink,
            tokio::sync::mpsc::UnboundedReceiver<BrowserSessionLifecycleEvent>,
        ) = tokio::sync::mpsc::unbounded_channel();
        (BrowserSessionManager::with_lifecycle_sink(Some(tx)), rx)
    }

    fn scope(id: &str) -> ResourceScopeKey {
        ResourceScopeKey::Work(WorkScopeId::parse(id).unwrap())
    }

    #[test]
    fn kill_attempt_result_is_immutable_across_retry() {
        let failed = super::KillAttempt::new();
        let retry = super::KillAttempt::new();

        assert!(failed.complete(Err("first attempt failed".to_string())));
        assert!(retry.complete(Ok(())));
        assert!(!failed.complete(Ok(())));

        assert!(matches!(
            failed.result(),
            Some(Err(error)) if error == "first attempt failed"
        ));
        assert!(matches!(retry.result(), Some(Ok(()))));
    }

    #[test]
    fn viewer_activity_distinguishes_initial_teardown_from_failed_retry() {
        assert!(BrowserSessionLifecycleKind::Active.viewer_active());
        assert!(BrowserSessionLifecycleKind::TeardownPending.viewer_active());
        assert!(!BrowserSessionLifecycleKind::TeardownRetryPending.viewer_active());
        assert!(!BrowserSessionLifecycleKind::TeardownFailed.viewer_active());
        assert!(!BrowserSessionLifecycleKind::Inactive.viewer_active());
    }

    #[tokio::test]
    async fn shutdown_all_closes_session_admission() {
        let manager = BrowserSessionManager::new();
        manager.shutdown_all().await.expect("empty shutdown");

        let Err(error) = manager.get_session(&scope("after-shutdown")).await else {
            panic!("shutdown manager must reject new sessions");
        };
        assert!(error.to_string().contains("shutting down"));
    }

    /// `kill_session` on a manager that never had a session for this scope
    /// must not emit a lifecycle edge — the UI never saw the up-edge, so
    /// emitting a down-edge would falsely signal a transition.
    #[tokio::test]
    async fn kill_session_no_op_does_not_emit() {
        let (manager, mut rx) = install_sink();
        let scope = scope("conv-never-existed");
        manager
            .kill_session(&scope)
            .await
            .expect("browser shutdown");
        assert!(
            rx.try_recv().is_err(),
            "kill_session on absent scope must not emit a lifecycle event"
        );
        assert!(!manager.is_active(&scope).await);
    }

    #[tokio::test]
    async fn shutdown_all_waits_for_process_exit_and_profile_removal() {
        if std::env::var_os("PHOENIX_CHROME_EXECUTABLE").is_none() {
            return;
        }

        let manager = BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(WorkScopeId::new());
        let profile = super::user_data_dir_for_key(&scope.stable_key());
        let session = manager.get_session(&scope).await.expect("launch browser");

        manager.shutdown_all().await.expect("browser shutdown");

        assert!(!manager.is_active(&scope).await);
        assert!(!std::path::Path::new(&profile).exists());
        assert!(
            session
                .write()
                .await
                .browser
                .try_wait()
                .expect("read browser process status")
                .is_some(),
            "shutdown_all returned before Chrome exited"
        );
    }

    /// `rekey_scope` on a manager with no session for `old` is a no-op:
    /// returns `false` and creates nothing under `new`. (The move and
    /// occupied-destination branches share the same map-level logic as the
    /// bash/tmux registries, which test those branches with constructible
    /// entries; a real `BrowserSession` needs a live chromium, so it cannot be
    /// staged here.)
    /// `is_active` must reflect the underlying `HashMap`. We can't create a
    /// real `BrowserSession` without chrome, so this just exercises the
    /// "absent" branch for both scope variants.
    #[tokio::test]
    async fn is_active_reflects_hashmap_membership() {
        let (manager, _rx) = install_sink();
        assert!(!manager.is_active(&scope("conv-1")).await);
        assert!(!manager.is_active(&scope("/tmp/wt-1")).await);
    }

    /// Distinct opaque work IDs and the structurally separate global terminal
    /// occupy disjoint resource namespaces.
    #[tokio::test]
    async fn is_active_disjoint_namespaces() {
        let (manager, _rx) = install_sink();
        let first = scope("opaque-a");
        let second = scope("opaque-b");
        let global = ResourceScopeKey::GlobalTerminal;
        assert!(!manager.is_active(&first).await);
        assert!(!manager.is_active(&second).await);
        assert!(!manager.is_active(&global).await);
        assert_ne!(first.stable_key(), second.stable_key());
        assert_ne!(first.stable_key(), global.stable_key());
        assert_ne!(second.stable_key(), global.stable_key());
    }

    #[test]
    fn restricted_actors_receive_isolated_keys_in_shared_scope() {
        let shared = scope("shared-work-scope");
        let work = EffectiveResourceAccess::new("work-parent", ResourceAuthority::Work);
        let restricted_a =
            EffectiveResourceAccess::new("explore-child-a", ResourceAuthority::Restricted);
        let restricted_b =
            EffectiveResourceAccess::new("explore-child-b", ResourceAuthority::Restricted);

        let work_key = super::session_key(&shared, &work);
        let first_child_key = super::session_key(&shared, &restricted_a);
        let sibling_key = super::session_key(&shared, &restricted_b);

        assert_ne!(work_key, first_child_key);
        assert_ne!(first_child_key, sibling_key);
        assert_eq!(first_child_key, super::session_key(&shared, &restricted_a));
    }

    #[test]
    fn actor_specific_stop_targets_only_restricted_actor_key() {
        let shared = scope("shared-stop-scope");
        let restricted_a =
            EffectiveResourceAccess::new("explore-child-a", ResourceAuthority::Restricted);
        let restricted_b =
            EffectiveResourceAccess::new("explore-child-b", ResourceAuthority::Restricted);
        assert_ne!(
            super::session_key(&shared, &restricted_a),
            super::session_key(&shared, &restricted_b)
        );
    }

    #[test]
    fn pre_rekey_explore_stop_targets_only_its_private_actor_key() {
        let shared = scope("pre-rekey-conversation-scope");
        let user_explore =
            EffectiveResourceAccess::new("user-explore", ResourceAuthority::Restricted);
        let same_user = EffectiveResourceAccess::new("user-explore", ResourceAuthority::Restricted);
        let private_sub_agent =
            EffectiveResourceAccess::new("explore-child", ResourceAuthority::Restricted);
        let work_actor = EffectiveResourceAccess::new("work-parent", ResourceAuthority::Work);

        assert_eq!(
            super::session_key(&shared, &user_explore),
            super::session_key(&shared, &same_user)
        );
        assert_ne!(
            super::session_key(&shared, &user_explore),
            super::session_key(&shared, &private_sub_agent)
        );
        assert_ne!(
            super::session_key(&shared, &user_explore),
            super::session_key(&shared, &work_actor)
        );
    }

    /// Test the full create-emit + kill-emit pair end-to-end using a
    /// hand-rolled `BrowserSession` substitute is not possible without a
    /// real chrome (the struct's fields require live `Browser` and `Page`
    /// values from chromiumoxide). This test instead directly exercises
    /// `emit_lifecycle` to confirm the sink wiring round-trips correctly.
    #[tokio::test]
    async fn emit_lifecycle_round_trips_through_sink() {
        let (manager, mut rx) = install_sink();
        let a = scope("conv-A");
        let b = scope("conv-B");
        manager.emit_lifecycle(
            &a,
            BrowserSessionAudience::Scope,
            BrowserSessionLifecycleKind::Active,
        );
        manager.emit_lifecycle(
            &a,
            BrowserSessionAudience::Scope,
            BrowserSessionLifecycleKind::Inactive,
        );
        manager.emit_lifecycle(
            &b,
            BrowserSessionAudience::Scope,
            BrowserSessionLifecycleKind::Active,
        );

        let e1 = rx.try_recv().expect("first event missing");
        assert_eq!(e1.work_scope, a);
        assert!(matches!(e1.kind, BrowserSessionLifecycleKind::Active));
        let e2 = rx.try_recv().expect("second event missing");
        assert_eq!(e2.work_scope, a);
        assert!(matches!(e2.kind, BrowserSessionLifecycleKind::Inactive));
        let e3 = rx.try_recv().expect("third event missing");
        assert_eq!(e3.work_scope, b);
        assert!(matches!(e3.kind, BrowserSessionLifecycleKind::Active));
        assert!(rx.try_recv().is_err(), "no more events expected");
    }

    /// When no sink is configured (`Default::default`), `emit_lifecycle` is
    /// a no-op — must not panic and must not allocate a phantom event.
    #[tokio::test]
    async fn emit_lifecycle_without_sink_is_no_op() {
        let manager = BrowserSessionManager::default();
        let scope = scope("conv-X");
        manager.emit_lifecycle(
            &scope,
            BrowserSessionAudience::Scope,
            BrowserSessionLifecycleKind::Active,
        );
        assert!(!manager.is_active(&scope).await);
    }

    /// Belt-and-braces: keep the unused-import lints quiet. `BrowserSession`
    /// / `RwLock` / `Instant` are pulled in for symmetry with future
    /// chrome-gated tests.
    #[allow(dead_code)]
    fn _phantom_uses(_b: Option<BrowserSession>, _r: Option<RwLock<()>>, _i: Option<Instant>) {}

    /// With no scope-liveness hook installed, every idle candidate is
    /// reapable — the historical age-only behavior must be preserved for the
    /// default / tool-level-test constructors.
    #[tokio::test]
    async fn filter_reapable_without_hook_reaps_all() {
        let manager = BrowserSessionManager::default();
        let candidates = vec![
            ("k1".to_string(), scope("conv-1"), None),
            ("k2".to_string(), scope("/tmp/wt-2"), None),
        ];
        let reap = manager.filter_reapable(candidates).await;
        assert_eq!(reap, vec!["k1".to_string(), "k2".to_string()]);
    }

    /// An idle session whose scope IS live is NOT reaped; an idle session
    /// whose scope is NOT live IS reaped. The hook stands in for the runtime's
    /// "does any non-terminal conversation resolve to this scope?" predicate.
    #[tokio::test]
    async fn filter_reapable_skips_live_scope_reaps_dead_scope() {
        let manager = BrowserSessionManager::default();
        let live = scope("alive");
        let dead = scope("abandoned");

        // Stub predicate: only `alive` is live.
        let live_key = live.stable_key();
        let hook: super::ScopeLivenessHook = Arc::new(
            move |scope: ResourceScopeKey, _restricted_creator: Option<String>| {
                let is_live = scope.stable_key() == live_key;
                Box::pin(async move { is_live }) as futures::future::BoxFuture<'static, bool>
            },
        );
        manager.set_scope_liveness_hook(hook);

        let candidates = vec![
            ("k-alive".to_string(), live, None),
            ("k-dead".to_string(), dead, None),
        ];
        let reap = manager.filter_reapable(candidates).await;

        // Live scope preserved, dead scope reaped.
        assert_eq!(
            reap,
            vec!["k-dead".to_string()],
            "live scope must be skipped, abandoned scope must be reaped"
        );
    }

    #[tokio::test]
    async fn filter_reapable_checks_restricted_creator_not_shared_scope() {
        let manager = BrowserSessionManager::default();
        let shared = scope("shared");
        let hook: super::ScopeLivenessHook = Arc::new(
            |_scope: ResourceScopeKey, restricted_creator: Option<String>| {
                Box::pin(async move { restricted_creator.as_deref() == Some("live-child") })
                    as futures::future::BoxFuture<'static, bool>
            },
        );
        manager.set_scope_liveness_hook(hook);

        let candidates = vec![
            ("live".into(), shared.clone(), Some("live-child".into())),
            ("done".into(), shared, Some("done-child".into())),
        ];

        assert_eq!(manager.filter_reapable(candidates).await, vec!["done"]);
    }

    /// `set_scope_liveness_hook` is set-once: the first install wins and a
    /// second call is a silent no-op (does not panic, does not replace).
    #[tokio::test]
    async fn scope_liveness_hook_is_set_once() {
        let manager = BrowserSessionManager::default();
        // First hook: everything live (nothing reapable).
        let first: super::ScopeLivenessHook = Arc::new(
            |_scope: ResourceScopeKey, _restricted_creator: Option<String>| {
                Box::pin(async { true }) as futures::future::BoxFuture<'static, bool>
            },
        );
        manager.set_scope_liveness_hook(first);
        // Second hook would say nothing is live — must be ignored.
        let second: super::ScopeLivenessHook = Arc::new(
            |_scope: ResourceScopeKey, _restricted_creator: Option<String>| {
                Box::pin(async { false }) as futures::future::BoxFuture<'static, bool>
            },
        );
        manager.set_scope_liveness_hook(second);

        let candidates = vec![("k".to_string(), scope("c"), None)];
        let reap = manager.filter_reapable(candidates).await;
        assert!(
            reap.is_empty(),
            "first hook (all-live) must win; second hook ignored"
        );
    }
}

#[cfg(test)]
mod console_level_tests {
    use super::ConsoleLevel;
    use chromiumoxide::cdp::js_protocol::runtime::ConsoleApiCalledType;

    /// All 18 CDP variants round-trip through the typed conversion to the
    /// canonical CDP wire identifier. If chromiumoxide adds a variant, the
    /// `From<ConsoleApiCalledType>` impl stops compiling — guaranteeing the
    /// "stringly-typed level" bug class cannot regress.
    #[test]
    fn from_cdp_total_and_canonical() {
        let cases = [
            (ConsoleApiCalledType::Log, "log"),
            (ConsoleApiCalledType::Debug, "debug"),
            (ConsoleApiCalledType::Info, "info"),
            (ConsoleApiCalledType::Warning, "warning"),
            (ConsoleApiCalledType::Error, "error"),
            (ConsoleApiCalledType::Dir, "dir"),
            (ConsoleApiCalledType::Dirxml, "dirxml"),
            (ConsoleApiCalledType::Table, "table"),
            (ConsoleApiCalledType::Trace, "trace"),
            (ConsoleApiCalledType::Clear, "clear"),
            (ConsoleApiCalledType::StartGroup, "startGroup"),
            (
                ConsoleApiCalledType::StartGroupCollapsed,
                "startGroupCollapsed",
            ),
            (ConsoleApiCalledType::EndGroup, "endGroup"),
            (ConsoleApiCalledType::Assert, "assert"),
            (ConsoleApiCalledType::Profile, "profile"),
            (ConsoleApiCalledType::ProfileEnd, "profileEnd"),
            (ConsoleApiCalledType::Count, "count"),
            (ConsoleApiCalledType::TimeEnd, "timeEnd"),
        ];
        for (cdp, expected) in cases {
            let level: ConsoleLevel = cdp.into();
            assert_eq!(level.as_str(), expected, "wrong wire form for {expected}");
        }
    }

    /// Wire serialization matches `as_str` so consumers downstream of
    /// `json!({"level": entry.level})` see the canonical identifier and not
    /// the `Debug` spelling the previous `format!("{:?}", …).to_lowercase()`
    /// produced (e.g. `"startgroup"` rather than CDP's `"startGroup"`).
    #[test]
    fn serializes_as_canonical_string() {
        let value = serde_json::to_value(ConsoleLevel::StartGroup).unwrap();
        assert_eq!(value, serde_json::Value::String("startGroup".into()));
        let value = serde_json::to_value(ConsoleLevel::Error).unwrap();
        assert_eq!(value, serde_json::Value::String("error".into()));
    }
}

#[cfg(test)]
mod console_arg_tests {
    use super::{extract_console_arg_text, truncate_unicode_safe, MAX_CAPTURE_ARG_BYTES};
    use chromiumoxide::cdp::js_protocol::runtime::RemoteObject;
    use serde_json::json;

    #[allow(clippy::needless_pass_by_value)]
    fn make_arg(value: Option<serde_json::Value>, description: Option<&str>) -> RemoteObject {
        serde_json::from_value(json!({
            "type": "string",
            "value": value,
            "description": description,
        }))
        .unwrap()
    }

    #[test]
    fn test_string_primitive() {
        let arg = make_arg(Some(json!("hello world")), None);
        assert_eq!(extract_console_arg_text(&arg), "hello world");
    }

    #[test]
    fn test_number_primitive() {
        let arg = make_arg(Some(json!(42)), None);
        assert_eq!(extract_console_arg_text(&arg), "42");
    }

    #[test]
    fn test_boolean_primitive() {
        let arg = make_arg(Some(json!(true)), None);
        assert_eq!(extract_console_arg_text(&arg), "true");
    }

    #[test]
    fn test_null_value() {
        // console.log(null): CDP sends description "null", value is absent (None after serde)
        let arg = make_arg(None, Some("null"));
        assert_eq!(extract_console_arg_text(&arg), "null");
    }

    #[test]
    fn test_json_object_in_value() {
        // When Chrome does serialize the value (e.g. simple JSON objects)
        let arg = make_arg(Some(json!({"foo": "bar"})), None);
        let result = extract_console_arg_text(&arg);
        assert!(result.contains("foo"), "Expected JSON, got: {result}");
        assert!(result.contains("bar"), "Expected JSON, got: {result}");
    }

    #[test]
    fn test_object_with_preview() {
        // console.log({foo: 'bar'}) — Chrome omits value but provides preview
        let arg: RemoteObject = serde_json::from_value(json!({
            "type": "object",
            "description": "Object",
            "preview": {
                "type": "object",
                "overflow": false,
                "properties": [
                    {"name": "foo", "type": "string", "value": "'bar'"}
                ]
            }
        }))
        .unwrap();
        let result = extract_console_arg_text(&arg);
        assert!(
            result.contains("foo"),
            "Expected property name, got: {result}"
        );
        assert!(
            result.contains("bar"),
            "Expected property value, got: {result}"
        );
        assert!(
            result.starts_with('{'),
            "Expected object notation: {result}"
        );
    }

    #[test]
    fn test_array_with_preview() {
        // console.log([1, 2, 3])
        let arg: RemoteObject = serde_json::from_value(json!({
            "type": "object",
            "subtype": "array",
            "description": "Array(3)",
            "preview": {
                "type": "object",
                "subtype": "array",
                "overflow": false,
                "properties": [
                    {"name": "0", "type": "number", "value": "1"},
                    {"name": "1", "type": "number", "value": "2"},
                    {"name": "2", "type": "number", "value": "3"}
                ]
            }
        }))
        .unwrap();
        let result = extract_console_arg_text(&arg);
        assert_eq!(result, "[1, 2, 3]");
    }

    #[test]
    fn test_object_overflow_in_preview() {
        let arg: RemoteObject = serde_json::from_value(json!({
            "type": "object",
            "description": "Object",
            "preview": {
                "type": "object",
                "overflow": true,
                "properties": [
                    {"name": "a", "type": "number", "value": "1"}
                ]
            }
        }))
        .unwrap();
        let result = extract_console_arg_text(&arg);
        assert!(
            result.contains('…'),
            "Expected overflow indicator: {result}"
        );
    }

    #[test]
    fn test_description_fallback_when_no_preview() {
        // Object with no preview — falls back to description
        let arg: RemoteObject = serde_json::from_value(json!({
            "type": "object",
            "description": "MyClass"
        }))
        .unwrap();
        assert_eq!(extract_console_arg_text(&arg), "MyClass");
    }

    #[test]
    fn test_short_string_not_truncated() {
        let arg = make_arg(Some(json!("hello")), None);
        assert_eq!(extract_console_arg_text(&arg), "hello");
    }

    #[test]
    fn test_memory_cap_applied_at_capture() {
        // Strings over MAX_CAPTURE_ARG_BYTES are capped in the buffer (memory protection)
        let huge = "x".repeat(MAX_CAPTURE_ARG_BYTES + 500);
        let arg = make_arg(Some(serde_json::Value::String(huge)), None);
        let result = extract_console_arg_text(&arg);
        assert!(
            result.len() <= MAX_CAPTURE_ARG_BYTES + 4,
            "Memory cap should apply"
        );
        assert!(result.ends_with('…'), "Should end with ellipsis");
    }

    #[test]
    fn test_moderate_string_not_capped() {
        // Strings under the display limit (500) pass through completely intact
        let medium = "a".repeat(600);
        let arg = make_arg(Some(serde_json::Value::String(medium.clone())), None);
        // 600 < MAX_CAPTURE_ARG_BYTES (10_000), so no cap applied
        assert_eq!(extract_console_arg_text(&arg), medium);
    }

    #[test]
    fn test_truncate_unicode_safe_ascii() {
        let s = "a".repeat(600);
        let result = truncate_unicode_safe(s, 500);
        assert!(result.ends_with('…'));
        assert!(result.len() <= 504); // 500 bytes + 3-byte ellipsis
    }

    #[test]
    fn test_truncate_unicode_safe_multibyte() {
        // Each '€' is 3 bytes; 167 of them = 501 bytes, just over the 500-byte limit
        let s = "€".repeat(167);
        let result = truncate_unicode_safe(s, 500);
        // Should cut at 166 chars (498 bytes) and append …
        assert!(result.ends_with('…'));
        assert!(
            !result.contains('\u{FFFD}'),
            "No replacement chars — unicode safe"
        );
        // The slice must be valid UTF-8
        let _ = result.as_str();
    }

    #[test]
    fn test_truncate_unicode_safe_fits_exactly() {
        let s = "hello".to_string();
        assert_eq!(truncate_unicode_safe(s.clone(), 5), s);
    }

    #[test]
    fn test_unserializable_undefined() {
        let arg: RemoteObject = serde_json::from_value(json!({
            "type": "undefined",
            "unserializableValue": "undefined"
        }))
        .unwrap();
        assert_eq!(extract_console_arg_text(&arg), "undefined");
    }
}
