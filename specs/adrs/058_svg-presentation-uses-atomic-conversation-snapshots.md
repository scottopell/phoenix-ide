# ADR-058: SVG presentation uses atomic conversation snapshots

- **Status:** Accepted
- **Date:** 2026-09-19
- **Affects:** REQ-SVG-001 through REQ-SVG-007

## Context

Agents need to present code-generated charts inline without large markup tool arguments. Phoenix's attachment directory is scratch storage, and a browser may run on a different machine. Explore can delete its command scratch root, including its temporary-directory fallback, when a command finishes. Tool results already have durable transcript identity and readable text across web and native clients.

## Options considered

1. Render SVG Markdown fences or HTML: convenient transcription, but obscures publication identity, increases model context, and creates an active-content boundary.
2. Reference staging files: simple but breaks after cleanup, scope retirement, and remote access.
3. Store accepted immutable SVG in a conversation-owned database row: bounded bytes and ownership commit together, and existing deletion/replay mechanisms suffice.

## Decision

Use a file-based `present_svg` tool with strict XML-aware static validation, an immutable SQLite BLOB snapshot, and a unique conversation/tool-invocation association. Return compact ordinary tool-result JSON, preserving unsupported-client fallback. Serve bytes through owner-matched authenticated endpoints; use image-context rendering and download disposition plus sandbox CSP. Source is escaped text.

Expose the tool in Direct/Work and Work subagents. Do not expose Explore until a separate decision establishes a reliable staging workflow. Generate SVG through code or chart libraries; explicitly reject unsupported features instead of changing visual content.

The publication is one atomic database operation, without a separate file-store lifecycle or a new artifact framework. Requirements and transaction/replay tests specify this bounded operation; an additional Allium state machine would not introduce a distinct lifecycle to model.

## Consequences

Snapshots survive staging cleanup, restart, and worktree retirement. Transcript deletion cascades snapshots. Replaying a committed invocation succeeds even after source deletion. A cancellation racing commit can suppress the success result while leaving a valid conversation-owned snapshot. Byte caps constrain per-artifact storage, and the database's normal backup/retention boundary includes these bytes.

Unsupported SVG features require regeneration. Chart-library defaults that include DOCTYPE or metadata require safe export configuration or explicit removal before validation. Neither parser acceptance nor publication claims visual inspection.
