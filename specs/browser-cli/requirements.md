# Standalone Browser CLI

## User Story

As a coding agent using a shell-based harness, I need Phoenix browser operations through a standalone executable so that I can inspect, interact with, debug, and profile a web application across separate command invocations.

## Scope

The `phoenix-browser` executable owns local browser sessions independently of the Phoenix server. It exposes the browser operations governed by [browser-tool requirements](../browser-tool/requirements.md), including accessibility references and profiling methodology. Remote hosting, MCP transport, importing browser profiles or credentials, sharing a live browser with Phoenix, and live-view UI are outside this contract. The local transport does not promise interoperability between executable versions; [compatibility requirements](../compatibility/requirements.md) apply.

## Requirements

### REQ-BCLI-001: Standalone Operation

THE SYSTEM SHALL provide a `phoenix-browser` executable usable without running the Phoenix server or constructing a conversation
AND SHALL reuse Phoenix browser operation behavior rather than implement a parallel browser engine.

**Rationale:** Coding harnesses need the same browser capabilities without adopting the Phoenix conversation runtime.

### REQ-BCLI-002: Predictable Session Selection

WHEN resolving a session
THE SYSTEM SHALL select the explicit `--session` value, otherwise `PHOENIX_BROWSER_SESSION`, otherwise the canonical Git worktree root, otherwise the canonical current directory outside Git
AND SHALL distinguish explicit names from directory-derived identities.

THE SYSTEM SHALL reuse one fallback session across subdirectories of a worktree
AND SHALL distinguish separate worktrees, including linked worktrees.

THE SYSTEM SHALL document that a worktree fallback shares state among callers and that agents needing isolation can bind a unique session name.

**Rationale:** Useful defaults avoid mandatory setup while explicit bindings give harnesses isolation.

### REQ-BCLI-003: Exclusive Local Ownership

WHEN separate processes address the same selected session concurrently
THE SYSTEM SHALL permit at most one daemon owner and one live browser lifetime for that session
AND SHALL serialize browser operations within the daemon.

THE SYSTEM SHALL restrict its local communication endpoint and session control files to the owning operating-system user.

**Rationale:** Concurrent startup and actions must not create competing browsers or race shared page state.

### REQ-BCLI-004: Lazy Startup and State Loss

WHEN `open` addresses a session without a live browser
THE SYSTEM SHALL lazily start the daemon and browser with a fresh transient profile.

WHEN a browser-dependent command other than `open` addresses a session without a live browser
THE SYSTEM SHALL return a structured state-loss error with instructions to use `open`
AND SHALL NOT silently create an empty replacement browser.

WHEN `status` or `close` addresses an absent session
THE SYSTEM SHALL report the absent state without launching a daemon or browser.

**Rationale:** Navigation is an intentional fresh start; other actions depend on existing page state.

### REQ-BCLI-005: Distinct Lifetimes

THE SYSTEM SHALL distinguish the stable selected session identity from each daemon lifetime and browser lifetime
AND SHALL expose the applicable lifetime identifiers in operation results.

WHEN its owned Chrome process exits while the daemon remains alive
THE SYSTEM SHALL report the browser as absent, reject state-dependent actions with state loss, and let a subsequent `open` retire the ended lifetime and start fresh

WHEN a browser lifetime ends
THE SYSTEM SHALL invalidate its accessibility references and active profiling state
AND SHALL NOT transfer those handles to a replacement browser.

**Rationale:** A repeated session name cannot make stale page handles valid again.

### REQ-BCLI-006: Idle Expiry and Explicit Shutdown

WHEN a daemon has accepted no operation for its configured idle interval and no operation is active
THE SYSTEM SHALL terminate its browser and daemon and remove its transient browser profile.

THE SYSTEM SHALL default the idle interval to 30 minutes
AND SHALL allow a startup flag or environment setting to configure the interval when the daemon starts
AND SHALL report the effective interval through session status.

WHEN `close`, SIGTERM, or an interrupt requests orderly shutdown
THE SYSTEM SHALL stop accepting browser work, terminate the owned browser, and release session ownership
AND SHALL surface cleanup failures rather than report successful cleanup.

WHEN browser or profile cleanup fails
THE SYSTEM SHALL retain session ownership and reject new browser work
AND SHALL expose the cleanup failure through status and allow explicit close to retry cleanup
AND SHALL release ownership only after authoritative cleanup succeeds.

THE SYSTEM SHALL NOT expire an active operation because it exceeds the idle interval.

WHEN `close` or an orderly termination signal arrives during an admitted browser operation
THE SYSTEM SHALL cancel the operation independently of its page-side completion
AND SHALL report `operation_cancelled` with an explicit warning that side effects may have occurred
AND SHALL retain ownership until the operation worker has exited and browser cleanup completes.

WHEN shutdown arrives while browser initialization is in progress
THE SYSTEM SHALL let initialization settle and register any successfully created browser before cancellation and cleanup
AND SHALL NOT abandon an initialization future that may own an unregistered Chrome process.

**Rationale:** Idle expiry preserves active work. Explicit shutdown releases resources promptly while retaining ownership across initialization and cancellation.

### REQ-BCLI-007: Actionable Navigation Result

WHEN `open` successfully navigates
THE SYSTEM SHALL return the final URL, page title, compact interactive accessibility snapshot with usable references, and relevant console errors observed for that navigation.

WHEN `open` targets the current URL
THE SYSTEM SHALL reload the document while retaining the browser session, cookies, and origin storage.

WHEN only the URL fragment changes
THE SYSTEM SHALL preserve native navigation semantics
AND SHALL NOT wait for a replacement document when the browser completes a same-document navigation.

WHEN navigation succeeds but snapshot collection, console capture, or final URL/title observation fails
THE SYSTEM SHALL preserve the successful navigation result and report each observation failure separately
AND SHALL NOT represent the failed snapshot as an empty successful tree.

THE SYSTEM SHALL NOT attribute console entries retained from earlier navigations to the new navigation.

THE SYSTEM SHALL retain navigation errors from child frames that detach before the navigation result is returned.

**Rationale:** One navigation result should supply the evidence and targets needed for the next agent action.

### REQ-BCLI-008: Complete Browser Operation Surface

THE SYSTEM SHALL expose navigation, accessibility snapshots, JavaScript evaluation, screenshots, viewport resizing, click, text input, key chords, selector waits, console retrieval and clearing, and every supported Phoenix profiling action through the CLI.

THE SYSTEM SHALL accept both accessibility references and CSS selectors for click and text input under REQ-BT-008, REQ-BT-009, and REQ-BT-025
AND SHALL support `eval --stdin` for JavaScript input.

WHEN `eval` returns a JSON-representable value
THE SYSTEM SHALL expose that value as typed JSON in machine-readable output without requiring callers to parse a text wrapper.

THE SYSTEM SHALL distinguish JavaScript `undefined` and non-JSON scalar values from JSON `null`
AND SHALL preserve user-returned object keys as data, including names also used for artifact metadata.

WHEN an evaluation result is externalized under REQ-BT-002
THE SYSTEM SHALL identify the artifact's encoding and expose its saved handle explicitly.

THE SYSTEM SHALL support `screenshot --output PATH`
AND SHALL resolve a relative output path against the calling process's working directory before sending the request to the daemon.

THE SYSTEM SHALL preserve profiling readiness boundaries, reset and warmup behavior, raw per-run samples, unavailable/null distinctions, warnings, and artifacts under REQ-BT-019
AND SHALL preserve structured failure data, including blocked-step and methodology information, when the operation fails.

**Rationale:** Moving between coding harnesses should preserve the debugging and measurement workflow.

### REQ-BCLI-009: File-Only Discovery and Analysis

WHEN requesting command help, schemas, profiling help, or CPU-profile summarization from a file
THE SYSTEM SHALL perform the request without launching a daemon or browser.

**Rationale:** Discovering the interface and analyzing saved evidence do not require live page state.

### REQ-BCLI-010: Predictable Output and Errors

THE SYSTEM SHALL provide concise human-readable output and a consistent `--json` result envelope containing success, selected session, applicable lifetime identifiers, and either result data or a structured error.

THE SYSTEM SHALL expose artifact paths explicitly
AND SHALL return nonzero exit status for failed commands
AND SHALL include stable error codes and actionable recovery instructions where recovery is possible.

THE SYSTEM SHALL reject requests exceeding the 32 MiB encoded JSON message budget, including framing, before session lookup or startup
AND SHALL report invalid input rather than an uncertain delivery outcome for requests that were never sent.

THE SYSTEM SHALL preserve each inline tool image as a verified existing file artifact or a newly written owned artifact
AND SHALL report image conversion failures visibly while retaining other returned observations and artifacts.

**Rationale:** Agents need parseable outcomes and humans need readable evidence without inferring success from incidental text.

### REQ-BCLI-011: No Ambiguous Operation Replay

WHEN a connection fails after a request may have reached the daemon
THE SYSTEM SHALL report the uncertain operation outcome
AND SHALL NOT automatically replay that operation or substitute a new browser lifetime.

**Rationale:** Retrying a click, script, or capture after an uncertain response can duplicate side effects.

### REQ-BCLI-012: Harness Usage Guidance

THE SYSTEM SHALL provide concise usage guidance covering session binding and fallback sharing, open/snapshot/reference interaction, debugging artifacts, profiling, and explicit close.

**Rationale:** Harness authors should discover the intended workflow without reverse engineering daemon internals.
