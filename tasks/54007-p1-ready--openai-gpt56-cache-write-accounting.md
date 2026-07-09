# Implement GPT-5.6 OpenAI cache write accounting and direct breakpoint support

## Context

GPT-5.6 introduces more predictable prompt caching, including explicit cache breakpoints and a 30-minute minimum cache life. Its usage payload includes cache-write accounting in `input_tokens_details.cache_write_tokens`, and pricing bills cache writes at 1.25x the model's uncached input rate while cache reads continue to receive the 90% cached-input discount.

Phoenix's OpenAI Responses normalization currently only parses `input_tokens_details.cached_tokens` and treats OpenAI cache creation as unavailable by hardcoding `Usage.cache_creation_tokens = 0`. That is stale for GPT-5.6 and later models.

## Scope

Update the direct OpenAI Responses path and provider specs for GPT-5.6-era cache accounting and explicit breakpoint support.

Primary code/spec surfaces:

- `crates/phoenix-llm/src/openai.rs`
  - `ResponsesApiInputTokensDetails`
  - streaming accumulator cache fields
  - `normalize_responses_api_response`
  - related tests/proptests
- `crates/phoenix-core/src/domain/llm_types.rs`
  - comments around `SystemContent.cache` and `PromptCacheKey`
- `specs/llm/responses.allium`
  - usage normalization rules that currently say cache creation is fixed at zero
- `specs/llm/llm.allium`
  - provider-agnostic cache semantics if OpenAI explicit breakpoints become representable
- `crates/phoenix-ide/src/api/usage.rs`
  - pricing already supports GPT-5.6 cache-write multipliers; verify end-to-end once usage normalization is fixed

## Implementation Notes

1. Parse `input_tokens_details.cache_write_tokens` from OpenAI Responses usage payloads.
2. Normalize token buckets so Phoenix's `Usage.context_window_used()` remains equal to the provider's effective input/output total without double-counting detail buckets.
   - Expected shape if OpenAI `input_tokens` includes both detail buckets:
     - `input_tokens = raw_input_tokens - cached_tokens - cache_write_tokens`
     - `cache_creation_tokens = cache_write_tokens`
     - `cache_read_tokens = cached_tokens`
   - Confirm this against direct OpenAI endpoint responses before locking the rule.
3. Update streaming accumulation to preserve both cached-read and cache-write details.
4. Add unit tests for non-streaming and streaming normalization with both `cached_tokens` and `cache_write_tokens` present.
5. Investigate direct OpenAI endpoint support for explicit cache breakpoints and model the actual supported wire shape.
6. If explicit breakpoints are supported, extend Phoenix's provider-neutral request model without making invalid states representable:
   - Anthropic should continue to emit per-block `cache_control`.
   - OpenAI should emit its direct endpoint's supported breakpoint shape.
   - Providers without segment breakpoint support should structurally ignore or reject unsupported breakpoint data rather than silently dropping it.
7. Update Allium/spec text so it no longer claims Responses has no cache-creation concept.

## Acceptance Criteria

- [ ] OpenAI Responses usage parsing captures `cache_write_tokens`.
- [ ] Phoenix stores/report cache-write tokens as `Usage.cache_creation_tokens` for OpenAI Responses turns.
- [ ] Context-window accounting remains non-double-counted when OpenAI reports cached-read and cache-write detail buckets.
- [ ] Streaming and non-streaming OpenAI paths share the same cache accounting semantics.
- [ ] GPT-5.6 Sol/Terra/Luna usage cost estimates use parsed cache-write/read buckets and the configured model pricing.
- [ ] Direct OpenAI explicit cache breakpoint support is verified and implemented if available.
- [ ] Specs/tests are updated so Responses cache creation is no longer documented as permanently zero.
- [ ] `./dev.py check` passes.
