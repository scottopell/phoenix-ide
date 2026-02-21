---
created: 2026-02-19
priority: p3
status: done
completed: 2026-02-21
---

# Dynamic model discovery from LLM gateway

## Summary

Replace the hardcoded model list in `src/llm/models.rs` with runtime discovery
from the LLM gateway's `/v1/models` endpoint. Currently, adding a new model
requires a code change and recompile. A gateway-aware discovery mechanism would
pick up new models automatically.

## Context

### How the LLM gateway works

Phoenix supports an LLM gateway mode (`LLM_GATEWAY` env var) where all
provider requests are routed through a single proxy. The gateway uses
path-prefix routing to dispatch to the correct upstream provider, each using
its native API format:

    {gateway}/anthropic/v1/messages          → Anthropic Messages API
    {gateway}/openai/v1/chat/completions     → OpenAI Chat Completions API
    {gateway}/openai/v1/responses            → OpenAI Responses API
    {gateway}/fireworks/inference/v1/...     → Fireworks (OpenAI-compat)

All requests use `"implicit"` as the API key — the gateway handles real
credential injection. The gateway is designed as a transparent proxy: it
forwards requests to upstream providers without translating between formats.

### Current model listing contract

The gateway contract currently does **not** require a `/models` endpoint.
Clients hardcode their model lists. However, since the gateway proxies to
upstream providers, and some providers expose model listing (e.g., OpenAI's
`GET /v1/models`), a gateway can forward these requests. In practice, querying
`{gateway}/openai/v1/models` returns models from all providers with prefixed
IDs (`openai/gpt-5.1`, `anthropic/claude-sonnet-4-5-20250929`, etc.).

Whether the gateway contract will be extended to guarantee model listing is
an open question. It is also possible to write a gateway proxy in front of
internal LLM providers; some of those support model listing and some don't.
This task should be designed to degrade gracefully when the endpoint is
unavailable.

## Design considerations

**Hybrid approach**: Query the gateway at startup, but keep a minimal hardcoded
fallback for metadata the gateway doesn't provide (descriptions, context window
sizes, preferred defaults). The gateway returns model IDs but not necessarily
the richer metadata Phoenix uses for UI display.

**Provider detection**: The gateway returns prefixed model IDs
(`openai/gpt-5.1`, `anthropic/claude-sonnet-4-5-20250929`). Phoenix would need
to map these to its internal user-facing IDs and provider categories, or adopt
the gateway's naming scheme directly.

**Graceful degradation**: If the gateway doesn't support `/v1/models` (returns
404 or error), fall back to the current hardcoded list. This keeps Phoenix
functional against gateways that don't implement the endpoint.

**Direct API key mode**: When running without a gateway (direct API keys),
there's no unified models endpoint to query. The hardcoded list remains
necessary for this mode.

**Caching**: Model lists don't change frequently. Query once at startup and
optionally refresh on a long interval (hours, not seconds).

## Open questions

- Should Phoenix adopt the gateway's model naming (`anthropic/claude-sonnet-4-5-20250929`)
  or continue mapping to user-facing names (`claude-4.5-sonnet`)?
- Where do context window sizes come from if not hardcoded? Some providers
  include this in their models response, others don't.
- Should the UI expose all discovered models or filter to a curated subset?
  787 models (observed from one gateway) is too many to show unfiltered.

## Implementation

**Completed 2026-02-21**

Added `src/llm/discovery.rs` module that queries gateway endpoints at startup:

- `{gateway}/anthropic/v1/models` - Returns 9 Anthropic models with `display_name`
- `{gateway}/openai/v1/models` - Returns 119 OpenAI models  
- `{gateway}/fireworks/inference/v1/models` - Returns 14 Fireworks chat models with `context_length` and capability flags

Total discovered: **142 models** from gateway.

`ModelRegistry::new_with_discovery()` is now called at startup when gateway is configured. It:
1. Queries all three provider endpoints (5s timeout each)
2. Filters hardcoded model list to only models the gateway actually supports
3. Falls back to full hardcoded list if discovery fails
4. Logs discovery results for observability

### What works

- ✅ Discovery runs at startup and logs results
- ✅ Models removed from gateway are automatically excluded (e.g., qwen3-coder)
- ✅ Graceful fallback if gateway doesn't support model listing
- ✅ Zero-downtime production deployment
- ✅ All existing models continue working

### Future work

**Full dynamic registration**: Currently we register the intersection of hardcoded models and discovered models. To support *arbitrary* discovered models requires:
- Mapping gateway model IDs to providers (some use prefixes like `accounts/fireworks/models/...`)
- Creating services dynamically without hardcoded `ModelDef` entries
- UI filtering (142 models is too many to show unfiltered)
- Metadata merging strategy (context windows, descriptions)

This is deferred - the current implementation solves the immediate problem of automatic removal when models disappear from the gateway.

## Files

- `src/llm/discovery.rs` — NEW: gateway model discovery
- `src/llm/registry.rs` — `new_with_discovery()` async constructor
- `src/main.rs` — calls `new_with_discovery()` at startup
- `specs/llm/*.md` — updated with gateway model endpoint documentation
