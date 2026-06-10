# MCP Client -- Design

## Behavioral Specification

The complete behavioral contract -- the per-server connection lifecycle, the
HTTP OAuth authorization sub-lifecycle, and the invariants binding them -- is
defined in `specs/mcp/mcp.allium`. This document covers the implementation
approach; the Allium spec is authoritative for what the system does.

The MCP client lives in `crates/phoenix-tools/src/mcp.rs`: `McpClientManager`
owns all connections, `McpServer` owns one connection, and `McpTool` adapts a
discovered tool to the `Tool` trait. The manager is constructed in
`crates/phoenix-ide/src/main.rs` and shared on `AppState`; tool definitions are
resolved live through it (`ToolRegistryExecutor`, `create_mcp_tool_by_name`).

---

## Transport Boundary

The protocol layer (`initialize`, `tools/list` pagination, `tools/call`,
notification handling, JSON-RPC id correlation) is identical across transports.
The transport layer (how a request's bytes leave and a response's bytes arrive)
is not. The design separates them with an `McpTransport` trait:

```rust
trait McpTransport: Send + Sync {
    async fn request(&self, method: &str, params: Value, timeout: Duration)
        -> Result<Value, String>;
    async fn notify(&self, notification: &Value) -> Result<(), String>;
    // Health / recovery is transport-specific (process exit vs connection drop).
}
```

`StdioTransport` holds the `Child` + the stdin/stdout mutexes that serialize a
stdio round trip. `HttpTransport` holds a `reqwest::Client`, the endpoint URL,
the session id, and the resolved auth. `McpServer` becomes transport-agnostic:
it owns a `Box<dyn McpTransport>`, the cached `Vec<McpToolDef>`, the
`tools_changed` flag, and the per-server name -- the protocol methods
(`initialize`, `list_tools`, `call_tool`) move to operate over the trait.

### Why a trait and not an enum

An enum (`Stdio(..) | Http(..)`) would force every protocol method to `match`
on the variant. The protocol methods are transport-agnostic by construction --
the whole point of REQ-MCP-002 -- so they should not see the variant at all.
The trait keeps the transport-specific code (spawn vs connect, respawn vs
reconnect) in the impls and the protocol code variant-free.

### Concurrency

Stdio serializes: one request-response pair at a time, because a single pipe
carries interleaved bytes and the reader must match the writer. `StdioTransport`
keeps the existing "lock stdin+stdout for the round trip" discipline.

HTTP does not serialize: each POST is an independent request correlated to its
response by HTTP itself, and JSON-RPC ids correlate messages within a stream.
`HttpTransport::request` issues concurrent POSTs without a per-server lock. The
serialization comment that documents the stdio constraint must not migrate to
the HTTP impl.

---

## Config as a Sum Type

`McpServerConfig` becomes an enum:

```rust
enum McpServerConfig {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Http  { url: String, headers: HashMap<String, String>, auth: HttpAuth },
}

enum HttpAuth { None, Static, OAuth }  // Static: token/headers already in `headers`
```

`read_all_configs` classifies each `mcpServers` entry into a variant
(REQ-MCP-001). The reload reconciler compares configs with `PartialEq` to
decide unchanged-vs-restart; the comparison extends to the HTTP variant so a
changed URL, header set, or auth scheme triggers a restart
(`reload_from_configs`, REQ-MCP-015).

The `Skipping HTTP transport` branch in `read_all_configs` is removed: HTTP
entries become `Http` configs instead of being dropped.

---

## Connection Lifecycle

A server moves through `connecting → ready`, with `reconnecting` and `failed`
as recovery/terminal states and `disabled` orthogonal to all of them. The
unified states (modeled in `mcp.allium` as `ConnState`) are:

- **connecting** -- spawning/connecting, `initialize`, first `tools/list`
- **ready** -- handshake complete, tools cached, available to conversations
- **reconnecting** -- stdio crash → respawn (REQ-MCP-003), or HTTP transport
  error / session 404 → reconnect-or-reinitialize (REQ-MCP-005, REQ-MCP-007)
- **unauthorized** -- HTTP 401; the OAuth sub-lifecycle drives recovery
  (REQ-MCP-009..012)
- **failed** -- connection or authorization gave up; entry retained with the
  error for the UI (REQ-MCP-018)
- **disabled** -- tools excluded from conversations, connection retained
  (REQ-MCP-016)

`reconnecting` unifies the two recovery paths at the state level while keeping
their triggers and mechanisms distinct at the transport level -- a crash-like
stdio I/O error respawns a process; an HTTP transport error reconnects a client.

---

## HTTP Authorization

Static auth (REQ-MCP-008) needs no lifecycle: the token/headers from config are
attached on every request and a server with valid static auth goes straight to
`ready`.

OAuth (REQ-MCP-009..012) is a sub-lifecycle entered on a 401, modeled in
`mcp.allium` as `OAuthPhase`:

1. **discovering** -- 401 → Protected Resource Metadata (RFC 9728) →
   Authorization Server Metadata (RFC 8414).
2. **registering** -- if no client registration is cached, Dynamic Client
   Registration (RFC 7591); the client id is persisted and reused.
3. **awaiting_user** -- an authorization URL (auth-code + PKCE) is surfaced via
   `/api/mcp/status`; the user opens it in a browser. Phoenix receives the
   redirect with the code on a local callback route.
4. **authorized** -- the code + PKCE verifier are exchanged for tokens at the
   token endpoint; tokens are persisted and the connection retries to `ready`.
5. **refreshing** -- on expiry or a post-authorization 401, the refresh token is
   exchanged for a new access token; failure returns the server to
   `unauthorized`.

The native flow produces the authorization URL as structured state
(`pending_auth_url`), rather than recovering it by watching an `mcp-remote`
child's stderr for a line containing `https://` -- the structured URL is what
the status API and UI consume.

### Token store

A `mcp_oauth_tokens` table holds `(server_name, client_id, access_token,
refresh_token, expires_at)`, alongside the existing `mcp_disabled_servers`
table (`crates/phoenix-db/src/lib.rs`). Tokens are stored in plaintext,
consistent with how the database already holds operator state; the database
file's on-disk protection is the trust boundary, not per-row encryption. A
stored, unexpired token lets reconnect skip the browser flow entirely
(REQ-MCP-012).

---

## Tool Exposure and Lifecycle

Tool exposure is unchanged by transport. The manager yields
`(server_name, McpToolDef)` pairs for connected, enabled servers
(`tool_definitions`); `create_mcp_tool_by_name` adapts one to an `McpTool`;
`tools/list_changed` flips `tools_changed`, refreshed lazily on the next
`tool_definitions` read (REQ-MCP-014). `McpTool::run` spawns the call as a
detached task and selects on the cancellation token so a cancel cannot drop an
in-flight write (REQ-MCP-017). HTTP servers ride all of this with no change --
they produce the same `McpToolDef`s and route through the same `call_tool`.

Reload reconciliation (`reload_from_configs`) and enable/disable
(`disable_server` / `enable_server`, persisted via the DB) are transport-
agnostic and operate on the unified `ConnState` (REQ-MCP-015, REQ-MCP-016).

---

## Status API and UI

`GET /api/mcp/status` returns one entry per server carrying its state, tool
list, enabled flag, and -- for OAuth servers mid-flow -- the authorization URL,
or -- for failed servers -- the error (REQ-MCP-013, REQ-MCP-018). The
`McpStatusPanel` (`ui/src/components/McpStatusPanel.tsx`) renders connected,
authorization-pending (with a clickable authorize affordance), and failed
states distinctly. The reload control retries failed and pending servers.

---

## Dependencies

The HTTP transport needs an HTTP client and an SSE reader in `phoenix-tools`.
`reqwest` (rustls) is already a workspace dependency used by `phoenix-ide`;
it is added to `phoenix-tools` for `HttpTransport`. The server→client SSE
stream (REQ-MCP-006) is read either with an `eventsource`-style line parser
over `reqwest`'s byte stream or a hand-rolled SSE framer; the same parser
handles `text/event-stream` POST responses (REQ-MCP-004).

---

## Relationship to the Existing Tool System

MCP tools are the only tools resolved live rather than from the static
`ToolRegistry` (`Tool` trait, `crates/phoenix-tools/src/tools.rs`). HTTP support
adds a transport beneath the existing MCP layer; it does not widen the tool
feature surface. MCP **resources** and **prompts**, and server→client
**sampling**/**elicitation**/**roots**, are out of scope -- the client consumes
**tools** only, identically across transports.
</content>
