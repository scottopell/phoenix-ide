//! Browser tool implementations
//!
//! REQ-BT-001: Navigate to URLs
//! REQ-BT-002: Execute JavaScript
//! REQ-BT-003: Take Screenshots
//! REQ-BT-004: Capture Console Logs
//! REQ-BT-005: Resize Viewport

use super::session::BrowserSession;
use crate::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Parse duration from string like "15s", "1m", "500ms"
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse().ok().map(Duration::from_millis)
    } else if let Some(s_val) = s.strip_suffix('s') {
        s_val.trim().parse().ok().map(Duration::from_secs)
    } else if let Some(m) = s.strip_suffix('m') {
        m.trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse().ok().map(Duration::from_secs)
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

// ============================================================================
// browser_navigate (REQ-BT-001)
// ============================================================================

#[derive(Debug, Deserialize)]
struct NavigateInput {
    url: String,
    #[serde(default)]
    timeout: Option<String>,
}

pub struct BrowserNavigateTool;

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &'static str {
        "browser_navigate"
    }

    fn description(&self) -> String {
        "Navigate the browser to a URL and wait for the page to load. The browser session persists across tool calls — cookies, JS state, and DOM are preserved until the conversation ends. Call this before any other browser interaction with a new URL. Prefer browser tools over bash curl/wget when you need a rendered page or JS-driven content.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to navigate to"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout duration (default: 15s). Examples: '5s', '1m', '500ms'"
                }
            },
            "required": ["url"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: NavigateInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(DEFAULT_TIMEOUT);

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        // Navigate with timeout
        let result = tokio::time::timeout(timeout, guard.page.goto(&input.url)).await;

        match result {
            Ok(Ok(_)) => ToolOutput::success("done"),
            Ok(Err(e)) => ToolOutput::error(format!("Navigation failed: {e}")),
            Err(_) => ToolOutput::error(format!("Timeout after {timeout:?} waiting for page load")),
        }
    }
}

// ============================================================================
// browser_eval (REQ-BT-002)
// ============================================================================

#[derive(Debug, Deserialize)]
struct EvalInput {
    expression: String,
    #[serde(default)]
    timeout: Option<String>,
    #[serde(default = "default_true")]
    r#await: bool,
}

fn default_true() -> bool {
    true
}

pub struct BrowserEvalTool;

#[async_trait]
impl Tool for BrowserEvalTool {
    fn name(&self) -> &'static str {
        "browser_eval"
    }

    fn description(&self) -> String {
        "Evaluate JavaScript in the current page context and return the result. Use for reading page state, extracting data, or complex interactions not covered by the dedicated tools. For clicks and typing, prefer browser_click and browser_type — they use CDP-level events that reliably trigger React/Vue/Angular handlers. Large outputs (>4KB) are saved to a temp file.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "JavaScript expression to evaluate"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout duration (default: 15s). Examples: '5s', '1m', '500ms'"
                },
                "await": {
                    "type": "boolean",
                    "description": "If true, wait for promises to resolve and return their resolved value (default: true)"
                }
            },
            "required": ["expression"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: EvalInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(DEFAULT_TIMEOUT);

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        // Evaluate JavaScript with timeout
        let expr = if input.r#await {
            // Wrap in async IIFE to await promises
            format!("(async () => {{ return {}; }})()", input.expression)
        } else {
            input.expression.clone()
        };

        let result = tokio::time::timeout(timeout, guard.page.evaluate(expr)).await;

        match result {
            Ok(Ok(eval_result)) => {
                let json_str = match eval_result.value() {
                    Some(v) => {
                        serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".to_string())
                    }
                    None => "undefined".to_string(),
                };

                // Check if output is large
                if json_str.len() > 4096 {
                    // Write to temp file
                    let path = format!("/tmp/phoenix-js-result-{}.json", uuid::Uuid::new_v4());
                    if let Err(e) = tokio::fs::write(&path, &json_str).await {
                        return ToolOutput::error(format!("Failed to write large output: {e}"));
                    }
                    ToolOutput::success(format!("Output written to {path} (use `cat` to view)"))
                } else {
                    ToolOutput::success(format!(
                        "<javascript_result>{json_str}</javascript_result>"
                    ))
                }
            }
            Ok(Err(e)) => ToolOutput::error(format!("JavaScript error: {e}")),
            Err(_) => ToolOutput::error(format!("Timeout after {timeout:?}")),
        }
    }
}

// ============================================================================
// browser_take_screenshot (REQ-BT-003)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ScreenshotInput {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    timeout: Option<String>,
}

pub struct BrowserTakeScreenshotTool;

#[async_trait]
impl Tool for BrowserTakeScreenshotTool {
    fn name(&self) -> &'static str {
        "browser_take_screenshot"
    }

    fn description(&self) -> String {
        "Capture a screenshot of the current page or a specific element. The image is saved to a temp file path returned in the result. To view the screenshot content yourself, follow up with read_image on that path.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the element to screenshot (optional)"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout duration (default: 15s). Examples: '5s', '1m', '500ms'"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ScreenshotInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(DEFAULT_TIMEOUT);

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        // Take screenshot with timeout
        let result = if let Some(selector) = &input.selector {
            // Element screenshot
            let element_result =
                tokio::time::timeout(timeout, guard.page.find_element(selector)).await;

            match element_result {
                Ok(Ok(element)) => {
                    match tokio::time::timeout(
                        timeout,
                        element.screenshot(CaptureScreenshotFormat::Png),
                    )
                    .await
                    {
                        Ok(Ok(data)) => Ok(Ok(data)),
                        Ok(Err(e)) => Ok(Err(e)),
                        Err(e) => Err(e),
                    }
                }
                Ok(Err(e)) => return ToolOutput::error(format!("Element not found: {e}")),
                Err(_) => return ToolOutput::error(format!("Timeout finding element: {selector}")),
            }
        } else {
            // Full page screenshot
            let params = ScreenshotParams::builder().build();
            tokio::time::timeout(timeout, guard.page.screenshot(params)).await
        };

        match result {
            Ok(Ok(png_data)) => {
                // Save to file
                let filename = format!("phoenix-screenshot-{}.png", uuid::Uuid::new_v4());
                let path = format!("/tmp/{filename}");

                if let Err(e) = tokio::fs::write(&path, &png_data).await {
                    return ToolOutput::error(format!("Failed to save screenshot: {e}"));
                }

                // Return base64 for vision
                let base64_data =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_data);

                ToolOutput::success(format!("Screenshot taken (saved as {path})")).with_display(
                    json!({
                        "type": "image",
                        "media_type": "image/png",
                        "data": base64_data,
                    }),
                )
            }
            Ok(Err(e)) => ToolOutput::error(format!("Screenshot failed: {e}")),
            Err(_) => ToolOutput::error(format!("Timeout after {timeout:?}")),
        }
    }
}

// ============================================================================
// browser_recent_console_logs (REQ-BT-004, REQ-BT-015)
// ============================================================================

/// Maximum characters per entry shown inline in the tool result.
/// Protects the LLM context window. Full content is written untruncated
/// when the file escape hatch fires (REQ-BT-015).
const DISPLAY_ENTRY_LEN: usize = 500;

#[derive(Debug, Deserialize)]
struct ConsoleLogsInput {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

pub struct BrowserRecentConsoleLogsTool;

#[async_trait]
impl Tool for BrowserRecentConsoleLogsTool {
    fn name(&self) -> &'static str {
        "browser_recent_console_logs"
    }

    fn description(&self) -> String {
        "Retrieve captured browser console logs (console.log, .warn, .error, etc.). Use after page interactions to check for JS errors or debug output. Logs accumulate for the session — use browser_clear_console_logs to reset before a focused interaction.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of log entries to return (default: 100)"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ConsoleLogsInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let guard = session.read().await;

        // Collect raw entries once; build display and full variants separately.
        let raw_entries: Vec<_> = {
            let console_logs = guard.console_logs.lock().unwrap();
            console_logs
                .iter()
                .rev()
                .take(input.limit)
                .map(|entry| (entry.level.clone(), entry.text.clone()))
                .collect()
        };

        // Display version: per-entry truncation for LLM context safety
        let display_logs: Vec<_> = raw_entries
            .iter()
            .map(|(level, text)| {
                let display_text = crate::tools::browser::session::truncate_unicode_safe(
                    text.clone(),
                    DISPLAY_ENTRY_LEN,
                );
                json!({"level": level, "text": display_text})
            })
            .collect();

        let json_str =
            serde_json::to_string_pretty(&display_logs).unwrap_or_else(|_| "[]".to_string());

        if json_str.len() > 4096 {
            // File escape hatch: write FULL untruncated entries so the agent
            // can retrieve complete content via bash/cat (REQ-BT-015)
            let full_logs: Vec<_> = raw_entries
                .iter()
                .map(|(level, text)| json!({"level": level, "text": text}))
                .collect();
            let full_json =
                serde_json::to_string_pretty(&full_logs).unwrap_or_else(|_| "[]".to_string());
            let path = format!("/tmp/phoenix-console-logs-{}.json", uuid::Uuid::new_v4());
            if let Err(e) = tokio::fs::write(&path, &full_json).await {
                return ToolOutput::error(format!("Failed to write logs: {e}"));
            }
            ToolOutput::success(format!("Logs written to {path} (use `cat` to view)"))
        } else {
            ToolOutput::success(json_str)
        }
    }
}

// ============================================================================
// browser_clear_console_logs (REQ-BT-004)
// ============================================================================

pub struct BrowserClearConsoleLogsTool;

#[async_trait]
impl Tool for BrowserClearConsoleLogsTool {
    fn name(&self) -> &'static str {
        "browser_clear_console_logs"
    }

    fn description(&self) -> String {
        "Clear the console log buffer. Use before a focused interaction to isolate logs from that specific action.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn run(&self, _input: Value, ctx: ToolContext) -> ToolOutput {
        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let guard = session.write().await;
        let count = {
            let mut console_logs = guard.console_logs.lock().unwrap();
            let len = console_logs.len();
            console_logs.clear();
            len
        };

        ToolOutput::success(format!("Cleared {count} console log entries."))
    }
}

// ============================================================================
// browser_resize (REQ-BT-005)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ResizeInput {
    width: u32,
    height: u32,
    #[serde(default)]
    timeout: Option<String>,
}

pub struct BrowserResizeTool;

#[async_trait]
impl Tool for BrowserResizeTool {
    fn name(&self) -> &'static str {
        "browser_resize"
    }

    fn description(&self) -> String {
        "Resize the browser viewport. Use to test responsive layouts or match a device width (e.g. 375 for mobile, 768 for tablet, 1280 for desktop).".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "width": {
                    "type": "integer",
                    "description": "Viewport width in pixels"
                },
                "height": {
                    "type": "integer",
                    "description": "Viewport height in pixels"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout duration (default: 15s). Examples: '5s', '1m', '500ms'"
                }
            },
            "required": ["width", "height"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;

        let input: ResizeInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        if input.width == 0 || input.height == 0 {
            return ToolOutput::error("Invalid dimensions: width and height must be positive");
        }

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(DEFAULT_TIMEOUT);

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        // Set viewport size using CDP Emulation domain
        let params = SetDeviceMetricsOverrideParams::builder()
            .width(input.width)
            .height(input.height)
            .device_scale_factor(1.0)
            .mobile(false)
            .build()
            .map_err(|e| format!("Invalid params: {e}"));

        let params = match params {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };

        let result = tokio::time::timeout(timeout, guard.page.execute(params)).await;

        match result {
            Ok(Ok(_)) => ToolOutput::success("done"),
            Ok(Err(e)) => ToolOutput::error(format!("Resize failed: {e}")),
            Err(_) => ToolOutput::error(format!("Timeout after {timeout:?}")),
        }
    }
}

// ============================================================================
// browser_wait_for_selector (TDD)
// ============================================================================

#[derive(Debug, Deserialize)]
struct WaitForSelectorInput {
    /// CSS selector to wait for
    selector: String,
    /// Timeout as duration string (default: "30s")
    #[serde(default)]
    timeout: Option<String>,
    /// If true, wait for element to be visible (not just present in DOM)
    #[serde(default)]
    visible: bool,
}

pub struct BrowserWaitForSelectorTool;

#[async_trait]
impl Tool for BrowserWaitForSelectorTool {
    fn name(&self) -> &'static str {
        "browser_wait_for_selector"
    }

    fn description(&self) -> String {
        "Poll until a CSS selector appears in (or becomes visible in) the DOM. Use after navigation or interactions that trigger async content. Prefer this over manually polling with browser_eval. Set visible:true when the element may be in the DOM but hidden.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector to wait for"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout as a duration string (default: 30s). Examples: '5s', '1m', '500ms'"
                },
                "visible": {
                    "type": "boolean",
                    "description": "If true, wait for element to be visible, not just in DOM (default: false)"
                }
            },
            "required": ["selector"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: WaitForSelectorInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(Duration::from_secs(30));

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let guard = session.read().await;

        // Build the JavaScript to check for the element
        let check_script = if input.visible {
            format!(
                r"(() => {{
                    const el = document.querySelector({selector});
                    if (!el) return false;
                    const style = window.getComputedStyle(el);
                    return style.display !== 'none' && 
                           style.visibility !== 'hidden' && 
                           style.opacity !== '0' &&
                           el.offsetParent !== null;
                }})()",
                selector = serde_json::to_string(&input.selector).unwrap()
            )
        } else {
            format!(
                "document.querySelector({}) !== null",
                serde_json::to_string(&input.selector).unwrap()
            )
        };

        let poll_interval = Duration::from_millis(100);
        let start = std::time::Instant::now();

        loop {
            // Check if element exists/is visible
            match guard.page.evaluate(check_script.clone()).await {
                Ok(result) => {
                    if let Ok(found) = result.into_value::<bool>() {
                        if found {
                            let elapsed = start.elapsed();
                            return ToolOutput::success(format!(
                                "Element '{}' found after {:.1}s",
                                input.selector,
                                elapsed.as_secs_f64()
                            ));
                        }
                    }
                }
                Err(e) => {
                    // Check if it's a selector syntax error
                    let err_str = e.to_string();
                    if err_str.contains("SyntaxError")
                        || err_str.contains("is not a valid selector")
                    {
                        return ToolOutput::error(format!(
                            "Invalid selector '{}': {}",
                            input.selector, e
                        ));
                    }
                    // Other errors might be transient, continue polling
                }
            }

            // Check timeout
            if start.elapsed() >= timeout {
                return ToolOutput::error(format!(
                    "Timeout after {:?}: element '{}' not found{}",
                    timeout,
                    input.selector,
                    if input.visible { " or not visible" } else { "" }
                ));
            }

            // Wait before next poll
            tokio::time::sleep(poll_interval).await;
        }
    }
}

// ============================================================================
// browser_click (TDD)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ClickInput {
    /// CSS selector for element to click
    selector: String,
    /// If true, wait for element to appear before clicking (default: false)
    #[serde(default)]
    wait: bool,
    /// Timeout for waiting (default: "30s")
    #[serde(default)]
    timeout: Option<String>,
}

pub struct BrowserClickTool;

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &'static str {
        "browser_click"
    }

    fn description(&self) -> String {
        "Click an element by CSS selector using CDP-level mouse events. Prefer this over browser_eval for clicks — CDP events reliably trigger React/Vue/Angular handlers. Set wait:true to automatically wait for the element to appear before clicking.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the element to click"
                },
                "wait": {
                    "type": "boolean",
                    "description": "If true, wait for element to appear before clicking (default: false)"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout for waiting (default: 30s)"
                }
            },
            "required": ["selector"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ClickInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(Duration::from_secs(30));

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let guard = session.read().await;

        // Optionally wait for element
        if input.wait {
            let check_script = format!(
                "document.querySelector({}) !== null",
                serde_json::to_string(&input.selector).unwrap()
            );
            let poll_interval = Duration::from_millis(100);
            let start = std::time::Instant::now();

            loop {
                if let Ok(result) = guard.page.evaluate(check_script.clone()).await {
                    if let Ok(true) = result.into_value::<bool>() {
                        break;
                    }
                }
                if start.elapsed() >= timeout {
                    return ToolOutput::error(format!(
                        "Timeout waiting for element '{}'",
                        input.selector
                    ));
                }
                tokio::time::sleep(poll_interval).await;
            }
        }

        // Find the element
        let element = match guard.page.find_element(&input.selector).await {
            Ok(el) => el,
            Err(e) => {
                return ToolOutput::error(format!(
                    "Could not find element '{}': {}",
                    input.selector, e
                ));
            }
        };

        // Click using CDP (works with React, Vue, etc.)
        match element.click().await {
            Ok(_) => ToolOutput::success(format!("Clicked element '{}'", input.selector)),
            Err(e) => ToolOutput::error(format!("Click failed: {e}")),
        }
    }
}

// ============================================================================
// browser_type (TDD)
// ============================================================================

#[derive(Debug, Deserialize)]
struct TypeInput {
    /// CSS selector for input element
    selector: String,
    /// Text to type
    text: String,
    /// If true, clear existing text before typing (default: false)
    #[serde(default)]
    clear: bool,
    /// Timeout (default: "30s")
    #[serde(default)]
    timeout: Option<String>,
}

pub struct BrowserTypeTool;

#[async_trait]
impl Tool for BrowserTypeTool {
    fn name(&self) -> &'static str {
        "browser_type"
    }

    fn description(&self) -> String {
        "Type text into an input element using CDP-level keyboard events. Prefer this over browser_eval for form input — CDP events fire the key events that React/Vue/Angular listen to. Set clear:true to replace existing text instead of appending.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the input element"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type into the element"
                },
                "clear": {
                    "type": "boolean",
                    "description": "If true, clear existing text before typing (default: false)"
                },
                "timeout": {
                    "type": "string",
                    "description": "Timeout (default: 30s)"
                }
            },
            "required": ["selector", "text"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: TypeInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let _timeout = input
            .timeout
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(Duration::from_secs(30));

        // Get browser session
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let guard = session.read().await;

        // Find the element
        let element = match guard.page.find_element(&input.selector).await {
            Ok(el) => el,
            Err(e) => {
                return ToolOutput::error(format!(
                    "Could not find element '{}': {}",
                    input.selector, e
                ));
            }
        };

        // Click to focus
        if let Err(e) = element.click().await {
            return ToolOutput::error(format!("Failed to focus element: {e}"));
        }

        // Small delay to ensure focus
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Clear existing text if requested
        if input.clear {
            // Select all and delete
            if let Err(e) = guard
                .page
                .evaluate(format!(
                    "document.querySelector({}).select()",
                    serde_json::to_string(&input.selector).unwrap()
                ))
                .await
            {
                return ToolOutput::error(format!("Failed to select text: {e}"));
            }

            // Press backspace to delete selected text
            if let Err(e) = element.press_key("Backspace").await {
                return ToolOutput::error(format!("Failed to clear text: {e}"));
            }
        }

        // Type the text using CDP keyboard events
        // Handle newlines specially since type_str doesn't support them
        let parts: Vec<&str> = input.text.split('\n').collect();
        for (i, part) in parts.iter().enumerate() {
            // Type the text part
            if !part.is_empty() {
                if let Err(e) = element.type_str(part).await {
                    return ToolOutput::error(format!("Type failed: {e}"));
                }
            }
            // Add Enter between parts (not after last)
            if i < parts.len() - 1 {
                if let Err(e) = element.press_key("Enter").await {
                    return ToolOutput::error(format!("Failed to press Enter: {e}"));
                }
            }
        }

        ToolOutput::success(format!(
            "Typed {} characters into '{}'",
            input.text.len(),
            input.selector
        ))
    }
}
