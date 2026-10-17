# ADR-108: Snapshot interactions enforce identity at native input dispatch

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-BT-008, REQ-BT-009, REQ-BT-025

## Context

Snapshot references promise an exact DOM node. A timer or framework render can replace a node between a CDP preflight check and native input dispatch. Accessibility tree rendering limits also apply after Chromiumoxide has already received an unlimited websocket message. Large snapshot artifacts contain page values and need bounded ownership.

## Options considered

1. **Check immediately before coordinate input:** small change, but leaves a reachable replacement race between CDP commands.
2. **Use JavaScript element activation:** targets an exact element atomically, but changes trusted native input and framework behavior.
3. **Guard native events before page handlers:** preserves native input while enforcing identity at the event boundary, with explicit cancellation and navigation handling.

For collection, a global Chromiumoxide websocket limit would affect legitimate large heap and trace captures. A bounded snapshot-only connection isolates the new constraint.

## Decision

Install a narrow reference-input capture guard before page scripts. Arm it with the original node and a unique operation token; reject input targeting another node before application handlers run. Receive click receipts through CDP so page navigation cannot erase proof of valid dispatch. Keep the input lease through cancellation cleanup before allowing another reference action.

Collect snapshots over a dedicated attached CDP connection with an 8 MiB frame and message cap before JSON deserialization. Keep existing text/node rendering limits. This bounds snapshot input to Phoenix; it does not promise a Chrome process memory sandbox.

Own a spilled snapshot file with its reference set. Create an owner-only directory/file and retire it on reference invalidation and teardown. Timeout errors describe possible partial browser effects.

## Consequences

- Dynamic pages cannot turn a stale reference into input on a replacement element between preflight and dispatch.
- Native trusted input, React handlers, and shadow-root interactions retain their behavior.
- Guard installation failure disables reference input visibly; CSS operations remain available.
- Snapshot collection adds one short-lived local CDP connection; oversized trees fail explicitly.
- Snapshot artifacts have the same lifetime as the refs they carry; callers must read them before taking another snapshot.

## References

- [Browser requirements](../browser-tool/requirements.md)
- `ReferenceInputGuard`, `SnapshotArtifact`, `snapshot::collection`
