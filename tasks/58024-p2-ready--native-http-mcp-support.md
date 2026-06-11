# EPIC: Native HTTP MCP server support

Research + scoping for letting Phoenix speak the MCP **Streamable HTTP**
transport natively, instead of only stdio. This is the umbrella; each
milestone below becomes its own task when it's picked up.

## Start here (implementing agent)

**M0 (scoping + spec) is done.** The authoritative design now lives in
[`specs/mcp/`](../specs/mcp/) — `requirements.md` (REQ-MCP-001..019),
`design.md`, `mcp.allium` (the connection + OAuth lifecycle state machine,
`allium check`-clean), and `executive.md` (per-REQ status + the milestone map).
That spec set is more precise than the "Architectural impact" sketch below and
**supersedes it** wherever they differ — it was hardened over six automated
review rounds (~51 findings), so treat it, not this file, as the source of truth
for exact contracts (typed `TransportError`, the `ServerMessageSink`,
authorization-server-keyed registrations, token↔resource binding, etc.).

**Next actionable: M1** — extract the `McpTransport` trait and turn
`McpServerConfig` into a `Stdio | Http` enum, *zero behavior change*. Spin it
into its own task (`taskmd new --slug mcp-transport-trait`), implement against
`specs/mcp/`, and keep the existing stdio tests green. Then M2 → M3 → M4 → M5
per the milestone list. `executive.md`'s status table tracks which REQs each
milestone covers.

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

> **Note:** this is the original scoping sketch. For exact contracts defer to
> `specs/mcp/design.md` + `mcp.allium`, which refined several of these (the
> transport returns a typed `TransportError` + a `ServerMessageSink` rather than
> `String`; `HttpAuth` distinguishes an explicit credential from generic
> headers; OAuth registrations are keyed by authorization server; tokens are
> audience-bound to the resource URI).

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

**M0 — Scoping & spec. ✅ DONE** → `specs/mcp/` (`requirements.md`,
`design.md`, `mcp.allium`, `executive.md`). spEARS + an Allium spec for the
transport + session + OAuth lifecycle; `allium check`-clean; reviewed over six
rounds. This is the contract M1–M5 build against.

**OAuth is the headline, not an afterthought.** The value driver is reaching
OAuth-protected remote servers (Atlassian/Slack/Linear-style) without the
`mcp-remote` shim. Static-token/header auth is not an independently valuable
release on its own — it falls out of the transport's header plumbing for free,
but the first *releasable* unit is the transport **plus** OAuth (M2 + M3).

**M1 — Transport abstraction refactor (no behavior change).**
Extract `McpTransport`, move existing stdio behind it, turn `McpServerConfig`
into an enum (stdio-only variant still the only one wired). All existing tests
green, zero behavior change. De-risks everything after it.

**M2 — Streamable HTTP transport substrate (prerequisite, not a release).**
Stop skipping `type: "http"`. Implement POST `initialize`/`tools/list`/
`tools/call` handling both `application/json` and `text/event-stream`
responses; session-id lifecycle; protocol-version header; arbitrary request
headers (static bearer/custom headers from config come for free here). This is
the substrate OAuth builds on — validated against an unauthenticated or
static-token test server, but **not shipped as a standalone feature**, because
static auth alone isn't the point.

**M3 — OAuth 2.1 (THE value driver). First releasable unit = M2 + M3.**
`401` + `WWW-Authenticate` → PRM (9728) → AS metadata (8414) → DCR (7591) →
auth-code + PKCE → `Authorization: Bearer` + refresh. Token store in SQLite
(plaintext, consistent with existing MCP state persistence). Native "Authorize"
affordance replacing today's stderr-scraped URL. Closes task 08639. This is the
largest chunk and the reason the epic exists; everything before it is
scaffolding to get here.

**M4 — Server-initiated stream + resumability.**
GET SSE stream for server→client messages, `notifications/tools/list_changed`
(already half-wired via `tools_changed`), reconnect with `Last-Event-ID`.
Sequenced after OAuth: the auth-code flow itself runs over POST + browser
redirect and doesn't depend on the GET stream, so it need not block the
headline.

**M5 — UI + config + ops polish.**
`McpStatusPanel`: show transport type and auth state, native authorize button
(vs today's scraped URL), surface connection errors (task 02685). Define HTTP
reload semantics. Document the config schema.

## Design decisions (resolved during scoping)
1. **Legacy HTTP+SSE (2024-11-05): skipped entirely.** Streamable HTTP only.
   Servers that speak only the old transport stay on the `mcp-remote` shim.
2. **mcp-remote retained as a fallback during the transition.** The stdio +
   `mcp-remote` path keeps working; the `type: "http"` skip is removed only
   once native HTTP + OAuth lands.
3. **OAuth token storage: plaintext in SQLite**, consistent with how MCP
   enable/disable state is already persisted.
4. **Auth does not phase.** Static bearer/header auth alone is not a shippable
   milestone; OAuth (M3) is the value driver and rides on the same M2 transport,
   so the first release bundles M2 + M3.

## Relationship to existing tasks
- 08639 `native-mcp-oauth` → folded into **M3**.
- 02685 `mcp-surface-connection-errors` → naturally lands in **M5** (and the
  transport refactor makes failure states cleaner to surface).
- 08613 `mcp-dynamic-tool-resolution` (done) → HTTP tools ride the same live
  resolution path; no extra work expected.
- 08614 / 08615 (project-local config, scoped enable/disable) are orthogonal
  (discovery & scoping), but the config-enum change in M1 should not conflict.
