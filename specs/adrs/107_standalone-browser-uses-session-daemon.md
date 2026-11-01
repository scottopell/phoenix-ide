# ADR-107: Standalone browser commands share a local session daemon

- **Status:** Accepted
- **Date:** 2026-10-16
- **Affects:** REQ-BCLI-001–012

## Context

Coding harnesses need Phoenix browser interactions and profiling across separate shell commands. Browser state and active captures outlive an individual invocation. Phoenix's browser engine already manages Chrome, references, console capture, and profiling; its ToolContext also carries unrelated conversation capabilities.

## Options considered

1. **One browser per command:** simple process ownership, but loses page and capture state between commands.
2. **MCP server:** persistent process and tool discovery, but requires harness configuration and does not supply the desired session identity granularity by itself.
3. **CLI and local session daemon:** shell-friendly calls with a persistent browser owner, at the cost of local startup coordination and cleanup.

## Decision

Use a CLI plus a private Unix-socket daemon per selected session. An exclusive OS file lock fences daemon ownership, a separate startup lock serializes concurrent launches, and the daemon serializes operations. Only navigation creates a fresh browser. Active operations suspend idle expiry; explicit close and idle expiry use the existing browser manager teardown.

Resolve the session from an explicit flag, then a harness environment binding, then the canonical Git worktree root or current directory. Give each daemon and Chrome launch separate lifetime identities. A failed request transport is never automatically replayed.

Build the executable in phoenix-tools, where existing interaction and profiling operations live. A narrow BrowserAccess interface supplies an already authorized session independently of ToolContext. This reuses operation schemas and implementations without moving tool output types into the engine or introducing another transport abstraction.

## Consequences

- Separate invocations preserve console and profiling state; worktree defaults need no setup.
- Harnesses sharing a worktree must bind different session names when isolation matters.
- The first executable supports macOS and Linux; Windows needs another local transport.
- Idle timeout defaults to 30 minutes and is selected at daemon startup. A caller can inspect effective configuration with status.
- No cross-version daemon protocol or recovery of a browser after an ungraceful daemon death is promised. Close sessions before replacing the executable. Unexpected termination may leave residual profiles; cleanup never imports them into a new session.
- The executable links phoenix-tools dependencies; further crate extraction can be justified independently by measured packaging costs.

## References

- [Standalone browser requirements](../browser-cli/requirements.md)
- [Lifecycle](../browser-cli/browser-cli.allium)
- [Browser engine requirements](../browser-tool/requirements.md)
- [Explicit compatibility policy](034_compatibility-guarantees-are-explicit-and-data-aware.md)
- `BrowserAccess`, `BrowserOperation`, `BrowserSessionManager::shutdown_all`
