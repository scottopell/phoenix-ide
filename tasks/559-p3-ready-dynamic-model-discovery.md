---
created: 2026-02-19
priority: p3
status: ready
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

## Files

- `src/llm/models.rs` — hardcoded model definitions
- `src/llm/registry.rs` — model registration at startup
- `src/api/handlers.rs` — `/api/models` endpoint
