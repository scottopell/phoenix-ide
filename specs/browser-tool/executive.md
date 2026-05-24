# Browser Automation Tool - Executive Summary

## Requirements Summary

The Phoenix Browser Tool enables AI agents to interact with web pages during development, testing, and debugging. Three user stories drive the scope: web development (navigate, screenshot, debug), automated testing (interact via click/type/JS, capture evidence), and PWA testing (service workers, offline mode — post-MVP).

The core set covers navigation, JavaScript evaluation, screenshots, viewport control, console log capture with accurate object representation, and dedicated click/type/wait tools that reliably trigger framework event handlers. Browser availability is automatic — if no system browser is found, a compatible Chromium is downloaded and cached transparently. Post-MVP scope covers PWA-specific inspection (service workers, network sources, offline simulation) and network request capture.

## Technical Summary

Built using the `chromiumoxide` crate for async CDP communication. Tools are stateless, receiving all context via `ToolContext`. The `ctx.browser()` method provides correct-by-construction session access — conversation ID is derived internally, preventing cross-session contamination.

`BrowserSessionManager` maps `WorkScope` keys to Chrome instances (REQ-BROWSER-WS-001). Worktree-backed conversations share a single Chrome window across continuation members so context-exhaustion continuations inherit the same tabs, cookies, and dev-tools state; Direct conversations fall back to per-conversation scoping. Sessions auto-start on first `browser()` call and auto-clean after 30-minute idle. The resource-cleanup cascade tears the Chrome process down on archive/abandon/mark-merged/hard-delete using scope-equality preservation — the same shape as tmux (REQ-BROWSER-WS-003). When no system Chrome is present, `BrowserFetcher` downloads a compatible binary to `~/.cache/phoenix-ide/chromium/` and caches it for future runs.

Console logs are captured via CDP event subscription. Objects and arrays are represented using the CDP preview field (key-value pairs) rather than generic type labels. Large output (>4096 bytes total) writes to a temp file with the path returned inline. Per-entry content is stored in full in the buffer (up to a memory-protection cap) and truncated only at retrieval time, ensuring the file escape hatch always contains complete entries.

## Status Summary

### Core Requirements

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BT-001:** Navigate to URLs | ✅ Complete | `browser_navigate` tool |
| **REQ-BT-002:** Execute JavaScript | ✅ Complete | `browser_eval` tool |
| **REQ-BT-003:** Take Screenshots | ✅ Complete | `browser_take_screenshot` + `read_image` |
| **REQ-BT-004:** Capture Console Logs | ✅ Complete | CDP event subscription |
| **REQ-BT-005:** Resize Viewport | ✅ Complete | `browser_resize` tool |
| **REQ-BT-006:** Read Image Files | ✅ Complete | `read_image` tool |
| **REQ-BT-007:** Reliable Browser Availability | ✅ Complete | `chromiumoxide` fetcher; caches to `~/.cache/phoenix-ide/chromium/` |
| **REQ-BT-008:** Reliable Element Clicking | ✅ Complete | `browser_click` tool; CDP-level events |
| **REQ-BT-009:** Reliable Text Input | ✅ Complete | `browser_type` tool; CDP-level keyboard events |
| **REQ-BT-010:** Implicit Session Model | ✅ Complete | `BrowserSessionManager` |
| **REQ-BT-011:** State Persistence | ✅ Complete | Session guard pattern |
| **REQ-BT-012:** Stateless Tools with Context | ✅ Complete | `ToolContext.browser()` |
| **REQ-BT-013:** Wait for Async Page Elements | ✅ Complete | `browser_wait_for_selector` tool |
| **REQ-BT-014:** Accurate Console Log Object Representation | ✅ Complete | CDP preview field; objects show `{k: v}`, arrays show `[v]` |
| **REQ-BT-015:** Access to Full Console Log Content | ✅ Complete | Buffer stores full content (10KB cap); display truncation at retrieval time only; file escape hatch writes untruncated entries |
| **REQ-BT-016:** Keyboard Shortcut Input | ✅ Complete | `browser_key_press` tool; CDP-level keydown/keyup for non-printable keys and modifier chords |
| **REQ-BT-017:** React Component Access | ✅ Complete | `browser_inject_react_devtools` + `browser_remove_react_devtools`; `window.__phoenix` helper via `__REACT_DEVTOOLS_GLOBAL_HOOK__` |
| **REQ-BT-018:** Live Browser View Side Panel | ✅ Complete | View-only CDP screencast relay via `/api/conversations/:id/browser-view` WS; mutex with prose/diff slot; auto-mount-when-empty on first `browser_*` tool |
| **REQ-BT-019:** Systematic Web Performance Testing | 🟡 In Progress | `browser_profile` tool (action enum). Tier 0+1 + cheap Tier 2; lading-style scenario harness returns raw per-run samples (never averaged). Lifecycle in `browser-profiling.allium`. Network emulation deferred (REQ-BT-019-NG-NETEMU) |

### WorkScope Ownership

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BROWSER-WS-001:** Sessions Keyed by WorkScope | ✅ Complete | `BrowserSessionManager` map keyed by `WorkScope::stable_key()`; user_data_dir derived from same key |
| **REQ-BROWSER-WS-002:** Continuation Inheritance and Lifecycle Fan-Out | ✅ Complete | `get_session` returns existing `Arc` on scope match; SSE bridge fans `BrowserSessionState` to every runtime resolving to the scope |
| **REQ-BROWSER-WS-003:** Cascade Integration | ✅ Complete | `cascade_browser_on_delete` invoked from `run_resource_cleanup_cascade`; scope-equality preservation |
| **REQ-BROWSER-WS-004:** Capability-Gap Logging | ✅ Complete | `debug`-level logs on `get_existing` miss, cascade-skip, and sink-drop |

### Post-MVP Requirements

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BT-020:** Service Worker Inspection | ❌ Not Started | PWA-specific |
| **REQ-BT-021:** Network Request Source | ❌ Not Started | PWA-specific |
| **REQ-BT-022:** Offline Mode Simulation | ❌ Not Started | PWA-specific |
| **REQ-BT-023:** Multi-Context Console | ❌ Not Started | PWA-specific |
| **REQ-BT-024:** Capture Network Requests | ❌ Not Started | API debugging |

**Core Progress:** 18 of 18 complete (REQ-BT-019 performance suite in progress)
**WorkScope Progress:** 4 of 4 complete
**Total Progress:** 22 of 28 complete
