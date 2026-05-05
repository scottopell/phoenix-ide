//! End-to-end tests for browser tools
//!
//! Chrome/Chromium is auto-downloaded via the fetcher if not in PATH.

use super::session::BrowserSessionManager;
use super::tools::*;
use crate::tools::{Tool, ToolContext};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Check if Chrome is available or obtainable.
///
/// `dev.py check` classifies the environment up front and sets internal
/// signal env vars so the test suite skips the classes of tests that
/// would otherwise fail for environmental reasons (no usable Chromium,
/// no outbound network). This function consults that signal — callers
/// never need to set anything by hand.
fn chrome_available() -> bool {
    !matches!(
        std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
        Ok("1") | Ok("true"),
    )
}

/// Check if outbound HTTPS to the public internet is available. The
/// `*_remote` browser tests navigate to real websites (example.com)
/// and need real network. `dev.py check` probes reachability and sets
/// `PHOENIX_SKIP_NETWORK_TESTS=1` in restricted envs (no outbound
/// HTTPS) so those tests skip cleanly instead of producing env-noise
/// failures.
fn network_available() -> bool {
    !matches!(
        std::env::var("PHOENIX_SKIP_NETWORK_TESTS").as_deref(),
        Ok("1") | Ok("true"),
    )
}

/// Skip macro for tests that require Chrome
macro_rules! require_chrome {
    () => {
        if !chrome_available() {
            eprintln!("Skipping test: Chrome/Chromium not available");
            return;
        }
    };
}

/// Skip macro for tests that require outbound HTTPS to public hosts.
macro_rules! require_network {
    () => {
        if !network_available() {
            eprintln!("Skipping test: outbound HTTPS not available in this env");
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
        Arc::new(crate::tools::BashHandleRegistry::new()),
        Arc::new(crate::llm::ModelRegistry::new_empty()),
        crate::terminal::ActiveTerminals::new(),
        Arc::new(crate::tools::TmuxRegistry::new()),
        None,
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
                                // Force-close so Chrome releases the keep-alive connection
                                let _ = socket.shutdown().await;
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
        // Timeout guards against Chrome keeping connections open past server teardown
        let _ = tokio::time::timeout(Duration::from_secs(5), self.handle).await;
    }
}

/// Shut down browser sessions before the test server so Chrome releases its
/// connections first, preventing server.shutdown() from hanging.
async fn shutdown_test(manager: Arc<BrowserSessionManager>, server: TestServer) {
    manager.shutdown_all().await;
    server.shutdown().await;
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

    shutdown_test(_manager, server).await;
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
    let nav_result = nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;
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

    shutdown_test(_manager, server).await;
}

// ============================================================================
// REQ-552: browser_eval returning undefined for valid DOM expressions
// ============================================================================

#[tokio::test]
async fn test_eval_inner_text_not_undefined() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html>
        <head><title>InnerText Test</title></head>
        <body><p>Hello from innerText</p></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-innertext");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.body.innerText"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "Got undefined instead of text: {}",
        result.output
    );
    assert!(
        result.output.contains("Hello from innerText"),
        "Expected page text, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_eval_inner_html_slice_not_undefined() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html><head><title>Slice Test</title></head>
        <body><div id="content">Slice test content</div></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-htmlslice");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.body.innerHTML.slice(0, 200)"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "Got undefined instead of HTML: {}",
        result.output
    );
    assert!(
        result.output.contains("content") || result.output.len() > 10,
        "Expected HTML content, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_eval_json_stringify_dom_not_undefined() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html><head><title>JSON Test</title></head>
        <body><p id="msg">test content</p></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-jsonstringify");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;
    // This is the exact pattern from the bug report
    let result = eval_tool
        .run(
            json!({"expression": "JSON.stringify({bodyText: document.body.innerText})"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "Got undefined instead of JSON: {}",
        result.output
    );
    assert!(
        result.output.contains("bodyText"),
        "Expected JSON with bodyText key, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_eval_complex_page_inner_text() {
    require_chrome!();

    // Serve a page closer to a real React app: scripts, dynamic DOM, lots of elements
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Complex Page</title></head>
        <body>
            <div id="app">
                <header><nav><a href="/">Home</a><a href="/about">About</a></nav></header>
                <main>
                    <article>
                        <h1>Article Title</h1>
                        <p>First paragraph with some text content for testing innerText extraction.</p>
                        <p>Second paragraph with <strong>bold</strong> and <em>italic</em> text.</p>
                        <ul><li>Item one</li><li>Item two</li><li>Item three</li></ul>
                        <table><tr><th>Name</th><th>Value</th></tr><tr><td>Key</td><td>42</td></tr></table>
                    </article>
                    <aside>
                        <div class="widget"><span>Widget content</span></div>
                        <div class="widget"><span>Another widget</span></div>
                    </aside>
                </main>
                <footer><p>Footer text here</p></footer>
            </div>
            <script>
                // Simulate React-like dynamic behavior
                document.getElementById('app').dataset.hydrated = 'true';
                window.__NEXT_DATA__ = {props: {pageProps: {data: Array(100).fill({id: 1, name: 'test'})}}};
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-complex");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;

    // Test 1: document.body.innerText on a complex page
    let result = eval_tool
        .run(
            json!({"expression": "document.body.innerText"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "innerText eval failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "innerText returned undefined: {}",
        result.output
    );
    assert!(
        result.output.contains("Article Title"),
        "Missing article title from innerText: {}",
        result.output
    );

    // Test 2: innerHTML.slice on complex page
    let result = eval_tool
        .run(
            json!({"expression": "document.body.innerHTML.slice(0, 200)"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "innerHTML.slice failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "innerHTML.slice returned undefined: {}",
        result.output
    );

    // Test 3: JSON.stringify of DOM properties
    let result = eval_tool
        .run(
            json!({"expression": "JSON.stringify({title: document.title, bodyLen: document.body.innerText.length, hydrated: document.getElementById('app').dataset.hydrated})"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "JSON.stringify failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "JSON.stringify returned undefined: {}",
        result.output
    );
    assert!(
        result.output.contains("Complex Page"),
        "Missing title in JSON: {}",
        result.output
    );

    // Test 4: Reading script-set global variable
    let result = eval_tool
        .run(
            json!({"expression": "JSON.stringify(window.__NEXT_DATA__.props.pageProps.data.length)"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "Global var eval failed: {}", result.output);
    assert!(
        result.output.contains("100"),
        "Expected 100 items, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_eval_await_false_returns_value() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html><head><title>Await Test</title></head>
        <body><p>Content</p></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-await-false");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;

    // With await: false, synchronous expressions should still work
    let result = eval_tool
        .run(
            json!({"expression": "document.title", "await": false}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "Eval failed: {}", result.output);
    assert!(
        result.output.contains("Await Test"),
        "Expected title, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_eval_promise_chain_awaited() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html><head><title>Promise Test</title></head>
        <body><script>
            window.getData = () => new Promise(resolve => setTimeout(() => resolve({status: 'ok', count: 42}), 100));
        </script></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-eval-promise");
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;

    // Promise-returning expression should be awaited and return the resolved value
    let result = eval_tool
        .run(
            json!({"expression": "window.getData().then(d => JSON.stringify(d))"}),
            ctx.clone(),
        )
        .await;
    assert!(result.success, "Promise eval failed: {}", result.output);
    assert!(
        !result.output.contains("undefined"),
        "Promise returned undefined: {}",
        result.output
    );
    assert!(
        result.output.contains("ok") && result.output.contains("42"),
        "Expected resolved data, got: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_browser_console_logs_local() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html>
        <head><title>Console Test</title></head>
        <body></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-console-local");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Small delay to ensure console listener is set up
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Log some messages
    let eval_tool = BrowserEvalTool;
    eval_tool
        .run(
            json!({"expression": "console.log('test message')"}),
            ctx.clone(),
        )
        .await;
    eval_tool
        .run(
            json!({"expression": "console.warn('warning message')"}),
            ctx.clone(),
        )
        .await;
    eval_tool
        .run(
            json!({"expression": "console.error('error message')"}),
            ctx.clone(),
        )
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

    shutdown_test(_manager, server).await;
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
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Take screenshot
    let screenshot_tool = BrowserTakeScreenshotTool;
    let result = screenshot_tool.run(json!({}), ctx.clone()).await;

    assert!(result.success, "Screenshot failed: {}", result.output);

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
                assert!(base64_data.starts_with("iVBORw0KGgo"), "Not valid PNG data");
            }
        }
    }

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_browser_resize_local() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html>
        <head><title>Resize Test</title></head>
        <body></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-resize-local");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

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

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_browser_session_persistence() {
    require_chrome!();

    let server = TestServer::start(
        r"<!DOCTYPE html>
        <html>
        <head><title>Persistence Test</title></head>
        <body><script>window.testCounter = 0;</script></body>
        </html>",
    )
    .await;

    let (ctx, _manager) = test_context("test-persistence");

    // Navigate
    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

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

    shutdown_test(_manager, server).await;
}

// ============================================================================
// Remote URL test (network-dependent)
// ============================================================================

#[tokio::test]
async fn test_browser_navigate_remote() {
    require_chrome!();
    require_network!();

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
    assert!(
        result.output.contains('2'),
        "Wrong result: {}",
        result.output
    );
}

#[tokio::test]
async fn test_browser_eval_syntax_error() {
    require_chrome!();

    let server = TestServer::start("<html><body></body></html>").await;
    let (ctx, _manager) = test_context("test-eval-syntax-error");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "this is not valid javascript {{{{"}),
            ctx.clone(),
        )
        .await;

    // Should fail gracefully
    assert!(!result.success, "Should have failed");
    assert!(
        result.output.to_lowercase().contains("error")
            || result.output.to_lowercase().contains("syntaxerror"),
        "Should mention error: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

// ============================================================================
// TDD: browser_wait_for_selector tests
// ============================================================================

#[tokio::test]
async fn test_wait_for_selector_immediate() {
    require_chrome!();

    // Element exists immediately
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Wait Test</title></head>
        <body><div id="exists">I exist</div></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-wait-immediate");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let wait_tool = BrowserWaitForSelectorTool;
    let result = wait_tool
        .run(json!({"selector": "#exists"}), ctx.clone())
        .await;

    assert!(result.success, "Wait failed: {}", result.output);
    assert!(
        result.output.contains("found") || result.output.contains("visible"),
        "Should indicate element found: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_wait_for_selector_delayed() {
    require_chrome!();

    // Element appears after 500ms
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Wait Test</title></head>
        <body>
            <div id="container"></div>
            <script>
                setTimeout(() => {
                    document.getElementById('container').innerHTML = '<span class="delayed">Appeared!</span>';
                }, 500);
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-wait-delayed");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let wait_tool = BrowserWaitForSelectorTool;
    let result = wait_tool
        .run(
            json!({"selector": ".delayed", "timeout": "5s"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Wait failed: {}", result.output);

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_wait_for_selector_timeout() {
    require_chrome!();

    // Element never appears
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Wait Test</title></head>
        <body><div id="only-this">Nothing else coming</div></body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-wait-timeout");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let wait_tool = BrowserWaitForSelectorTool;
    let result = wait_tool
        .run(
            json!({"selector": "#never-exists", "timeout": "200ms"}),
            ctx.clone(),
        )
        .await;

    assert!(!result.success, "Should have timed out");
    assert!(
        result.output.to_lowercase().contains("timeout")
            || result.output.to_lowercase().contains("not found"),
        "Should mention timeout: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_wait_for_selector_hidden_then_visible() {
    require_chrome!();

    // Element exists but is hidden, then becomes visible
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Wait Test</title></head>
        <body>
            <div id="target" style="display: none;">Hidden initially</div>
            <script>
                setTimeout(() => {
                    document.getElementById('target').style.display = 'block';
                }, 500);
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-wait-visible");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let wait_tool = BrowserWaitForSelectorTool;

    // With visible: true, should wait for element to be visible
    let result = wait_tool
        .run(
            json!({"selector": "#target", "visible": true, "timeout": "5s"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Wait for visible failed: {}", result.output);

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_wait_for_selector_invalid_selector() {
    require_chrome!();

    let server = TestServer::start("<html><body></body></html>").await;
    let (ctx, _manager) = test_context("test-wait-invalid");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let wait_tool = BrowserWaitForSelectorTool;
    let result = wait_tool
        .run(json!({"selector": "###invalid[[["}), ctx.clone())
        .await;

    assert!(!result.success, "Should fail on invalid selector");
    assert!(
        result.output.to_lowercase().contains("invalid")
            || result.output.to_lowercase().contains("error")
            || result.output.to_lowercase().contains("syntax"),
        "Should mention invalid selector: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

// ============================================================================
// TDD: browser_click tests
// ============================================================================

#[tokio::test]
async fn test_click_button() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Click Test</title></head>
        <body>
            <button id="btn" onclick="document.getElementById('result').textContent = 'clicked'">Click me</button>
            <div id="result">not clicked</div>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-click-button");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Click the button
    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(json!({"selector": "#btn"}), ctx.clone())
        .await;

    assert!(result.success, "Click failed: {}", result.output);

    // Verify the click worked
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("clicked"),
        "Button click didn't work: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_click_link() {
    require_chrome!();

    let server = TestServer::start(
        r##"<!DOCTYPE html>
        <html>
        <head><title>Click Test</title></head>
        <body>
            <a id="link" href="#clicked">Click this link</a>
        </body>
        </html>"##,
    )
    .await;

    let (ctx, _manager) = test_context("test-click-link");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Click the link
    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(json!({"selector": "#link"}), ctx.clone())
        .await;

    assert!(result.success, "Click failed: {}", result.output);

    // Verify URL changed
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "window.location.hash"}), ctx.clone())
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("clicked"),
        "Link click didn't navigate: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_click_element_not_found() {
    require_chrome!();

    let server = TestServer::start("<html><body><div>No buttons here</div></body></html>").await;
    let (ctx, _manager) = test_context("test-click-not-found");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(json!({"selector": "#nonexistent"}), ctx.clone())
        .await;

    assert!(!result.success, "Should fail when element not found");
    assert!(
        result.output.to_lowercase().contains("not found")
            || result.output.to_lowercase().contains("no element")
            || result.output.to_lowercase().contains("could not find"),
        "Should mention element not found: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_click_checkbox() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Click Test</title></head>
        <body>
            <input type="checkbox" id="check" />
            <label for="check">Check me</label>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-click-checkbox");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Verify unchecked initially
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('check').checked"}),
            ctx.clone(),
        )
        .await;
    assert!(result.output.contains("false"), "Should start unchecked");

    // Click the checkbox
    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(json!({"selector": "#check"}), ctx.clone())
        .await;
    assert!(result.success, "Click failed: {}", result.output);

    // Verify checked
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('check').checked"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.output.contains("true"),
        "Checkbox should be checked: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_click_with_wait() {
    require_chrome!();

    // Element appears after delay
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Click Test</title></head>
        <body>
            <div id="container"></div>
            <div id="result">waiting</div>
            <script>
                setTimeout(() => {
                    const btn = document.createElement('button');
                    btn.id = 'delayed-btn';
                    btn.textContent = 'Click me';
                    btn.onclick = () => document.getElementById('result').textContent = 'success';
                    document.getElementById('container').appendChild(btn);
                }, 500);
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-click-wait");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Click with wait - should wait for element to appear
    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(
            json!({"selector": "#delayed-btn", "wait": true, "timeout": "5s"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Click with wait failed: {}", result.output);

    // Verify click worked
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.output.contains("success"),
        "Click didn't work: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

// ============================================================================
// TDD: browser_type tests
// ============================================================================

#[tokio::test]
async fn test_type_in_input() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="text" id="input" />
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-input");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type into input
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#input", "text": "Hello World"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("Hello World"),
        "Input value wrong: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_in_textarea() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <textarea id="textarea"></textarea>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-textarea");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type multiline text
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#textarea", "text": "Line 1\nLine 2\nLine 3"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('textarea').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("Line 1") && result.output.contains("Line 2"),
        "Textarea value wrong: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_triggers_react_events() {
    require_chrome!();

    // Simulates React-like behavior: tracks input via event listeners
    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="text" id="input" />
            <div id="mirror"></div>
            <script>
                const input = document.getElementById('input');
                const mirror = document.getElementById('mirror');
                
                // React-style: only updates on input event
                input.addEventListener('input', (e) => {
                    mirror.textContent = 'Value: ' + e.target.value;
                });
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-react");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type into input - should trigger input events
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#input", "text": "React test"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify event handler was triggered
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('mirror').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("React test"),
        "React-style event not triggered: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_with_clear() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="text" id="input" value="existing text" />
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-clear");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type with clear option - should replace existing text
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#input", "text": "new text", "clear": true}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify old text is gone
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("new text") && !result.output.contains("existing"),
        "Clear didn't work: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_append() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="text" id="input" value="Hello " />
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-append");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type without clear - should append
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(json!({"selector": "#input", "text": "World"}), ctx.clone())
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify text was appended
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("Hello World") || result.output.contains("Hello  World"),
        "Append didn't work: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_element_not_found() {
    require_chrome!();

    let server = TestServer::start("<html><body><div>No inputs here</div></body></html>").await;
    let (ctx, _manager) = test_context("test-type-not-found");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#nonexistent", "text": "hello"}),
            ctx.clone(),
        )
        .await;

    assert!(!result.success, "Should fail when element not found");
    assert!(
        result.output.to_lowercase().contains("not found")
            || result.output.to_lowercase().contains("no element")
            || result.output.to_lowercase().contains("could not find"),
        "Should mention element not found: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_special_characters() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="text" id="input" />
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-special");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type special characters
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#input", "text": "Test <>&\"' special!@#$%"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("<>&"),
        "Special chars not typed: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_type_password_field() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Type Test</title></head>
        <body>
            <input type="password" id="password" />
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-type-password");

    let nav_tool = BrowserNavigateTool;
    nav_tool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // Type into password field
    let type_tool = BrowserTypeTool;
    let result = type_tool
        .run(
            json!({"selector": "#password", "text": "secret123"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success, "Type failed: {}", result.output);

    // Verify value (password fields still have value attribute)
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('password').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.success);
    assert!(
        result.output.contains("secret123"),
        "Password not typed: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}

// ============================================================================
// TDD: browser_key_press tests
// ============================================================================

#[tokio::test]
async fn test_key_press_escape_fires_keydown_listener() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Key Press Test</title></head>
        <body>
            <div id="result">open</div>
            <script>
              document.addEventListener('keydown', function(e) {
                if (e.key === 'Escape') {
                  document.getElementById('result').textContent = 'closed';
                }
              });
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-key-escape");
    BrowserNavigateTool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let result = BrowserKeyPressTool
        .run(json!({"key": "Escape"}), ctx.clone())
        .await;

    assert!(result.success, "key_press failed: {}", result.output);
    assert!(
        result.output.contains("Escape"),
        "Output should mention key: {}",
        result.output
    );

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output.contains("closed"),
        "Escape keydown listener not fired: {}",
        eval_result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_key_press_ctrl_modifier_fires_capture_listener() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Modifier Key Test</title></head>
        <body>
            <div id="result">none</div>
            <script>
              window.addEventListener('keydown', function(e) {
                if (e.ctrlKey && e.key === 'k') {
                  document.getElementById('result').textContent = 'ctrl+k';
                }
              }, true);
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-key-ctrl-k");
    BrowserNavigateTool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let result = BrowserKeyPressTool
        .run(json!({"key": "k", "modifiers": ["ctrl"]}), ctx.clone())
        .await;

    assert!(result.success, "key_press failed: {}", result.output);

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output.contains("ctrl+k"),
        "Ctrl+K capture listener not fired: {}",
        eval_result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_key_press_arrow_down() {
    require_chrome!();

    let server = TestServer::start(
        r#"<!DOCTYPE html>
        <html>
        <head><title>Arrow Key Test</title></head>
        <body>
            <div id="result">none</div>
            <script>
              document.addEventListener('keydown', function(e) {
                if (e.key === 'ArrowDown') {
                  document.getElementById('result').textContent = 'down';
                }
              });
            </script>
        </body>
        </html>"#,
    )
    .await;

    let (ctx, _manager) = test_context("test-key-arrow-down");
    BrowserNavigateTool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let result = BrowserKeyPressTool
        .run(json!({"key": "ArrowDown"}), ctx.clone())
        .await;

    assert!(result.success, "key_press failed: {}", result.output);

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output.contains("down"),
        "ArrowDown keydown not received: {}",
        eval_result.output
    );

    shutdown_test(_manager, server).await;
}

#[tokio::test]
async fn test_key_press_unknown_key_returns_error() {
    require_chrome!();

    let server = TestServer::start("<html><body></body></html>").await;
    let (ctx, _manager) = test_context("test-key-unknown");
    BrowserNavigateTool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    let result = BrowserKeyPressTool
        .run(json!({"key": "NotAKey"}), ctx.clone())
        .await;

    assert!(!result.success, "Should have failed for unknown key");
    assert!(
        result.output.to_lowercase().contains("unknown"),
        "Should mention unknown key: {}",
        result.output
    );

    shutdown_test(_manager, server).await;
}
