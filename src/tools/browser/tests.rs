//! End-to-end tests for browser tools
//!
//! These tests require Chrome/Chromium to be installed.
//! They will be skipped automatically if no browser is found.

use super::session::BrowserSessionManager;
use super::tools::*;
use crate::tools::{Tool, ToolContext};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Check if Chrome is available on the system
fn chrome_available() -> bool {
    which::which("google-chrome")
        .or_else(|_| which::which("chromium-browser"))
        .or_else(|_| which::which("chromium"))
        .is_ok()
}

/// Skip macro for tests that require Chrome
macro_rules! require_chrome {
    () => {
        if !chrome_available() {
            eprintln!("Skipping test: Chrome/Chromium not found in PATH");
            return;
        }
    };
}

/// Create a test context with a fresh browser session manager
fn test_context(conversation_id: &str) -> (ToolContext, Arc<BrowserSessionManager>) {
    let manager = Arc::new(BrowserSessionManager::default());
    let ctx = ToolContext::new(
        CancellationToken::new(),
        conversation_id.to_string(),
        std::env::temp_dir(),
        manager.clone(),
        Arc::new(crate::llm::ModelRegistry::new_empty()),
    );
    (ctx, manager)
}

/// Simple HTTP test server that serves static content
struct TestServer {
    addr: std::net::SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// Start a test server with the given HTML content
    async fn start(html: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let html = html.to_string();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        if let Ok((mut socket, _)) = accept {
                            let html = html.clone();
                            tokio::spawn(async move {
                                let mut buf = [0u8; 1024];
                                let _ = socket.read(&mut buf).await;
                                
                                let response = format!(
                                    "HTTP/1.1 200 OK\r\n\
                                     Content-Type: text/html\r\n\
                                     Content-Length: {}\r\n\
                                     Connection: close\r\n\
                                     \r\n\
                                     {}",
                                    html.len(),
                                    html
                                );
                                let _ = socket.write_all(response.as_bytes()).await;
                            });
                        }
                    }
                }
            }
        });

        Self {
            addr,
            shutdown: shutdown_tx,
            handle,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }
}

// ============================================================================
// Local server tests (deterministic)
// ============================================================================

#[tokio::test]
async fn test_browser_navigate_local() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Test Page</title></head>
        <body><h1 id="heading">Hello Browser Test</h1></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-navigate-local");
    let tool = BrowserNavigateTool;

    let result = tool.run(json!({"url": server.url()}), ctx).await;

    assert!(result.success, "Navigate failed: {}", result.output);
    assert!(
        result.output.contains("done"),
        "Unexpected output: {}",
        result.output
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_eval_local() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Eval Test</title></head>
        <body><div id="data" data-value="42"></div></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-local");

    // First navigate
    let nav_tool = BrowserNavigateTool;
    let nav_result = nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;
    assert!(nav_result.success, "Navigate failed: {}", nav_result.output);

    // Then eval
    let eval_tool = BrowserEvalTool;

    // Test getting document title
    let result = eval_tool
        .run(json!({"expression": "document.title"}), ctx.clone())
        .await;
    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        result.output.contains("Eval Test"),
        "Title not found: {}",
        result.output
    );

    // Test getting element attribute
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('data').dataset.value"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        result.output.contains("42"),
        "Data value not found: {}",
        result.output
    );

    // Test arithmetic
    let result = eval_tool
        .run(json!({"expression": "2 + 2"}), ctx.clone())
        .await;
    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        result.output.contains('4'),
        "Arithmetic wrong: {}",
        result.output
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_console_logs_local() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Console Test</title></head>
        <body></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-console-local");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;

    // Small delay to ensure console listener is set up
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Log some messages
    let eval_tool = BrowserEvalTool;
    eval_tool
        .run(json!({"expression": "console.log('test message')"}), ctx.clone())
        .await;
    eval_tool
        .run(json!({"expression": "console.warn('warning message')"}), ctx.clone())
        .await;
    eval_tool
        .run(json!({"expression": "console.error('error message')"}), ctx.clone())
        .await;

    // Small delay to allow async event capture
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get logs
    let logs_tool = BrowserRecentConsoleLogsTool;
    let result = logs_tool.run(json!({}), ctx.clone()).await;

    assert!(result.success, "Get logs failed: {}", result.output);
    assert!(
        result.output.contains("test message"),
        "Log message not found: {}",
        result.output
    );
    assert!(
        result.output.contains("warning message"),
        "Warning not found: {}",
        result.output
    );
    assert!(
        result.output.contains("error message"),
        "Error not found: {}",
        result.output
    );

    // Clear logs
    let clear_tool = BrowserClearConsoleLogsTool;
    let result = clear_tool.run(json!({}), ctx.clone()).await;
    assert!(result.success, "Clear logs failed: {}", result.output);
    assert!(
        result.output.contains("Cleared"),
        "Clear message missing: {}",
        result.output
    );

    // Verify cleared
    let result = logs_tool.run(json!({}), ctx.clone()).await;
    assert!(result.success);
    assert!(
        result.output.contains("[]"),
        "Logs not cleared: {}",
        result.output
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_screenshot_local() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Screenshot Test</title></head>
        <body style="background: red; width: 100vw; height: 100vh;"></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-screenshot-local");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;

    // Take screenshot
    let screenshot_tool = BrowserTakeScreenshotTool;
    let result = screenshot_tool.run(json!({}), ctx.clone()).await;

    assert!(
        result.success,
        "Screenshot failed: {}",
        result.output
    );

    // Check that we got image data (either inline base64 or file path)
    let has_image = result.output.contains("Screenshot saved")
        || result.output.contains("Screenshot captured")
        || result.display_data.is_some();
    assert!(has_image, "No screenshot data: {}", result.output);

    // If we have display_data, verify it's valid PNG
    if let Some(ref display) = result.display_data {
        if let Some(data) = display.get("data") {
            if let Some(base64_data) = data.as_str() {
                // PNG magic bytes after base64 decode start with iVBORw0KGgo
                assert!(
                    base64_data.starts_with("iVBORw0KGgo"),
                    "Not valid PNG data"
                );
            }
        }
    }

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_resize_local() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Resize Test</title></head>
        <body></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-resize-local");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;

    // Resize
    let resize_tool = BrowserResizeTool;
    let result = resize_tool
        .run(json!({"width": 1024, "height": 768}), ctx.clone())
        .await;

    assert!(result.success, "Resize failed: {}", result.output);

    // Verify via JS
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "window.innerWidth"}), ctx.clone())
        .await;
    assert!(result.success);
    // innerWidth should be close to 1024 (may vary slightly due to scrollbars)
    assert!(
        result.output.contains("1024") || result.output.contains("1008"),
        "Width mismatch: {}",
        result.output
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_session_persistence() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Persistence Test</title></head>
        <body><script>window.testCounter = 0;</script></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-persistence");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;

    let eval_tool = BrowserEvalTool;

    // Increment counter multiple times across separate tool calls
    eval_tool
        .run(json!({"expression": "window.testCounter++"}), ctx.clone())
        .await;
    eval_tool
        .run(json!({"expression": "window.testCounter++"}), ctx.clone())
        .await;
    eval_tool
        .run(json!({"expression": "window.testCounter++"}), ctx.clone())
        .await;

    // Verify counter persisted
    let result = eval_tool
        .run(json!({"expression": "window.testCounter"}), ctx.clone())
        .await;

    assert!(result.success);
    assert!(
        result.output.contains('3'),
        "Counter should be 3, got: {}",
        result.output
    );

    server.shutdown().await;
}

// ============================================================================
// Remote URL test (network-dependent)
// ============================================================================

#[tokio::test]
async fn test_browser_navigate_remote() {
    require_chrome!();

    let (ctx, _manager) = test_context("test-navigate-remote");

    // Navigate to a real website
    let nav_tool = BrowserNavigateTool;
    let result = nav_tool
        .run(json!({"url": "https://example.com"}), ctx.clone())
        .await;

    assert!(result.success, "Navigate failed: {}", result.output);

    // Verify we can read the page
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "document.title"}), ctx.clone())
        .await;

    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        result.output.contains("Example Domain"),
        "Wrong title: {}",
        result.output
    );

    // Verify page content
    let result = eval_tool
        .run(
            json!({"expression": "document.querySelector('h1').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("Example Domain"),
        "Wrong h1: {}",
        result.output
    );
}

// ============================================================================
// Error handling tests
// ============================================================================

#[tokio::test]
async fn test_browser_eval_before_navigate() {
    require_chrome!();

    let (ctx, _manager) = test_context("test-eval-no-nav");

    // Try to eval without navigating first - should still work on about:blank
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "1 + 1"}), ctx.clone())
        .await;

    // This should work - browser starts on about:blank
    assert!(result.success, "Eval failed: {}", result.output);
    assert!(result.output.contains('2'), "Wrong result: {}", result.output);
}

#[tokio::test]
async fn test_browser_eval_syntax_error() {
    require_chrome!();

    let server = TestServer::start("<html><body></body></html>").await;
    let (ctx, _manager) = test_context("test-eval-syntax-error");

    let nav_tool = BrowserNavigateTool;
    nav_tool.run(json!({"url": server.url()}), ctx.clone()).await;

    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "this is not valid javascript {{{{"}), ctx.clone())
        .await;

    // Should fail gracefully
    assert!(!result.success, "Should have failed");
    assert!(
        result.output.to_lowercase().contains("error")
            || result.output.to_lowercase().contains("syntaxerror"),
        "Should mention error: {}",
        result.output
    );

    server.shutdown().await;
}

#[tokio::test]
async fn test_browser_navigate_invalid_url() {
    require_chrome!();

    let (ctx, _manager) = test_context("test-invalid-url");

    let nav_tool = BrowserNavigateTool;
    let result = nav_tool
        .run(json!({"url": "not-a-valid-url"}), ctx.clone())
        .await;

    // Should fail or handle gracefully
    // (chromiumoxide may accept weird URLs, so we just check it doesn't panic)
    assert!(
        !result.output.is_empty(),
        "Should have some output"
    );
}
