# Restart MCP stdio servers when config args/env/command change

## Problem

Phoenix currently reads MCP server definitions from config files such as `~/.claude.json`, but the reload path only reconciles servers by name:

- removed server names are killed and removed;
- newly added server names are spawned;
- existing server names are treated as `unchanged` even if their `command`, `args`, or `env` changed.

This means a user can edit `~/.claude.json` to change a stdio MCP server's args, click MCP reload, and Phoenix will keep the already-running process with the old args. If that server later crashes, the respawn path still uses the cached `spawn_args`, so the old config can persist until Phoenix itself restarts.

This is surprising and makes the reload affordance incomplete: users reasonably expect config reload to apply edits to an existing server definition.

## Current code shape

Relevant implementation points:

- `crates/phoenix-ide/src/tools/mcp.rs`
  - `McpClientManager::read_all_configs()` reads `~/.claude.json`, Cursor MCP config, project `.mcp.json`, and global MCP config.
  - `McpClientManager::reload()` compares only configured server names with currently connected server names.
  - `McpServer` stores `spawn_command`, `spawn_args`, and `spawn_env` for crash respawn.
  - `McpServer::respawn()` reuses those cached fields, not fresh config.
- `crates/phoenix-ide/src/api/handlers.rs`
  - `POST /api/mcp/reload` calls `state.mcp_manager.reload()` and reports `added`, `removed`, `unchanged`.
- `ui/src/components/McpStatusPanel.tsx`
  - Reload toast only knows about added/removed/unchanged counts.

## Goal

When an MCP stdio server's effective config changes, Phoenix should restart that server and use the new config without requiring a Phoenix process restart.

Changes to any of these fields must count as config changes:

- `command`
- `args`
- `env`

The reload API/UI should make restarts visible enough that users understand the new config was applied.

## Implementation plan

Compare full config in `McpClientManager::reload()` and restart changed servers immediately.

Make `McpServerConfig` comparable (`PartialEq`, probably `Eq`) and teach `McpServer` to expose or compare its current spawn config. During `McpClientManager::reload()`:

1. Read all config entries.
2. Remove servers whose names disappeared.
3. For each configured server name:
   - if absent: spawn as `added`;
   - if present and config identical: keep as `unchanged`;
   - if present and config differs: restart with the new config and report as `restarted`.

Changed-server restart should:

1. Extract the existing server under the `servers` write lock.
2. Abort its stderr drain task.
3. Kill the old child process.
4. Drop the lock before doing any potentially slow connect/initialize/list-tools work.
5. Clear stale pending OAuth state for that server before reconnect.
6. Spawn/connect using the freshly-read `McpServerConfig`.
7. Reinsert the new `McpServer` under the same name after successful initialization.
8. Report the server as `restarted`, not `unchanged`.

Locking is important: do not hold the manager's `servers` lock across child process startup, initialize, OAuth waiting, or `tools/list`.

## Reload result shape

Extend the reload result to distinguish changed-config restarts from unchanged servers.

Preferred shape:

```rust
pub struct McpReloadResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub restarted: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<McpReloadFailure>, // optional but preferable
}
```

Adding `restarted: Vec<String>` is backward-compatible for JSON consumers. A structured `failed` list is preferable if restart/connect failures can happen after the HTTP response waits for changed-server reconnects.

If new servers remain background-spawned, changed-config restarts can either:

- also run in the background and be reported as `restarted` once scheduled; or
- synchronously wait for restart completion so failure can be surfaced accurately.

Prefer synchronous changed-server restart if practical, because the user explicitly asked reload to apply a changed existing server. Keep the lock-free connect pattern either way.

## Failure behavior

Failed restart behavior must be explicit.

Do not silently classify a changed server as `unchanged` if restart fails.

Acceptable failure behavior:

- remove the old stale-process server before attempting restart;
- attempt reconnect with the new config;
- if reconnect fails, leave the server disconnected/pending and report/log the failure.

Avoid keeping the old process alive after detecting a config change unless the API result clearly reports that stale config is still running. The simpler and cleaner behavior is: changed config invalidates the old process.

## Behavioral expectations

- Editing only `args` for an existing server in `~/.claude.json` and calling `/api/mcp/reload` restarts that server with the new args.
- Editing `command` restarts the server with the new command.
- Editing `env` restarts the server with the new env.
- Removing a server still kills/removes it.
- Adding a server still connects it in the background.
- Unchanged servers are not restarted.
- Disabled servers preserve their disabled state across restart; disabled means excluded from conversations, not disconnected forever.
- Pending OAuth state for a restarted server is cleared before reconnect, same as new-server reload currently does.
- If restart/connect fails, Phoenix does not claim the server is unchanged. It surfaces failure in logs and, if feasible, the reload result/status.

## Testing plan

Add focused tests around `McpClientManager::reload()` using lightweight test MCP server commands/scripts already used by existing MCP tests, or introduce a test-only helper server if needed.

Coverage should include:

1. Same name, same config -> `unchanged`, no new process.
2. Same name, changed `args` -> old process terminated, new process spawned, result includes `restarted`.
3. Same name, changed `command` -> restart.
4. Same name, changed `env` -> restart.
5. Removed name -> process killed and result includes `removed`.
6. Added name -> background connection starts and result includes `added`.
7. Crash respawn after a changed-config reload uses the new config, not stale `spawn_args`.

If direct process assertions are hard, use a helper MCP server that exposes the received args/env through a tool response or writes a marker file during initialization.

## UI/API updates

- Update `McpReloadResult` TypeScript type in `ui/src/api.ts` if needed.
- Update `McpStatusPanel` reload toast to include restarted servers, e.g. `↻1 restarted` or `1 restarted`.
- Consider displaying failed reload/restart attempts in the panel if backend returns failures.

## Acceptance criteria

- Changing `args` for an existing stdio MCP server in `~/.claude.json` and clicking reload applies the new args without restarting Phoenix.
- Existing unchanged MCP servers are not unnecessarily restarted.
- Reload response/logs distinguish `restarted` from `unchanged`.
- Crash respawn after reload uses the latest config.
- Automated tests cover changed-config restart behavior.
- `./dev.py check` passes.
