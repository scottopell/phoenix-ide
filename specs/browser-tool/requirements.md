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

### US-4: Live Browser View (Collaborative Watching)

As a user watching an agent work on a web app, I want to see what the agent is actually doing in the browser in real time, so I can give feedback while it works rather than asking the agent to take a screenshot or describe what it sees.

**Motivation:** The agent's headless Chromium is invisible to the user by default. When the agent is iterating on a UI bug, building a feature, or scraping a page, the natural collaborative loop is "agent does X, user sees the result, user nudges." Without a live view, every step requires either a screenshot tool call (slow, snapshot-only) or the user opening the dev server URL in their own browser (different from what the agent sees, no shared frame of reference). A view-only mirror over CDP screencast closes that loop without introducing input arbitration questions.

### US-5: Systematic Web Performance Testing (Specialized)

As an AI agent optimizing a web app, I need to measure performance under a reproducible scenario, capture raw per-run samples around a baseline, and attribute cost to a root cause, so I can apply the scientific method (baseline → change → significance test) rather than guessing.

**Motivation:** Browser performance swings 5x by machine, thermal state, and GC timing. An optimization "hunt" is only valid if the scenario is reproducible, the metric is low-noise, and the variance is owned by the caller computing significance — not hidden behind a harness mean. The agent needs: a deterministic scenario driver (fixed app state + synthetic input + deterministic readiness signal), CPU throttling to normalize the host, macro counters and React commit metrics as the headline numbers, a multi-run loop that returns **raw per-run samples** (never pre-averaged), forced-GC heap reads, and root-cause tools (CPU sampling profile, why-did-render, long-task extraction, heap-snapshot diff). Without these the method collapses regardless of how nice the API is.

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

### REQ-BT-016: Keyboard Shortcut Input

The `browser_key_press` tool SHALL send a key chord to the page using CDP-level keyboard events

The `key` parameter SHALL accept:
- Named keys: `Escape`, `Enter`, `Tab`, `ArrowUp`, `ArrowDown`, `ArrowLeft`, `ArrowRight`, `Backspace`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `F1`–`F12`
- Single printable characters: `a`–`z`, `0`–`9`

The `modifiers` parameter SHALL accept a list of modifier keys: `ctrl`, `shift`, `alt`, `meta`

WHEN modifiers are specified
`browser_key_press` SHALL hold those modifiers while dispatching the key event, producing chords such as Ctrl+K, Ctrl+Shift+Z, Meta+K

WHEN a key chord conflicts with a browser-native shortcut (Ctrl+P=print, Ctrl+W=close tab, Ctrl+T=new tab, Ctrl+Tab=switch tab)
`browser_key_press` SHALL NOT be able to send that chord — Chrome intercepts these before dispatching to the page

**Rationale:** `browser_type` types printable text into an input element. It cannot send non-printable keys (Escape, Enter, Arrow keys) or modifier chords (Ctrl+K, Meta+K). These are needed to trigger keyboard shortcuts registered on `window` or `document` via `addEventListener('keydown', ...)` — common in React apps for command palettes, modal dismissal, and navigation. Events target the focused element and bubble normally through the DOM, so capture listeners on `window` and `document` receive them.
WHEN no element is focused
`browser_key_press` SHALL dispatch the event to the page root (equivalent to pressing a key with no element focused)

The tool SHALL fire keydown, keypress (where applicable), and keyup events in sequence so that all framework keyboard listeners receive the full event sequence

**Rationale:** `browser_type` types printable text into an input element. It cannot send non-printable keys (Escape, Enter, Arrow keys) or modifier chords (Ctrl+P, Ctrl+K). These are needed to trigger keyboard shortcuts registered on `window` or `document` via `addEventListener('keydown', ...)` — common in React apps for command palettes, modal dismissal, and navigation.

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

THE SYSTEM SHALL maintain browser state across tool calls within a `WorkScope`

THE SYSTEM SHALL automatically start the browser on first browser tool call

THE SYSTEM SHALL automatically close the browser after idle timeout (30 minutes)

THE SYSTEM SHALL isolate browser state between different `WorkScope`s. Continuation members that resolve to the same `WorkScope` deliberately share a session — see REQ-BROWSER-WS-001 / REQ-BROWSER-WS-002.

WHEN browser tools receive `ToolContext`
THE SYSTEM SHALL use `ctx.browser()` to obtain the session for `ctx.work_scope`
AND the mapping from `WorkScope` to browser SHALL be enforced by construction

**Rationale:** Agents should not need to manage session IDs or browser lifecycle. The `ToolContext.browser()` method provides correct-by-construction session access — tools cannot accidentally use the wrong scope's browser. Identity is `WorkScope`, not conversation id, because worktree-backed conversations share resources across continuation members (REQ-BROWSER-WS-001).

**User Stories:** US-1, US-2, US-3

---

### REQ-BT-011: State Persistence

WHILE a `WorkScope`'s last-touch is within the idle window
THE SYSTEM SHALL persist browser state (cookies, cache, current page, open tabs) across tool calls and across continuation members resolving to the same scope

WHEN `ctx.browser()` is called
THE SYSTEM SHALL update the session's last-activity timestamp
AND return a guard that provides access to the browser session

**Rationale:** Natural testing flows like "login → navigate → verify" require state to persist between steps. The continuation-inheritance dimension (REQ-BROWSER-WS-002) extends "between steps" to "between continuation members" — the on-disk profile dir is keyed by `WorkScope`, so two members on the same scope see the same cookies/cache/tabs.

**User Stories:** US-2

---

### REQ-BT-012: Stateless Tools with Context Injection

WHEN browser tools are invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext` parameter
AND derive scope identity from `ToolContext.work_scope`
AND access browser session via `ToolContext.browser()` method

WHEN browser tools are constructed
THE SYSTEM SHALL NOT store per-`WorkScope` state
AND tool instances SHALL be reusable across scopes

THE `ToolContext.browser()` method SHALL:
- Resolve `ToolContext.work_scope` internally (not exposed to tool)
- Return a guard that updates activity timestamp on drop
- Lazily initialize Chrome on first call

**Rationale:** Stateless tools with context injection make invalid states unrepresentable. Tools cannot use the wrong scope's browser because `browser()` derives identity from the context's `WorkScope`. Direct-mode conversations resolve to `WorkScope::Conversation(<id>)` (per-conversation scoping fallback); worktree-backed conversations resolve to `WorkScope::Worktree(<path>)` (shared across continuations).

**User Stories:** US-1, US-2, US-3

---

## WorkScope Ownership

These requirements migrate browser sessions from per-conversation ownership to `WorkScope` ownership, the same primitive tmux integration adopted in REQ-TMUX-WS-001. Worktree-backed conversations share a single Chrome window across continuation members; Direct conversations fall back to per-conversation scoping because no durable owner exists for them to inherit.

### REQ-BROWSER-WS-001: Sessions Keyed by WorkScope

WHEN `BrowserSessionManager::get_session` resolves a session
THE SYSTEM SHALL key the lookup by `WorkScope::stable_key()`

WHEN a conversation is worktree-backed (Work, Branch, top-level managed Explore)
THE SYSTEM SHALL resolve `WorkScope::Worktree(<path>)` for the lookup

WHEN a conversation is Direct (no durable owner)
THE SYSTEM SHALL resolve `WorkScope::Conversation(<id>)` for the lookup

WHEN `BrowserSession::new` constructs a Chrome instance
THE SYSTEM SHALL derive the user data directory from the same scope key
AND two sessions resolving to the same scope SHALL share the same on-disk profile

**Rationale:** A Work/Branch conversation and any context-exhaustion continuation that inherits its worktree are the same unit of work. Their tools should drive the same Chrome window — including open tabs, cookies, and dev-tools state — without the continuation observing a silent re-login.

**User Stories:** US-1, US-2, US-3

---

### REQ-BROWSER-WS-002: Continuation Inheritance and Lifecycle Fan-Out

WHILE a worktree-scoped browser session is live
WHEN a continuation conversation resolves to the same `WorkScope`
THE SYSTEM SHALL return the existing `Arc<RwLock<BrowserSession>>` on `get_session`
AND SHALL NOT spawn a fresh Chrome instance

WHEN `BrowserSessionLifecycleEvent` is published
THE SYSTEM SHALL carry the `WorkScope` of the affected session
AND the SSE bridge SHALL fan out `BrowserSessionState` to every live runtime
    handle whose conversation resolves to that scope

WHEN a continuation member's enriched view is hydrated
THE SYSTEM SHALL derive `browser_session_active` from `is_active(&WorkScope)` of the member's resolved scope

**Rationale:** Inheritance is structural — the agent does not opt in. The SSE fan-out keeps every continuation member's UI synchronized with the underlying Chrome process, so a kill triggered by the leaf is reflected in every member's "browser session live" indicator.

**User Stories:** US-2

---

### REQ-BROWSER-WS-003: Cascade Integration

WHEN the resource-cleanup cascade runs (archive / abandon / mark-merged / hard-delete)
THE SYSTEM SHALL invoke `cascade_browser_on_delete(manager, &WorkScope, inheritor_scope)`
AND `inheritor_scope` SHALL be the continuation's resolved `WorkScope`, or `None` if there is no continuation
AND failures SHALL log WARN and continue
    (consistent with the bash / tmux / projects cascade error policy)

WHEN `inheritor_scope == Some(work_scope)` (scope equality holds)
THE SYSTEM SHALL skip the session kill
    (the inheritor is still driving the same Chrome window)

WHEN `inheritor_scope` is `None` OR differs from `work_scope`
THE SYSTEM SHALL tear the session down
    (Direct continuations always fall here — their `Conversation(<child_id>)` scope is never equal to the parent's `Conversation(<parent_id>)` scope, so the equality rule subsumes the per-kind case-analysis)

**Rationale:** Before this requirement, archive/abandon killed bash + tmux but leaked Chrome until Phoenix restart. Scope-equality preservation is correct by construction — it asks "are my resources still owned by someone live?" rather than relying on the implicit invariant "Worktree continuations always inherit the same worktree." The same shape applies to tmux (REQ-TMUX-WS-002).

**User Stories:** US-2, US-3

---

### REQ-BROWSER-WS-004: Capability-Gap Logging

WHEN `BrowserSessionManager::get_existing` returns `None`
THE SYSTEM SHALL log at `debug` level with the queried `WorkScope`

WHEN `cascade_browser_on_delete` skips a kill because the worktree is shared with a continuation
THE SYSTEM SHALL log at `debug` level with the work scope and the continuation id

WHEN `BrowserSessionLifecycleEvent` cannot be delivered (sink closed)
THE SYSTEM SHALL log at `debug` level

**Rationale:** The absence of a session — viewer attached but agent never opened anything; cascade asked to kill a session that already died — is the failure mode that hides hardest. Audit-trail logging makes these silent paths observable without changing the user-facing surface.

**User Stories:** US-3

---

## Performance Profiling Requirements

### REQ-BT-019: Systematic Web Performance Testing

A single `browser_profile` tool exposes performance measurement and root-cause analysis through an `action` discriminator. The tool is stateful: profiling/tracing/coverage sessions have explicit start→stop lifecycles with preconditions enforced (see `browser-profiling.allium`). Sub-requirements are tiered by what the scientific method needs, not by implementation ease.

#### Tier 0 — Method-critical (the method collapses without these)

**REQ-BT-019.1 — Deterministic scenario driver.**
THE SYSTEM SHALL accept a declarative scenario (an ordered list of steps: navigate, reload, click, type, key, eval, wait-for-selector, wait-for-user-timing-mark, wait-for-eval-predicate) and execute it to a deterministic readiness signal.
WHEN a readiness step's condition is not met within its timeout THE SYSTEM SHALL fail the run and report which step blocked, rather than returning a measurement against an indeterminate state.

**REQ-BT-019.2 — CPU throttling.**
THE SYSTEM SHALL set a fixed CPU throttling rate (`Emulation.setCPUThrottlingRate`) for the duration of a scenario and restore the prior rate afterward, so measurements are comparable across hosts and thermal states. A rate of `1` means no throttling.

**REQ-BT-019.3 — Macro counter snapshot.**
THE SYSTEM SHALL capture `Performance.getMetrics` before and after each scenario run and report the delta and absolute for at least: ScriptDuration, TaskDuration, LayoutCount, RecalcStyleCount, JSHeapUsedSize, Nodes, JSEventListeners.

**REQ-BT-019.4 — React commit metrics.**
WHEN React is present THE SYSTEM SHALL report, per scenario run, the React commit count and summed `actualDuration`, split by mount vs update phase and keyed by component. This is collected via the existing `__phoenix` helper (REQ-BT-017) extended with a commit hook installed before React loads.

**REQ-BT-019.5 — Multi-run with raw per-run samples (hard constraint).**
THE SYSTEM SHALL repeat a scenario N times (N caller-configurable, with optional warmup runs excluded from results) and return the **raw per-run sample array**.
THE SYSTEM SHALL NOT return a pre-averaged or otherwise statistically reduced result in place of the raw samples. Significance, mean, and variance are owned by the caller, not the harness.

**REQ-BT-019.6 — Forced GC then heap read.**
THE SYSTEM SHALL, on request, force garbage collection (`HeapProfiler.collectGarbage`) and then read JSHeapUsedSize, so the memory metric is deterministic rather than GC-timing noise.

#### Tier 1 — Root-cause (turns "symptom found" into "root cause found")

**REQ-BT-019.7 — CPU sampling profile.**
THE SYSTEM SHALL start/stop a `Profiler` CPU sampling session and persist the profile to a file loadable in Chrome DevTools.
THE SYSTEM SHALL additionally return, inline on stop, an agent-readable hot-function ranking: top-N by self time aggregated per function (the non-double-counting "where is CPU spent" metric) and top-N call-tree nodes by total time (self + descendants, labelled as possibly double-counting recursion). A profile file an agent cannot parse is an artifact, not an answer; the file is still kept for a human/DevTools deep-dive.
THE SYSTEM SHALL provide a `cpu_summary` action that re-renders that ranking from a saved profile path without a browser (so an earlier or externally-captured profile can be re-read). WHEN sampling data (`samples`/`timeDeltas`) is absent THE SYSTEM SHALL fall back to `hitCount` weighting and label it as relative weight, not absolute time — never present hit counts as milliseconds.

**REQ-BT-019.8 — Why-did-render.**
WHEN React is present THE SYSTEM SHALL report, per commit, which components re-rendered and a best-effort attribution of the cause (changed props/state/hooks keys) derived from the fiber alternate.

**REQ-BT-019.9 — Timeline trace + long-task extraction.**
THE SYSTEM SHALL start/stop a `Tracing` session (default categories include `devtools.timeline`, `disabled-by-default-v8.cpu_profiler`, `blink.user_timing`), persist the trace to a `chrome://tracing`-loadable file, AND extract tasks longer than 50 ms into a summary.

**REQ-BT-019.10 — Heap-snapshot diff.**
THE SYSTEM SHALL take heap snapshots and, given a baseline snapshot and a post-scenario snapshot, report retained-size growth and detached-DOM-node count, so a leak across repeated mount/unmount is detectable.

#### Tier 2 — Supporting (cheap, folded in)

**REQ-BT-019.11 — JS coverage.**
THE SYSTEM SHALL start/stop `Profiler` precise coverage and persist per-script coverage.

**REQ-BT-019.12 — Trace persisted to disk.**
THE SYSTEM SHALL write traces (REQ-BT-019.9) as `{"traceEvents":[...]}` JSON for human audit.

#### Hardening — pit-of-success (a misread sample is worse than no sample)

These exist because the tool is consumed by LLM agents doing performance work, where a plausible-looking wrong number is the dominant failure mode. The governing rule: a measurement that was not actually taken MUST NOT be representable as a value that looks taken. Loud-wrong or labeled-absent always beats silent-wrong.

**REQ-BT-019.13 — No silent "not measured" for React metrics.**
THE SYSTEM SHALL distinguish, in every run sample, three React states: `measured` (a profiling-capable build, `actualDuration` available), `absent` (no React on the page), and `no_profiling_build` (React present but a production build that does not expose `actualDuration`).
WHEN the state is not `measured` THE SYSTEM SHALL report the React timing field as null (not `0`) and carry the state discriminator, so "zero React cost" and "React cost not measured" are not the same value. Commit *count* MAY still be reported when React is present (the commit hook fires regardless of build), with the timing field null.

**REQ-BT-019.14 — Counter-reset safety.**
`Performance.getMetrics` counters are cumulative since document load and reset on navigation. THE SYSTEM SHALL NOT compute a before/after delta across a navigation within a run.
THE SYSTEM SHALL reject a `run_scenario` whose `steps` contain a `navigate` or `reload` step (these belong in the per-run `reset`, REQ-BT-019.18, which executes before the before-snapshot), with an error that names the offending step and explains the counter reset — rather than returning a negative or meaningless delta.

**REQ-BT-019.15 — Forced-GC heap inside the run loop, default-on.**
THE SYSTEM SHALL, by default, force a full GC (`HeapProfiler.collectGarbage`) once per run and read `JSHeapUsedSize` only at that post-GC point — the single consistent point in the GC cycle (the V8 analog of a post-mark live-heap read).
THE SYSTEM SHALL take the GC strictly outside the duration bracket (snapshot the duration counters, then GC, then read heap) so the collect pause does not inflate ScriptDuration/TaskDuration.
WHEN per-run GC is explicitly disabled THE SYSTEM SHALL report the heap field as null plus a flag — never a populated mid-cycle sample under the same key as a real metric.

**REQ-BT-019.16 — Method-safe defaults and methodology warnings.**
THE SYSTEM SHALL default `warmup` to at least 1 (cold JIT/first-paint excluded by default).
THE SYSTEM SHALL emit a `methodology_warnings` list alongside (never in place of) the raw samples, populated when the run was unguarded in a way that invalidates a naive reading: no CPU throttle, `warmup` explicitly 0, no readiness step present in `steps`, per-run GC disabled, or per-run reset disabled. This is metadata, not a statistical reduction — REQ-BT-019.5 still holds.

**REQ-BT-019.17 — why_render: label, do not diagnose.**
THE SYSTEM SHALL classify each changed prop reported by `why_render` as `reference_changed` vs `value_changed` where cheaply determinable, and annotate that the comparison is a shallow reference compare.
**Rationale:** inline object/array/function props mint a new reference every render (the most common React pattern); a bare `!==` reports them as "changed" and an agent reads that as a root cause. Labeling stops the #1 false positive being stated as fact.

**REQ-BT-019.18 — Determinism by construction (per-run reset + readiness-anchored window).**
THE SYSTEM SHALL, before each run, reset to a fixed state, by default: an explicit `reset` (`navigate{url}` or `reload`) if supplied, otherwise a reload of the current URL. `reset: "none"` opts out and MUST emit a `methodology_warnings` entry. State bleed across runs SHALL NOT be the silent default.
THE SYSTEM SHALL treat the `reset` AND the first readiness step (`wait_selector`/`wait_timing`/`wait_eval`) as **untimed setup**: page load, framework mount, and async settle happen BEFORE the measured window opens. The window opens only once readiness is satisfied (REQ-BT-019.20). A scenario with no readiness step opens the window immediately after reset and MUST emit a `methodology_warnings` entry (the mount/settle is then unavoidably in-window — the F3 footgun).

**REQ-BT-019.20 — Page-anchored measurement window (F3/F5 root-cause fix).**
The measured window SHALL be defined IN THE PAGE, not inferred from two host-side `Performance.getMetrics` round-trips. A document-start-injected harness SHALL install a `longtask` `PerformanceObserver` and expose reset/read entry points.
- **Open:** immediately after the first readiness step satisfies, the harness resets in-page accumulators — `t0 = performance.now()`, longtask sum/count zeroed, the `__phoenix` React commit buffer cleared.
- **Close:** after the remaining (measured) steps, the harness reads the accumulators in one in-page call.
**Rationale:** host-bracketed CDP counter deltas are unanchored to the page's own scheduling and collapse to ~0 when a renderer-blocking burst delays the *before* read past itself (F5), or capture the mount when it lands between the two reads (F3). A page-anchored window is immune to renderer-block and CDP timing because the boundaries are `performance.now()` marks the page sets itself. This mirrors the validated consuming methodology.

**REQ-BT-019.19 — Canonical per-run sample schema (the contract consumers adapt to).**
The raw per-run sample emitted by `run_scenario` has a canonical key set. THESE NAMES ARE AUTHORITATIVE; a downstream statistics/significance consumer adapts its extraction to these — the harness does not rename to match a consumer, and (per REQ-BT-019-NG-STATS) does not reduce. The canonical keys are:

| key | meaning | null when |
|-----|---------|-----------|
| `run_index` | 0-based post-warmup run ordinal | never |
| `script_ms` | sum of `longtask` durations within the page-anchored window (ms) — REQ-BT-019.20, NOT a CDP `ScriptDuration` delta | never |
| `long_tasks` | count of `longtask` entries (>50 ms) within the window | never |
| `wall_ms` | `performance.now()` span of the measured window | never |
| `dom_nodes` | `document.getElementsByTagName('*').length` at window close (absolute) | never |
| `gc_ran` | whether a forced GC ran this run (REQ-BT-019.15) | never |
| `js_heap_used` | post-full-GC live-heap bytes — a one-shot gauge read once post-GC (F5 does not apply to gauges) | `gc_ran=false` |
| `react_status` | `measured` \| `absent` \| `no_profiling_build` | never |
| `react_commits` | commit count over the window (React present) | `react_status=absent` |
| `react_actual_ms` | summed per-commit ROOT-fiber `actualDuration` over the window, ms (REQ-BT-019.4 — never a per-fiber sum) | `react_status≠measured` |

The pre-REQ-BT-019.20 CDP-counter delta keys (`script_duration`, `task_duration`, `layout_count`, `recalc_style_count`, `nodes`, `js_event_listeners`) are **removed**: F5 proved the host-bracketed counter delta reads ~0 for real in-window work. The standalone `metrics` action still exposes a one-shot `Performance.getMetrics` snapshot (a gauge read, honest); only the per-run *windowed delta* use was unsound. A consumer keying off the old names, or off other names while silently skipping absent metrics, reduces to "heap only" — a methodology failure that looks like success; the fix belongs in the consumer's extraction table. Any change to a key here is breaking and MUST update this table.

#### Non-goals (this requirement)

- **REQ-BT-019-NG-NETEMU** — Network emulation (`Network.emulateNetworkConditions`, table item 2.1) is deferred. It introduces a stateful mode that interacts with scenario determinism and is the least method-critical item; tracked separately.
- **REQ-BT-019-NG-STATS** — The harness does not compute significance, p-values, means, or variance. Per REQ-BT-019.5 the caller (skill) owns all statistics. A harness that reduces samples is non-conforming.
- **REQ-BT-019-NG-AUTOSCENARIO** — The harness does not infer a scenario from the page. The scenario is always caller-supplied (REQ-BT-019.1); a "guess what to measure" mode is out of scope.

**Rationale:** Lading-style rigor = reproducible scenario + baseline-before-change + significance threshold + variance. Each tier is ordered by what that method requires. Tier 0 items are individually load-bearing: drop the scenario driver and runs aren't comparable; drop throttling and the host dominates; pre-average the samples and the caller can't test significance.

**User Stories:** US-5, US-1, US-2

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

### REQ-BT-017: React Component Access

The `browser_inject_react_devtools` tool SHALL install a lightweight `window.__phoenix` helper
into the page BEFORE page JavaScript runs, using CDP's `Page.addScriptToEvaluateOnNewDocument`.

THE SYSTEM SHALL hook into React's `__REACT_DEVTOOLS_GLOBAL_HOOK__` interface so that React
automatically registers its fiber roots into the helper at startup.

The `window.__phoenix` API SHALL provide:
- `getContext(keys: string[])` — find a context value by duck-typing (all keys present using `in`)
- `callContext(keys: string[], method: string, ...args)` — find a context and call a method on it
- `getState(componentName: string)` — get hook state array for a named component
- `listContexts()` — enumerate all ContextProvider values for discovery

WHEN `browser_inject_react_devtools` is called
THE SYSTEM SHALL return a script identifier usable with `browser_remove_react_devtools` for cleanup.

The `browser_remove_react_devtools` tool SHALL remove the injected script from future new documents
via `Page.removeScriptToEvaluateOnNewDocument`.

The injected script SHALL be idempotent: calling the tool twice before navigation SHALL NOT
double-register the helper or produce errors.

WHEN injected into a non-React page
THE SYSTEM SHALL install the hook harmlessly — the hook exists but React never calls it.

WHEN the page already has a `__REACT_DEVTOOLS_GLOBAL_HOOK__` installed (e.g. from a browser
extension or the app itself)
THE SYSTEM SHALL wrap the existing hook's `onCommitFiberRoot` callback rather than replacing it.

**Rationale:** Accessing React component state via raw fiber walking requires 6+ sequential
`browser_eval` calls, is fragile against minification (display names stripped in production),
and relies on internal React internals that can change. The `__REACT_DEVTOOLS_GLOBAL_HOOK__`
interface is stable across React 16-18 and explicitly maintained for DevTools integration.
Injecting before page JS runs is the only way to capture fiber roots — React only calls
`onCommitFiberRoot` if the hook exists at startup.

The tool is explicit and opt-in (not auto-injected on every navigate) so agents reading a
conversation trace know the hook is active, and pages with their own DevTools integration are
not silently broken.

**User Stories:** US-1, US-2

---

### REQ-BT-018: Live Browser View Side Panel

The SYSTEM SHALL provide a view-only live mirror of the conversation's browser session as a right-hand side panel in the conversation UI. The panel SHALL render frames received from the headless Chromium over CDP `Page.startScreencast` so the user can watch the agent's browser activity in real time.

WHEN the panel is mounted
THE SYSTEM SHALL stream JPEG frames over a per-conversation WebSocket at `GET /api/conversations/:id/browser-view`, framed with a 1-byte tag (`0x00` = JPEG frame, `0x01` = URL change, `0x02` = status string).

WHEN no browser session exists yet for the conversation
THE SYSTEM SHALL respond with a `0x02 "no-session"` status frame and close cleanly, without spawning a Chromium just to satisfy the panel.

WHEN the page navigates
THE SYSTEM SHALL emit a `0x01` URL frame so the panel header reflects the current location.

WHEN no viewer is attached
THE SYSTEM SHALL stop the screencast (`Page.stopScreencast`) so the headless Chrome is not paying the per-frame paint cost when no one is watching. The screencast restarts on the next viewer attach.

WHEN multiple viewers attach to the same conversation
THE SYSTEM SHALL fan out a single screencast source to all viewers via a broadcast channel.

The panel SHALL share a mutually-exclusive slot with the prose reader (REQ-FE-PR) and the diff viewer: at most one of {prose, diff, browser, none} occupies the right-hand viewer slot at a time.

WHEN `browser_session_active` transitions false→true on the conversation's SSE stream
  AND the right-hand viewer slot is empty
THE SYSTEM SHALL automatically mount the live browser view in that slot.

WHEN `browser_session_active` transitions false→true on the conversation's SSE stream
  AND the slot is already occupied (prose / diff)
THE SYSTEM SHALL NOT displace the existing viewer; the user's reading is not interrupted. A persistent manual-open affordance SHALL remain available for the user to switch to the browser view explicitly.

WHEN the user navigates to a conversation that already has `browser_session_active = true` at hydration time
THE SYSTEM SHALL NOT auto-mount the browser view; only the manual-open affordance is shown. Auto-mount fires on the live transition observed during this page's lifetime, not on entry-with-existing-state.

WHEN a browser session is created or destroyed in `BrowserSessionManager` (including idle cleanup)
THE SYSTEM SHALL emit a `browser_session_state` SSE event carrying the new `active` value on the conversation's SSE stream.

WHEN the UI receives a `browser_session_state` event
THE SYSTEM SHALL update the conversation's `browser_session_active` field as the single source of truth for live-session state. The UI SHALL NOT infer session state from message history.

The panel canvas SHALL have `pointer-events: none` so clicks and key presses on the rendered surface have no effect on the underlying page (REQ-BT-018-NG-INPUT below).

WHEN the conversation is hard-deleted (existing cascade in `BrowserSessionManager::kill_session`)
THE SYSTEM SHALL close any active live-view WebSocket cleanly, signalling "session ended" so the frontend stops reconnecting.

WHEN no browser session exists for a conversation
THE SYSTEM SHALL NOT poll or reconnect the live-view WebSocket for that conversation. Reconnect attempts are reserved for transient drops on a session that the server-authoritative signal indicates is still live.

**Non-goals (locked in for MVP):**

- **REQ-BT-018-NG-INPUT** — User input into the browser view (clicks, typing, scroll, keyboard shortcuts) is out of scope. The mirror is read-only. Adding input would create an arbitration problem with the agent's tool-driven activity (REQ-BT-008 / REQ-BT-009 / REQ-BT-016) that this feature deliberately avoids.
- **REQ-BT-018-NG-MULTITAB** — Tab / window management UI is out of scope. The panel always shows `BrowserSession.page` (the canonical first page). Additional CDP targets opened by the agent or by the page are not exposed.
- **REQ-BT-018-NG-HANDOFF** — Take-over / driving handoff between agent and user is out of scope. The agent is the sole driver.
- **REQ-BT-018-NG-HEADED** — Headed Chrome / VNC / Xvfb is not the architectural answer. The CDP screencast over the existing headless instance is.
- **REQ-BT-018-NG-PROXY** — Inferring session liveness from `browser_*` tool calls in message history is explicitly out of scope. The server-authoritative `browser_session_state` SSE event (sourced from `BrowserSessionManager`'s create/destroy edges) is the only signal the UI is permitted to consult. Walking the message stream to decide whether a session "should" exist is a leaky proxy that diverges from server truth as soon as a session is reaped, killed, or never created in the first place.

These non-goals are recorded so a future change does not silently "fix" one of them without re-opening the design discussion behind the locked decisions.

**Rationale:** The agent's headless Chromium is invisible to the user by default. When the agent is iterating on a UI, the natural collaborative loop is "agent does X, user sees the result, user nudges." The screenshot tool covers point-in-time captures but not the continuous-feedback case. A live view over the existing CDP channel adds pure additive user value without changing any tool semantics; the agent's browser tools work identically whether or not anyone is watching.

The slot-mutex + auto-mount-when-empty rule is the conservative choice: it never disrupts what the user is already reading, but the first time the agent needs the browser the panel just appears — no manual setup. The persistent manual-open affordance covers the "I had a diff open during first activation but now I want to see the browser" case.

The auto-mount trigger is the server-authoritative `browser_session_state` lifecycle event, broadcast from `BrowserSessionManager` on session create and destroy (including idle cleanup) and bridged onto the per-conversation SSE stream. The UI watches the false→true edge of `browser_session_active` and reacts only to live transitions; entering a conversation that is already mid-session shows the manual-open affordance but does not auto-mount, and a conversation with no live session never causes the panel WebSocket to poll. This replaces an earlier message-walk proxy that produced two failure modes — auto-opening an empty panel for past conversations whose server-side session was long gone, and a "no-session"/"connecting" reconnect loop — both of which are structurally impossible under the lifecycle-event signal.

**User Stories:** US-4 (also surfaces during US-1, US-2)

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
| REQ-BT-016: Keyboard Shortcut Input | US-2 | ✅ |
| REQ-BT-017: React Component Access | US-1, US-2 | ✅ |
| REQ-BT-018: Live Browser View Side Panel | US-4, US-1, US-2 | ✅ |
| REQ-BT-019: Systematic Web Performance Testing | US-5, US-1, US-2 | 🟡 |
| REQ-BT-020: Service Worker Inspection | US-3 | ❌ |
| REQ-BT-021: Network Request Source | US-3 | ❌ |
| REQ-BT-022: Offline Mode Simulation | US-3 | ❌ |
| REQ-BT-023: Multi-Context Console | US-3 | ❌ |
| REQ-BT-024: Capture Network Requests | US-1, US-2 | ❌ |
