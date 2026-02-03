---
created: 2026-02-03
priority: p2
status: not_started
tags: [llm, providers]
---

# Implement Additional LLM Providers

## Summary

Implement OpenAI, Fireworks, and Gemini providers to support the full range of models available through the exe.dev gateway.

## Context

The centralized model registry (task 100) defined these models but the provider implementations are missing:
- OpenAI: gpt-5.2-codex
- Fireworks: glm-4.7-fireworks, qwen3-coder-fireworks, glm-4p6-fireworks
- Gemini: gemini-3-pro, gemini-3-flash

The gateway URLs are:
- OpenAI: `{gateway}/_/gateway/openai/v1`
- Fireworks: `{gateway}/_/gateway/fireworks/inference/v1`
- Gemini: `{gateway}/_/gateway/gemini/v1/models/generate`

## Acceptance Criteria

- [ ] Implement OpenAI service provider
- [ ] Implement Fireworks service provider (uses OpenAI API format)
- [ ] Implement Gemini service provider
- [ ] Update model factories to create actual services
- [ ] Test each provider with exe.dev gateway
- [ ] Add provider-specific request/response translation

## Implementation Notes

- Fireworks uses OpenAI's API format
- Gemini has a different API structure
- All providers should support gateway mode with "implicit" API keys
