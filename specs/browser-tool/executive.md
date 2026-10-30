# Browser Automation Tool - Executive Summary

## Requirements Summary

The Phoenix Browser Tool enables AI agents to interact with web pages during development, testing, and debugging. Three user stories drive the scope: web development (navigate, screenshot, debug), automated testing (interact via click/type/JS, capture evidence), and PWA testing (service workers, offline mode — post-MVP).

The core set covers navigation, JavaScript evaluation, screenshots, viewport control, console log capture with accurate object representation, accessibility snapshots, and dedicated click/type/wait tools that reliably trigger framework event handlers. Click/type accept CSS selectors or snapshot references. Browser availability is automatic — if no system browser is found, a compatible Chromium is downloaded and cached transparently. Post-MVP scope covers PWA-specific inspection (service workers, network sources, offline simulation) and network request capture.

## Technical Summary

Built using the `chromiumoxide` crate for async CDP communication. Tools are stateless, receiving all context via `ToolContext`. The `ctx.browser()` method provides correct-by-construction session access — WorkScope identity is derived internally, preventing cross-session contamination.

`BrowserSessionManager` maps `WorkScope` keys to Chrome instances (REQ-BROWSER-WS-001). Ordinary conversations share their persisted WorkScope browser across continuation members, including Direct/non-Git members, so context-exhaustion continuations inherit the same tabs, cookies, and dev-tools state. Distinct persisted scopes remain separate even when their cwd matches. Sessions auto-start on first `browser()` call and auto-clean after 30-minute idle. The resource-cleanup cascade tears the Chrome process down on archive/abandon/mark-merged/hard-delete using scope-equality preservation — the same shape as tmux (REQ-BROWSER-WS-003). When no system Chrome is present, `BrowserFetcher` downloads a compatible binary to `~/.cache/phoenix-ide/chromium/` and caches it for future runs.

Console logs are captured through one ordered CDP reader that associates per-target execution contexts with immutable document identities. Recursive iframe attachment captures isolated cross-origin targets, with bounded child-stream barriers at navigation boundaries. Navigation capture preserves errors from removed child frames while excluding prior-document errors; it leaves ordinary console history intact. The connection shares bounded target attachment with snapshot collection and limits frames/messages to 8 MiB. Ownership or listener failures make navigation capture unavailable explicitly. Objects and arrays use the CDP preview field. Large output (>4096 bytes total) writes to a temp file; per-entry content is truncated only at retrieval after the buffer's memory-protection cap. See ADR-110 and ADR-111 for ordering, document ownership, and isolated-frame capture.

Navigation uses one bounded native CDP connection for destination dispatch, current-URL reload, and document readiness. It handles same-document completion without a missing-load-event timeout, returns native navigation errors, and avoids leaving a library navigation watcher queued after timeout. Reload checks the observed loader before dispatch.

### Reusable device and touch primitives

`browser_profile run_scenario` accepts a session-persistent device configuration and single-contact `touch_start`, `touch_move`, `touch_end`, and bounded `pause` steps. `DeviceState` acknowledges configuration only after CDP success; `TouchState` preserves contact identity and sends empty `touchPoints` arrays for end/cancel. `BrowserSession::resize_viewport` preserves device scale, mobile mode, and touch emulation. `ScenarioOwner` holds broker-side task custody independently of the session lock, while scenario execution and cleanup retain exclusive session access.

The scope is reusable input/configuration primitives, not retained visual diagnostics or application-specific gesture assertions. CDP acknowledgement is neither physical-device equivalence nor proof of paint. Managed navigation and scenario reset are gated by contact state; arbitrary eval- or click-initiated navigation is not fenced. The 512-step limit, 5000ms per-pause limit, and 30000ms total pause limit per run do not impose a total scenario wall-time deadline. Explicit device application has a 15-second deadline; the aggregate default-initialization deadline is under repair; contact cancellation and throttle restoration share a five-second cleanup deadline. Cancellation coverage concerns the shared operation's caller future. Killing/disconnecting the short-lived CLI does not cancel its daemon-owned operation; explicit daemon close/shutdown is the CLI cancellation path. Native/CLI parity here means shared operation/schema and composition behavior, not client-disconnect parity.

Qualification-host evidence: v3 focused suite **31 passed**; v7 touch suite **7 passed**; v8 registered-tool checks **1 native passed + 1 CLI passed**. On v10, profile tests **19 passed**, including the 512-step boundary; registered-native caller-abort acceptance **1 passed** (1.14s); strengthened native composition/cleanup acceptance **1 passed** (0.87s). Each v10 group had zero failures and zero ignored tests. v9 was never executed; unchanged CLI acceptance was not repeated.

The qualified v10 code pin is `751047116a48dc48ca4f775f82b38f65c1327756`, based on `df7077ad88595640ca8fb87620333a4cbae68719`; complete source manifest SHA-256 `65deec9d8a2e6d56ca9bdb5f7c4a4c79d0830cef4d3394cc541ca35191f81cbd`. The host reports all 3391 manifest entries matched before and after execution, installed Chrome explicit, browser-test skips unset, owned process groups absent, compilers empty, and locks released. Host-local receipt: `/Users/sopell/.phoenix-ide/qualification/browser-minimal-65deec9d-evidence/focused-tests-1793339910622047000.json`, SHA-256 `a691bd8f194578c78792ca206f59e4e4cf2097739c2db3eac8d44f3f257fee24`.

This is focused qualification, not a full repository check or merge/deploy approval. Changed Rust formatting and diff-whitespace checks passed. The five-fix spec alignment preserves the Allium baseline: zero errors, one warning, and 18 informational diagnostics; it is not a zero-diagnostic checker pass. Existing profiling conversation-ownership wording versus lifecycle WorkScope ownership remains a follow-up. This receipt update changes documentation only.

### Five-fix follow-up — qualification pending

The follow-up is limited to: (1) one aggregate 15-second default-device initialization deadline with common owned-launch cleanup; (2) atomic standalone throttle dispatch/bookkeeping under one session write guard; (3) acknowledged/unknown throttle state, with unknown blocking scenarios until explicit successful standalone throttle; (4) busy scenario admission without waiting for the running session write guard; and (5) scenario click/type honoring the native-input lease before target resolution and through completion. Requirements and the profiling lifecycle specify these contracts. Implementation and focused regressions for this follow-up are **pending qualification**; no new build or browser pass is claimed.

Retained exact **v12** qualification: strict all-target Clippy passed; **19 profile + 1 native caller-abort + 1 strengthened native composition + 1 CLI composition test passed**, zero failures/ignored. CLI was rerun because shared dispatch changed. All 3391 manifest entries matched before/after; owned processes were absent and locks released. Manifest SHA-256: `d448c1dfa30266ea40c27ef9c16f4628ce8e88c43fde9895ac697ee3d7a71372`; receipt SHA-256: `ec2f55d71cc6d3bbdf798e8ad56049440c54714c80e17b07e6f5cf316fed4f64`. v11's failed Clippy run receives no test credit.

Retained exact **v13** qualification: only two device-test assertions changed to exact float-bit comparisons, with no tolerance or suppression. Strict browser all-target Clippy and all **5 device tests passed**. Production and native/CLI test code were unchanged from v12; those runtime receipts retain v12 attribution and were not rerun on v13. v13 manifest SHA-256: `6eb9af42794d0d64b748fe9499e0a6793f22b0cfb783ef1a854ad101e75eabc4`; receipt SHA-256: `4b6740e0d6d97e77661060221ab3b6a4db9fe8d41b8d8260bd9fdf419eae947e`. Neither receipt qualifies the five-fix follow-up.

## Status Summary

### Core Requirements

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BT-001:** Navigate to URLs | ✅ Complete | `browser_navigate` tool |
| **REQ-BT-002:** Execute JavaScript | ✅ Complete | `browser_eval` tool |
| **REQ-BT-003:** Take Screenshots | ✅ Complete | `browser_take_screenshot` + `read_image`; optional absolute output path with visible save failures. |
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
| **REQ-BT-019.2:** CPU throttling | 🟡 Follow-up qualification pending | Atomic standalone dispatch/bookkeeping and acknowledged/unknown throttle admission/restoration fixes/regressions pending qualification; historical profile receipts are not a new pass. |
| **REQ-BT-019.21:** Persistent scenario device configuration | 🟡 Follow-up qualification pending | Retained v12/v13 evidence above; aggregate default-initialization deadline and common owned-launch cleanup fixes/regressions pending qualification. |
| **REQ-BT-019.22:** Persistent touch contact and scenario cleanup | 🟡 Follow-up qualification pending | Retained native/CLI composition receipts above; busy admission and click/type lease fixes/regressions pending qualification. CLI disconnect limitation remains. |
| **REQ-BT-025:** Accessibility Snapshots and Element References | ✅ Complete | `browser_snapshot`; main-document AX tree and interactive filtering; exact-node click/type refs; explicit limits and large-output artifacts. Rendering/schema/browser regressions cover reference lifetime, native/React input, shadow roots, iframe exclusion, replacement at native input dispatch, navigation, cancellation, forwarded activation, and typing focus retargeting. Snapshot collection has an 8 MiB wire cap; spilled files are private and retired with their refs/session. |

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

**Core Progress:** 19 of 20 complete; performance suite in progress
**WorkScope Progress:** 4 of 4 complete
**Total Progress:** 23 of 29 complete
