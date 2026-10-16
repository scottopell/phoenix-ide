# Standalone Browser CLI — Executive Summary

## Requirements Summary

Coding agents can use Phoenix's browser operations through a standalone executable, retaining browser state across separate shell commands. Session selection supports an explicit name, an environment binding, and a worktree-based fallback. Navigation starts fresh state when needed; other browser actions report state loss. Navigation supplies accessibility targets and debugging evidence, while profiling preserves the browser tool's measurement contract.

## Technical Summary

The CLI reuses the browser-specific operation boundary from `phoenix-tools` and the CDP engine in `phoenix-browser`. A user-private Unix socket connects short-lived callers to a session daemon. An operating-system startup lock prevents competing daemon owners. A serialized worker executes operations while the daemon independently accepts shutdown. Browser initialization settles under ownership before cancellation; admitted operations can be interrupted with an explicit partial-side-effects warning. The daemon owns browser termination, transient profile cleanup, and a configurable idle interval. Session selection, daemon lifetime, and browser lifetime have separate identities. The local transport provides no cross-version interoperability promise.

The lifecycle contract is specified in `browser-cli.allium`; browser operation semantics remain in `specs/browser-tool/`.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-BCLI-001: Standalone Operation | ✅ Complete | Installed executable smoke: help, schema, open, eval, close without Phoenix. |
| REQ-BCLI-002: Predictable Session Selection | ✅ Complete | Explicit/environment precedence, canonical paths, clone and linked-worktree boundaries. |
| REQ-BCLI-003: Exclusive Local Ownership | ✅ Complete | Concurrent initial opens share one owner; named sessions isolate pages. |
| REQ-BCLI-004: Lazy Startup and State Loss | ✅ Complete | Absent discovery, idle expiry, dead-Chrome detection and fresh open. |
| REQ-BCLI-005: Distinct Lifetimes | ✅ Complete | Daemon/browser identities appear on operation success and failure; browser-only replacement rotates identity. |
| REQ-BCLI-006: Idle Expiry and Explicit Shutdown | ✅ Complete | Active work survives idle; close interrupts blocked eval; startup SIGTERM retains cleanup ownership; immediate reopen and descriptor isolation. |
| REQ-BCLI-007: Actionable Navigation Result | ✅ Complete | URL/title/interactive refs, partial snapshot failure, isolated console source fences with main-document and iframe errors. |
| REQ-BCLI-008: Complete Browser Operation Surface | ✅ Complete | Shared operation catalog; real-browser interaction, console, screenshot, scenario, CPU capture and saved summary journey. |
| REQ-BCLI-009: File-Only Discovery and Analysis | ✅ Complete | Browser-free schema/help/saved analysis; malformed startup idle configuration does not block these commands. |
| REQ-BCLI-010: Predictable Output and Errors | ✅ Complete | JSON envelopes, typed error codes, structured measurement failures, bounded stdin, semantic preflight and exit statuses. |
| REQ-BCLI-011: No Ambiguous Operation Replay | ✅ Complete | Missing/malformed response reports transport uncertainty without starting or replaying another request. |
| REQ-BCLI-012: Harness Usage Guidance | ✅ Complete | [Usage and installation](../../docs/phoenix-browser.md), including session binding and profiling. |

**Progress:** 12 of 12 implemented and covered. PR review and final CI qualification are tracked in [PR #925](https://github.com/scottopell/phoenix-ide/pull/925).

## Verification

Lifecycle and installed-command coverage live in `crates/phoenix-tools/tests/browser_cli.rs`. Browser-free preflight and operation-catalog parity live in `browser::access_tests`; profiling validation and methodology retain their existing unit coverage. Engine snapshot and console tests use actual Chrome for document replacement, native input, artifacts, source-event capture, and iframe diagnostics.

Local broad validation passed Clippy, E2E, Allium, spec shape/anchors, task validation, and dev-tool tests. Its Rust run stopped on the unchanged database test `current_schema_controller_accepts_compiled_123_and_128_shapes`: the macOS Python SQLite controller reported `unable to open database file` during integrity checking; an isolated rerun reproduced it. Full-suite success is not claimed from that run.
