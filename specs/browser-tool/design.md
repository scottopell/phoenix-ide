# Browser Automation Tool - Design

## Architecture Overview

Native Rust tool using Chrome DevTools Protocol (CDP) for browser automation:

1. **CDP Client Layer** - WebSocket connection to Chrome debugging port
2. **Command Interface** - Simple method calls that map to CDP domains
3. **Chrome Process Manager** - Lifecycle management of headless Chrome
4. **Result Formatting** - Structured output for AI agent consumption

## Technology Stack

- **Language**: Rust (consistent with Phoenix IDE)
- **CDP Library**: Chromium Oxide (or similar, pending source code review)
- **Chrome**: Headless Chrome with remote debugging enabled
- **Protocol**: Chrome DevTools Protocol via WebSocket

## Component Design

### Chrome Process Manager

Handles Chrome lifecycle:

- Launch headless Chrome with debugging port
- Ensure single instance per tool invocation
- Clean shutdown of Chrome process
- Automatic port allocation to avoid conflicts

### CDP Connection (All Requirements)

Core WebSocket connection to Chrome:

- Connect to Chrome debugging port
- Handle connection failures gracefully
- Automatic reconnection if needed
- Message serialization/deserialization

### Page Navigation (REQ-BT-001)

Uses CDP Page domain:

- `Page.navigate` for URL navigation
- `Page.loadEventFired` for basic readiness
- Error detection from navigation response
- Map CDP errors to user-friendly messages

### Service Worker Inspector (REQ-BT-002) 

Uses CDP ServiceWorker domain:

- `ServiceWorker.enable` to start tracking
- Query registrations via Runtime evaluation
- Check controller state for current page
- Simple active/inactive status reporting

### Network Source Tracker (REQ-BT-003)

Uses CDP Network domain:

- `Network.enable` to intercept requests
- `Network.responseReceived` events
- Check `response.fromServiceWorker` flag
- Track `response.fromDiskCache` flag
- Categorize into network/sw/cache buckets

### Offline Simulator (REQ-BT-004)

Uses CDP Network domain:

- `Network.emulateNetworkConditions` with offline flag
- Simple boolean offline state
- No complex throttling profiles needed

### Console Aggregator (REQ-BT-005)

Uses CDP Runtime and Log domains:

- `Runtime.consoleAPICalled` for console messages
- `Log.entryAdded` for service worker logs
- Tag messages with source context
- Forward all to unified output

### JavaScript Executor (REQ-BT-006)

Uses CDP Runtime domain:

- `Runtime.evaluate` with expression string
- `awaitPromise: true` for async code
- Serialize return values to JSON
- Capture exception details

### Screenshot Capture (REQ-BT-007)

Uses CDP Page domain:

- `Page.captureScreenshot` for viewport
- PNG format by default
- Base64 encoded result
- Optional file save

## Implementation Approach

1. Start with provided Chromium Oxide source code
2. Create minimal CLI binary
3. Implement one command at a time
4. Test against Phoenix IDE service worker
5. Add commands as needed

## Error Handling

All CDP errors mapped to user-friendly messages:

- Connection failures
- Navigation errors  
- Runtime exceptions
- Timeout handling

## Output Format

Structured text suitable for AI agents:

```
SUCCESS: Navigation complete
URL: http://localhost:8000
Status: 200
Ready: true

ERROR: Navigation failed
URL: http://localhost:9999
Reason: net::ERR_CONNECTION_REFUSED
```

## What This Design Explicitly Excludes

- Cross-browser support (Chrome only)
- Complex wait strategies (just load event)
- Full CDP protocol exposure
- GUI or interactive mode
- Test framework integration

Focused on solving the PWA testing problems with minimal complexity.
