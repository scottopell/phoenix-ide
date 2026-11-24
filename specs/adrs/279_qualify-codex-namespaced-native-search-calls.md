# ADR-279: Qualify Codex's namespaced native search call

- **Status:** Accepted
- **Date:** 2026-11-23
- **Affects:** REQ-LLM-004n, REQ-LLM-005

## Context

Live qualification using Phoenix's own Codex login exposed a response shape
absent from the deterministic native-search fixtures. GPT-6.2 Sol returned a
`function_call` with namespace `tool_search`, name `tool_search_tool`, and a
JSON-string argument object. Both HTTP/SSE and WebSocket showed this shape.
Phoenix treated the name as an ordinary unavailable function.

The request's native declaration and Responses Lite framing matched upstream
Codex. Adding upstream-style prefix identifiers did not change the response.
The backend accepted a native client `tool_search_output` for this call identity,
loaded its supplied function schema, executed the synthetic function exchange,
and returned the requested final reply. Canonical native search history also
remained accepted after settlement, including a fresh native discovery.

## Options considered

1. Reject the namespaced response as an unavailable ordinary function. This
   preserves refusal safety but prevents the requested live native loop.
2. Alias function names generally or return an ordinary function result. This
   would confuse native schema loading with unrelated function calls.
3. Qualify the exact native representation and retain native output, trusted
   selections, original active replay, and independent execution permissions.

## Decision

Choose option 3.

On qualified Codex client-search requests, recognize the exact namespaced shape
as another representation of native client discovery. A shared typed parser
owns its identity and argument decoding for provider projection, replay
validation, and canonical result classification. Preserve the original full
provider item in active private replay. Persist the existing native-client
protocol and selected schemas; reconstruct canonical native call/result items
after private settlement. Continue returning native `tool_search_output` and
loading schemas at the discovery position.

Do not install a generic function-name alias, advertise an ordinary replacement
tool, or reinterpret ordinary function-search records as native. Require the
qualified request mode, exact namespace and function name, nonempty call ID,
object arguments, and authentic owner/result evidence.

## Consequences

The bridge handles its observed native search representation while retaining
independent permissions and prefix placement. The same parser prevents provider
projection and durable protocol classification from disagreeing. No schema
migration or new compatibility guarantee is introduced. Fixtures must cover both
wire representations and negative namespace/route cases; live qualification
must exercise the runtime, frozen catalog, canonical receipt, deferred function,
and final response rather than only accepting the first search call.

## References

- [ADR-278](278_native-openai-and-shared-local-tool-discovery.md).
- [Discovery contract](../llm/tool-discovery.allium).
- [OpenAI native client search](https://developers.openai.com/api/docs/guides/tools-tool-search).
- [Upstream Codex tool router](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/router.rs).
