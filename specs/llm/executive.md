# LLM Provider - Executive Summary

## Requirements Summary

The LLM provider abstracts communication with multiple LLM APIs behind a common interface. Users select their preferred model while the system handles provider-specific translation internally. When exe.dev gateway is configured, all models are registered as available since the gateway manages API keys; otherwise only models with locally configured API keys are available. Requests use a common format (system prompt, messages, tools) that gets translated per-provider. Responses are normalized to text blocks, tool use requests, end-of-turn indicators, and usage statistics. Errors are classified as retryable (network, rate limit) or non-retryable (auth) to enable appropriate state machine handling.

## Technical Summary

Implements `LlmService` trait with `complete()` method returning `LlmResponse`. Provider implementations (Anthropic, OpenAI, Fireworks) translate common request format to provider-specific JSON and normalize responses back. Gateway URLs constructed by appending provider suffix to base gateway URL. `ModelRegistry` registers all models when gateway configured, or only models with API keys in direct mode. `LlmError` includes `LlmErrorKind` enum with `is_retryable()` method. `LoggingService` wrapper records model, duration, and token counts. Usage tracking includes input/output tokens and cache statistics for context window computation.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-LLM-001:** Provider Abstraction | ✅ Complete | LlmService trait with async complete() |
| **REQ-LLM-002:** Gateway Support | ✅ Complete | Gateway URL construction for Anthropic |
| **REQ-LLM-003:** Model Registry | ✅ Complete | ModelRegistry with available_models() |
| **REQ-LLM-004:** Request Format | ✅ Complete | LlmRequest with system, messages, tools |
| **REQ-LLM-005:** Response Handling | ✅ Complete | Normalized to ContentBlock variants |
| **REQ-LLM-006:** Error Classification | ✅ Complete | LlmErrorKind with is_retryable() |
| **REQ-LLM-007:** Usage Tracking | ✅ Complete | Usage struct with token counts |
| **REQ-LLM-008:** Request Logging | ✅ Complete | LoggingService wrapper with tracing |

**Progress:** 8 of 8 complete
