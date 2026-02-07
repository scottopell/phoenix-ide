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

THE SYSTEM SHALL navigate to a specified URL and wait for the page to be ready for interaction

WHEN navigation fails (network error, DNS failure, timeout, HTTP error)
THE SYSTEM SHALL return a clear error message indicating the failure type

WHEN the URL triggers a file download instead of navigation
THE SYSTEM SHALL report the download completion and file location

**Rationale:** Navigation is the foundation of all browser automation. Agents need reliable feedback about whether navigation succeeded and when the page is ready.

**User Stories:** US-1, US-2, US-3

---

### REQ-BT-002: Execute JavaScript

THE SYSTEM SHALL execute JavaScript expressions in the page context and return results

WHEN the expression returns a Promise
THE SYSTEM SHALL await the Promise and return the resolved value (configurable)

WHEN execution throws an exception
THE SYSTEM SHALL return the error message and context

WHEN the result is large
THE SYSTEM SHALL write output to a file and return the file path

**Rationale:** JavaScript execution is the universal interface for page interaction. Rather than providing separate tools for click/type/scroll/wait, a flexible JS eval tool handles all interactions with one capability.

**User Stories:** US-1, US-2

---

### REQ-BT-003: Take Screenshots

THE SYSTEM SHALL capture screenshots of the current viewport

WHEN a CSS selector is provided
THE SYSTEM SHALL capture only the specified element

THE SYSTEM SHALL return the screenshot as base64-encoded image data suitable for LLM vision input

WHEN the image exceeds LLM size limits
THE SYSTEM SHALL resize the image to fit within limits

THE SYSTEM SHALL save screenshots to a known location for later retrieval

**Rationale:** Visual verification is essential for web development. Screenshots provide evidence of current state and enable agents to see what they've built.

**User Stories:** US-1, US-2

---

### REQ-BT-004: Capture Console Logs

THE SYSTEM SHALL capture console messages (log, warn, error, info) from the page context

THE SYSTEM SHALL provide a way to retrieve recent console logs

THE SYSTEM SHALL provide a way to clear captured logs

WHEN output exceeds a size threshold
THE SYSTEM SHALL write logs to a file and return the file path

**Rationale:** Console output is the primary debugging channel for web applications. Agents need visibility into errors and diagnostic output.

**User Stories:** US-1, US-2

---

### REQ-BT-005: Resize Viewport

THE SYSTEM SHALL resize the browser viewport to specified dimensions

THE SYSTEM SHALL use a sensible default viewport size (e.g., 1280x720)

**Rationale:** Responsive design verification requires testing at different viewport sizes. Agents need control over viewport dimensions.

**User Stories:** US-1

---

### REQ-BT-006: Read Image Files

THE SYSTEM SHALL read image files from disk and return them as base64-encoded data for LLM vision input

WHEN the image exceeds LLM size limits
THE SYSTEM SHALL resize the image to fit within limits

THE SYSTEM SHALL support common image formats (PNG, JPEG, GIF, WebP)

**Rationale:** Agents may need to analyze existing images (screenshots from disk, user-provided images) in addition to taking new screenshots.

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

## Requirements Traceability

| Requirement | User Story | MVP |
|-------------|------------|-----|
| REQ-BT-001: Navigate to URLs | US-1, US-2, US-3 | ✅ |
| REQ-BT-002: Execute JavaScript | US-1, US-2 | ✅ |
| REQ-BT-003: Take Screenshots | US-1, US-2 | ✅ |
| REQ-BT-004: Capture Console Logs | US-1, US-2 | ✅ |
| REQ-BT-005: Resize Viewport | US-1 | ✅ |
| REQ-BT-006: Read Image Files | US-1, US-2 | ✅ |
| REQ-BT-010: Implicit Session Model | US-1, US-2, US-3 | ✅ |
| REQ-BT-011: State Persistence | US-2 | ✅ |
| REQ-BT-012: Stateless Tools with Context | US-1, US-2, US-3 | ✅ |
| REQ-BT-020: Service Worker Inspection | US-3 | ❌ |
| REQ-BT-021: Network Request Source | US-3 | ❌ |
| REQ-BT-022: Offline Mode Simulation | US-3 | ❌ |
| REQ-BT-023: Multi-Context Console | US-3 | ❌ |
