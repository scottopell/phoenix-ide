# ADR-115: Successful positioning settles geometry without retaining command capability

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-VT-003, REQ-VT-006, REQ-VT-007

## Context

The physical executor can finish a semantic start command before its header mounts. A subsequent measurement must preserve the successful start anchor, including its bounded native overshoot correction. The parent removes command authority after an applied result. Treating that removal identically to supersession discards the geometric intent; retaining active command authority instead would allow obsolete command work.

Retained artifact tracing identifies a late header-ref continuity write after the initial start execution. A real-reducer parent regression separately reproduces loss of directional geometry after applied completion and authority removal. The trace does not directly observe the authority value, so it does not alone prove the retirement event.

## Options considered

1. Retain command authority until layout stops changing. This leaves command lifetime unbounded and conflates success with active positioning.
2. Retire all directional geometry on every terminal result. This loses the successful anchor during later measurements.
3. Explicitly settle successful geometry, scoped to the current history view, while ending command ownership. Retire it independently on takeover or scope change.

## Decision

Choose explicit settlement. Before notifying its parent of an applied result, the executor settles the matching authorized physical owner. Settlement creates no command, preserves no unresolved target intent, and emits no terminal result. The same-view idle state may preserve the settled geometric anchor through measurements. Cancellation, replacement authority, view-identity changes, user takeover, and detach retire it. Supersession and missing targets do not settle.

Directional correction remains the existing single bounded correction; this decision does not relax visibility, readiness, continuity tolerance, or quantization assumptions.

## Consequences

- **Positive:** Late measurement can preserve successful geometry without reviving command capability.
- **Negative:** The executor needs explicit success/takeover handoffs and current view identity even when no command is active.
- **Neutral:** Browser qualification remains necessary; source regressions and artifact diagnosis are distinct evidence.

## References

- `specs/virtual-transcript/requirements.md`
- `specs/virtual-transcript/virtual_transcript.allium`
- `VirtualTranscriptHandle::completePosition`, `VirtualTranscriptHandle::interruptPosition`
- `MessageList` finish-effect execution
