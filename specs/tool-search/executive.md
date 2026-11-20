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

Implementation has focused and live qualification. Core, tool-context, registry,
and persistence tests pass. Real SQLite/runtime tests passed local and
native-client search through tool execution and reopen/retry; schema accounting
and all five `provider_tool_policy_tests` also pass. The shared fallback passed
12 live cases across GLM 5.3, GLM 5.3 Flash, and all four Gemini 4 models in
streaming and non-streaming modes, restoring signed private envelopes for
continuation. Actual adapter hosted search and continuation passed on the
configured Responses gateway in both modes, including a new tool turn after
private-envelope settlement. Codex native client behavior has official-source
and deterministic-fixture evidence; live bridge qualification is unavailable
without Phoenix-native credentials. Full broad checks and independent review
remain pending. See
[ADR-273](../adrs/273_native-openai-and-shared-local-tool-discovery.md).

| Requirement | Status | Verification needed |
| --- | --- | --- |
| REQ-LLM-004n | Implemented; qualification in progress | Full broad checks and independent review pending; Codex live unavailable. Focused runtime, persistence, schema accounting, policy, hosted gateway, and 12 shared-fallback live cases pass. |
