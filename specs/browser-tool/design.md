# Browser Automation Tool - Design

## Architecture Overview

Native Rust tool using Chrome DevTools Protocol (CDP) for browser automation:

1. **Chrome Process Manager** - Lifecycle management of headless Chrome with idle timeout
2. **CDP Client Layer** - WebSocket connection to Chrome debugging port
3. **Tool Interface** - JSON-based tool definitions matching Phoenix tool schema
4. **Session Manager** - Per-conversation browser isolation

## Technology Stack

- **Language**: Rust (consistent with Phoenix IDE)
- **CDP Library**: Chromium Oxide or direct CDP WebSocket
- **Chrome**: Headless Chrome with remote debugging enabled
- **Protocol**: Chrome DevTools Protocol via WebSocket

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
Chrome process launched
    |
    v
Subsequent tool calls reuse browser
    |
    v
[30 min idle OR conversation ends]
    |
    v
Chrome process terminated
```

### Chrome Launch Configuration

- Headless mode enabled
- No sandbox (for containerized environments)
- WebSocket debugging enabled
- Default viewport: 1280x720 (16:9)
- Download directory: `/tmp/phoenix-downloads`
- Screenshot directory: `/tmp/phoenix-screenshots`

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

### File Locations (TBD)

- Browser tool module: `src/tools/browser/`
- Chrome process manager: `src/tools/browser/chrome.rs`
- CDP client: `src/tools/browser/cdp.rs`
- Tool definitions: `src/tools/browser/tools/`

### Dependencies

- Chrome/Chromium installation (headless-shell in containers)
- WebSocket client for CDP
- JSON serialization
- Image processing (resize, format conversion)
