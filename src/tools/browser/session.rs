//! Browser session management
//!
//! REQ-BT-010: Implicit Session Model
//! REQ-BT-011: State Persistence

#![allow(dead_code)] // Work in progress - browser tools being integrated

use chromiumoxide::{
    browser::{Browser, BrowserConfig},
    cdp::js_protocol::runtime::{EventConsoleApiCalled, RemoteObject},
    fetcher::{BrowserFetcher, BrowserFetcherOptions},
    Page,
};
use futures::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Maximum console log entries to keep per session
const MAX_CONSOLE_LOGS: usize = 1000;

/// Idle timeout before session cleanup (30 minutes)
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Cleanup check interval (60 seconds)
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Default viewport dimensions
const DEFAULT_VIEWPORT_WIDTH: u32 = 1280;
const DEFAULT_VIEWPORT_HEIGHT: u32 = 720;

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
}

impl From<chromiumoxide::error::CdpError> for BrowserError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        BrowserError::OperationFailed(e.to_string())
    }
}

/// Console log entry captured from the browser
#[derive(Debug, Clone)]
pub struct ConsoleEntry {
    pub level: String,
    pub text: String,
    pub timestamp: Instant,
}

/// Per-conversation browser instance
pub struct BrowserSession {
    #[allow(dead_code)] // Browser must stay alive
    browser: Browser,
    #[allow(dead_code)] // Task must stay alive
    handler_task: JoinHandle<()>,
    #[allow(dead_code)] // Task must stay alive
    console_task: Option<JoinHandle<()>>,
    /// The current page (public for tool access)
    pub page: Page,
    /// Console logs captured from the page (separate lock to avoid contention)
    pub console_logs: Arc<StdMutex<VecDeque<ConsoleEntry>>>,
    /// Last activity timestamp (for idle timeout)
    pub last_activity: Instant,
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
            other => other.to_string(),
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
pub(crate) fn truncate_unicode_safe(s: String, max_bytes: usize) -> String {
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
    format!("{}…", &s[..boundary])
}

impl BrowserSession {
    /// Directory where the fetcher caches downloaded Chrome binaries
    pub(crate) fn fetcher_cache_dir() -> PathBuf {
        let base = std::env::var("HOME").map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from);
        base.join(".cache/phoenix-ide/chromium")
    }

    /// Build a `BrowserConfig` with optional explicit Chrome executable path
    fn browser_config(
        conversation_id: &str,
        executable: Option<&Path>,
    ) -> Result<BrowserConfig, BrowserError> {
        let user_data_dir = format!("/tmp/phoenix-chrome-{conversation_id}");

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
        conversation_id: &str,
        executable: Option<&Path>,
    ) -> Result<Self, BrowserError> {
        let config = Self::browser_config(conversation_id, executable)?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(e) = event {
                    tracing::warn!("CDP handler error: {e}");
                }
            }
        });

        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        Ok(Self {
            browser,
            handler_task,
            console_task: None,
            page,
            console_logs: Arc::new(StdMutex::new(VecDeque::with_capacity(MAX_CONSOLE_LOGS))),
            last_activity: Instant::now(),
        })
    }

    /// Create a new browser session.
    ///
    /// Tries system Chrome first (zero download). On failure, downloads a
    /// compatible Chromium via `BrowserFetcher` and caches it for future runs.
    async fn new(conversation_id: &str) -> Result<Self, BrowserError> {
        // 1. Try system Chrome (no explicit executable — chromiumoxide finds it)
        match Self::launch_and_init(conversation_id, None).await {
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
            .build()
            .map_err(|e| BrowserError::LaunchFailed(format!("Fetcher config error: {e}")))?;

        let fetcher = BrowserFetcher::new(fetcher_opts);
        let info = fetcher
            .fetch()
            .await
            .map_err(|e| BrowserError::LaunchFailed(format!("Chrome download failed: {e:#}")))?;

        tracing::info!("Using Chrome at {:?}", info.executable_path);

        Self::launch_and_init(conversation_id, Some(&info.executable_path)).await
    }

    /// Set up console log listener (called after session is wrapped in Arc<RwLock>)
    pub async fn setup_console_listener(session: Arc<RwLock<Self>>) -> Result<(), BrowserError> {
        // Get the page event listener and console_logs handle
        let (mut console_events, console_logs) = {
            let guard = session.read().await;
            let events = guard.page.event_listener::<EventConsoleApiCalled>().await?;
            let logs = guard.console_logs.clone();
            (events, logs)
        };

        // Spawn task to capture console events (uses separate lock, no contention)
        let task = tokio::spawn(async move {
            while let Some(event) = console_events.next().await {
                // Extract log level and message
                let level = format!("{:?}", event.r#type).to_lowercase();
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
}

/// RAII guard for browser session access
/// Updates `last_activity` timestamp on drop
pub struct BrowserSessionGuard<'a> {
    session: tokio::sync::RwLockWriteGuard<'a, BrowserSession>,
}

impl BrowserSessionGuard<'_> {
    pub fn page(&self) -> &Page {
        &self.session.page
    }

    pub fn page_mut(&mut self) -> &mut Page {
        &mut self.session.page
    }

    pub fn console_logs(&self) -> &Arc<StdMutex<VecDeque<ConsoleEntry>>> {
        &self.session.console_logs
    }
}

impl Drop for BrowserSessionGuard<'_> {
    fn drop(&mut self) {
        self.session.last_activity = Instant::now();
    }
}

/// Global manager for all browser sessions
pub struct BrowserSessionManager {
    sessions: RwLock<HashMap<String, Arc<RwLock<BrowserSession>>>>,
    cleanup_task: Option<JoinHandle<()>>,
}

impl BrowserSessionManager {
    /// Create a new session manager and start cleanup task
    pub fn new() -> Arc<Self> {
        let manager = Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            cleanup_task: None,
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

    /// Get or create a browser session for a conversation
    pub async fn get_or_create(
        &self,
        conversation_id: &str,
    ) -> Result<BrowserSessionGuard<'_>, BrowserError> {
        // Check if session exists
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(conversation_id) {
                let _guard = session.write().await;
                // We need to return a guard that holds the lock
                // This is tricky with the current design - let's simplify
            }
        }

        // Create new session if needed
        let mut sessions = self.sessions.write().await;

        if !sessions.contains_key(conversation_id) {
            tracing::info!(conversation_id, "Creating new browser session");
            let session = BrowserSession::new(conversation_id).await?;
            sessions.insert(conversation_id.to_string(), Arc::new(RwLock::new(session)));
        }

        // Get the session and return guard
        let _session = sessions
            .get(conversation_id)
            .ok_or_else(|| BrowserError::SessionNotFound(conversation_id.to_string()))?
            .clone();

        drop(sessions); // Release the sessions lock

        // Now acquire the session lock
        // Note: This creates a lifetime issue - we need a different approach
        // For now, let's use a simpler API
        Err(BrowserError::OperationFailed(
            "Session guard API needs redesign - use get_session instead".to_string(),
        ))
    }

    /// Get a session for a conversation (creates if needed)
    /// Returns Arc to the session - caller manages locking
    pub async fn get_session(
        &self,
        conversation_id: &str,
    ) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        // Check if session exists
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(conversation_id) {
                return Ok(session.clone());
            }
        }

        // Create new session
        let mut sessions = self.sessions.write().await;

        // Double-check after acquiring write lock
        if let Some(session) = sessions.get(conversation_id) {
            return Ok(session.clone());
        }

        tracing::info!(conversation_id, "Creating new browser session");
        let session = BrowserSession::new(conversation_id).await?;
        let session_arc = Arc::new(RwLock::new(session));

        // Set up console log listener
        if let Err(e) = BrowserSession::setup_console_listener(session_arc.clone()).await {
            tracing::warn!(error = %e, "Failed to set up console listener");
        }

        sessions.insert(conversation_id.to_string(), session_arc.clone());

        Ok(session_arc)
    }

    /// Kill a specific session (called on conversation delete)
    pub async fn kill_session(&self, conversation_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(conversation_id) {
            tracing::info!(conversation_id, "Killing browser session");
            // Session will be dropped, which closes the browser
            drop(session);

            // Clean up user data directory
            let user_data_dir = format!("/tmp/phoenix-chrome-{conversation_id}");
            if let Err(e) = tokio::fs::remove_dir_all(&user_data_dir).await {
                tracing::warn!(path = %user_data_dir, error = %e, "Failed to clean up browser data dir");
            }
        }
    }

    /// Kill all sessions (called on shutdown)
    pub async fn shutdown_all(&self) {
        let mut sessions = self.sessions.write().await;
        let count = sessions.len();
        if count > 0 {
            tracing::info!(count, "Shutting down all browser sessions");
            sessions.clear();
        }
    }

    /// Clean up sessions that have been idle too long
    async fn cleanup_idle_sessions(&self) {
        let now = Instant::now();
        let mut to_remove = Vec::new();

        // Find idle sessions
        {
            let sessions = self.sessions.read().await;
            for (conv_id, session) in sessions.iter() {
                if let Ok(guard) = session.try_read() {
                    if now.duration_since(guard.last_activity) > IDLE_TIMEOUT {
                        to_remove.push(conv_id.clone());
                    }
                }
            }
        }

        // Remove idle sessions
        if !to_remove.is_empty() {
            let mut sessions = self.sessions.write().await;
            for conv_id in to_remove {
                tracing::info!(conversation_id = %conv_id, "Cleaning up idle browser session");
                sessions.remove(&conv_id);
            }
        }
    }
}

impl Default for BrowserSessionManager {
    fn default() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            cleanup_task: None,
        }
    }
}

impl Drop for BrowserSessionManager {
    fn drop(&mut self) {
        // Cancel cleanup task if running
        if let Some(task) = self.cleanup_task.take() {
            task.abort();
        }
        // Note: sessions will be dropped automatically
        tracing::info!("BrowserSessionManager dropped - all sessions will be closed");
    }
}

#[cfg(test)]
mod console_arg_tests {
    use super::{extract_console_arg_text, truncate_unicode_safe, MAX_CAPTURE_ARG_BYTES};
    use chromiumoxide::cdp::js_protocol::runtime::RemoteObject;
    use serde_json::json;

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
