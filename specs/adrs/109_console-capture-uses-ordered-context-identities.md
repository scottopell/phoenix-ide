# ADR-109: Navigation console capture uses ordered context identities

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-BT-004, REQ-BCLI-007

## Context

Navigation results need errors from the new document, including child frames removed before the result is assembled. The ordinary session console history must survive navigation. Clearing a ring buffer cannot fence already queued CDP events, and enumerating only attached frames loses errors from frames that have already disappeared.

Chrome's numeric execution-context identifiers can be reused across processes. Its context lifecycle events also provide system-unique identities. The browser library distributes different event methods through independently polled channels, so merging those channels cannot establish source ordering between context lifecycle and console events.

## Options considered

1. **Final attached-frame allowlist:** small and excludes old documents, but drops errors from detached new frames.
2. **Capture-start numeric-context exclusion set:** retains detached frames, but confuses contexts when numeric identifiers are reused.
3. **One ordered context-and-console stream:** associate each console entry with the system-unique context identity at receipt, retaining that association after destruction. This requires an owned CDP reader and a small target-attachment helper.

## Decision

Replace the console subscription with one bounded CDP connection that processes context lifecycle and console events in source order. Stamp console entries with unique context identities. Acknowledged isolated-world markers establish capture boundaries and snapshot the prior context identities. Full-document navigation excludes those prior identities; same-document navigation retains entries after the marker. Entries from new frames remain attributable after those frames detach.

Preserve the ordinary console ring buffer. Unknown ownership or listener failure makes navigation capture fail explicitly instead of returning a fabricated empty result. Share only the bounded connection/target-attachment helper with snapshot collection; keep each consumer's command and event processing local.

## Consequences

- **Positive:** queued old errors stay excluded, detached-frame errors remain available, and numeric ID reuse cannot collapse distinct document contexts.
- **Negative:** each browser owns an additional CDP connection and its listener task. Capture-connection frames/messages are limited to 8 MiB; unsupported larger messages fail capture visibly. Teardown must abort the listener.
- **Neutral:** this adds no transport exposed to harnesses, reconnection framework, dependencies, or cross-version protocol guarantee. Profiling continues through its existing browser connection.

## References

- [Browser requirements](../browser-tool/requirements.md), REQ-BT-004.
- [Standalone browser requirements](../browser-cli/requirements.md), REQ-BCLI-007.
- [ADR-107](107_standalone-browser-uses-session-daemon.md), [ADR-108](108_snapshot-input-and-collection-boundaries.md).
- `session::console::listen`, `ConsoleCapture::observe`, `BrowserSession::finish_console_navigation_capture`, `cdp::connect_target`.
- [Official CDP Runtime schema](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/json/js_protocol.json), `ExecutionContextDescription.uniqueId` and `Runtime.evaluate.uniqueContextId`.
