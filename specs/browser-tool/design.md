# Browser Automation Tool - Design

## Architecture Overview

Minimal browser control tool focused on PWA testing needs:

1. **Page Controller** - Navigation and readiness detection
2. **Service Worker Inspector** - Registration and state monitoring  
3. **Network Observer** - Request source identification
4. **Console Aggregator** - Multi-context log collection

## Component Design

### Page Navigation (REQ-BT-001)

Simple navigation with readiness detection:

- Navigate to URL and wait for load event
- Detect common error conditions from navigation result
- Provide specific error types for debugging
- No complex wait strategies - load event is sufficient for most cases

### Service Worker Monitor (REQ-BT-002) 

Basic service worker state inspection:

- Check if any service worker is registered for the page
- Determine if worker is active vs installing/waiting
- Verify if worker is controlling the current page
- Simple boolean/enum states, not full debugging info

### Network Source Tracker (REQ-BT-003)

Identify where each response was served from:

- Hook browser's network events
- Check response headers and timing info to determine source
- Categorize as: network, service-worker, disk-cache, or memory-cache
- Focus on the critical distinction for offline testing

### Offline Simulator (REQ-BT-004)

Simple network blocking:

- Toggle network access for the page context
- Allow service worker to continue serving from cache
- No complex network condition simulation - just on/off
- Browser's offline mode is sufficient

### Console Aggregator (REQ-BT-005)

Collect logs from all contexts:

- Listen to console events from page
- Subscribe to service worker console output
- Tag each message with its source context
- Include timestamp and log level

### JavaScript Executor (REQ-BT-006)

Basic script execution:

- Execute JavaScript strings in page context
- Wait for promise resolution if returned
- Serialize return values to JSON
- Capture and return errors with stack traces

### Screenshot Capture (REQ-BT-007)

Viewport screenshots only:

- Capture current viewport as PNG
- No element selection or full-page capture
- Return as base64 or save to file
- Simple and reliable

## What This Design Explicitly Excludes

- Complex state management (cookies, storage)
- Performance metrics beyond basic timing
- Accessibility testing
- Full DevTools protocol exposure
- Multiple browser contexts
- Advanced screenshot features

These exclusions keep the tool focused on solving the concrete PWA testing problems that motivated its creation.
