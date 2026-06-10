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
| REQ-MCP-001 | Transport-Tagged Config Discovery | Partial | `read_all_configs` discovers + merges stdio entries; HTTP entries are skipped today. HTTP classification is the change. |
| REQ-MCP-002 | Transport-Agnostic JSON-RPC Protocol | Partial | Protocol (initialize / paginated `tools/list` / `tools/call` / notifications) implemented on `McpServer`, but welded to stdio. M1 extracts the `McpTransport` trait. |
| REQ-MCP-003 | Stdio Transport | Complete | `McpServer::spawn` / `initialize` / `list_tools`; crash detection (`is_alive`, `is_crash_like_error`) + `respawn` in `mcp.rs`. |
| REQ-MCP-004 | Streamable HTTP Transport | Not started | M2. POST + `application/json`/`text/event-stream` response handling; `MCP-Protocol-Version` header. |
| REQ-MCP-005 | HTTP Session Lifecycle | Not started | M2. `Mcp-Session-Id` capture/echo; DELETE on shutdown; 404 → re-initialize. |
| REQ-MCP-006 | Server-Initiated Stream and Resumability | Not started | M4. GET SSE stream; `tools/list_changed`; `Last-Event-ID` replay. |
| REQ-MCP-007 | HTTP Connection Recovery | Not started | M2. Reconnect on transport error, distinct from stdio respawn. |
| REQ-MCP-008 | Static Token / Header Authentication | Not started | M2. Config-supplied bearer/headers attached per request. Falls out of transport header plumbing. |
| REQ-MCP-009 | OAuth 2.1 Authorization Discovery | Not started | M3. 401 → PRM (RFC 9728) → AS metadata (RFC 8414). |
| REQ-MCP-010 | Client Identity Acquisition | Not started | M3. Prefer pre-configured / cached registration / Client ID Metadata Document; RFC 7591 DCR as fallback. `OAuthRegistration` keyed by authorization server. |
| REQ-MCP-011 | Authorization Code Flow with PKCE | Not started | M3. Native auth-code + PKCE; no `mcp-remote`. |
| REQ-MCP-012 | Token Storage, Refresh, Invalidation, and Step-Up | Not started | M3. `mcp_oauth_tokens` (plaintext SQLite); refresh; discard-and-re-auth on refresh failure; 403 `insufficient_scope` step-up. |
| REQ-MCP-013 | Authorization Status Surfaced to the UI | Partial | `GET /api/mcp/status` exists and carries a `pending_oauth_url`, but it is scraped from an `mcp-remote` child's stderr. M3 replaces it with the native flow's structured URL. |
| REQ-MCP-014 | Tool Exposure and Live Resolution | Complete | `tool_definitions` / `create_mcp_tool_by_name`; live resolution via `ToolRegistryExecutor`. HTTP servers ride this unchanged. |
| REQ-MCP-015 | Config Reload Reconciliation | Complete | `reload_from_configs` (add / remove / restart / unchanged / failed). M1 extends `PartialEq` to the HTTP config variant. |
| REQ-MCP-016 | Per-Server Enable/Disable | Complete | `disable_server` / `enable_server`; persisted in `mcp_disabled_servers` (`crates/phoenix-db/src/lib.rs`). |
| REQ-MCP-017 | Tool Call Cancellation and Error Surfacing | Complete | `McpTool::run` spawns a detached task + selects on the cancel token; `tools/call` honors `isError`. |
| REQ-MCP-018 | Connection Failure Visibility | Not started | M5. Failed servers retained in the status response with their error, cleared on reconnect. |
| REQ-MCP-019 | Legacy HTTP+SSE Not Natively Supported | Complete | By decision; `legacy_sse_native = false`. Such servers use the `mcp-remote` stdio bridge. |

## Milestones

This spec set is M0 of the native HTTP MCP build-out. The remaining milestones:

- **M1** -- Extract the `McpTransport` trait; turn `McpServerConfig` into a
  `Stdio | Http` enum. No behavior change (REQ-MCP-002, REQ-MCP-015).
- **M2** -- Streamable HTTP transport substrate with static/no auth
  (REQ-MCP-004, -005, -007, -008). Prerequisite for OAuth, not an independent
  release.
- **M3** -- OAuth 2.1, the value driver. First releasable unit = M2 + M3
  (REQ-MCP-009 .. -013).
- **M4** -- Server-initiated SSE stream + resumability (REQ-MCP-006).
- **M5** -- UI / config / ops polish, including connection-failure visibility
  (REQ-MCP-018).

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

## Allium Spec

Behavioral specification: `specs/mcp/mcp.allium`

Models the per-server `ConnState` lifecycle
(`connecting → ready`, with `reconnecting` / `unauthorized` / `failed`
recovery), the `OAuthPhase` authorization sub-lifecycle
(`discovering → registering → awaiting_user → authorized`, plus `refreshing`),
the `McpServer` / `OAuthRegistration` / `OAuthToken` entities, and invariants
binding session ids and tokens to the HTTP/OAuth servers that own them.
</content>
