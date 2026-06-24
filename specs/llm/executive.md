# LLM Provider - Executive Summary

## Requirements Summary

The LLM provider abstracts communication with multiple LLM APIs behind a common interface. Users select their preferred model while the system handles backend-specific translation internally. Phoenix supports built-in Anthropic/OpenAI models plus additive `PHOENIX_LLM_MODELS` entries that declare a backend (`anthropic` or `openai_responses`) and optional wire model name. Base URL overrides are exact endpoints, not gateway roots; Phoenix does not append hidden provider path suffixes. With credential-helper auth and base URL overrides, Phoenix can opportunistically query derived `/v1/models` endpoints to filter the configured model set; if listing is unavailable, Phoenix falls back to configured models. In direct API mode, only models with configured API keys or a credential helper are registered. Requests use a common format (system prompt, messages, tools) that gets translated per backend. Responses are normalized to text blocks, tool use requests, end-of-turn indicators, and usage statistics. Errors are classified into exhaustive named categories plus explicit policies for automatic runtime retry and user-triggered resume. Network failures, transient rate-limit throttles, server errors, and timeouts are auto-retryable; auth failures are not auto-retryable but are user-resumable after credentials are refreshed. Quota exhaustion and context exhaustion remain non-resumable terminal paths. When traffic routes through the codex backend, 429 responses carrying structured quota state (plan type, reset time, window snapshots, credits, promo message) are parsed into a plan-aware terminal error rather than an opaque retryable message.

## Technical Summary

Implements `LlmService` trait with `complete()` method returning `LlmResponse`. Backend implementations translate common request format to Anthropic Messages or OpenAI Responses JSON and normalize responses back. Base URL overrides are exact endpoints; Phoenix does not construct gateway paths. `ModelRegistry` merges built-in specs with additive configured specs, registers only models with an available auth route, and keeps Codex bridge routing scoped to built-in OpenAI Responses models. `LlmError` includes exhaustive `LlmErrorKind` enum (no `Unknown` variant, no catch-all) with separate `auto_retry_policy()` and `user_resume_policy()` methods. `LoggingService` wrapper records model, duration, and token counts. Usage tracking includes input/output tokens and cache statistics for context window computation.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-LLM-001:** Provider Abstraction | ✅ Complete | LlmService trait with async complete() |
| **REQ-LLM-002:** Backend-Compatible Endpoint Support | ✅ Complete | Exact base URL overrides, no hidden suffix append |
| **REQ-LLM-003:** Model Registry | ✅ Complete | ModelRegistry with available_models() |
| **REQ-LLM-003a:** Model Discovery | ✅ Complete | Opportunistic backend-scoped `/v1/models` discovery, falls back to configured models |
| **REQ-LLM-004:** Request Format | ✅ Complete | LlmRequest with system, messages, tools |
| **REQ-LLM-005:** Response Handling | ✅ Complete | Normalized to ContentBlock variants |
| **REQ-LLM-006:** Error Classification | 🚧 Extension pending | Base classification complete; split of transient throttle vs quota exhaustion vs model-overloaded tracked in task 67002 |
| **REQ-LLM-006a:** Plan-Aware Quota Messages (Codex Backend) | 📋 Planned | Task 67002 — parse codex 429 body + `x-codex-*` headers into structured `QuotaDetails`, render plan-aware messages matching codex CLI wording. Phases 2/3 (mid-stream SSE event, UI surface) tracked as 67003/67004. |
| **REQ-LLM-007:** Usage Tracking | ✅ Complete | Usage struct with token counts |
| **REQ-LLM-008:** Request Logging | ✅ Complete | LoggingService wrapper with tracing |
| **REQ-LLM-009:** Streaming Responses | ✅ Complete | Task 582. `complete_streaming()` on `LlmClient` trait, Anthropic implemented, OpenAI falls back |

**Progress:** 9 of 10 complete; REQ-LLM-006 extension + REQ-LLM-006a in flight (task 67002)
