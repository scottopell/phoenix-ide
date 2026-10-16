# Standalone Browser CLI — Executive Summary

## Requirements Summary

Coding agents can use Phoenix's browser operations through a standalone executable, retaining browser state across separate shell commands. Session selection supports an explicit name, an environment binding, and a worktree-based fallback. Navigation starts fresh state when needed; other browser actions report state loss. Navigation supplies accessibility targets and debugging evidence, while profiling preserves the browser tool's measurement contract.

## Technical Summary

The CLI reuses the browser-specific operation boundary from `phoenix-tools` and the CDP engine in `phoenix-browser`. A user-private Unix socket connects short-lived callers to a session daemon. An operating-system startup lock prevents competing daemon owners. The daemon serializes operations and owns browser termination, transient profile cleanup, and a configurable idle interval. Session selection, daemon lifetime, and browser lifetime have separate identities. The local transport provides no cross-version interoperability promise.

The lifecycle contract is specified in `browser-cli.allium`; browser operation semantics remain in `specs/browser-tool/`.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-BCLI-001: Standalone Operation | 🔄 In Progress | Implementation and validation pending. |
| REQ-BCLI-002: Predictable Session Selection | 🔄 In Progress | Precedence and worktree tests pending. |
| REQ-BCLI-003: Exclusive Local Ownership | 🔄 In Progress | Concurrent startup and isolation tests pending. |
| REQ-BCLI-004: Lazy Startup and State Loss | 🔄 In Progress | Lifecycle journey pending. |
| REQ-BCLI-005: Distinct Lifetimes | 🔄 In Progress | Lifetime and stale-reference tests pending. |
| REQ-BCLI-006: Idle Expiry and Explicit Shutdown | 🔄 In Progress | Expiry, active-operation, and cleanup tests pending. |
| REQ-BCLI-007: Actionable Navigation Result | 🔄 In Progress | Navigation and partial snapshot failure tests pending. |
| REQ-BCLI-008: Complete Browser Operation Surface | 🔄 In Progress | CLI browser journey and existing regression tests pending. |
| REQ-BCLI-009: File-Only Discovery and Analysis | 🔄 In Progress | Browser-free invocation tests pending. |
| REQ-BCLI-010: Predictable Output and Errors | 🔄 In Progress | Envelope and exit status tests pending. |
| REQ-BCLI-011: No Ambiguous Operation Replay | 🔄 In Progress | Transport failure evidence pending. |
| REQ-BCLI-012: Harness Usage Guidance | 🔄 In Progress | Usage documentation pending. |

**Progress:** 0 of 12 complete.
