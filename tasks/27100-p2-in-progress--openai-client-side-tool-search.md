# Native OpenAI search and shared deferred tool discovery

## User need and verified route constraints

Scott wants native OpenAI tool search on both direct API and Codex bridge paths, plus shared fallback search for GLM 5.3 and gateway Gemini (2026-11-20). Direct Responses uses hosted native search; the Codex bridge uses OpenAI's native client-executed protocol with Phoenix's shared lookup. Chat-compatible models use that same lookup through an ordinary function tool. Models with large MCP catalogs initially receive core tools plus search, then load only the schemas needed for the task. Native Anthropic search remains available independently.

The verified AI Gateway Gemini Chat route rejects the native tool_search type. Its native generateContent route works, but the documented Interactions endpoints return 404; Google-native deferred retrieval is not an available drop-in on this deployment. Phoenix-owned discovery can use ordinary function calling on the working route. Do not infer support from display family or turn on the existing supports_tool_search boolean: that flag currently controls Anthropic's native search path. Qualify each route and preserve the distinction between native, local, and unavailable discovery.

## Scope and existing authorities

MCP tool definitions already carry defer_loading=true in ToolRegistryExecutor::definitions_for_language (`crates/phoenix-ide/src/runtime/traits.rs`). The Chat translator sends every declaration; filtering declarations alone would break execution's exact-schema admission checks. The old task's pre-workspace paths and claim that dispatch needs no changes are obsolete.

Relevant owners:

- `crates/phoenix-core/src/domain/tool_availability.rs`: authoritative declarations, callable policy, and request-bound execution evidence.
- `crates/phoenix-db/src/tool_availability.rs`: retained schemas and durable per-conversation policy preparation.
- `crates/phoenix-ide/src/runtime/executor.rs`: request preparation, exact-schema execution admission, restart/model-switch/capacity boundaries.
- `crates/phoenix-ide/src/runtime/traits.rs` and `crates/phoenix-tools`: MCP catalog, tool registration, local search execution.
- `crates/phoenix-llm/src/anthropic.rs` and `openai.rs`: provider-specific advertised declarations and native search.
- `specs/llm/requirements.md`, `llm.allium`, and provider Allium: tool-history integrity and current execution policy remain authoritative. Specify discovery before implementing it; the catalog and advertised selection have different consumers and must have non-overlapping typed contracts.

## Bounded implementation path

OpenAI-specific integration: qualify canonical built-in model identity and exact routes, keep direct hosted and Codex-native client wire shapes distinct, preserve search output in private Responses replay, and restore the original protocol after ordinary exchange settlement. Hosted searchable candidates consume current policy because native allowed_tools does not accept a selector for deferred functions or tool_search. Codex client schemas are emitted at the search-result position, preserving the stable leading prefix. Direct live exchange, sparse streams, mock bridge HTTP/SSE, and cross-provider history must be covered; live bridge qualification requires Phoenix-native credentials rather than borrowing another app's login.

1. Introduce a typed route-specific discovery mode. Keep native Anthropic behavior separate from local function-based discovery; verify other native mechanisms rather than asserting that OpenAI lacks one.
2. Retain the authentic complete catalog and current callability as authority. Derive a separate advertised-schema projection: non-deferred tools plus a local tool_search function and previously discovered eligible tools. Historical calls/results must retain their real schemas and identities.
3. Search names, descriptions, MCP server identity, and schema terms with a bounded deterministic lexical implementation first. A new ranking dependency is not a prerequisite. Return selected authentic schemas through a typed result, then adopt those selections at the owning runtime boundary.
4. Bind selections to the exact catalog/schema and current policy before expanding the advertised projection. Discovery never grants execution permission; disabled, removed, unavailable, or rewritten tools remain refused honestly. Do not parse arbitrary public result JSON as authority.
5. Persist or reconstruct selections from committed authoritative evidence so restart/retry does not lose a discovered tool or trust a stale process-local index. Define retirement on schema change, MCP disable, policy change, and model/route switch.
6. Count advertised schemas in provider request estimates while retaining authentic historical/private replay accounting. A hidden full catalog must not consume the request's schema budget indirectly.

## Acceptance

- Both direct OpenAI hosted search and Codex-native client search execute the native search-to-function-to-result loop, preserving actual provider search records and current execution restrictions.

- A Gemini conversation searches, discovers, advertises, and executes an authentic deferred MCP tool through the existing gateway Chat route.
- Capture the actual outgoing requests: initial deferred schemas are absent, selected schemas appear afterward, and total schema tokens decrease materially for a representative large catalog.
- The same exchange works after restart/retry and preserves tool history when switching models/routes.
- Removed/disabled/schema-changed tools and stale or forged search results cannot expand execution authority.
- Current tool availability and exact-schema dispatch regressions still pass; native Anthropic search is unchanged.
- Add focused deterministic tests at each ownership seam and a bounded opt-in live qualification. Do not rely on an in-memory index, arbitrary sleeps, or a provider accepting an unknown field as proof of support.
