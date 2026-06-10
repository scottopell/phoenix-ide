# EPIC: Native HTTP MCP server support

Research + scoping for letting Phoenix speak the MCP **Streamable HTTP**
transport natively, instead of only stdio. This is the umbrella; each
milestone below becomes its own task when it's picked up. This file is the
scope of record until a `specs/mcp/` spec exists (M0).

## Where we are today

- MCP is **stdio-only**. `phoenix-tools/src/mcp.rs` spawns each server as a
  child process and talks JSON-RPC over its stdin/stdout.
- HTTP entries are **explicitly skipped**: `read_all_configs()` drops any
  config with `"type": "http"` (see the `Skipping HTTP transport` branch).
- The only way to reach an HTTP MCP server right now is to configure
  `npx mcp-remote <url>` as a *stdio* command — a Node subprocess that bridges
  stdio↔HTTP. That shim is the source of real pain:
  - extra npm/Node dependency and a subprocess per remote server;
  - OAuth handled by scraping the child's **stderr for a URL** and opening a
    browser tab on every reload (task 08639 `native-mcp-oauth`);
  - crash/respawn semantics that only make sense for a local process.
- `reqwest` (rustls) is already a dependency — but in **phoenix-ide**, not
  **phoenix-tools** where `mcp.rs` lives. Native HTTP means bringing an HTTP
  client (and an SSE reader) into the tools crate, or moving the transport.

"Native HTTP MCP support" = Phoenix implements the MCP HTTP transport itself
and deletes the reason to ever shell out to `mcp-remote`.

## What "HTTP MCP servers" actually means (protocol surface)

The scope is defined by the MCP transport + auth specs:

1. **Streamable HTTP transport** (current; spec rev 2025-03-26, refined
   2025-06-18). This is the target.
   - One endpoint URL handling **POST** (client→server JSON-RPC) and **GET**
     (open a server→client SSE stream).
   - A POST response is **either** `application/json` (single reply) **or**
     `text/event-stream` (a stream of messages) — the client must handle both.
   - `Mcp-Session-Id` response header at `initialize`; echoed on every
     subsequent request; `DELETE` to end the session; 404 on an expired
     session means re-initialize.
   - `MCP-Protocol-Version` header on every request after negotiation.
   - Resumability: SSE event `id`s + `Last-Event-ID` on reconnect.
2. **Legacy HTTP+SSE transport** (2024-11-05): two endpoints (`GET /sse`
   to receive + `POST` to send). Deprecated by Streamable HTTP but still
   deployed in the wild. **Decision needed** (see open questions).
3. **Authorization** — OAuth 2.1 for remote servers:
   - `401` carries `WWW-Authenticate` → **Protected Resource Metadata**
     (RFC 9728) → **Authorization Server metadata** (RFC 8414,
     `.well-known`) → **Dynamic Client Registration** (RFC 7591) →
     auth-code + **PKCE** → `Authorization: Bearer` on every request.
   - Simpler, very common case first: **static bearer token / custom headers**
     supplied directly in config (covers most internal/enterprise servers).

### Non-goals (at least initially)
- Phoenix as an MCP *server*. We are a client.
- Server→client features we don't do for stdio either: sampling, elicitation,
  roots.
- MCP resources/prompts — current code only consumes **tools**. HTTP support
  shouldn't expand the feature surface; it adds a transport.

## Architectural impact

The central refactor is that `McpServer` is currently a concrete struct welded
to a child process (`Child` + `stdin`/`stdout` mutexes). HTTP has no process.

- **Transport abstraction.** Introduce an `McpTransport` trait
  (`send_request`, `send_notification`, lifecycle/health) with `StdioTransport`
  and `HttpTransport` impls; `McpServer` holds a transport instead of a
  `Child`. The request/response loop, id-matching, notification handling, and
  pagination in `list_tools` are transport-agnostic and lift cleanly.
- **Config becomes a sum type.** `McpServerConfig` is a struct today
  (`command/args/env`). It becomes an enum: `Stdio { command, args, env }` |
  `Http { url, headers, auth }`. The reload reconciler compares configs with
  `PartialEq` to decide unchanged/restart — that comparison must extend to the
  HTTP variant.
- **Health & recovery differ per transport.** stdio = "process exited" →
  respawn. HTTP = connection dropped / session 404 → reconnect + resume (or
  re-initialize). `is_alive` / `respawn` / `is_crash_like_error` need
  transport-specific behavior.
- **Dependencies.** Add an HTTP client + SSE reader to **phoenix-tools**
  (reqwest is already used in phoenix-ide; an SSE line parser is needed —
  `eventsource-stream` or hand-rolled over `reqwest`'s byte stream).
- **OAuth replaces stderr-scraping.** The `pending_oauth_urls` map fed by the
  child's stderr drain becomes a native OAuth state machine + a token store
  (the DB already persists MCP enable/disable state; add a credentials table).
  This is where task 08639 gets resolved.
- **Concurrency.** stdio serializes on a single pipe (one in-flight call). HTTP
  is naturally concurrent — multiple POSTs in flight, correlated by JSON-RPC
  id. The "lock stdin+stdout for the whole round trip" comment in `mcp.rs`
  is a stdio-only constraint; HTTP can drop it.

## Milestones

**M0 — Scoping & spec (this task → `specs/mcp/`).**
No spec exists for MCP at all today. Write spEARS (`requirements.md`,
`design.md`, `executive.md`). Because this has a real lifecycle (connect →
initialize → session → reconnect/resume), an auth state machine, and a
cross-transport contract, it warrants an **Allium** spec for the
transport+session+auth lifecycle. Resolve the open questions below into design
decisions here.

**M1 — Transport abstraction refactor (no behavior change).**
Extract `McpTransport`, move existing stdio behind it, turn `McpServerConfig`
into an enum (stdio-only variant still the only one wired). All existing tests
green, zero behavior change. De-risks everything after it.

**M2 — Streamable HTTP transport, static/no auth.**
Stop skipping `type: "http"`. Implement POST `initialize`/`tools/list`/
`tools/call` handling both `application/json` and `text/event-stream`
responses; session-id lifecycle; protocol-version header. Auth limited to
**none + static bearer/custom headers from config**. This alone unlocks the
majority of "internal HTTP MCP server behind an API key" use cases and lets us
drop `mcp-remote` for those.

**M3 — Server-initiated stream + resumability.**
GET SSE stream for server→client messages, `notifications/tools/list_changed`
(the main payoff — already half-wired via `tools_changed`), reconnect with
`Last-Event-ID`. Somewhat optional; sequenced before OAuth because OAuth reuses
the reconnect machinery.

**M4 — OAuth 2.1.**
PRM (9728) → AS metadata (8414) → DCR (7591) → auth-code + PKCE → bearer +
refresh; token store in the DB; native "Authorize" affordance. Replaces
`mcp-remote`'s OAuth entirely and closes task 08639. Largest chunk.

**M5 — Legacy HTTP+SSE transport (conditional).**
Only if we decide to support pre-2025 deployed servers. Two-endpoint flow as a
second `HttpTransport` mode.

**M6 — UI + config + ops polish.**
`McpStatusPanel`: show transport type and auth state, native authorize button
(vs today's scraped URL), surface connection errors (task 02685). Define HTTP
reload semantics. Document the config schema.

## Relationship to existing tasks
- 08639 `native-mcp-oauth` → folded into **M4**.
- 02685 `mcp-surface-connection-errors` → naturally lands in **M6** (and the
  transport refactor makes failure states cleaner to surface).
- 08613 `mcp-dynamic-tool-resolution` (done) → HTTP tools ride the same live
  resolution path; no extra work expected.
- 08614 / 08615 (project-local config, scoped enable/disable) are orthogonal
  (discovery & scoping), but the config-enum change in M1 should not conflict.

## Open questions (resolve in M0)
1. **Legacy HTTP+SSE (2024-11-05):** support, skip, or defer to M5? (Many
   currently-deployed remote servers still speak only this.)
2. **mcp-remote during transition:** keep it working as a fallback path, or cut
   over and remove the skip-on-`type:http` once native lands?
3. **OAuth token storage:** plaintext in SQLite (consistent with current DB
   usage) vs OS keychain vs encrypted-at-rest. Security call.
4. **Auth phasing:** is static bearer/header auth (M2) enough to ship value
   before full OAuth (M4), or must they land together?
