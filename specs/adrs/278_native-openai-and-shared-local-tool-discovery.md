# ADR-278: Use native OpenAI discovery with a shared local selector

- **Status:** Accepted
- **Date:** 2026-11-20
- **Affects:** REQ-LLM-004i, REQ-LLM-004j, REQ-LLM-004m, REQ-LLM-004n, REQ-LLM-005

## Context

Scott requested native OpenAI tool search and a shared fallback for GLM and
gateway Gemini, including both Codex bridge and direct API execution. Large MCP
catalogs already marked deferred tools, but compatible request paths sent every
schema. Anthropic had a separate native mechanism. Google's native retrieval
interface was not exposed through the configured gateway.

OpenAI's direct Responses API documented hosted search. A small configured
gateway probe accepted native hosted search and returned a search call, search
output, and selected function call. The official Codex client used native
execution:client search instead. Treating both OpenAI routes as hosted, or
emulating both as ordinary functions, would miss that protocol and cache boundary.

## Options considered

1. Keep eager loading on every route except Anthropic. Simple, but leaves the
   catalog context cost and requested discovery capability unresolved.
2. Use an ordinary local search function for every non-Anthropic model. Shares
   lookup code, but bypasses supported OpenAI native discovery and its placement.
3. Use qualified hosted Responses search, native client search on Codex, and an
   ordinary Chat fallback backed by the same local selector and canonical rows.

## Decision

Choose option 3. A resolved service selects a typed discovery mode; protocol
labels and remapped model names do not establish native support. A surface that
lacks the required executable search tool keeps eager loading. Anthropic's
native behavior remains separate.

Hosted Responses search receives permitted deferred candidates and auto/none
selection. Native allowed_tools must not enumerate deferred function names on
that path; independent execution admission and advisory restrictions remain.
Accepted whole Responses output envelopes retain hosted search items within the
active exchange. Codex uses native execution:client results at their discovery
position, preserving the leading declaration prefix. Compatible Chat routes use
an ordinary function and add authenticated selected schemas to subsequent calls.

The shared selector searches the permitted catalog frozen for the originating
request. It returns a compact names-only public summary. The canonical owned
tool-round transaction recomputes and validates a successful result against that
same snapshot, then writes immutable selection parent and ordered schema child
rows with the tool message and state. Empty and failed results retain protocol
identity. Stored per-call Function/OpenAiClient identity controls historical
wire encoding even after active provider replay is retired.

Native client admission uses eager tools and current-schema selections from
native-client records only. Function-search records and historical-only deferred
names cannot load native schemas. Their original function-call history remains
valid evidence; a new native search can load the tools after a mode switch. Local
mode may reuse either protocol and authentic historical definitions because its
request physically advertises the validated schemas. History retention and
admission eligibility remain different consumers of the records.

The full permission snapshot and actual advertised names have distinct
consumers. Execution requires both plus the authentic schema and current live
authority. Restart loads committed records; an in-memory index or arbitrary
public JSON never substitutes for those facts. Capacity planning follows actual
schema placement rather than counting every hidden local definition.

## Consequences

Native OpenAI discovery can retain its provider-specific cache behavior while
GLM and Gemini share a small deterministic lookup implementation. Discovery does
not expand permissions or manufacture schemas. The cost is normalized selection
storage, per-protocol adapter behavior, and tests spanning canonical persistence,
advertisement, policy changes, replay, and request accounting. Adding a native
route requires explicit qualification rather than a shared-protocol assumption.

## References

- [Tool discovery requirements](../llm/requirements.md#req-llm-004n-route-qualified-tool-discovery).
- [Tool discovery contract](../llm/tool-discovery.allium).
- [OpenAI tool search](https://developers.openai.com/api/docs/guides/tools-tool-search).
- [Official Codex native client search definition](https://github.com/openai/codex/blob/2b5111917c1497ed945da543221f67ebe6e95280/codex-rs/core/src/tools/handlers/tool_search_spec.rs).
- [ADR-084](084_tool-policy-is-independent-of-provider-history.md).
- `ToolDiscoveryMode`, `ToolSearchQuery`, `ToolDiscoveryResult`, and `ToolSearch`.
