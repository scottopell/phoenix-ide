M4 of the native HTTP MCP build-out (umbrella: tasks/58024). Implements
REQ-MCP-006: the server-initiated GET SSE stream and its resumability.

## Why

HTTP MCP servers deliver server-initiated messages -- notably
`notifications/tools/list_changed` -- over a long-lived GET stream the client
opens against the same endpoint. Without it, an HTTP server's tool list goes
stale: `tools_changed` is already wired on the protocol side (mcp.rs flips it
on the notification and refreshes lazily), and `SseFramer` already parses
SSE for POST replies, but nothing ever opens the GET stream that would carry
the notification. M2 left a breadcrumb at mcp/http.rs (the POST framer ignores
`id:` because "Last-Event-ID replay belongs to the server-initiated GET
stream, REQ-MCP-006").

## Scope (per specs/mcp/, authoritative)

- Open a GET to the MCP endpoint with `Accept: text/event-stream` to receive
  server-initiated JSON-RPC messages, feeding them into the *existing*
  `ServerMessageSink` (do not grow a second list_changed handler -- the
  protocol layer owns that, design.md "Transport Boundary").
- Attach the resolved auth on the GET exactly as on POST: an OAuth access
  token rides the GET (and the session DELETE) per REQ-MCP-012; send the
  `Mcp-Session-Id` and negotiated `MCP-Protocol-Version` headers.
- Resumability: capture each SSE event `id:` on the GET stream and, on a
  dropped stream, reconnect supplying `Last-Event-ID` so the server can replay
  missed messages. (Per http.rs the POST framer stays id-agnostic; only the
  GET stream resumes.)
- Run the stream as a background task tied to the HTTP server's lifetime;
  tear it down on shutdown/reconnect alongside the session DELETE.

## Out of scope

- M5 (connection-failure visibility / UI polish, REQ-MCP-018, task 02685).
- Legacy HTTP+SSE two-endpoint transport (REQ-MCP-019, decided out).

## Acceptance

- An HTTP server that emits `notifications/tools/list_changed` on its GET
  stream causes Phoenix to refresh that server's tool list (the existing lazy
  `tools_changed` path fires from the GET-delivered notification).
- A dropped GET stream reconnects with `Last-Event-ID` set to the last
  received event id.
- The GET carries `Accept: text/event-stream`, the bearer token (OAuth/static),
  session id, and protocol-version headers.
- `mcp.allium` already models this (list_changed over the GET stream,
  REQ-MCP-006); implementation matches the spec. `./dev.py check` green.
