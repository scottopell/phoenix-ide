# MCP Client -- Executive Summary

## Overview

Phoenix is an MCP client. It connects to MCP servers, discovers their tools,
and exposes them to the conversation runtime as `{server}__{tool}`. Servers are
reached over **stdio** (child process) or **HTTP (Streamable HTTP)**. The HTTP
transport adds an OAuth 2.1 authorization concern absent from stdio.

The stdio transport, tool exposure, reload reconciliation, and enable/disable
are established. The HTTP transport and its authorization are the build-out this
spec set scopes; the value driver is reaching OAuth-protected remote servers
natively, without the `mcp-remote` subprocess bridge.

## Status

| Requirement | Title | Status | Notes |
|---|---|---|---|
| REQ-MCP-001 | Transport-Tagged Config Discovery | Complete | `read_all_configs` merges entries first-seen-wins; `classify_config_entry` tags each as `Stdio` (a `command`) or `Http` (`type: "http"` + `url`, with generic `headers` and an optional `auth` credential), skipping unusable entries at `debug`. |
| REQ-MCP-002 | Transport-Agnostic JSON-RPC Protocol | Complete | Protocol (initialize / paginated `tools/list` / `tools/call` / notification handling via `ServerMessageSink`) lives on `McpServer` over the `McpTransport` trait; failures are the typed `TransportError`. `StdioTransport` is the sole impl until M2. |
| REQ-MCP-003 | Stdio Transport | Complete | `StdioTransport::spawn` / `McpServer::initialize` / `list_tools`; crash detection (`is_alive`, `TransportError::Disconnected` classification) + `respawn` in `mcp.rs`. |
| REQ-MCP-004 | Streamable HTTP Transport | Complete | `HttpTransport` (`mcp/http.rs`): POST with the dual `Accept` pair, unary-JSON and SSE-stream response handling (`SseFramer`), `MCP-Protocol-Version` echoed after `initialize`, 202-with-empty-body accepted for notifications. |
| REQ-MCP-005 | HTTP Session Lifecycle | Complete | `Mcp-Session-Id` captured at `initialize` and echoed on every request; DELETE on shutdown (`HttpTransport::shutdown`); 404 → `TransportError::SessionExpired` → re-initialize before one retry. |
| REQ-MCP-006 | Server-Initiated Stream and Resumability | Complete | `ServerStream` (`mcp/http.rs`): a detached GET (`Accept: text/event-stream`, bearer + session + protocol-version) opened once `initialize` negotiates, feeding server-initiated messages to the shared `NotificationSink`; reconnect resumes via `Last-Event-ID` with capped backoff; aborted on `shutdown`/`Drop`. |
| REQ-MCP-007 | HTTP Connection Recovery | Complete | `McpServer::should_reestablish`: HTTP recovers from `Disconnected`/`Timeout`/`SessionExpired` by rebuilding the client + handshake, distinct from the stdio respawn (which recovers only from `Disconnected`). |
| REQ-MCP-008 | Static Token / Header Authentication | Complete | `HttpAuth::Static` (bearer or designated auth headers) attached to every request; generic `headers` ride every request without classifying the server. The 401-versus-OAuth routing distinction becomes observable when the OAuth flow exists (M3). |
| REQ-MCP-009 | OAuth 2.1 Authorization Discovery | Complete | `mcp/oauth.rs`: 401 challenge parsing → PRM (`resource_metadata` or the path-aware/root well-knowns, RFC 9728) → AS metadata via both RFC 8414 and OIDC discovery forms. A resource-advertised authorization server's self-declared issuer is accepted even when it differs from the fetch URL (`IssuerTrust::ResourceAdvertised`); a directly-named issuer is held to exact equality. |
| REQ-MCP-010 | Client Identity Acquisition | Complete | Cached registrations keyed by authorization server are reused; a pre-configured public client (Claude Code's top-level `oauth.clientId`, no secret — PKCE only) seeds the registration once discovery resolves the issuer; RFC 7591 DCR is the fallback. Phoenix hosts no Client ID Metadata Document, so that step resolves to nothing (logged at `debug`) and falls through to DCR. |
| REQ-MCP-011 | Authorization Code Flow with PKCE | Complete | Native flow in `mcp.rs` (`begin_oauth_flow` / `complete_oauth_authorization`): S256 PKCE (refused when not advertised), unguessable `state` bound to the pending flow, `iss` validation, RFC 8707 `resource` on both requests, callback at `GET /api/mcp/oauth/callback`. |
| REQ-MCP-012 | Token Storage, Refresh, Invalidation, and Step-Up | Complete | `mcp_oauth_registrations` + `mcp_oauth_tokens` (phoenix-db migration 22, plaintext); bearer on every request via the shared cell; silent restore (resource-matched, unexpired-or-refreshable); refresh with rotation persisted; refresh rejection discards and re-prompts; 403 `insufficient_scope` steps up with the scope union while the triggering call replays via the recovery claim. |
| REQ-MCP-013 | Authorization Status Surfaced to the UI | Complete | `GET /api/mcp/status` carries each server's `state` (`ready`/`unauthorized`/`failed`), `transport`, `auth`, and the native flow's structured `pending_oauth_url` (the stdio `mcp-remote` path still feeds the same map from its stderr drain). |
| REQ-MCP-014 | Tool Exposure and Live Resolution | Complete | `tool_definitions` / `create_mcp_tool_by_name`; live resolution via `ToolRegistryExecutor`. HTTP servers ride this unchanged. |
| REQ-MCP-015 | Config Reload Reconciliation | Complete | `reload_from_configs` (add / remove / restart / unchanged / failed); the `PartialEq` comparison spans the `Stdio \| Http` config variants. |
| REQ-MCP-016 | Per-Server Enable/Disable | Complete | `disable_server` / `enable_server`; persisted in `mcp_disabled_servers` (`crates/phoenix-db/src/lib.rs`). |
| REQ-MCP-017 | Tool Call Cancellation and Error Surfacing | Complete | `McpTool::run` spawns a detached task + selects on the cancel token; `tools/call` honors `isError`. |
| REQ-MCP-018 | Connection Failure Visibility | Complete | A give-up at any connect/handshake/reestablish site records the cause in `failed_servers` (parallel to `pending_oauth_urls`) via the shared `record_connect_failure`; `status()` retains it as `state = failed` with `last_error`, cleared on the next successful (re)connect and on config removal. A server still awaiting auth is `unauthorized`, not failed. `McpStatusPanel` renders the three states distinctly. |
| REQ-MCP-019 | Legacy HTTP+SSE Not Natively Supported | Complete | By decision; `legacy_sse_native = false`. Such servers use the `mcp-remote` stdio bridge. |
| REQ-MCP-020 | OAuth Redirect Origin Resolution | Complete | The redirect base is the canonical external origin: `PHOENIX_EXTERNAL_URL` override, else derived from the TLS host config (`ConfigSource::external_host`, first non-loopback host in order) + scheme + bind port (`resolve_external_origin`). Derived from trusted config, not request headers. A cached DCR client registered with a different `redirect_uri` is re-registered (`acquire_client_registration`; registrations carry the redirect_uri, phoenix-db migration 23). An all-interfaces bind that still resolves to loopback warns at startup and on every `unauthorized` status entry (`auth_redirect_warning`). |

## Milestones

This spec set is M0 of the native HTTP MCP build-out. The milestones:

- **M1** (done) -- Extract the `McpTransport` trait; turn `McpServerConfig`
  into a `Stdio | Http` enum. No behavior change (REQ-MCP-002, REQ-MCP-015).
- **M2** (done) -- Streamable HTTP transport substrate with static/no auth
  (REQ-MCP-001, -004, -005, -007, -008). Prerequisite for OAuth, not an
  independent release.
- **M3** (done) -- OAuth 2.1, the value driver. First releasable unit = M2 + M3
  (REQ-MCP-009 .. -013).
- **M4** (done) -- Server-initiated SSE stream + resumability (REQ-MCP-006).
- **M5** (done) -- UI / config / ops polish: connection-failure visibility
  (REQ-MCP-018) and the consolidated OAuth redirect origin (REQ-MCP-020).

## Design Decisions

- **Legacy HTTP+SSE is not implemented natively** (REQ-MCP-019). Streamable HTTP
  only; `mcp-remote` covers legacy servers during their decline.
- **OAuth tokens are stored plaintext in SQLite** (REQ-MCP-012), consistent with
  existing operator-state storage; the database file's on-disk protection is the
  trust boundary.
- **`mcp-remote` is retained as a transition fallback.** Native HTTP does not
  remove the stdio path; an HTTP server can still be configured as
  `npx mcp-remote <url>`.
- **Static auth is not an independent milestone.** It costs nothing on top of
  the M2 transport, but OAuth (M3) is the deliverable users feel.
- **The OAuth redirect origin is derived from the TLS host config, not request
  headers** (REQ-MCP-020). The reachable domain an operator already sets for the
  certificate is the single source of truth for the callback origin, so a
  self-hosted remote deployment needs no separate knob. Deriving from trusted
  config rather than the `Host`/`Forwarded` headers removes the redirect-target
  injection surface, so no trusted-proxy or origin-allowlist machinery exists.
  `PHOENIX_EXTERNAL_URL` overrides for proxy-terminated TLS or manual certs.

## Allium Spec

Behavioral specification: `specs/mcp/mcp.allium`

Models the per-server `ConnState` lifecycle
(`connecting → ready`, with `reconnecting` / `unauthorized` / `failed`
recovery), the `OAuthPhase` authorization sub-lifecycle
(`discovering → registering → awaiting_user → authorized`, plus `refreshing`),
the `McpServer` / `OAuthRegistration` / `OAuthToken` entities, and invariants
binding session ids and tokens to the HTTP/OAuth servers that own them.
</content>
