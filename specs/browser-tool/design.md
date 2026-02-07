# Browser Automation Tool - Design

## Architecture Overview

Native Rust tool using Chrome DevTools Protocol (CDP) for browser automation:

1. **Chrome Process Manager** - Lifecycle management of headless Chrome with idle timeout
2. **CDP Client Layer** - WebSocket connection to Chrome debugging port
3. **Tool Interface** - JSON-based tool definitions matching Phoenix tool schema
4. **Session Manager** - Per-conversation browser isolation

## Technology Stack

- **Language**: Rust (consistent with Phoenix IDE)
- **CDP Library**: `chromiumoxide` - async-first, code-generated CDP client
- **Chrome**: Headless Chrome with remote debugging enabled
- **Protocol**: Chrome DevTools Protocol via WebSocket (handled by chromiumoxide)

### Why chromiumoxide?

| Factor | chromiumoxide | headless_chrome | fantoccini |
|--------|---------------|-----------------|-------------|
| Async runtime | Tokio-native ✓ | Mixed sync/async | Tokio |
| Protocol coverage | Full CDP (code-gen) | Partial | WebDriver only |
| Process control | Full (Child access) | Full | Requires chromedriver |
| Performance | High-concurrency design | Heavier | WebDriver overhead |

chromiumoxide is the right layer: it handles CDP protocol complexity while giving us full control over the Chrome process lifecycle.

---

## Session Model (REQ-BT-010, REQ-BT-011)

### Implicit Session Design

The browser tool follows an implicit session model for "pit of success" ergonomics:

- **No session IDs**: Agents never pass context or session identifiers
- **Auto-initialization**: First browser tool call in a conversation starts Chrome
- **State persistence**: Browser state (cookies, cache, page) persists across tool calls
- **Conversation isolation**: Each conversation gets its own browser instance
- **Auto-cleanup**: Browser closes after 30-minute idle timeout or conversation end

### Lifecycle Flow

```
Conversation starts
    |
    v
First browser_* tool call
    |
    v
Chrome process launched (PID stored)
    |
    v
Subsequent tool calls reuse browser (updates last_activity timestamp)
    |
    v
[30 min idle OR conversation deleted OR server shutdown]
    |
    v
Chrome process terminated (SIGKILL to process group)
```

### Chrome Process Cleanup Strategy

Chrome processes are heavyweight (~100MB+ RAM each), so robust cleanup is critical:

**1. Idle Timeout (Primary)**
- Global `BrowserSessionManager` holds `HashMap<ConversationId, BrowserSession>`
- Each session tracks `last_activity: Instant` and `chrome_pid: u32`
- Background task runs every 60 seconds, kills sessions idle > 30 minutes
- Uses process group kill (`killpg`) to ensure all Chrome child processes die

**2. Explicit Cleanup Hooks**
- `delete_conversation` API calls `BrowserSessionManager::kill_session(conv_id)`
- Server shutdown triggers `Drop` impl on `BrowserSessionManager` which kills all sessions
- Each `BrowserSession` has `Drop` impl that kills its Chrome process group

**3. Orphan Recovery (Startup)**
- On Phoenix startup, scan for orphaned Chrome processes with `--remote-debugging-port`
- Kill any Chrome processes spawned by previous Phoenix instances (matching user/cwd)
- Prevents resource leaks across Phoenix crashes/restarts

**4. Emergency Fallback**
- Store Chrome PIDs in a temp file (`/tmp/phoenix-chrome-pids.txt`)
- If process group kill fails, fall back to direct PID kill
- Log warnings for any cleanup failures

### Chrome Launch Configuration

- Headless mode enabled (`--headless=new`)
- No sandbox (`--no-sandbox` for containerized environments)
- WebSocket debugging enabled (`--remote-debugging-port=0` for auto-assign)
- Disable GPU (`--disable-gpu`)
- Default viewport: 1280x720 (16:9)
- User data dir: `/tmp/phoenix-chrome-{conversation_id}/`
- Download directory: `/tmp/phoenix-downloads/`
- Screenshot directory: `/tmp/phoenix-screenshots/`

---

## Stateless Tool Pattern (REQ-BT-012)

All browser tools follow the stateless pattern - no per-conversation state:

```rust
/// Browser navigation tool - completely stateless
pub struct BrowserNavigateTool;

impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str { "browser_navigate" }
    
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: NavigateInput = serde_json::from_value(input)?;
        let timeout = input.timeout.unwrap_or(Duration::from_secs(15));
        
        // Get browser session via context (correct conversation guaranteed)
        let mut browser = ctx.browser().await?;
        
        // Use chromiumoxide Page API
        let result = tokio::time::timeout(
            timeout,
            browser.page_mut().goto(&input.url)
        ).await;
        
        match result {
            Ok(Ok(_)) => ToolOutput::success("done"),
            Ok(Err(e)) => ToolOutput::error(format!("Navigation failed: {e}")),
            Err(_) => ToolOutput::error(format!("Timeout after {timeout:?}")),
        }
    }
}
```

---

## Tool Definitions

### browser_navigate (REQ-BT-001)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "url": { "type": "string", "description": "URL to navigate to" },
    "timeout": { "type": "string", "description": "Timeout duration (default: 15s)" }
  },
  "required": ["url"]
}
```

**Implementation:**
- Uses CDP `Page.navigate`
- Waits for `Page.loadEventFired` and body ready
- Detects download triggers (ERR_ABORTED with download event)
- Returns clear error messages for failures

**Output:**
- Success: `"done"` (or download info if triggered)
- Error: Descriptive message with failure type

---

### browser_eval (REQ-BT-002)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "expression": { "type": "string", "description": "JavaScript to evaluate" },
    "timeout": { "type": "string", "description": "Timeout duration (default: 15s)" },
    "await": { "type": "boolean", "description": "Await promises (default: true)" }
  },
  "required": ["expression"]
}
```

**Implementation:**
- Uses CDP `Runtime.evaluate`
- Configurable Promise awaiting
- JSON serialization of return values
- Large output (>1KB) written to file

**Output:**
- Success: `<javascript_result>JSON_VALUE</javascript_result>` (or file path for large output)
- Error: Exception message and context

**Usage Examples:**
```javascript
// Click a button
document.querySelector('#submit-btn').click()

// Get text content
document.querySelector('.message').textContent

// Fill a form field
document.querySelector('#email').value = 'test@example.com'

// Wait for element
new Promise(r => {
  const check = () => document.querySelector('.loaded') ? r('ready') : setTimeout(check, 100);
  check();
})

// Scroll to element
document.querySelector('#section').scrollIntoView()
```

---

### browser_take_screenshot (REQ-BT-003)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "selector": { "type": "string", "description": "CSS selector (optional, for element screenshot)" },
    "timeout": { "type": "string", "description": "Timeout duration (default: 15s)" }
  }
}
```

**Implementation:**
- Uses CDP `Page.captureScreenshot`
- Element screenshots via `DOM.getBoxModel` + clip
- PNG format
- Auto-resize for LLM limits (configurable max dimension)
- Saves to `/tmp/phoenix-screenshots/{uuid}.png`

**Output:**
- Text description: `"Screenshot taken (saved as /path/to/file.png)"`
- Base64 image data for LLM vision

---

### browser_recent_console_logs (REQ-BT-004)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "limit": { "type": "integer", "description": "Max entries to return (default: 100)" }
  }
}
```

**Implementation:**
- Listens to CDP `Runtime.consoleAPICalled` events
- Stores last N entries in ring buffer
- Large output (>1KB) written to file

**Output:**
- JSON array of log entries with type, args, timestamp
- File path for large output

---

### browser_clear_console_logs (REQ-BT-004)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {}
}
```

**Implementation:**
- Clears internal log buffer

**Output:**
- `"Cleared N console log entries."`

---

### browser_resize (REQ-BT-005)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "width": { "type": "integer", "description": "Viewport width in pixels" },
    "height": { "type": "integer", "description": "Viewport height in pixels" },
    "timeout": { "type": "string", "description": "Timeout duration (default: 15s)" }
  },
  "required": ["width", "height"]
}
```

**Implementation:**
- Uses CDP `Emulation.setDeviceMetricsOverride`

**Output:**
- `"done"`

---

### read_image (REQ-BT-006)

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "Path to image file" },
    "timeout": { "type": "string", "description": "Timeout duration (default: 15s)" }
  },
  "required": ["path"]
}
```

**Implementation:**
- Reads file from disk
- Validates image format (PNG, JPEG, GIF, WebP)
- Auto-resize for LLM limits
- HEIC conversion if needed

**Output:**
- Text description: `"Image from /path (type: image/png)"`
- Base64 image data for LLM vision

---

## CDP Domain Usage

| Requirement | CDP Domains |
|-------------|-------------|
| REQ-BT-001: Navigate | Page |
| REQ-BT-002: JavaScript | Runtime |
| REQ-BT-003: Screenshots | Page, DOM |
| REQ-BT-004: Console | Runtime |
| REQ-BT-005: Viewport | Emulation |
| REQ-BT-010: Session | Browser |
| REQ-BT-020: Service Workers | ServiceWorker, Runtime |
| REQ-BT-021: Network Source | Network |
| REQ-BT-022: Offline Mode | Network |
| REQ-BT-023: Multi-Context | Runtime, Log |

---

## Error Handling

All tools provide clear error messages:

| Error Type | Example Message |
|------------|----------------|
| Navigation failure | `"net::ERR_CONNECTION_REFUSED: Failed to connect to localhost:8000"` |
| Timeout | `"Timeout after 15s waiting for page load"` |
| JS execution error | `"ReferenceError: foo is not defined at line 1"` |
| Element not found | `"Selector '#missing' not found"` |
| File not found | `"Image file not found: /path/to/file.png"` |
| Invalid input | `"Invalid dimensions: width and height must be positive"` |

---

## Post-MVP: Service Worker Tools (REQ-BT-020 - REQ-BT-023)

These tools extend the browser tool for PWA testing:

### browser_get_service_workers (REQ-BT-020)

Uses `ServiceWorker.enable` and `Runtime.evaluate` to query:
- Registration status
- Active state
- Controlling state

### browser_get_network_sources (REQ-BT-021)

Uses `Network.enable` to track:
- `response.fromServiceWorker` flag
- `response.fromDiskCache` flag
- Request/response categorization

### browser_set_offline (REQ-BT-022)

Uses `Network.emulateNetworkConditions` with offline flag.

### Multi-Context Logs (REQ-BT-023)

Extends console capture to include:
- `Log.entryAdded` for service worker logs
- Context tagging (page vs service worker)

---

## Output Format Conventions

Structured text suitable for LLM parsing:

- Success outputs are concise and actionable
- Errors include context and suggestions when possible
- Large outputs redirect to files with `cat` instructions
- Image data returned as base64 with media type for vision models

---

## What This Design Explicitly Excludes

- **Cross-browser support**: Chrome only (Chromium/Chrome required)
- **Complex wait strategies**: JS-based waiting via `browser_eval`
- **Full CDP exposure**: Only essential domains wrapped
- **GUI/interactive mode**: Headless only
- **Test framework integration**: Tool-level, not framework-level
- **Multi-tab support**: Single tab per conversation
- **Cookie/storage manipulation tools**: Achievable via `browser_eval`

---

## Implementation Notes

### File Locations

```
src/tools/
├── browser.rs                    # Module entry, exports tools and session manager
└── browser/
    ├── session.rs                # BrowserSession + BrowserSessionManager
    └── tools.rs                  # Tool implementations (navigate, eval, screenshot, etc.)
```

Note: No `chrome.rs` or `cdp.rs` needed - chromiumoxide handles process spawning and CDP protocol internally.

### ToolContext Integration (REQ-BT-012)

All tools receive `ToolContext` which provides access to browser sessions:

```rust
/// All context needed for a tool invocation.
/// Created fresh for each tool call with validated conversation context.
#[derive(Clone)]
pub struct ToolContext {
    /// Cancellation signal for long-running operations
    pub cancel: CancellationToken,
    
    /// The conversation this tool is executing within
    pub conversation_id: String,
    
    /// Working directory for file operations
    pub working_dir: PathBuf,
    
    /// Browser session manager (private - access via method only)
    browser_sessions: Arc<BrowserSessionManager>,
}

impl ToolContext {
    /// Get or create browser session for this conversation.
    /// 
    /// - Lazily initializes Chrome on first call
    /// - Returns guard that updates last_activity on drop
    /// - Conversation ID is derived internally (cannot be wrong)
    pub async fn browser(&self) -> Result<BrowserSessionGuard<'_>, BrowserError> {
        self.browser_sessions.get_or_create(&self.conversation_id).await
    }
}
```

**Key invariant:** `browser_sessions` is private. Tools call `ctx.browser()` which internally uses `ctx.conversation_id`. This makes it impossible to accidentally access another conversation's browser.

### Key Types

```rust
use chromiumoxide::{Browser, Page, Handler};

/// Global manager for all browser sessions (owned by Runtime)
pub struct BrowserSessionManager {
    sessions: RwLock<HashMap<String, BrowserSession>>,
    cleanup_handle: Option<JoinHandle<()>>,
}

impl BrowserSessionManager {
    /// Get or create session for conversation (called by ToolContext::browser())
    pub async fn get_or_create(&self, conversation_id: &str) -> Result<BrowserSessionGuard<'_>, BrowserError>;
    
    /// Kill specific session (called on conversation delete)
    pub async fn kill_session(&self, conversation_id: &str);
    
    /// Kill all sessions (called on shutdown)
    pub async fn shutdown_all(&self);
}

impl Drop for BrowserSessionManager {
    fn drop(&mut self) {
        // Synchronously kill all Chrome processes on server shutdown
    }
}

/// Per-conversation browser instance  
/// Wraps chromiumoxide::Browser with Phoenix-specific state
pub struct BrowserSession {
    browser: Browser,              // chromiumoxide Browser (owns Chrome child process)
    handler_task: JoinHandle<()>,  // CDP event handler task
    page: Page,                    // Current page (single-tab model)
    console_logs: VecDeque<ConsoleEntry>,
    last_activity: Instant,
}

/// RAII guard returned by ToolContext::browser()
/// Updates last_activity timestamp on drop
pub struct BrowserSessionGuard<'a> {
    session: RwLockWriteGuard<'a, BrowserSession>,
    conversation_id: String,
    manager: &'a BrowserSessionManager,
}

impl<'a> BrowserSessionGuard<'a> {
    pub fn page(&self) -> &Page { &self.session.page }
    pub fn page_mut(&mut self) -> &mut Page { &mut self.session.page }
    pub fn console_logs(&self) -> &VecDeque<ConsoleEntry> { &self.session.console_logs }
    pub fn console_logs_mut(&mut self) -> &mut VecDeque<ConsoleEntry> { &mut self.session.console_logs }
}

impl Drop for BrowserSessionGuard<'_> {
    fn drop(&mut self) {
        self.session.last_activity = Instant::now();
    }
}
```

chromiumoxide handles all CDP protocol complexity internally:
- WebSocket connection management
- Request/response correlation  
- Event dispatching
- Process lifecycle (spawn, kill, wait)

### Integration Points

1. **Tool Registration**: Add browser tools to `ToolRegistry::standard()` in `src/tools.rs` (tools are now stateless singletons)
2. **ToolContext Creation**: Runtime creates `ToolContext` at tool invocation time with `browser_sessions: Arc<BrowserSessionManager>`
3. **Cleanup Hook**: `delete_conversation` handler calls `runtime.browser_sessions.kill_session(&id)`
4. **Shutdown Hook**: `BrowserSessionManager::drop()` kills all Chrome processes
5. **Runtime Ownership**: `Runtime` owns `Arc<BrowserSessionManager>`, passes clone to each `ToolContext`

### Dependencies

- `chromiumoxide` - Async CDP client with built-in browser lifecycle management
- `image` - Screenshot resizing (already used by `read_image`)
- `futures` - For `StreamExt` on the CDP handler
- Chrome/Chromium installation (headless-shell in containers)
