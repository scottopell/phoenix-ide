# ADR-111: Console capture attaches isolated frame targets

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-BT-004, REQ-BCLI-007

## Context

A cross-origin iframe can run in a separate Chrome target. A Runtime subscription to the main page does not receive that frame's console events. Navigation capture must preserve errors from child frames that disappear before the result is assembled, and distinguish their document ownership across navigation.

## Options considered

1. **Subscribe only to the page target:** smallest implementation, but silently omits isolated child-frame errors.
2. **Detect isolated frames and report capture unavailable:** makes the gap visible, but prevents complete navigation diagnostics for applications embedding cross-origin content.
3. **Attach iframe targets on the existing bounded connection:** captures their events while keeping one listener owner and the document-incarnation model from ADR-110.

## Decision

Recursively auto-attach flattened iframe sessions on the console listener's existing connection. Enable Page and Runtime before resuming a newly paused target, so its initial script errors are observable. Scope context identifiers by target session and stamp each context with its owning main-document incarnation. Frame ancestry updates the owner for new contexts in a reused child target without rewriting ownership of earlier contexts or retained entries.

Fence live child streams at capture boundaries with acknowledged Runtime commands, alongside the root source marker. Bound these drains and fail capture explicitly when ownership or delivery cannot be established. Retain collected entries when child targets detach. The listener owns child initialization, command acknowledgements, and teardown; it adds no reconnection or public session surface.

## Consequences

- **Positive:** navigation diagnostics include out-of-process child errors, including frames removed before the result; old-document errors remain distinguishable.
- **Negative:** capture owns target attachment and cross-target barriers in addition to context events. Native tests must prove process isolation and exercise detach and target reuse, rather than relying only on same-process `srcdoc` frames.
- **Neutral:** ADR-110's document attribution, ordinary history retention, and bounded connection remain in force.

## References

- Extends [ADR-110](110_console-capture-tracks-document-incarnations.md).
- [Browser requirements](../browser-tool/requirements.md), REQ-BT-004.
- [Standalone browser requirements](../browser-cli/requirements.md), REQ-BCLI-007.
- `ConsoleCapture::observe_from`, `Listener::enable_target`, `Listener::begin_drain`, `BrowserSession::finish_console_navigation_capture`.
