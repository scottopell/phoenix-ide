//! End-to-end tests for browser tools
//!
//! Chrome/Chromium is auto-downloaded via the fetcher if not in PATH.

use super::profile::BrowserProfileTool;
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

    assert!(result.is_success(), "Navigate failed: {}", result.output());
    assert!(
        result.output().contains("done"),
        "Unexpected output: {}",
        result.output()
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
    assert!(
        nav_result.is_success(),
        "Navigate failed: {}",
        nav_result.output()
    );

    // Then eval
    let eval_tool = BrowserEvalTool;

    // Test getting document title
    let result = eval_tool
        .run(json!({"expression": "document.title"}), ctx.clone())
        .await;
    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains("Eval Test"),
        "Title not found: {}",
        result.output()
    );

    // Test getting element attribute
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('data').dataset.value"}),
            ctx.clone(),
        )
        .await;
    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains("42"),
        "Data value not found: {}",
        result.output()
    );

    // Test arithmetic
    let result = eval_tool
        .run(json!({"expression": "2 + 2"}), ctx.clone())
        .await;
    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains('4'),
        "Arithmetic wrong: {}",
        result.output()
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

    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        !result.output().contains("undefined"),
        "Got undefined instead of text: {}",
        result.output()
    );
    assert!(
        result.output().contains("Hello from innerText"),
        "Expected page text, got: {}",
        result.output()
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

    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        !result.output().contains("undefined"),
        "Got undefined instead of HTML: {}",
        result.output()
    );
    assert!(
        result.output().contains("content") || result.output().len() > 10,
        "Expected HTML content, got: {}",
        result.output()
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

    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        !result.output().contains("undefined"),
        "Got undefined instead of JSON: {}",
        result.output()
    );
    assert!(
        result.output().contains("bodyText"),
        "Expected JSON with bodyText key, got: {}",
        result.output()
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
    assert!(
        result.is_success(),
        "innerText eval failed: {}",
        result.output()
    );
    assert!(
        !result.output().contains("undefined"),
        "innerText returned undefined: {}",
        result.output()
    );
    assert!(
        result.output().contains("Article Title"),
        "Missing article title from innerText: {}",
        result.output()
    );

    // Test 2: innerHTML.slice on complex page
    let result = eval_tool
        .run(
            json!({"expression": "document.body.innerHTML.slice(0, 200)"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.is_success(),
        "innerHTML.slice failed: {}",
        result.output()
    );
    assert!(
        !result.output().contains("undefined"),
        "innerHTML.slice returned undefined: {}",
        result.output()
    );

    // Test 3: JSON.stringify of DOM properties
    let result = eval_tool
        .run(
            json!({"expression": "JSON.stringify({title: document.title, bodyLen: document.body.innerText.length, hydrated: document.getElementById('app').dataset.hydrated})"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.is_success(),
        "JSON.stringify failed: {}",
        result.output()
    );
    assert!(
        !result.output().contains("undefined"),
        "JSON.stringify returned undefined: {}",
        result.output()
    );
    assert!(
        result.output().contains("Complex Page"),
        "Missing title in JSON: {}",
        result.output()
    );

    // Test 4: Reading script-set global variable
    let result = eval_tool
        .run(
            json!({"expression": "JSON.stringify(window.__NEXT_DATA__.props.pageProps.data.length)"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.is_success(),
        "Global var eval failed: {}",
        result.output()
    );
    assert!(
        result.output().contains("100"),
        "Expected 100 items, got: {}",
        result.output()
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
    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains("Await Test"),
        "Expected title, got: {}",
        result.output()
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
    assert!(
        result.is_success(),
        "Promise eval failed: {}",
        result.output()
    );
    assert!(
        !result.output().contains("undefined"),
        "Promise returned undefined: {}",
        result.output()
    );
    assert!(
        result.output().contains("ok") && result.output().contains("42"),
        "Expected resolved data, got: {}",
        result.output()
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

    assert!(result.is_success(), "Get logs failed: {}", result.output());
    assert!(
        result.output().contains("test message"),
        "Log message not found: {}",
        result.output()
    );
    assert!(
        result.output().contains("warning message"),
        "Warning not found: {}",
        result.output()
    );
    assert!(
        result.output().contains("error message"),
        "Error not found: {}",
        result.output()
    );

    // Clear logs
    let clear_tool = BrowserClearConsoleLogsTool;
    let result = clear_tool.run(json!({}), ctx.clone()).await;
    assert!(
        result.is_success(),
        "Clear logs failed: {}",
        result.output()
    );
    assert!(
        result.output().contains("Cleared"),
        "Clear message missing: {}",
        result.output()
    );

    // Verify cleared
    let result = logs_tool.run(json!({}), ctx.clone()).await;
    assert!(result.is_success());
    assert!(
        result.output().contains("[]"),
        "Logs not cleared: {}",
        result.output()
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

    assert!(
        result.is_success(),
        "Screenshot failed: {}",
        result.output()
    );

    // The screenshot must flow to the LLM via the typed `images` channel,
    // not via `display_data` (which is UI-only and never threaded to the
    // LLM). Mirrors the read_image.rs regression assertion.
    assert_eq!(
        result.images().len(),
        1,
        "Expected 1 image in images field, got {}",
        result.images().len()
    );
    assert_eq!(result.images()[0].media_type, "image/png");
    assert!(
        result.images()[0].data.starts_with("iVBORw0KGgo"),
        "image data is not a valid PNG (base64 should start with iVBORw0KGgo)"
    );
    assert!(
        result.display_data().is_none(),
        "browser_take_screenshot must not duplicate the image payload into display_data, got {:?}",
        result.display_data()
    );

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

    assert!(result.is_success(), "Resize failed: {}", result.output());

    // Verify via JS
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "window.innerWidth"}), ctx.clone())
        .await;
    assert!(result.is_success());
    // innerWidth should be close to 1024 (may vary slightly due to scrollbars)
    assert!(
        result.output().contains("1024") || result.output().contains("1008"),
        "Width mismatch: {}",
        result.output()
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

    assert!(result.is_success());
    assert!(
        result.output().contains('3'),
        "Counter should be 3, got: {}",
        result.output()
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

    assert!(result.is_success(), "Navigate failed: {}", result.output());

    // Verify we can read the page
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "document.title"}), ctx.clone())
        .await;

    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains("Example Domain"),
        "Wrong title: {}",
        result.output()
    );

    // Verify page content
    let result = eval_tool
        .run(
            json!({"expression": "document.querySelector('h1').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("Example Domain"),
        "Wrong h1: {}",
        result.output()
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
    assert!(result.is_success(), "Eval failed: {}", result.output());
    assert!(
        result.output().contains('2'),
        "Wrong result: {}",
        result.output()
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
    assert!(!result.is_success(), "Should have failed");
    assert!(
        result.output().to_lowercase().contains("error")
            || result.output().to_lowercase().contains("syntaxerror"),
        "Should mention error: {}",
        result.output()
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

    assert!(result.is_success(), "Wait failed: {}", result.output());
    assert!(
        result.output().contains("found") || result.output().contains("visible"),
        "Should indicate element found: {}",
        result.output()
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

    assert!(result.is_success(), "Wait failed: {}", result.output());

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

    assert!(!result.is_success(), "Should have timed out");
    assert!(
        result.output().to_lowercase().contains("timeout")
            || result.output().to_lowercase().contains("not found"),
        "Should mention timeout: {}",
        result.output()
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

    assert!(
        result.is_success(),
        "Wait for visible failed: {}",
        result.output()
    );

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

    assert!(!result.is_success(), "Should fail on invalid selector");
    assert!(
        result.output().to_lowercase().contains("invalid")
            || result.output().to_lowercase().contains("error")
            || result.output().to_lowercase().contains("syntax"),
        "Should mention invalid selector: {}",
        result.output()
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

    assert!(result.is_success(), "Click failed: {}", result.output());

    // Verify the click worked
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("clicked"),
        "Button click didn't work: {}",
        result.output()
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

    assert!(result.is_success(), "Click failed: {}", result.output());

    // Verify URL changed
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(json!({"expression": "window.location.hash"}), ctx.clone())
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("clicked"),
        "Link click didn't navigate: {}",
        result.output()
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

    assert!(!result.is_success(), "Should fail when element not found");
    assert!(
        result.output().to_lowercase().contains("not found")
            || result.output().to_lowercase().contains("no element")
            || result.output().to_lowercase().contains("could not find"),
        "Should mention element not found: {}",
        result.output()
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
    assert!(result.output().contains("false"), "Should start unchecked");

    // Click the checkbox
    let click_tool = BrowserClickTool;
    let result = click_tool
        .run(json!({"selector": "#check"}), ctx.clone())
        .await;
    assert!(result.is_success(), "Click failed: {}", result.output());

    // Verify checked
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('check').checked"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.output().contains("true"),
        "Checkbox should be checked: {}",
        result.output()
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

    assert!(
        result.is_success(),
        "Click with wait failed: {}",
        result.output()
    );

    // Verify click worked
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        result.output().contains("success"),
        "Click didn't work: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("Hello World"),
        "Input value wrong: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('textarea').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("Line 1") && result.output().contains("Line 2"),
        "Textarea value wrong: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify event handler was triggered
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('mirror').textContent"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("React test"),
        "React-style event not triggered: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify old text is gone
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("new text") && !result.output().contains("existing"),
        "Clear didn't work: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify text was appended
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("Hello World") || result.output().contains("Hello  World"),
        "Append didn't work: {}",
        result.output()
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

    assert!(!result.is_success(), "Should fail when element not found");
    assert!(
        result.output().to_lowercase().contains("not found")
            || result.output().to_lowercase().contains("no element")
            || result.output().to_lowercase().contains("could not find"),
        "Should mention element not found: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify value
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('input').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("<>&"),
        "Special chars not typed: {}",
        result.output()
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

    assert!(result.is_success(), "Type failed: {}", result.output());

    // Verify value (password fields still have value attribute)
    let eval_tool = BrowserEvalTool;
    let result = eval_tool
        .run(
            json!({"expression": "document.getElementById('password').value"}),
            ctx.clone(),
        )
        .await;

    assert!(result.is_success());
    assert!(
        result.output().contains("secret123"),
        "Password not typed: {}",
        result.output()
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

    assert!(result.is_success(), "key_press failed: {}", result.output());
    assert!(
        result.output().contains("Escape"),
        "Output should mention key: {}",
        result.output()
    );

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output().contains("closed"),
        "Escape keydown listener not fired: {}",
        eval_result.output()
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

    assert!(result.is_success(), "key_press failed: {}", result.output());

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output().contains("ctrl+k"),
        "Ctrl+K capture listener not fired: {}",
        eval_result.output()
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

    assert!(result.is_success(), "key_press failed: {}", result.output());

    let eval_result = BrowserEvalTool
        .run(
            json!({"expression": "document.getElementById('result').textContent"}),
            ctx.clone(),
        )
        .await;
    assert!(
        eval_result.output().contains("down"),
        "ArrowDown keydown not received: {}",
        eval_result.output()
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

    assert!(!result.is_success(), "Should have failed for unknown key");
    assert!(
        result.output().to_lowercase().contains("unknown"),
        "Should mention unknown key: {}",
        result.output()
    );

    shutdown_test(_manager, server).await;
}

// ============================================================================
// Live View / Screencast (REQ-BT-018)
// ============================================================================

/// Smoke test: navigate, attach a viewer, observe at least one URL event
/// and one frame, detach and verify the broker drops cleanly.
///
/// Validates the broker's lifecycle invariants in an end-to-end shape:
///   1. attach_viewer() lazily starts the screencast on first attach.
///   2. The viewer receives URL events (initial + post-navigation) and
///      frame events (JPEG bytes after base64 decode).
///   3. Dropping the last `Arc<ScreencastBroker>` stops the screencast
///      — verified by re-attaching and getting a new broker instance.
#[tokio::test]
async fn test_screencast_attach_emits_frames_and_url() {
    require_chrome!();
    use crate::tools::browser::screencast::ScreencastEvent;

    let server =
        TestServer::start("<html><body><h1 id='hdr'>screencast probe</h1></body></html>").await;
    let (ctx, manager) = test_context("test-screencast-frames");

    // Trigger session creation by navigating; the screencast doesn't fire
    // without a Page so we can't shortcut this.
    let nav = BrowserNavigateTool
        .run(json!({ "url": server.url() }), ctx.clone())
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    let session_arc = manager
        .get_existing("test-screencast-frames")
        .await
        .expect("session should exist after navigate");

    // First attach: broker is created, screencast starts.
    let (broker_a, mut rx_a, initial_url) = {
        let s = session_arc.read().await;
        s.attach_viewer().await.expect("attach_viewer")
    };
    assert!(
        initial_url
            .as_deref()
            .map(|u| u.starts_with("http://"))
            .unwrap_or(false),
        "initial url should be the navigated page, got {initial_url:?}"
    );
    assert_eq!(
        broker_a.viewer_count(),
        1,
        "first attach should leave 1 viewer"
    );

    // Wait for at least one frame to arrive. With a freshly painted page
    // and everyNthFrame=1 this should land within a few hundred ms.
    let mut got_frame = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !got_frame {
        match tokio::time::timeout(Duration::from_millis(500), rx_a.recv()).await {
            Ok(Ok(ScreencastEvent::Frame { jpeg })) => {
                assert!(!jpeg.is_empty(), "frame should have non-empty JPEG bytes");
                // JPEG SOI marker FFD8 — sanity check on the decode path.
                assert_eq!(
                    jpeg.get(0..2),
                    Some([0xff, 0xd8].as_slice()),
                    "frame should be a JPEG (SOI marker)"
                );
                got_frame = true;
            }
            Ok(Ok(ScreencastEvent::Url(_))) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    assert!(got_frame, "never received a frame within 5s");

    // Second attach: same broker, viewer count goes to 2.
    let (broker_b, mut _rx_b, _) = {
        let s = session_arc.read().await;
        s.attach_viewer().await.expect("attach_viewer #2")
    };
    assert_eq!(
        broker_b.viewer_count(),
        2,
        "second attach should make 2 viewers"
    );
    assert!(
        Arc::ptr_eq(&broker_a, &broker_b),
        "both viewers should share the same broker instance"
    );

    // Drop both viewers — the broker should now be free to drop too.
    let broker_a_ptr = Arc::as_ptr(&broker_a);
    drop(broker_a);
    drop(broker_b);
    drop(rx_a);
    drop(_rx_b);

    // Tiny pause to let Drop run (it spawns a task to fire stopScreencast).
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Third attach: a fresh broker should be allocated since the previous
    // one died with the last viewer.
    let (broker_c, _rx_c, _) = {
        let s = session_arc.read().await;
        s.attach_viewer().await.expect("attach_viewer #3")
    };
    assert!(
        !std::ptr::eq(broker_a_ptr, Arc::as_ptr(&broker_c)),
        "new broker after all viewers dropped"
    );
    drop(broker_c);

    shutdown_test(manager, server).await;
}

// ============================================================================
// browser_profile (REQ-BT-019)
// ============================================================================

/// Non-browser unit test: the tool is registered and its input schema
/// advertises every action. No Chrome required.
#[test]
fn test_browser_profile_registered_and_schema_lists_actions() {
    use super::profile::PROFILE_ACTIONS;

    let registry = crate::tools::ToolRegistry::standard();
    let names: Vec<String> = registry
        .definitions()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert!(
        names.contains(&"browser_profile".to_string()),
        "browser_profile not registered"
    );

    let schema = BrowserProfileTool.input_schema();
    let enum_vals = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum present in schema");
    let action_names: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
    for a in PROFILE_ACTIONS {
        assert!(action_names.contains(a), "input_schema missing action {a}");
    }
    // The Tier-0 method-critical actions must all be present.
    for required in ["help", "metrics", "throttle", "gc_heap", "run_scenario"] {
        assert!(
            action_names.contains(&required),
            "schema missing required action {required}"
        );
    }
}

/// help works without a browser session.
#[tokio::test]
async fn test_browser_profile_help_no_browser() {
    let (ctx, manager) = test_context("test-profile-help");
    let result = BrowserProfileTool
        .run(json!({"action": "help"}), ctx.clone())
        .await;
    assert!(
        result.is_success(),
        "help should succeed: {}",
        result.output()
    );
    assert!(result.output().contains("run_scenario"));
    assert!(
        result.output().contains("RAW per-run"),
        "help must state the raw-samples constraint"
    );
    manager.shutdown_all().await;
}

/// REQ-BT-019.14 / Allium RunScenarioRejectsInlineNavigation: a
/// `navigate` (or `reload`) step inside `steps` resets the cumulative
/// Performance counters mid-bracket. The harness must reject it BEFORE
/// any run — naming the offending step — and produce NO sample set.
/// Non-gated: rejection happens at validation, before any browser use.
#[tokio::test]
async fn test_browser_profile_rejects_inline_navigation() {
    let (ctx, manager) = test_context("test-profile-reject-nav");

    let nav = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 2,
                "steps": [
                    { "kind": "eval", "expression": "1+1" },
                    { "kind": "navigate", "url": "about:blank" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(!nav.is_success(), "navigate in steps must be rejected");
    assert!(
        nav.output().contains("steps[1]") && nav.output().contains("navigate"),
        "error must name the offending step index/kind: {}",
        nav.output()
    );
    assert!(
        nav.display_data().is_none(),
        "rejection must produce NO ScenarioRunResult/display_data: {:?}",
        nav.display_data()
    );

    let rel = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 2,
                "steps": [ { "kind": "reload" } ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(!rel.is_success(), "reload in steps must be rejected");
    assert!(
        rel.output().contains("steps[0]") && rel.output().contains("reload"),
        "error must name the reload step: {}",
        rel.output()
    );

    manager.shutdown_all().await;
}

/// Gated end-to-end test: `metrics` returns numbers, and `run_scenario`
/// with runs>=2 returns a RAW array of length == runs (asserting it is NOT
/// statistically reduced — the REQ-BT-019.5 hard constraint).
#[tokio::test]
async fn test_browser_profile_metrics_and_raw_scenario() {
    require_chrome!();

    let server =
        TestServer::start("<html><body><h1 id='ready'>profile probe</h1></body></html>").await;
    let (ctx, manager) = test_context("test-profile-scenario");
    BrowserNavigateTool
        .run(json!({"url": server.url()}), ctx.clone())
        .await;

    // metrics: must succeed and surface a tracked counter.
    let m = BrowserProfileTool
        .run(json!({"action": "metrics"}), ctx.clone())
        .await;
    assert!(m.is_success(), "metrics should succeed: {}", m.output());
    assert!(
        m.output().contains("JSHeapUsedSize") || m.output().contains("Performance metrics"),
        "metrics output unexpected: {}",
        m.output()
    );

    // run_scenario with runs=3: a trivial deterministic scenario.
    let runs = 3u32;
    let r = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": runs,
                "warmup": 1,
                "steps": [
                    { "kind": "wait_selector", "selector": "#ready", "timeout": "5s" },
                    { "kind": "eval", "expression": "1+1" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(
        r.is_success(),
        "run_scenario should succeed: {}",
        r.output()
    );

    // The raw-samples hard constraint: display_data.raw_samples is an
    // array of EXACTLY `runs` entries — NOT a reduced scalar/object.
    let display = r
        .display_data()
        .expect("run_scenario must attach display_data");
    assert_eq!(
        display["outcome"], "completed",
        "expected completed outcome: {display}"
    );
    let raw = display["raw_samples"]
        .as_array()
        .expect("raw_samples must be an ARRAY (never a reduction)");
    assert_eq!(
        raw.len(),
        runs as usize,
        "raw_samples length must equal runs ({runs}); a reduced result is non-conforming"
    );
    // Each entry is a per-run sample object (has run_index), not an aggregate.
    for (i, s) in raw.iter().enumerate() {
        assert!(
            s.get("run_index").is_some(),
            "sample {i} missing run_index — not a raw per-run sample: {s}"
        );
    }

    // A blocked readiness step must yield ZERO samples and fail.
    let blocked = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 2,
                "steps": [
                    { "kind": "wait_selector", "selector": "#never-exists", "timeout": "500ms" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(!blocked.is_success(), "blocked scenario must fail");
    let bd = blocked
        .display_data()
        .expect("blocked attaches display_data");
    assert_eq!(bd["outcome"], "blocked");
    assert_eq!(
        bd["raw_samples"].as_array().map(Vec::len),
        Some(0),
        "blocked scenario must return ZERO samples"
    );

    shutdown_test(manager, server).await;
}

/// Gated: REQ-BT-019.16/.18 — a run with `reset:"none"` and no throttle
/// must surface a non-empty `methodology_warnings` containing the reset
/// and throttle warnings, ALONGSIDE a `raw_samples` array whose length
/// still equals `runs` (the REQ-BT-019.5 hard constraint is intact —
/// warnings are sibling metadata, never a reduction of the samples).
#[tokio::test]
async fn test_browser_profile_methodology_warnings_intact_raw_samples() {
    require_chrome!();

    let (ctx, manager) = test_context("test-profile-warnings");
    // about:blank is the default page; no server needed.
    BrowserNavigateTool
        .run(json!({"url": "about:blank"}), ctx.clone())
        .await;

    let runs = 2u32;
    let r = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": runs,
                "warmup": 0,
                "reset": "none",
                "steps": [ { "kind": "eval", "expression": "1+1" } ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(
        r.is_success(),
        "run_scenario should succeed: {}",
        r.output()
    );
    let display = r.display_data().expect("must attach display_data");

    let warnings = display["methodology_warnings"]
        .as_array()
        .expect("methodology_warnings must be an array");
    assert!(
        !warnings.is_empty(),
        "warnings must be non-empty for reset=none + no throttle: {display}"
    );
    let joined = warnings
        .iter()
        .filter_map(|w| w.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("reset disabled"),
        "must warn about disabled reset: {joined}"
    );
    assert!(
        joined.contains("no CPU throttle"),
        "must warn about missing throttle: {joined}"
    );

    // Hard constraint still intact: raw_samples length == runs, and the
    // warnings did NOT replace or reduce the sample array.
    let raw = display["raw_samples"]
        .as_array()
        .expect("raw_samples must be an ARRAY (never a reduction)");
    assert_eq!(
        raw.len(),
        runs as usize,
        "raw_samples length must still equal runs ({runs}) with warnings present"
    );
    // Each sample carries the tri-state discriminators (REQ-BT-019.13/.15).
    for (i, s) in raw.iter().enumerate() {
        assert!(s.get("run_index").is_some(), "sample {i} missing run_index");
        assert!(
            s.get("react_status").is_some(),
            "sample {i} missing react_status: {s}"
        );
        assert!(s.get("gc_ran").is_some(), "sample {i} missing gc_ran: {s}");
        // gc_per_run defaulted true → js_heap_used should be a number here.
        assert!(
            s.get("js_heap_used").is_some(),
            "sample {i} missing js_heap_used key (must be present even if null): {s}"
        );
    }

    manager.shutdown_all().await;
}

/// Gated (chrome + network): the React `measured` path end-to-end.
///
/// This is the marquee REQ-BT-019.4/.13 signal and was previously only
/// exercised against about:blank (status `absent`). Here a REAL React
/// **profiling** build (records `actualDuration` unconditionally) is
/// driven through the auto-injected `__phoenix` commit hook and the
/// page-anchored `__perfRead` → `PerfReading` Rust mapping, asserting
/// `react_status == "measured"` with a non-null `react_actual_ms` —
/// proving the happy path the rest of the suite never reaches.
#[tokio::test]
async fn test_browser_profile_react_measured_path() {
    require_chrome!();
    require_network!(); // pulls React UMD from unpkg

    // Profiling build => fibers carry numeric `actualDuration` without
    // any DevTools backend toggle (our hook does not implement that).
    // Script order matters: react -> scheduler -> react-dom.profiling.
    let html = r#"<!doctype html><html><head><meta charset="utf-8"></head>
<body><div id="root"></div>
<script crossorigin src="https://unpkg.com/react@18.3.1/umd/react.production.min.js"></script>
<script crossorigin src="https://unpkg.com/scheduler@0.23.2/umd/scheduler.production.min.js"></script>
<script crossorigin src="https://unpkg.com/react-dom@18.3.1/umd/react-dom.profiling.min.js"></script>
<script>
window.__renders = 0;
var e = React.createElement;
function App() {
  var st = React.useState(0); var n = st[0], set = st[1];
  window.__renders++;
  React.useEffect(function () { window.__ready = true; }, []);
  return e('div', null,
    e('div', { id: 'ready' }, 'ready'),
    e('button', { id: 'inc', onClick: function () { set(function (x) { return x + 1; }); } }, 'n=' + n));
}
ReactDOM.createRoot(document.getElementById('root')).render(e(App));
</script></body></html>"#;

    let server = TestServer::start(html).await;
    let (ctx, manager) = test_context("test-profile-react-measured");

    let nav = BrowserNavigateTool
        .run(
            json!({ "url": server.url(), "timeout": "30s" }),
            ctx.clone(),
        )
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    // Mount confirmed once React renders the #ready node.
    let waited = BrowserWaitForSelectorTool
        .run(
            json!({ "selector": "#ready", "timeout": "30s" }),
            ctx.clone(),
        )
        .await;
    assert!(
        waited.is_success(),
        "React did not mount (unpkg/profiling build issue?): {}",
        waited.output()
    );

    // reset:"none" keeps the mounted app (one unpkg fetch). Readiness
    // step FIRST (`window.__ready` set in the App's useEffect) → the
    // page-anchored window opens AFTER mount/settle; the click then
    // forces an in-window update-phase commit whose fiber carries
    // actualDuration.
    let r = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 1,
                "warmup": 0,
                "reset": "none",
                "gc_per_run": false,
                "steps": [
                    { "kind": "wait_eval", "expression": "window.__ready === true", "timeout": "20s" },
                    { "kind": "click", "selector": "#inc" },
                    { "kind": "wait_eval", "expression": "window.__renders >= 2", "timeout": "20s" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(r.is_success(), "run_scenario failed: {}", r.output());

    let display = r.display_data().expect("display_data present");
    let sample = &display["raw_samples"][0];

    assert_eq!(
        sample["react_status"].as_str(),
        Some("measured"),
        "real React profiling build must read as `measured`, not absent/no_profiling_build: {sample}"
    );
    assert!(
        sample["react_actual_ms"].is_number(),
        "measured path must carry a non-null react_actual_ms: {sample}"
    );
    assert!(
        sample["react_commits"].as_u64().is_some_and(|c| c >= 1),
        "at least one commit must be recorded for a re-rendering app: {sample}"
    );

    shutdown_test(manager, server).await;
}

/// Extract the first `prefix...suffix` path substring from a tool's
/// output. Avoids pulling the `regex` crate into test code (the path
/// format is a fixed prefix + uuid + fixed extension). Uses `split_once`
/// rather than byte-index slicing so it cannot panic on multi-byte UTF-8.
fn extract_tmp_path(haystack: &str, prefix: &str, suffix: &str) -> Option<String> {
    let (_, after_prefix) = haystack.split_once(prefix)?;
    let (uuid, _) = after_prefix.split_once(suffix)?;
    Some(format!("{prefix}{uuid}{suffix}"))
}

/// Gated (chrome only): real `Profiler.Profile` round-trip.
///
/// The CPU summarizer (`summarize_cpu_profile`/`cpu_self_times`) was only
/// ever unit-tested against hand-built JSON. This drives a live Chrome
/// capture: `cpu_start` → a real ~400ms busy loop → `cpu_stop`, proving
/// the chromiumoxide `Profile` struct deserialises and summarises. It
/// then re-summarises the written file via `cpu_summary` to prove the
/// serde round-trip through disk is independent and lossless.
#[tokio::test]
async fn test_browser_profile_cpu_start_stop_real_profile_serde() {
    require_chrome!();

    let (ctx, manager) = test_context("test-profile-cpu-real");
    let nav = BrowserNavigateTool
        .run(json!({ "url": "about:blank" }), ctx.clone())
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    let start = BrowserProfileTool
        .run(json!({ "action": "cpu_start" }), ctx.clone())
        .await;
    assert!(
        start.is_success(),
        "cpu_start should succeed: {}",
        start.output()
    );

    // ~400ms busy loop so the sampler collects real frames/timeDeltas.
    let busy = BrowserEvalTool
        .run(
            json!({
                "expression": "var s=0; var t=Date.now(); while(Date.now()-t<400){ s+=Math.sqrt(s+1); } s"
            }),
            ctx.clone(),
        )
        .await;
    assert!(
        busy.is_success(),
        "busy-loop eval failed: {}",
        busy.output()
    );

    let stop = BrowserProfileTool
        .run(json!({ "action": "cpu_stop" }), ctx.clone())
        .await;
    // Primary assertion: real Profile deserialisation + summary did NOT error.
    assert!(
        stop.is_success(),
        "cpu_stop should succeed: {}",
        stop.output()
    );
    assert!(
        stop.output().contains("Sampled wall time:"),
        "cpu_stop summary must report sampled wall time (real timeDeltas path): {}",
        stop.output()
    );
    assert!(
        stop.output().contains("by SELF time"),
        "cpu_stop summary must rank functions by SELF time: {}",
        stop.output()
    );
    let path = extract_tmp_path(stop.output(), "/tmp/phoenix-cpu-profile-", ".json")
        .unwrap_or_else(|| panic!("cpu_stop must report a profile path: {}", stop.output()));

    // Independent round-trip: cpu_summary reads the file back through
    // `serde_json::from_str::<CpuProfile>` and re-summarises it.
    let summ = BrowserProfileTool
        .run(
            json!({ "action": "cpu_summary", "path": path }),
            ctx.clone(),
        )
        .await;
    assert!(
        summ.is_success(),
        "cpu_summary must NOT error on a real cpu_stop file (proves serde round-trip): {}",
        summ.output()
    );
    assert!(
        !summ.output().trim().is_empty(),
        "cpu_summary output must be non-empty: {}",
        summ.output()
    );
    // A 400ms busy loop normally yields samples → "by SELF time"; tolerate
    // a very fast machine producing too few samples (the "carries no
    // samples" / "empty" fallbacks) — it must still parse without error.
    assert!(
        summ.output().contains("by SELF time")
            || summ.output().contains("Sampled wall time")
            || summ.output().contains("carries no samples")
            || summ.output().contains("CPU profile is empty"),
        "cpu_summary must produce a parseable summary (not a serde error): {}",
        summ.output()
    );

    manager.shutdown_all().await;
}

/// Gated (chrome only): real Tracing long-task extraction.
///
/// The `Tracing.dataCollected` listener, the `tracingComplete`
/// notify-wait (with the `Notified::enable()` lost-wakeup fix), and the
/// `dur > 50_000us` long-task parse have never run against a live trace.
/// A single >50ms blocking task is generated; the load-bearing assertion
/// is that `trace_stop` completes without timing out and reports a
/// long-task count (trace category timing varies by Chrome build, so the
/// count itself is not hard-asserted).
#[tokio::test]
async fn test_browser_profile_trace_stop_long_task_real() {
    require_chrome!();

    let (ctx, manager) = test_context("test-profile-trace-real");
    let nav = BrowserNavigateTool
        .run(json!({ "url": "about:blank" }), ctx.clone())
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    let start = BrowserProfileTool
        .run(json!({ "action": "trace_start" }), ctx.clone())
        .await;
    assert!(
        start.is_success(),
        "trace_start should succeed: {}",
        start.output()
    );

    // One blocking task well over the 50ms long-task threshold.
    let block = BrowserEvalTool
        .run(
            json!({ "expression": "var t=Date.now(); while(Date.now()-t<120){}; 'done'" }),
            ctx.clone(),
        )
        .await;
    assert!(
        block.is_success(),
        "blocking eval failed: {}",
        block.output()
    );

    let stop = BrowserProfileTool
        .run(json!({ "action": "trace_stop" }), ctx.clone())
        .await;
    // Load-bearing: trace_stop completed (drain path + enable() race fix
    // worked end-to-end; a timeout would still succeed but append a note).
    assert!(
        stop.is_success(),
        "trace_stop should succeed: {}",
        stop.output()
    );
    assert!(
        stop.output().contains("Trace saved to"),
        "trace_stop must report a saved trace: {}",
        stop.output()
    );
    assert!(
        extract_tmp_path(stop.output(), "/tmp/phoenix-trace-", ".json").is_some(),
        "trace_stop must report a /tmp/phoenix-trace- path: {}",
        stop.output()
    );
    // The extraction ran and reported a count: "Long tasks (>50ms): <n>".
    let marker = "Long tasks (>50ms):";
    let (_, after) = stop.output().split_once(marker).unwrap_or_else(|| {
        panic!(
            "trace_stop must report a long-task count: {}",
            stop.output()
        )
    });
    let after = after.trim_start();
    assert!(
        after.chars().next().is_some_and(|c| c.is_ascii_digit()),
        "long-task marker must be followed by a numeric count: {}",
        stop.output()
    );

    manager.shutdown_all().await;
}

/// Gated (chrome only): real heap snapshot streaming + diff.
///
/// The `EventAddHeapSnapshotChunk` collector, the 2s post-completion
/// drain, and `parse_heap_stats` (flat-node parse, "Detached" string
/// match, `self_size` sum) have zero real coverage. Two snapshots are
/// taken around extra allocations; the load-bearing assertion is that
/// the diff call parsed BOTH real Chrome snapshots and produced a
/// node-count / detached / `self_size` delta without error.
#[tokio::test]
async fn test_browser_profile_heap_snapshot_streaming_and_diff() {
    require_chrome!();

    let (ctx, manager) = test_context("test-profile-heap-real");
    let nav = BrowserNavigateTool
        .run(json!({ "url": "about:blank" }), ctx.clone())
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    // Retain detached DOM nodes (held by a JS array) + a big array.
    let alloc1 = BrowserEvalTool
        .run(
            json!({
                "expression": "window.__leak=[]; for(var i=0;i<50;i++){var d=document.createElement('div'); d.innerHTML='x'.repeat(100); window.__leak.push(d);} window.__leak.push(new Array(100000).fill(7)); 'ok'"
            }),
            ctx.clone(),
        )
        .await;
    assert!(
        alloc1.is_success(),
        "alloc1 eval failed: {}",
        alloc1.output()
    );

    let snap1 = BrowserProfileTool
        .run(json!({ "action": "heap_snapshot" }), ctx.clone())
        .await;
    assert!(
        snap1.is_success(),
        "first heap_snapshot should succeed (chunk streaming): {}",
        snap1.output()
    );
    let baseline = extract_tmp_path(snap1.output(), "/tmp/phoenix-heap-", ".heapsnapshot")
        .unwrap_or_else(|| {
            panic!(
                "first heap_snapshot must report a snapshot path: {}",
                snap1.output()
            )
        });
    assert!(
        snap1.output().contains("Heap snapshot saved to"),
        "no-baseline heap_snapshot must report the saved path: {}",
        snap1.output()
    );

    // Allocate more so the diff has a positive delta to report.
    let alloc2 = BrowserEvalTool
        .run(
            json!({ "expression": "window.__leak.push(new Array(200000).fill(9)); 'ok'" }),
            ctx.clone(),
        )
        .await;
    assert!(
        alloc2.is_success(),
        "alloc2 eval failed: {}",
        alloc2.output()
    );

    let snap2 = BrowserProfileTool
        .run(
            json!({ "action": "heap_snapshot", "baseline": baseline }),
            ctx.clone(),
        )
        .await;
    // Load-bearing: the second call parsed BOTH real snapshots and
    // produced a diff without error.
    assert!(
        snap2.is_success(),
        "heap_snapshot diff must NOT error (proves chunk streaming + parse_heap_stats on real snapshots): {}",
        snap2.output()
    );
    assert!(
        snap2.output().contains("Heap diff (post"),
        "diff output must be a heap diff: {}",
        snap2.output()
    );
    assert!(
        snap2.output().contains("node count:"),
        "diff must report a node-count line: {}",
        snap2.output()
    );
    assert!(
        snap2.output().contains("detached DOM nodes:"),
        "diff must report a detached-DOM-node count: {}",
        snap2.output()
    );
    assert!(
        snap2.output().contains("self_size:"),
        "diff must report a self_size line: {}",
        snap2.output()
    );
    let display = snap2
        .display_data()
        .expect("heap diff must attach display_data");
    assert!(
        display.get("node_count_delta").is_some(),
        "display_data must carry node_count_delta: {display}"
    );
    assert!(
        display.get("self_size_delta_bytes").is_some(),
        "display_data must carry self_size_delta_bytes: {display}"
    );
    assert!(
        display
            .get("detached_dom_nodes")
            .and_then(|d| d.get("post"))
            .is_some(),
        "display_data must carry detached_dom_nodes.post: {display}"
    );

    manager.shutdown_all().await;
}

/// Gated (chrome + network): the React `no_profiling_build` tri-state
/// branch.
///
/// Clones `test_browser_profile_react_measured_path` but loads the
/// **production** react-dom build. React is present and the auto-injected
/// commit hook still fires (commit COUNT observed), but a production
/// build exposes no `actualDuration` — so the status must read
/// `no_profiling_build` and `react_actual_ms` must be null, NOT a silent
/// zero. This is the footgun that fix #1 closed; only `measured` and
/// `absent` were previously proven against real Chrome.
#[tokio::test]
async fn test_browser_profile_react_no_profiling_build_path() {
    require_chrome!();
    require_network!(); // pulls React UMD from unpkg

    // Production react-dom => fibers do NOT carry actualDuration even
    // though the commit hook fires. Script order: react -> scheduler ->
    // react-dom.production.
    let html = r#"<!doctype html><html><head><meta charset="utf-8"></head>
<body><div id="root"></div>
<script crossorigin src="https://unpkg.com/react@18.3.1/umd/react.production.min.js"></script>
<script crossorigin src="https://unpkg.com/scheduler@0.23.2/umd/scheduler.production.min.js"></script>
<script crossorigin src="https://unpkg.com/react-dom@18.3.1/umd/react-dom.production.min.js"></script>
<script>
window.__renders = 0;
var e = React.createElement;
function App() {
  var st = React.useState(0); var n = st[0], set = st[1];
  window.__renders++;
  React.useEffect(function () { window.__ready = true; }, []);
  return e('div', null,
    e('div', { id: 'ready' }, 'ready'),
    e('button', { id: 'inc', onClick: function () { set(function (x) { return x + 1; }); } }, 'n=' + n));
}
ReactDOM.createRoot(document.getElementById('root')).render(e(App));
</script></body></html>"#;

    let server = TestServer::start(html).await;
    let (ctx, manager) = test_context("test-profile-react-noprof");

    let nav = BrowserNavigateTool
        .run(
            json!({ "url": server.url(), "timeout": "30s" }),
            ctx.clone(),
        )
        .await;
    assert!(nav.is_success(), "navigate failed: {}", nav.output());

    let waited = BrowserWaitForSelectorTool
        .run(
            json!({ "selector": "#ready", "timeout": "30s" }),
            ctx.clone(),
        )
        .await;
    assert!(
        waited.is_success(),
        "React did not mount (unpkg/production build issue?): {}",
        waited.output()
    );

    // Readiness step FIRST (`window.__ready` set in the App's
    // useEffect) → the page-anchored window opens after mount/settle;
    // the in-window click commit count is still observed even though a
    // production build never exposes actualDuration.
    let r = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 1,
                "warmup": 0,
                "reset": "none",
                "gc_per_run": false,
                "steps": [
                    { "kind": "wait_eval", "expression": "window.__ready === true", "timeout": "20s" },
                    { "kind": "click", "selector": "#inc" },
                    { "kind": "wait_eval", "expression": "window.__renders >= 2", "timeout": "20s" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(r.is_success(), "run_scenario failed: {}", r.output());

    let display = r.display_data().expect("display_data present");
    let sample = &display["raw_samples"][0];

    assert_eq!(
        sample["react_status"].as_str(),
        Some("no_profiling_build"),
        "production React build (commit hook fires but no actualDuration) must read as `no_profiling_build`: {sample}"
    );
    assert!(
        sample["react_actual_ms"].is_null(),
        "no_profiling_build must NOT carry a numeric react_actual_ms (the whole point of the tri-state — no silent zero): {sample}"
    );
    assert!(
        sample["react_commits"].as_u64().is_some_and(|c| c >= 1),
        "commit COUNT must still be observed on a production build: {sample}"
    );

    shutdown_test(manager, server).await;
}

/// F3/F5 FIX GUARD (gated: chrome only, network-free).
///
/// Was `test_browser_profile_f3_mount_in_window_characterization` — a
/// characterization that pinned the BUG. Now flipped to guard the
/// PAGE-ANCHORED measurement window fix (REQ-BT-019.20): one test that
/// proves BOTH F3 (pre-readiness mount/settle excluded) and F5
/// (real in-window work captured) are fixed.
///
/// The page burns ~180ms of synchronous JS and builds an 8000-node
/// subtree BEFORE setting `window.__ready` (the async-mount shape). The
/// scenario's FIRST step is `wait_eval window.__ready` — the readiness
/// step — so the page-anchored window opens AFTER it: that pre-readiness
/// burn is structurally outside the window (F3 fixed). A post-readiness
/// ~70ms blocking task is then run as a positive control: it lands in
/// the window and must be captured (F5 fixed — `script_ms` reflects real
/// in-window longtask cost, where the old host-bracketed ScriptDuration
/// delta collapsed to ~0).
#[tokio::test]
async fn test_browser_profile_window_excludes_pre_readiness_work() {
    require_chrome!();

    // The in-window positive control is the CLICK HANDLER's ~70ms
    // blocking loop, not a CDP `eval` step: a Runtime.evaluate task is
    // NOT observed by the page's `longtask` PerformanceObserver, but a
    // page-driven event handler IS. The pre-readiness ~180ms burn runs
    // in a post-load `setTimeout` BEFORE `__ready` (untimed setup).
    let html = r#"<!doctype html><html><body><button id="ping">ping</button>
<script>
window.__ready = false; window.__pinged = false;
document.getElementById('ping').addEventListener('click', function () {
  var t = Date.now(); while (Date.now() - t < 70) {}   // ~70ms in-window longtask
  window.__pinged = true;
});
// Heavy work scheduled AFTER load, BEFORE __ready — the async-mount
// shape. ~180ms busy + 8000 DOM nodes. With a page-anchored window
// that opens after the readiness step this is UNTIMED setup.
setTimeout(function () {
  var t = Date.now(); while (Date.now() - t < 180) {}
  var box = document.createElement('div');
  for (var i = 0; i < 8000; i++) { var d = document.createElement('div'); d.textContent = i; box.appendChild(d); }
  document.body.appendChild(box);
  window.__ready = true;
}, 0);
</script></body></html>"#;

    let server = TestServer::start(html).await;
    let (ctx, manager) = test_context("test-profile-window-anchor");

    BrowserNavigateTool
        .run(
            json!({ "url": server.url(), "timeout": "20s" }),
            ctx.clone(),
        )
        .await;

    // First step is the readiness wait → the page-anchored window opens
    // AFTER `window.__ready` (the ~180ms pre-readiness burn is untimed
    // setup). The post-readiness click fires the page's ~70ms blocking
    // handler — a page-driven task the `longtask` observer DOES see —
    // as the positive control: it must be captured in `script_ms`.
    let r = BrowserProfileTool
        .run(
            json!({
                "action": "run_scenario",
                "runs": 1,
                "warmup": 0,
                "gc_per_run": false,
                "steps": [
                    { "kind": "wait_eval", "expression": "window.__ready === true", "timeout": "20s" },
                    { "kind": "click", "selector": "#ping" },
                    { "kind": "wait_eval", "expression": "window.__pinged === true", "timeout": "10s" }
                ]
            }),
            ctx.clone(),
        )
        .await;
    assert!(r.is_success(), "run_scenario failed: {}", r.output());

    let display = r.display_data().expect("display_data present");
    let sample = &display["raw_samples"][0];
    let script_ms = sample["script_ms"]
        .as_f64()
        .expect("script_ms is a number (page-anchored longtask sum)");
    let long_tasks = sample["long_tasks"]
        .as_u64()
        .expect("long_tasks is a number");
    let wall_ms = sample["wall_ms"]
        .as_f64()
        .expect("wall_ms is a number (window performance.now span)");

    // F3 FIXED — the ~180ms pre-readiness burn is EXCLUDED: it ran
    // before `window.__ready`, i.e. before the readiness step that
    // opens the window. F5 FIXED — the ~70ms post-readiness blocking
    // task IS captured (the old host-bracketed ScriptDuration delta
    // read ~0 here). So `script_ms` must include the in-window ~70ms
    // longtask but exclude the pre-window 180ms.
    assert!(
        (50.0..160.0).contains(&script_ms),
        "page-anchored window: script_ms must capture the in-window \
         ~70ms longtask (>= 50ms, F5 fixed) but exclude the \
         pre-readiness ~180ms burn (< 160ms, F3 fixed); got \
         {script_ms}ms. sample: {sample}"
    );
    assert!(
        long_tasks >= 1,
        "the in-window ~70ms blocking task must register at least one \
         longtask entry; got {long_tasks}. sample: {sample}"
    );
    assert!(
        wall_ms > 0.0,
        "wall_ms must be a positive performance.now() span; got \
         {wall_ms}. sample: {sample}"
    );

    shutdown_test(manager, server).await;
}
