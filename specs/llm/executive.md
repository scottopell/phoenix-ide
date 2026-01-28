# LLM Provider - Executive Summary

## Requirements Summary

The LLM provider abstracts communication with multiple LLM APIs behind a common interface. Users select their preferred model while the system handles provider-specific translation internally. When exe.dev gateway is configured, all models are registered as available since the gateway manages API keys; otherwise only models with locally configured API keys are available. Requests use a common format (system prompt, messages, tools) that gets translated per-provider. Responses are normalized to text blocks, tool use requests, end-of-turn indicators, and usage statistics. Errors are classified as retryable (network, rate limit) or non-retryable (auth) to enable appropriate state machine handling.

## Technical Summary

Implements `LlmService` trait with `complete()` method returning `LlmResponse`. Provider implementations (Anthropic, OpenAI, Fireworks) translate common request format to provider-specific JSON and normalize responses back. Gateway URLs constructed by appending provider suffix to base gateway URL. `ModelRegistry` registers all models when gateway configured, or only models with API keys in direct mode. `LlmError` includes `LlmErrorKind` enum with `is_retryable()` method. `LoggingService` wrapper records model, duration, and token counts. Usage tracking includes input/output tokens and cache statistics for context window computation.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-LLM-001:** Provider Abstraction | ❌ Not Started | LlmService trait |
| **REQ-LLM-002:** Gateway Support | ❌ Not Started | URL construction |
| **REQ-LLM-003:** Model Registry | ❌ Not Started | Available model enumeration |
| **REQ-LLM-004:** Request Format | ❌ Not Started | Common request structure |
| **REQ-LLM-005:** Response Handling | ❌ Not Started | Normalization logic |
| **REQ-LLM-006:** Error Classification | ❌ Not Started | LlmErrorKind enum |
| **REQ-LLM-007:** Usage Tracking | ❌ Not Started | Token counts, cost |
| **REQ-LLM-008:** Request Logging | ❌ Not Started | LoggingService wrapper |

**Progress:** 0 of 8 complete
