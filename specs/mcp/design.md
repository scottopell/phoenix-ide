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
    // `sink` receives any server-initiated JSON-RPC messages (requests or
    // notifications) that arrive on the same channel before the matching
    // response -- e.g. notifications/tools/list_changed or progress on a
    // text/event-stream POST reply. The transport frames and forwards them;
    // it does not interpret them. `request` returns only the correlated result.
    async fn request(&self, method: &str, params: Value, timeout: Duration,
                     sink: &dyn ServerMessageSink)
        -> Result<Value, TransportError>;
    async fn notify(&self, notification: &Value) -> Result<(), TransportError>;
    // Health / recovery is transport-specific (process exit vs connection drop).
}

// Failures the lifecycle dispatches on must be typed, not stringly-encoded.
// Each variant maps to a distinct ConnState/OAuthPhase transition, so the
// transport classifies the failure once and the state machine never
// string-matches to recover it. String payloads are human-readable detail
// for logs and surfaced errors; dispatch is on the variant alone.
enum TransportError {
    Unauthorized { www_authenticate: Option<String> }, // 401 -> OAuth discovery
    InsufficientScope { www_authenticate: Option<String> }, // 403 -> step-up
    SessionExpired,                                     // HTTP 404 -> re-initialize
    Disconnected(String),                               // reset/EOF -> reconnect/respawn
    Timeout(String),                                    // deadline elapsed -> per-transport policy
    Rpc { code: i64, message: String },                 // JSON-RPC error result
    Protocol(String),                                   // malformed frame, etc.
}
```

A timeout is classified apart from `Disconnected` because the two demand
different recovery under stdio: `Disconnected` evidences a dead pipe/process
and triggers the respawn path (`StdioCrashed` in `mcp.allium` requires process
exit), while a deadline elapsing against a live-but-slow server is surfaced as
the call's error without killing the server. The HTTP transport, by contrast,
treats an elapsed deadline as a reconnectable transport error (REQ-MCP-007).

`StdioTransport` holds the `Child` + the stdin/stdout mutexes that serialize a
stdio round trip. `HttpTransport` holds a `reqwest::Client`, the endpoint URL,
the session id, and the resolved auth. `McpServer` is transport-agnostic: it
owns a `Box<dyn McpTransport>`, the cached `Vec<McpToolDef>`, the
`tools_changed` flag, the per-server name, and the `McpServerConfig` it was
built from (the reload comparison key, and the recipe for rebuilding the
transport on respawn) -- the protocol methods (`initialize`, `list_tools`,
`call_tool`) operate over the trait.

The `ServerMessageSink` keeps protocol dispatch in the protocol layer: a
`text/event-stream` POST reply may carry server-initiated requests/notifications
ahead of the response, and the transport forwards them to the sink rather than
swallowing them or growing its own `tools/list_changed` handling (REQ-MCP-002,
REQ-MCP-004). The same sink backs the GET stream (REQ-MCP-006).

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

// Auth credential, distinct from `headers`. `headers` are generic per-request
// headers (org id, beta flag, …) attached under ANY scheme; they do not imply
// auth and must not preempt OAuth.
enum HttpAuth {
    None,                  // no credential; a 401 starts OAuth discovery
    Static(StaticCred),    // an explicit config credential (see below)
    OAuth(Option<PreconfiguredClient>),  // OAuth; client may be pre-configured
}

// An explicit, config-supplied auth credential -- a 401 against it is a hard
// StaticAuthRejected, never an OAuth flow. Covers both shapes Story 2 / REQ-
// MCP-008 allow: a bearer token, or one/more named auth headers (API key).
enum StaticCred {
    Bearer(String),
    Headers(HashMap<String, String>),  // auth headers, NOT the generic `headers`
}

// A pre-configured OAuth client for an authorization server that disables DCR.
// It is registration *metadata*, not a credential: it seeds the
// mcp_oauth_registrations row so a later 401 reuses it instead of attempting
// DCR. It does NOT pre-authorize the server -- there is still no token until
// the flow completes (REQ-MCP-010).
struct PreconfiguredClient {
    auth_server: String,
    client_id: String,
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
}
```

`read_all_configs` classifies each `mcpServers` entry into a variant
(REQ-MCP-001). The JSON shape of an HTTP entry is `{"type": "http", "url":
..., "headers": {...}, "auth": ...}` where `auth` is absent (`HttpAuth::None`),
`{"bearer": "<token>"}` or `{"headers": {...}}` (the two `StaticCred` shapes),
or `{"oauth": true}` / `{"oauth": {...}}` (`HttpAuth::OAuth`). The `oauth`
object pre-configures a client for a DCR-less authorization server:
`auth_server` + `client_id` are required together, `client_secret` and
`token_endpoint_auth_method` (default `none`) optional -- a partial
pre-configured client skips the server rather than falling back to the DCR the
user said is unavailable. An entry whose `auth` has an unrecognized shape is
skipped at `debug` rather than downgraded to no-auth -- silently dropping an
intended credential would change which authorization path a 401 takes. Crucially, presence of the generic `headers` map alone does
**not** make a server `Static`: only an explicit auth credential
(`StaticCred`) does. So three cases are distinct: a header-authed internal
server (`Static(Headers)`) whose rejected key yields `StaticAuthRejected`; an
OAuth server that *also* needs a non-auth header (the header rides every request
under `OAuth` while the 401 still drives the flow); and an OAuth server behind a
DCR-less authorization server (`OAuth(Some(PreconfiguredClient))`). A config
`OAuth` server is **not** materialized pre-authorized: unless a usable stored
token exists, it starts with no credential (`auth_scheme = none` in
`mcp.allium`) and the `PreconfiguredClient` only seeds the registration row, so
the first 401 runs `OAuthRequired → discovering → OAuthClientReused` (skipping
DCR) rather than being stuck classified as already-authorized. The reload
reconciler compares configs with `PartialEq` to decide unchanged-vs-restart; the
comparison extends to the HTTP variant so a changed URL, header set, or auth
scheme triggers a restart (`reload_from_configs`, REQ-MCP-015).

The `Skipping HTTP transport` branch in `read_all_configs` is removed: HTTP
entries become `Http` configs instead of being dropped.

---

## Connection Lifecycle

A server moves through `connecting → ready`, with `reconnecting` and `failed`
as recovery/terminal states. The unified states (modeled in `mcp.allium` as
`ConnState`) are:

- **connecting** -- spawning/connecting, `initialize`, first `tools/list`
- **ready** -- handshake complete, tools cached, available to conversations
- **reconnecting** -- stdio crash → respawn (REQ-MCP-003), or HTTP transport
  error / session 404 → reconnect-or-reinitialize (REQ-MCP-005, REQ-MCP-007)
- **unauthorized** -- HTTP 401; the OAuth sub-lifecycle drives recovery
  (REQ-MCP-009..012)
- **failed** -- connection or authorization gave up; entry retained with the
  error for the UI (REQ-MCP-018)

Enable/disable is **not** a `ConnState` value: a server can be `ready` and
disabled simultaneously. It is modeled as the orthogonal `enabled` field
(`mcp.allium`), gating tool exposure while the underlying connection is retained
for instant re-enable (REQ-MCP-016).

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

1. **discovering** -- 401 → Protected Resource Metadata (RFC 9728, from the
   `resource_metadata` challenge or the well-known locations) → Authorization
   Server Metadata (RFC 8414 or OIDC discovery).
2. **registering** -- acquire a client identity for the authorization server,
   preferring a pre-configured or cached registration (keyed by authorization
   server), then a Client ID Metadata Document, then Dynamic Client
   Registration (RFC 7591) as the fallback; a new registration is persisted and
   reused.
3. **awaiting_user** -- an authorization URL (auth-code + PKCE, with the
   `resource` indicator) is surfaced via `/api/mcp/status`; the user opens it in
   a browser. Phoenix receives the redirect with the code on a local callback
   route, `GET /api/mcp/oauth/callback`, which is exempt from password auth: a
   browser redirect carries no Phoenix credential, and the unguessable `state`
   nonce already binds the request to its pending flow.
4. **authorized** -- the code + PKCE verifier are exchanged for tokens at the
   token endpoint; tokens are persisted and the connection retries to `ready`.
5. **refreshing** -- on expiry or a post-authorization 401, the refresh token is
   exchanged for a new access token; failure discards the stored token and
   returns the server to `unauthorized`.

The native flow produces the authorization URL as structured state
(`pending_auth_url`), rather than recovering it by watching an `mcp-remote`
child's stderr for a line containing `https://` -- the structured URL is what
the status API and UI consume.

### Redirect origin (REQ-MCP-020)

The `redirect_uri` baked into the authorization request, the DCR registration,
and the token exchange must name an origin the operator's browser can reach and
that routes back to this instance's `GET /api/mcp/oauth/callback`. That origin
is the **canonical external origin**, resolved once at startup
(`resolve_external_origin`) and held on the `OAuthRuntime`:

- an explicit `PHOENIX_EXTERNAL_URL` override, otherwise
- `{scheme}://{host}:{port}` where the scheme follows TLS presence, the host is
  the reachable domain the operator already configures for the certificate
  (`ConfigSource::external_host` -- the first non-loopback TLS host), falling
  back to the bind-derived host (loopback for an unspecified/loopback bind, the
  bare IP otherwise), and the default port for the scheme is dropped.

Binding the redirect origin to the TLS host configuration means a self-hosted
remote deployment sets its reachable domain once. The origin is taken from
trusted configuration, never from request-controlled `Host`/`Forwarded`
headers: a forged header would otherwise redirect an authorization code to an
attacker-chosen destination, a target the `state` nonce and `iss` check do not
defend. Consequently there is no trusted-proxy flag or origin allowlist -- the
attack surface those would guard does not exist. An all-interfaces bind that
still resolves to loopback has no reachable name configured; this is surfaced at
startup so the operator supplies one before a remote authorization round trip
fails.

### Token store

Two tables back the OAuth flow, alongside the existing `mcp_disabled_servers`
table (`crates/phoenix-db/src/lib.rs`):

- `mcp_oauth_registrations` -- `(auth_server, client_id, client_secret?,
  token_endpoint_auth_method)`, keyed by the authorization server so resources
  sharing one authorization server share a client identity (REQ-MCP-010,
  `OAuthRegistration`). The secret/auth method are persisted alongside the id:
  a confidential client recovered after restart needs them to authenticate at
  the token endpoint for code exchange and refresh.
- `mcp_oauth_tokens` -- `(server_name, resource_uri, scopes, access_token,
  refresh_token, expires_at)`, looked up by MCP server name but carrying the
  canonical `resource_uri` the token is audience-bound to (`OAuthToken`). The
  granted `scopes` are persisted so a post-restart `insufficient_scope` step-up
  can request the union of prior and challenged scopes. The client id is **not**
  duplicated here; it lives in the registration table.

Tokens are stored in plaintext, consistent with how the database already holds
operator state; the database file's on-disk protection is the trust boundary,
not per-row encryption. A stored, unexpired token lets reconnect skip the
browser flow entirely -- but only when its `resource_uri` still matches the
server's configured URL. Because the config key (server name) is mutable while
the token's audience is not, the reload path discards the token when the URL or
authorization server changes, so a renamed-or-repointed server never sends an
old credential to a new endpoint. A successful refresh persists any rotated
refresh token; a failed refresh discards the row (REQ-MCP-012).

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

`GET /api/mcp/status` returns one entry per server carrying its `state`
(`ready`/`unauthorized`/`failed`), `transport`, `auth`, tool list, enabled flag,
and -- for OAuth servers mid-flow -- the authorization URL, or -- for failed
servers -- the `last_error` (REQ-MCP-013, REQ-MCP-018). The status unions three
sources: connected servers (`ready`), `pending_oauth_urls` (`unauthorized`), and
`failed_servers` (`failed`), deduped by name with precedence
`ready > unauthorized > failed` so a server that has since reconnected or is
awaiting auth is never also reported failed. `failed_servers` is the retention
map -- a give-up at any connect/handshake/reestablish site records the cause
through `record_connect_failure` (which leaves an awaiting-auth server out, as
its pending URL makes it `unauthorized`), and the entry clears on the next
successful (re)connect and on config removal. The `McpStatusPanel`
(`ui/src/components/McpStatusPanel.tsx`) renders ready, authorization-pending
(yellow, with a clickable authorize affordance), and failed (red, with the
error) distinctly. The reload control retries failed and pending servers.

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
`ToolRegistry` (`Tool` trait, `crates/phoenix-tools/src/lib.rs`). HTTP support
adds a transport beneath the existing MCP layer; it does not widen the tool
feature surface. MCP **resources** and **prompts**, and server→client
**sampling**/**elicitation**/**roots**, are out of scope -- the client consumes
**tools** only, identically across transports.
</content>
