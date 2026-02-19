# Browser Automation Tool

## User Stories

### US-1: Web Development and Debugging (Primary)

As an AI agent building web applications, I need to navigate to pages, inspect content, take screenshots, and verify UI functionality so I can develop and debug web apps effectively.

**Motivation:** This is the dominant use case for LLM agents with browser access. When building web services on localhost, agents need to:
- Navigate to the running application
- Verify visual output matches expectations
- Debug issues by inspecting console output
- Test different viewport sizes for responsive design

### US-2: Automated Testing and Verification

As an AI agent, I need to interact with web pages (click buttons, fill forms, check results) and capture evidence (screenshots, console logs) so I can verify application behavior.

**Motivation:** After implementing features, agents need to verify they work:
- Execute JavaScript to interact with UI elements
- Capture screenshots as evidence of current state
- Check console for errors or expected log output
- Wait for async operations to complete

### US-3: Progressive Web App Testing (Specialized)

As an AI agent testing PWAs, I need to verify service worker registration, caching behavior, and offline functionality so I can ensure PWAs work correctly.

**Motivation:** PWA development requires specialized verification that DevTools typically provides:
- Verify service workers are registered and active
- Confirm requests are served from cache
- Test offline behavior in isolation

---

## Core Requirements (MVP)

### REQ-BT-001: Navigate to URLs

The `browser_navigate` tool SHALL navigate to a specified URL and wait for the page to be ready for interaction

WHEN navigation fails (network error, DNS failure, timeout, HTTP error)
`browser_navigate` SHALL return a clear error message indicating the failure type

WHEN the URL triggers a file download instead of navigation
`browser_navigate` SHALL report the download completion and file location

**Rationale:** Navigation is the foundation of all browser automation. Agents need reliable feedback about whether navigation succeeded and when the page is ready.

**User Stories:** US-1, US-2, US-3

---

### REQ-BT-002: Execute JavaScript

The `browser_eval` tool SHALL execute JavaScript expressions in the page context and return results

WHEN the expression returns a Promise
`browser_eval` SHALL await the Promise and return the resolved value (configurable via the `await` parameter)

WHEN execution throws an exception
`browser_eval` SHALL return the error message and context

WHEN the result exceeds 4096 bytes
`browser_eval` SHALL write output to a temp file and return the file path

**Rationale:** JavaScript execution is the universal interface for reading page state and complex interactions. For clicks and typing, prefer `browser_click` and `browser_type` which reliably trigger framework event handlers.

**User Stories:** US-1, US-2

---

### REQ-BT-003: Take Screenshots

The `browser_take_screenshot` tool SHALL capture a screenshot of the current viewport and save it to a known file path

WHEN a CSS selector is provided
`browser_take_screenshot` SHALL capture only the matching element

THE SYSTEM SHALL make the screenshot visible to the agent by passing the saved path to `read_image`

WHEN the image exceeds LLM vision size limits
THE SYSTEM SHALL resize the image to fit within limits

**Rationale:** Visual verification is essential for web development. Screenshots provide evidence of current state. The two-step pattern (`browser_take_screenshot` then `read_image`) is intentional: the screenshot is saved for later retrieval even if the agent does not immediately inspect it.

**User Stories:** US-1, US-2

---

### REQ-BT-004: Capture Console Logs

THE SYSTEM SHALL automatically capture console messages (log, warn, error, info) from the page context throughout the browser session

The `browser_recent_console_logs` tool SHALL retrieve recent captured log entries, newest first, up to a configurable limit (default: 100)

The `browser_clear_console_logs` tool SHALL discard all captured log entries, resetting the buffer

WHEN output from `browser_recent_console_logs` exceeds 4096 bytes
`browser_recent_console_logs` SHALL write the full output to a temp file and return the file path

**Rationale:** Console output is the primary debugging channel for web applications. Agents need visibility into errors and diagnostic output without having to inject logging instrumentation manually.

**User Stories:** US-1, US-2

---

### REQ-BT-005: Resize Viewport

The `browser_resize` tool SHALL resize the browser viewport to specified width and height in pixels

THE SYSTEM SHALL use a default viewport of 1280×720 pixels when a session starts

**Rationale:** Responsive design verification requires testing at different viewport sizes. Common reference points: 375px wide for mobile, 768px for tablet, 1280px for desktop.

**User Stories:** US-1

---

### REQ-BT-006: Read Image Files

The `read_image` tool SHALL read an image file from disk and make its contents visible to the agent for visual analysis

WHEN the image exceeds LLM vision size limits
`read_image` SHALL resize the image to fit within limits

`read_image` SHALL support PNG, JPEG, GIF, and WebP formats

**Rationale:** Agents use `read_image` both to view screenshots taken by `browser_take_screenshot` and to analyze any other image file on disk (e.g. user-provided images, generated assets).

**User Stories:** US-1, US-2

---

### REQ-BT-007: Reliable Browser Availability

WHEN browser tools are first invoked in a conversation
THE SYSTEM SHALL make a browser available without requiring manual installation

WHEN no browser is found in the system
THE SYSTEM SHALL automatically obtain a compatible browser and cache it for future use

WHEN a browser has been previously obtained
THE SYSTEM SHALL use the cached browser without downloading again

**Rationale:** Agents should not fail silently or require setup steps to use browser tools. Browser availability should be automatic and transparent.

**User Stories:** US-1, US-2, US-3

---

### REQ-BT-008: Reliable Element Clicking

The `browser_click` tool SHALL click a page element identified by CSS selector using CDP-level mouse events

WHEN the target element does not exist
`browser_click` SHALL return a clear error indicating the element was not found

WHEN the `wait` parameter is set to true
`browser_click` SHALL wait for the element to appear in the DOM before clicking

`browser_click` SHALL reliably trigger event handlers regardless of the UI framework in use (React, Vue, Angular, plain DOM)

**Rationale:** Clicking elements is a fundamental interaction that must work reliably across all web frameworks. JavaScript `.click()` can fail to trigger React/Vue synthetic event handlers; CDP-level mouse events do not have this limitation.

**User Stories:** US-2

---

### REQ-BT-009: Reliable Text Input

The `browser_type` tool SHALL type text into an input element identified by CSS selector using CDP-level keyboard events

WHEN the target element does not exist
`browser_type` SHALL return a clear error indicating the element was not found

WHEN the `clear` parameter is set to true
`browser_type` SHALL replace existing field content; otherwise it appends

`browser_type` SHALL reliably trigger input and change event handlers regardless of the UI framework in use (React, Vue, Angular, plain DOM)

**Rationale:** Directly setting an element's value property does not fire the synthetic events that React/Vue listen to. CDP-level keyboard events correctly trigger all framework event handlers.

**User Stories:** US-2

---

### REQ-BT-013: Wait for Async Page Elements

The `browser_wait_for_selector` tool SHALL poll the page until a CSS selector matches an element in the DOM

WHEN the `visible` parameter is set to true
`browser_wait_for_selector` SHALL additionally wait for the element to be visually visible (not just present in DOM)

WHEN the element does not appear within the timeout (default: 30 seconds)
`browser_wait_for_selector` SHALL return a clear timeout error

**Rationale:** Modern web apps load content asynchronously. Agents should use `browser_wait_for_selector` rather than manually polling with `browser_eval` — it is more concise and handles the polling loop internally.

**User Stories:** US-1, US-2

---

### REQ-BT-014: Accurate Console Log Object Representation

WHEN `console.log()` is called with an object
THE SYSTEM SHALL represent the object as `{key: value, ...}` using its actual properties, not the generic label "Object"

WHEN `console.log()` is called with an array
THE SYSTEM SHALL represent the array as `[value, value, ...]` using its actual elements

WHEN an object or array has more properties than fit in the preview
THE SYSTEM SHALL include a `…` overflow indicator in the representation

**Rationale:** "Object" is not a useful representation of `{userId: 123, status: 'active'}`. Agents debugging applications need to see actual values to understand program state without resorting to manual `JSON.stringify` calls.

**User Stories:** US-1, US-2

---

### REQ-BT-015: Access to Full Console Log Content

WHEN a single console log entry's text representation exceeds the per-entry display limit
`browser_recent_console_logs` SHALL include the truncated text with a visible `…` truncation indicator

WHEN the total output from `browser_recent_console_logs` exceeds 4096 bytes (whether due to many entries or large individual entries)
`browser_recent_console_logs` SHALL write the complete output to a temp file and return only the file path

WHEN `browser_recent_console_logs` returns a file path instead of inline content
THE SYSTEM SHALL ensure the file contains all entries in full, without per-entry truncation, so the agent can read it using `bash` or similar

**Rationale:** Console logs can contain large serialized objects critical for debugging. Truncation exists to protect the LLM context window, not the UI. Per-entry truncation must happen only when formatting output for the tool result (what the LLM sees), not at capture time — so the internal buffer always retains full content, and the file escape hatch always contains complete untruncated data.

**User Stories:** US-1, US-2

---

## Session Management Requirements

### REQ-BT-010: Implicit Session Model

THE SYSTEM SHALL maintain browser state across tool calls within a conversation

THE SYSTEM SHALL automatically start the browser on first browser tool call

THE SYSTEM SHALL automatically close the browser after idle timeout (30 minutes)

THE SYSTEM SHALL isolate browser state between different conversations

WHEN browser tools receive `ToolContext`
THE SYSTEM SHALL use `ctx.browser()` to obtain the session for `ctx.conversation_id`
AND the mapping from conversation to browser SHALL be enforced by construction

**Rationale:** Agents should not need to manage session IDs or browser lifecycle. The `ToolContext.browser()` method provides correct-by-construction session access - tools cannot accidentally use the wrong conversation's browser.

**User Stories:** US-1, US-2, US-3

---

### REQ-BT-011: State Persistence

WHILE a conversation is active
THE SYSTEM SHALL persist browser state (cookies, cache, current page) across tool calls

WHEN `ctx.browser()` is called
THE SYSTEM SHALL update the session's last-activity timestamp
AND return a guard that provides access to the browser session

**Rationale:** Natural testing flows like "login → navigate → verify" require state to persist between steps.

**User Stories:** US-2

---

### REQ-BT-012: Stateless Tools with Context Injection

WHEN browser tools are invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext` parameter
AND derive conversation identity from `ToolContext.conversation_id`
AND access browser session via `ToolContext.browser()` method

WHEN browser tools are constructed
THE SYSTEM SHALL NOT store per-conversation state
AND tool instances SHALL be reusable across conversations

THE `ToolContext.browser()` method SHALL:
- Use `conversation_id` internally (not exposed to tool)
- Return a guard that updates activity timestamp on drop
- Lazily initialize Chrome on first call

**Rationale:** Stateless tools with context injection make invalid states unrepresentable. Tools cannot use wrong conversation's browser because `browser()` derives identity from the context.

**User Stories:** US-1, US-2, US-3

---

## Extended Requirements (Post-MVP)

### REQ-BT-020: Service Worker Inspection

WHEN checking a page with service workers
THE SYSTEM SHALL report if a service worker is registered, active, and controlling the page

**Rationale:** PWA testing requires verification that service workers are properly configured.

**User Stories:** US-3

---

### REQ-BT-021: Network Request Source Identification

WHEN network requests complete
THE SYSTEM SHALL indicate if each request was served from network, service worker, or browser cache

**Rationale:** Verifying caching strategies requires knowing where responses originated.

**User Stories:** US-3

---

### REQ-BT-022: Offline Mode Simulation

THE SYSTEM SHALL block network requests on demand to simulate offline conditions

WHEN offline
THE SYSTEM SHALL allow the page to continue using cached resources

**Rationale:** Testing offline functionality requires controlled network conditions independent of the host system.

**User Stories:** US-3

---

### REQ-BT-023: Multi-Context Console Capture

THE SYSTEM SHALL capture console messages from service worker contexts in addition to page context

WHEN displaying messages
THE SYSTEM SHALL indicate which context (page, service worker) produced each message

**Rationale:** Service worker debugging requires visibility into worker-context logs that are separate from page logs.

**User Stories:** US-3

---

### REQ-BT-024: Capture Network Requests

THE SYSTEM SHALL capture HTTP network requests made by the page

THE SYSTEM SHALL provide a way to retrieve recent network requests with:
- Request URL
- HTTP method
- Response status code
- Response content type
- Timing information (request start, response received)

THE SYSTEM SHALL provide a way to clear captured network requests

WHEN a request fails (network error, timeout, CORS blocked)
THE SYSTEM SHALL capture the failure reason

WHEN output exceeds a size threshold
THE SYSTEM SHALL write requests to a file and return the file path

**Rationale:** Network request visibility is essential for debugging API integrations and understanding application behavior. Agents need to verify that requests are made correctly and responses are received as expected, complementing console logs for comprehensive debugging.

**User Stories:** US-1, US-2

---

## Requirements Traceability

| Requirement | User Story | MVP |
|-------------|------------|-----|
| REQ-BT-001: Navigate to URLs | US-1, US-2, US-3 | ✅ |
| REQ-BT-002: Execute JavaScript | US-1, US-2 | ✅ |
| REQ-BT-003: Take Screenshots | US-1, US-2 | ✅ |
| REQ-BT-004: Capture Console Logs | US-1, US-2 | ✅ |
| REQ-BT-005: Resize Viewport | US-1 | ✅ |
| REQ-BT-006: Read Image Files | US-1, US-2 | ✅ |
| REQ-BT-007: Reliable Browser Availability | US-1, US-2, US-3 | ✅ |
| REQ-BT-008: Reliable Element Clicking | US-2 | ✅ |
| REQ-BT-009: Reliable Text Input | US-2 | ✅ |
| REQ-BT-010: Implicit Session Model | US-1, US-2, US-3 | ✅ |
| REQ-BT-011: State Persistence | US-2 | ✅ |
| REQ-BT-012: Stateless Tools with Context | US-1, US-2, US-3 | ✅ |
| REQ-BT-013: Wait for Async Page Elements | US-1, US-2 | ✅ |
| REQ-BT-014: Accurate Console Log Object Representation | US-1, US-2 | ✅ |
| REQ-BT-015: Access to Full Console Log Content | US-1, US-2 | 🟡 |
| REQ-BT-020: Service Worker Inspection | US-3 | ❌ |
| REQ-BT-021: Network Request Source | US-3 | ❌ |
| REQ-BT-022: Offline Mode Simulation | US-3 | ❌ |
| REQ-BT-023: Multi-Context Console | US-3 | ❌ |
| REQ-BT-024: Capture Network Requests | US-1, US-2 | ❌ |
