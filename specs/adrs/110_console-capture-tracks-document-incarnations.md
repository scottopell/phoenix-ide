# ADR-110: Navigation console capture tracks document incarnations

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-BT-004, REQ-BCLI-007

## Context

An old document can create a child execution context after the navigation capture's begin marker but before navigation takes effect. Excluding only the contexts known at the marker misattributes that child's errors to the new navigation. Detached new frames and intermediate documents must still contribute their errors, while ordinary console history remains intact.

## Options considered

1. **Prior-context exclusion from ADR-109:** preserves detached frames, but cannot classify contexts created by the old document after capture begins.
2. **Final attached-frame ownership:** excludes old contexts, but loses errors from detached new frames and intermediate documents.
3. **Ordered document incarnations:** stamp entries with a typed document identity, advanced by the target's `Runtime.executionContextsCleared` event on the same ordered stream as context and console events.

## Decision

Retain ADR-109's bounded, dedicated CDP connection and acknowledged isolated-world capture markers. Track the main-document incarnation on that stream, and stamp console entries with its identity when their execution context is recognized. Context creation records numeric and system-unique identities; destruction matches the unique identity so a delayed destroy cannot remove a reused numeric context.

When begin and drain observe different document incarnations, exclude entries from the initial incarnation. Retain entries from subsequent incarnations, including detached frames and intermediate navigation documents. For same-document navigation, retain entries after the begin marker. Preserve ordinary console history and report unknown ownership or listener failures explicitly.

## Consequences

- **Positive:** old-document contexts created after the marker remain distinguishable from new-document output; detached and intermediate errors remain attributable.
- **Negative:** attribution depends on ordered context-clear events on the target's Runtime stream, which requires native-browser coverage alongside deterministic event-order tests.
- **Neutral:** the connection, message bounds, lifecycle ownership, and public tool surface remain as selected in ADR-109.

## References

- Supersedes [ADR-109](109_console-capture-uses-ordered-context-identities.md).
- [Browser requirements](../browser-tool/requirements.md), REQ-BT-004.
- [Standalone browser requirements](../browser-cli/requirements.md), REQ-BCLI-007.
- `ConsoleCapture::observe`, `ConsoleCapture::accept`, `BrowserSession::finish_console_navigation_capture`.
