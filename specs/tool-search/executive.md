# Tool Search - Executive Summary

## Requirements Summary

The ordinary `tool_search` function lets a model discover deferred tools from its
permitted catalog without loading every schema into context. Native Codex client
search reuses the same selector. The normative requirements live in
[REQ-LLM-004n](../llm/requirements.md#req-llm-004n-route-qualified-tool-discovery)
and [the discovery contract](../llm/tool-discovery.allium).

## Technical Summary

`ToolSearch` is an eager, read-only, nonclearable tool. `ToolContext` supplies the
catalog frozen for the originating request; the tool owns no mutable search
index and performs no LLM or filesystem operation. The core selector validates
bounded queries and deterministically ranks deferred definitions by name,
description, and schema terms. Public output contains names only. Schema loading
and execution authority come from canonical normalized selection records and
current policy, not by parsing that output.

Tool registration uses the common read-only catalog. Explicit tool surfaces that
do not install search keep eager schema loading. Native Anthropic search remains
provider-managed; qualified OpenAI hosted search runs on the provider, while the
Codex native client protocol emits schemas in its result at discovery time. Native
client admission does not promote ordinary function-search history into loaded
schemas; the local route can reuse either protocol because it physically sends
the validated selected definitions.

## Status Summary

Live Codex qualification on 2026-11-23 passed using Phoenix's own login and
GPT-6.2 Sol: actual adapter loops on HTTP/SSE and WebSockets, settled next-turn
replay, and a complete SQLite/runtime loop using the real search handler over
65 deferred schemas. All three runtime requests used WebSockets and the native
selection, deferred execution, and final `OK` persisted correctly. The observed
namespaced native call is qualified under
[ADR-274](../adrs/274_qualify-codex-namespaced-native-search-calls.md); ordinary
function history remains ordinary. All 513 LLM unit tests pass. Final follow-up
checks and independent review are in progress.

Initial qualification on 2026-11-20: core, tool-context, registry,
and persistence tests pass. Real SQLite/runtime tests passed local and
native-client search through tool execution and reopen/retry; schema accounting
and all five `provider_tool_policy_tests` also pass. The shared fallback passed
12 live cases across GLM 5.3, GLM 5.3 Flash, and all four Gemini 4 models in
streaming and non-streaming modes, restoring signed private envelopes for
continuation. Actual adapter hosted search and continuation passed on the
configured Responses gateway in both modes, including a new tool turn after
private-envelope settlement. Codex native client behavior has official-source
and actual mock HTTP/SSE/WebSocket exchange evidence; all 509 LLM library tests
pass. Native-client WebSocket continuation retains the leading tool prefix and
sends the authenticated schema only in its search-result suffix. Review caught
and verified the fix for missing withdrawal notices alongside historical native
schemas. Live bridge qualification is unavailable without Phoenix-native
credentials.

The broad check reported one failed check (Rust tests); other executed checks,
including end-to-end and static checks, passed. UI lanes were unchanged and
skipped; optional ast-grep/cargo-deny were unavailable. The Rust stage passed
6,339 of 6,342 tests. The replay migration fixture was fixed
and all five owning tests pass. Both browser lifecycle failures passed isolated
reruns and the complete Chrome-enabled integration file (37/37); their original
cause remains unproven (task 41021). Task 41022 tracks preserving the original
failure diagnostics. See
[ADR-273](../adrs/273_native-openai-and-shared-local-tool-discovery.md).

| Requirement | Status | Verification and limits |
| --- | --- | --- |
| REQ-LLM-004n | Complete | Live Codex adapter and SQLite runtime, prior hosted/fallback live cases, and focused regressions pass. Final follow-up checks/review in progress; earlier broad-run exceptions classified above. |
