# Browser Automation Tool

## User Story

As an AI agent, I need to test Progressive Web Applications and verify their caching and offline behavior so that I can ensure they work correctly without manual DevTools intervention.

## Requirements

### REQ-BT-001: Navigate and Wait for Ready State

THE SYSTEM SHALL navigate to URLs and indicate when the page is ready for interaction

WHEN navigation fails
THE SYSTEM SHALL provide the specific error type (network, DNS, timeout, 404, 500)

**Rationale:** AI agents need reliable navigation to start any testing sequence. Without knowing when a page is ready, tests would use arbitrary sleeps.

---

### REQ-BT-002: Verify Service Worker State

WHEN checking a page with service workers
THE SYSTEM SHALL report if a service worker is registered, active, and controlling the page

**Rationale:** AI agents testing PWAs cannot verify the most basic requirement (SW registration) without this. This blocked service worker testing completely.

---

### REQ-BT-003: Identify Request Cache Source

WHEN network requests complete
THE SYSTEM SHALL indicate if each request was served from network, service worker, or browser cache

**Rationale:** AI agents cannot verify caching strategies work correctly without knowing where responses came from. This is the difference between a working and broken offline app.

---

### REQ-BT-004: Simulate Offline Mode

THE SYSTEM SHALL block network requests on demand to simulate offline conditions

WHEN offline
THE SYSTEM SHALL allow the page to continue using cached resources

**Rationale:** AI agents need to test offline functionality without disconnecting the entire VM's network. Manual offline testing doesn't scale.

---

### REQ-BT-005: Capture All Console Output

THE SYSTEM SHALL capture console messages from all contexts including service workers

WHEN displaying messages
THE SYSTEM SHALL indicate which context (page, service worker) produced each message

**Rationale:** AI agents debugging service worker issues are blind without console output from worker contexts. Page-only logs miss critical error messages.

---

### REQ-BT-006: Execute JavaScript and Get Results

THE SYSTEM SHALL execute JavaScript in the page context and return results

WHEN execution fails
THE SYSTEM SHALL return the error message and stack trace

**Rationale:** AI agents need to interact with SPAs, check application state, and trigger actions that aren't possible through basic navigation.

---

### REQ-BT-007: Take Screenshots for Verification

THE SYSTEM SHALL capture screenshots of the current viewport

**Rationale:** AI agents need visual verification when testing UI changes or debugging layout issues that can't be detected through DOM inspection alone.
