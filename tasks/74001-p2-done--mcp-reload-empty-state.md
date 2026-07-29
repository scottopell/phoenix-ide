# Keep MCP reload available in the empty state

## Observed journey

- A user with no currently detected MCP servers sees the MCP panel’s empty state but no reload control.
- After adding the first server to a supported MCP config file, the user cannot ask Phoenix to rescan from the UI; they must call `POST /api/mcp/reload` manually or restart Phoenix.

## Verified findings

- `McpStatusPanel` renders its reload action only when `servers.length > 0 || pendingOAuth.length > 0`. Because `pendingOAuth` is derived from `servers`, an empty status response always hides the action.
- The existing reload handler already supports the zero-to-one-server journey through `api.reloadMcp()`, backed by `POST /api/mcp/reload` and `McpClientManager::reload()`.
- The empty panel continues polling status, but `GET /api/mcp/status` does not rescan configuration, so polling cannot discover a newly added first server.
- REQ-MCP-015 requires reload reconciliation and per-server outcomes; the backend behavior is already implemented. The missing piece is access to that behavior from the UI’s zero-server state.
- There is no focused `McpStatusPanel` component test today.

## Failure model

The UI treats reload as an action on an existing server list, but reload is also the discovery action for newly configured servers. Gating it on a non-empty list makes the zero-to-first-server transition unreachable from the UI.

## Proposed scope

1. Make the MCP reload action available whenever the panel is writable, including when `servers` is empty. Preserve read-only behavior, in-flight disabling/spinner behavior, status polling, and existing reload outcome toasts.
2. Add focused component regression coverage that:
   - renders the reload control after an empty MCP status response;
   - activates it and verifies `api.reloadMcp()` is called;
   - confirms the control remains absent in read-only mode.
3. Run the focused UI test and repository validation appropriate to the changed paths (`./dev.py check`, or report any unrelated/environmental failure precisely).
4. Commit the completed change, push the worktree branch, and open a pull request with a concise user-journey-focused description.

## Acceptance criteria

- A writable MCP panel with zero detected servers shows the reload control.
- Clicking the control issues the existing MCP reload request and retains current feedback/polling behavior.
- Read-only views do not expose the mutating action.
- Regression tests cover the empty writable state and read-only state.
- No backend endpoint, config discovery path, or MCP protocol behavior changes.

## Risks and non-goals

- The component owns timers and asynchronous polling, so tests must clean up fake timers and mock API calls to avoid leaks or flaky assertions.
- This task does not add project-aware `.mcp.json` discovery, change supported config paths, change reload reconciliation, or redesign the MCP empty-state copy.
