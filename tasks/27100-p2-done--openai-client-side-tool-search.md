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

## Delivery and verification (2026-11-20)

Implemented route-qualified native OpenAI hosted search on direct Responses, native client-executed search on Codex Lite, and a shared bounded selector exposed as an ordinary function on GLM/Gemini Chat routes. Normalized immutable discovery records commit with canonical tool results; frozen permissions, actual advertised schemas, and live execution admission have separate consumers. Native client schemas load at the result position without changing the leading tool prefix. Current policy and exact schemas revoke stale discoveries. Historical protocol identity survives settlement, reopen, retry, and route switches. Request accounting follows actual schema placement.

- Core tests: 64 focused tests passed. Tool/context/registry tests and 13 SQLite discovery regressions passed. All five runtime provider-policy tests passed, including real search-to-execution and reopen/retry for Local/OpenAiClient modes.
- All 509 LLM library tests passed. Actual mock Codex HTTP/SSE and WebSocket exchanges verify native search -> authenticated schema result -> function call -> final response. WebSocket requests use exact previous-response suffixes without prepending deferred declarations.
- Live actual-adapter qualification: native hosted search, function continuation, and settled replay succeeded on the configured Responses gateway in both modes. Shared fallback succeeded for GLM 5.3/Flash and Gemini 4 Neon/Hydrogen/Carbon/Argon in streaming/nonstreaming modes (12 cases), including signed replay and >95% initial schema-byte reduction on a 129-tool synthetic catalog. A raw Neon gateway probe additionally accepted settled unsigned foreign-provider search/function history; this is wire evidence, not a runtime model-switch test.
- Isolated adversarial review froze `15ec903e20eb4e46ad419b07d56c959867fc9a0a..e6f669043e45e87336dfb8f124ce282d23d685df`. It found missing model-visible withdrawal notices for historical native schemas. Follow-up `da12b7ca216bd1edf8046cbe29a3fbcdd1f490f9` fixed both paths; anchored review confirmed the disposition. Later changes added test-only WebSocket coverage and scoped an existing migration fixture.
- Broad command: `PHOENIX_CHECK_BASE=15ec903e20eb4e46ad419b07d56c959867fc9a0a PHOENIX_CHECK_CARGO_TEST_TIMEOUT_SECS=3600 ./dev.py check`. Twelve checks passed, six were skipped and one failed (Rust tests). Passed checks included clippy, format, generated-binding freshness, specifications, dev.py tests, task validation and end-to-end. UI lanes were unchanged; optional ast-grep/cargo-deny were unavailable. Rust ran 6,342 tests: 6,339 passed, three failed. The migration fixture assumed exactly one pending migration; it now targets version169 and all five owning replay tests pass. Both browser failures passed isolated reruns (2/2) and the full Chrome-enabled browser_cli file (37/37, no skips). Their broad-run cause is unproven and tracked in task41021; lost failure excerpts are tracked in task41022. The broad Rust run was not repeated after these test-only corrections. Final DB all-target clippy and the five format/task/spec checks also passed.

Phoenix-native Codex credentials were absent, so live bridge entitlement remains untested. No other app's credentials were borrowed. No production deployment is claimed.
