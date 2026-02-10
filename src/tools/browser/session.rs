//! Browser session management
//!
//! REQ-BT-010: Implicit Session Model
//! REQ-BT-011: State Persistence

#![allow(dead_code)] // Work in progress - browser tools being integrated

use chromiumoxide::{
    browser::{Browser, BrowserConfig},
    cdp::js_protocol::runtime::EventConsoleApiCalled,
    Page,
};
use futures::StreamExt;
use std::collections::{HashMap, VecDeque};
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

impl BrowserSession {
    /// Create a new browser session
    async fn new(conversation_id: &str) -> Result<Self, BrowserError> {
        // Configure Chrome launch
        let user_data_dir = format!("/tmp/phoenix-chrome-{conversation_id}");

        let config = BrowserConfig::builder()
            .new_headless_mode() // Uses --headless=new for modern headless
            .no_sandbox() // Required for running as root / in containers
            .arg("--disable-gpu") // No GPU in server environment
            .arg("--disable-software-rasterizer")
            .user_data_dir(&user_data_dir)
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
                device_scale_factor: Some(1.0),
                emulating_mobile: false,
                is_landscape: true,
                has_touch: false,
            })
            .build()
            .map_err(|e| BrowserError::LaunchFailed(e.clone()))?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        // Spawn handler task
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                // Handle CDP events - logging for now
                if let Err(e) = event {
                    tracing::warn!("CDP handler error: {e}");
                }
            }
        });

        // Create initial page
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserError::LaunchFailed(e.to_string()))?;

        Ok(Self {
            browser,
            handler_task,
            console_task: None, // Set up later via setup_console_listener
            page,
            console_logs: Arc::new(StdMutex::new(VecDeque::with_capacity(MAX_CONSOLE_LOGS))),
            last_activity: Instant::now(),
        })
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
                    .map(|arg| {
                        // Try value first (for JSON-serializable primitives)
                        if let Some(value) = &arg.value {
                            match value {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            }
                        // Fall back to description (string representation of any object)
                        } else if let Some(desc) = &arg.description {
                            desc.clone()
                        // Fall back to unserializable value (undefined, NaN, Infinity, etc.)
                        } else if let Some(unser) = &arg.unserializable_value {
                            unser.inner().clone()
                        } else {
                            String::from("[unknown]")
                        }
                    })
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
