# MCP Client -- Requirements

## Background

Phoenix is an MCP (Model Context Protocol) **client**. It connects to MCP
servers, discovers the tools they expose, and surfaces those tools to the
conversation runtime as ordinary Phoenix tools named `{server}__{tool}`.

A server is reached over one of two transports:

- **stdio** -- Phoenix spawns the server as a child process and exchanges
  JSON-RPC 2.0 messages over its stdin/stdout.
- **HTTP (Streamable HTTP)** -- Phoenix is an HTTP client of a server that
  exposes a single MCP endpoint, exchanging the same JSON-RPC 2.0 messages
  over POST (and an optional server→client SSE stream over GET).

The two transports differ only below the JSON-RPC layer: the handshake
(`initialize`), tool discovery (`tools/list`), invocation (`tools/call`), and
server notifications (`notifications/tools/list_changed`) are identical across
both. HTTP additionally carries an authorization concern -- remote servers are
frequently protected by OAuth 2.1 -- which stdio servers never have.

## User Stories

### Story 1: Use a remote OAuth-protected MCP server

As a developer, I want to add a remote MCP server (e.g. an Atlassian, Slack, or
Linear endpoint) by URL and authorize it through my browser once, so that its
tools are available to my agent without standing up a local subprocess bridge
or re-authorizing on every reload.

### Story 2: Use a remote server behind a static token

As a developer with an internal MCP server gated by an API key, I want to
supply that token (or arbitrary headers) in config so the server's tools are
available with no interactive auth.

### Story 3: Trust the connection state

As a developer, I want to see which MCP servers are connected, which need
authorization, and which failed and why, so I can tell a misconfigured server
from one that is merely waiting on me.

---

### REQ-MCP-001: Transport-Tagged Config Discovery

THE SYSTEM SHALL read MCP server definitions from the known config files in a
fixed priority order, merging by server name (first-seen wins).

For each `mcpServers` entry THE SYSTEM SHALL select a transport:

- an entry with a `command` field is a **stdio** server
- an entry with `"type": "http"` and a `url` field is an **HTTP** server

WHERE an entry declares neither a usable `command` nor an HTTP `url`
THE SYSTEM SHALL skip it and record the reason at `debug` level.

**Rationale:** The config schema already distinguishes transports via `type`
and `command`/`url`. Discovery is the single place transport selection happens,
so the rest of the system is transport-agnostic above the JSON-RPC layer.

---

### REQ-MCP-002: Transport-Agnostic JSON-RPC Protocol

THE SYSTEM SHALL drive every server through the same protocol sequence
regardless of transport:

- `initialize` handshake advertising client capabilities and protocol version,
  followed by the `notifications/initialized` notification
- `tools/list` with cursor-based pagination to discover tools
- `tools/call` to invoke a tool, extracting text content blocks and honoring
  the result-level `isError` flag
- inbound `notifications/tools/list_changed`, which marks the server's tool
  list stale for refresh

**Rationale:** The protocol is identical across transports. Modeling it once,
above a transport boundary, keeps stdio and HTTP from diverging in handshake,
pagination, or error handling.

---

### REQ-MCP-003: Stdio Transport

WHERE a server is stdio
THE SYSTEM SHALL spawn it as a child process with piped stdin/stdout/stderr,
drain stderr to logs, detect process exit, and respawn-and-reinitialize on a
crash-like I/O failure before retrying the failed call once.

**Rationale:** A local subprocess can crash; transparent respawn keeps its
tools usable without operator intervention. Stderr drain prevents the child
blocking on a full pipe.

---

### REQ-MCP-004: Streamable HTTP Transport

WHERE a server is HTTP
THE SYSTEM SHALL POST JSON-RPC requests to the server's MCP endpoint with an
`Accept` header listing both `application/json` and `text/event-stream`, and
accept a response that is **either** `application/json` (a single JSON-RPC
reply) **or** `text/event-stream` (a sequence of JSON-RPC messages delivered as
SSE events), parsing both into the same JSON-RPC result.

THE SYSTEM SHALL send the negotiated `MCP-Protocol-Version` header on every
request after the `initialize` response.

**Rationale:** The Streamable HTTP transport lets a server answer a single POST
with either a unary reply or a stream. The client advertises both shapes via
`Accept` so a conformant server or gateway can choose framing rather than
rejecting the request; a client that handles only one shape fails against
servers that choose the other.

---

### REQ-MCP-005: HTTP Session Lifecycle

WHEN an HTTP server returns an `Mcp-Session-Id` header on the `initialize`
response
THE SYSTEM SHALL include that session id on the `Mcp-Session-Id` header of every
subsequent request to that server
AND issue an HTTP `DELETE` to end the session on shutdown.

WHEN a request to a session-bearing server returns HTTP 404
THE SYSTEM SHALL treat the session as expired and re-initialize before retrying.

**Rationale:** Session id binds a sequence of requests to server-side state. A
404 is the protocol's signal that the session is gone; silently failing the
call instead of re-initializing would surface as a spurious tool error.

---

### REQ-MCP-006: Server-Initiated Stream and Resumability

WHERE a server is HTTP and supports it
THE SYSTEM SHALL open a server→client SSE stream via GET to receive
server-initiated messages, notably `notifications/tools/list_changed`
AND, on a dropped stream, reconnect supplying the last received event id via
`Last-Event-ID` so the server can replay missed messages.

**Rationale:** Without the GET stream, `tools/list_changed` from an HTTP server
never arrives and the tool list goes stale. Resumability avoids losing
notifications across a transient disconnect.

---

### REQ-MCP-007: HTTP Connection Recovery

WHEN an HTTP request fails with a transport error (connection reset, timeout)
THE SYSTEM SHALL retry the connection rather than treating the server as
permanently failed, distinct from the stdio respawn path which restarts a
process.

**Rationale:** HTTP has no process to respawn; recovery is reconnection. The
recovery trigger and mechanism differ from stdio and must not be conflated.

---

### REQ-MCP-008: Static Token / Header Authentication

WHERE an HTTP server config supplies a bearer token or arbitrary request
headers
THE SYSTEM SHALL attach them to every request to that server.

**Rationale:** Many internal and enterprise servers gate access with a static
API key rather than OAuth. This auth scheme falls out of the transport's header
plumbing and requires no interactive flow.

---

### REQ-MCP-009: OAuth 2.1 Authorization Discovery

WHEN an HTTP request returns HTTP 401 with a `WWW-Authenticate` header
THE SYSTEM SHALL discover the authorization server by:

- locating the **Protected Resource Metadata** (RFC 9728): the
  `resource_metadata` URI from the `WWW-Authenticate` challenge when present,
  otherwise the well-known PRM document at both the endpoint path and the host
  root, to learn the protected resource's authorization server(s)
- fetching the **Authorization Server Metadata** via both OAuth Authorization
  Server Metadata (RFC 8414, `.well-known/oauth-authorization-server`) and
  OpenID Connect Discovery (`.well-known/openid-configuration`) well-known
  endpoints, to learn the authorization, token, and registration endpoints

**Rationale:** OAuth 2.1 for MCP is discovery-driven; the client is not
pre-configured with endpoints. A 401 is the entry point. A server may signal
its metadata via `resource_metadata` or rely on the well-known locations, and
its authorization server may expose only OIDC discovery -- a conformant client
handles all of these rather than assuming one.

---

### REQ-MCP-010: Client Identity Acquisition

THE SYSTEM SHALL obtain a client identity for the authorization server,
preferring in order:

- a pre-configured client id/secret supplied for that authorization server
- a cached client registration previously obtained for that authorization
  server
- a Client ID Metadata Document, where the authorization server supports it
- Dynamic Client Registration (RFC 7591) as the fallback, where the
  authorization server advertises a registration endpoint

A newly obtained registration is persisted, keyed by the authorization server,
and reused across sessions and across MCP servers that share that authorization
server.

**Rationale:** The MCP authorization model prefers pre-registered or
metadata-document client identity and treats DCR as the fallback. Keying the
registration by the authorization server -- not the MCP server -- lets multiple
resources behind one authorization server share a single client identity.

---

### REQ-MCP-011: Authorization Code Flow with PKCE

THE SYSTEM SHALL complete the OAuth 2.1 authorization code flow with PKCE:
verify the authorization server advertises `code_challenge_methods_supported`
(and refuse to proceed if it does not), generate a code verifier/challenge,
surface the authorization URL for the user to open in a browser, receive the
redirect with the authorization code, and exchange the code (plus verifier) for
an access token at the token endpoint.

THE SYSTEM SHALL include the MCP server's canonical URI as the `resource`
parameter (RFC 8707 Resource Indicators) on both the authorization request and
the token request, so the issued token is audience-bound to that server.

THE SYSTEM SHALL perform this flow natively, without delegating to an external
process such as `mcp-remote`.

**Rationale:** PKCE is mandatory under OAuth 2.1; a server that omits
`code_challenge_methods_supported` cannot be used safely, so the client refuses
before the browser round trip rather than failing after it. Resource Indicators
bind the token's audience to the MCP server, which compliant authorization and
resource servers require. Performing the flow natively removes the
npm/subprocess dependency and the browser-popup-on-every-reload behavior of the
external bridge.

---

### REQ-MCP-012: Token Storage, Refresh, Invalidation, and Step-Up

THE SYSTEM SHALL persist OAuth access tokens, refresh tokens, and expiry to the
database
AND reuse a stored, unexpired token on reconnect without re-prompting the user
AND, on expiry or a post-authorization 401, refresh using the refresh token
AND, when refresh fails, discard the stored token and return the server to an
unauthorized state requiring a new authorization
AND, when a tool call returns HTTP 403 `insufficient_scope` with a
`WWW-Authenticate` challenge, re-authorize for the expanded scope and retry
rather than surfacing a permanent tool failure.

**Rationale:** The whole value of native OAuth is silent reconnect. Tokens
survive restarts; refresh keeps a session alive. A failed refresh must discard
the stale token so it cannot be reused or duplicated, and is the condition that
re-prompts the user. A `403 insufficient_scope` is a step-up request, not a
terminal error -- treating it as one would make scope-gated tools permanently
unusable.

---

### REQ-MCP-013: Authorization Status Surfaced to the UI

THE SYSTEM SHALL expose, per server, whether it is connected, awaiting
authorization (with the authorization URL), or failed (with the error), via
`GET /api/mcp/status`.

**Rationale:** A server blocked on authorization must be distinguishable from
one that failed to connect and from one that is healthy. The authorization URL
is surfaced as structured data, not scraped from a subprocess's stderr.

---

### REQ-MCP-014: Tool Exposure and Live Resolution

THE SYSTEM SHALL expose each connected, enabled server's tools to the
conversation runtime under the name `{server}__{tool}`
AND resolve tool definitions live from the manager at LLM-request time, so that
servers finishing connection (or arriving via reload) after a conversation
starts still contribute their tools.

**Rationale:** Snapshotting tools at conversation start makes late-connecting or
reloaded servers invisible. Live resolution keeps the tool list correct without
restarting conversations.

---

### REQ-MCP-015: Config Reload Reconciliation

WHEN MCP config is reloaded
THE SYSTEM SHALL reconcile the running set against the new config: connect added
servers, disconnect removed servers, restart servers whose config changed, and
leave unchanged servers untouched
AND report the per-server outcome (added / removed / restarted / unchanged /
failed).

**Rationale:** Reload must not tear down healthy connections. Reconciliation
applies the minimum change and reports what happened so the operator sees the
effect of an edit.

---

### REQ-MCP-016: Per-Server Enable/Disable

THE SYSTEM SHALL let a server be disabled so its tools are excluded from
conversations while its connection is retained for instant re-enable
AND persist the disabled set across restarts.

**Rationale:** A user may want to silence a redundant server (e.g. a browser
MCP when Phoenix has built-in browser tools) without losing the connection or
re-authorizing on re-enable.

---

### REQ-MCP-017: Tool Call Cancellation and Error Surfacing

WHEN a `tools/call` is cancelled
THE SYSTEM SHALL not corrupt the transport: an in-flight stdio write must
complete rather than be dropped mid-message.

WHEN a server reports a tool error or the call fails
THE SYSTEM SHALL surface it as a tool error to the conversation, not a success.

**Rationale:** Dropping a partial JSON-RPC write mid-frame desynchronizes a
stdio server's input stream. Reporting a failed call as success is the
wrong-state bug class the `ToolOutput` enum exists to prevent.

---

### REQ-MCP-018: Connection Failure Visibility

WHEN a server fails to connect or authorize
THE SYSTEM SHALL retain the failed entry with its error in `GET /api/mcp/status`
rather than dropping it silently
AND clear the error on a successful reconnect.

**Rationale:** A server that vanishes from the status response is
indistinguishable from one that was never configured. Retaining the failure
with its cause is what makes the reload-to-retry loop legible.

---

### REQ-MCP-019: Legacy HTTP+SSE Not Natively Supported

THE SYSTEM SHALL NOT implement the deprecated two-endpoint HTTP+SSE transport
(2024-11-05) natively. Servers speaking only that transport remain reachable by
configuring the `mcp-remote` bridge as a stdio server.

**Rationale:** The Streamable HTTP transport supersedes HTTP+SSE. Native support
for both doubles the HTTP surface for a shrinking set of legacy servers; the
stdio bridge already covers them during their decline.
</content>
