Add an outbound-egress gate for agent-influenced URLs (browser tool + MCP HTTP transport), blocking private/loopback/link-local ranges and the cloud-metadata IP, done DNS-rebinding-safe (resolve, then check the resolved address, then connect to that address).

Affected sinks:
- crates/phoenix-tools/src/browser/tools.rs — `page.goto(&input.url)` and `browser_eval`'s in-page fetch take an LLM-supplied URL with no scheme/host/IP validation. Reaches http://169.254.169.254/ (cloud metadata), localhost services, and file:// with response feedback via screenshot/eval.
- crates/phoenix-tools/src/mcp/http.rs and mcp/oauth.rs — connect to config-supplied URLs (reachable if the agent writes .mcp.json into the cwd then triggers POST /api/mcp/reload). Same SSRF class, lower reach.

Why this is gated, not urgent: in the current threat model the agent already holds the `bash` tool with unrestricted egress, so the browser/MCP path is not a privilege escalation — anything reachable here is reachable with `curl`. This work only becomes meaningful once bash egress is sandboxed/network-isolated; at that point an unguarded browser/MCP fetch would be the bypass. Implement together with (or after) bash network sandboxing, not before.

Design notes when picked up:
- A naive scheme/IP allowlist gives false confidence against DNS rebinding — gate on the *resolved* connection IP, e.g. a custom reqwest DNS resolver / connector or an explicit egress proxy, and re-check on redirects.
- Decide policy for file:// (block outright for the browser tool) and for redirects to blocked ranges.
- Keep the block list configurable (operators behind a proxy may legitimately reach RFC-1918).

Source: security review of the API surface (see branch claude/security-review-e65xnw). Companion fixes already landed there: mkdir jail bypass, git arg-injection hardening, constant-time suggest-token compare, 0600-on-create for the CA key, and read-only share SSE no longer starting a runtime.
