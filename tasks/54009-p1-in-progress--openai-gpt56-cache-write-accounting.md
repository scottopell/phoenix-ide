# Implement GPT-5.6 OpenAI cache write accounting and direct breakpoint support

## Context

GPT-5.6 introduces more predictable prompt caching, including explicit cache breakpoints and a 30-minute minimum cache life. Its usage payload includes cache-write accounting in `input_tokens_details.cache_write_tokens`, and pricing bills cache writes at 1.25x the model's uncached input rate while cache reads continue to receive the 90% cached-input discount.

Phoenix's OpenAI Responses normalization currently only parses `input_tokens_details.cached_tokens` and treats OpenAI cache creation as unavailable by hardcoding `Usage.cache_creation_tokens = 0`. That is stale for GPT-5.6 and later models.

## Scope

Update the direct OpenAI Responses path and provider specs for GPT-5.6-era cache accounting and explicit breakpoint support. Integrate the breakpoint design with stale tool-result clearing (`specs/stale-tool-results/`): its monotonic watermark, maximal batched sweeps, and protected recent-round floor exist specifically to bound cache invalidation and keep the compacted prefix byte-stable between sweeps.

OpenAI's documented Responses wire shape is not symmetric with Anthropic's: `prompt_cache_breakpoint` is supported only on `input_text`, `input_image`, and `input_file` blocks, not on `function_call_output`/tool-result blocks. Do not mechanically copy Anthropic's trailing-`tool_result` breakpoint. Choose and specify a valid OpenAI anchor strategy that preserves the compaction feature's stable-prefix guarantees, and treat unsupported block placement as a structural provider capability difference.

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
5. Implement the documented OpenAI Responses request shape:
   - request-wide `prompt_cache_options` (`implicit` or `explicit`; TTL is currently only `30m`),
   - per-block `prompt_cache_breakpoint: { "mode": "explicit" }`,
   - markers only on supported `input_text`, `input_image`, and `input_file` blocks,
   - no markers on `function_call_output` or other unsupported blocks, which OpenAI rejects with HTTP 400,
   - no explicit-cache fields for older models that reject them.
6. Extend Phoenix's provider-neutral request model without making invalid states representable:
   - Anthropic continues to emit per-block `cache_control`.
   - OpenAI emits only its supported breakpoint shape.
   - Provider capability/model eligibility and unsupported placement must be represented structurally or rejected explicitly, never silently dropped.
7. Design OpenAI breakpoint placement together with stale tool-result clearing (`specs/stale-tool-results/requirements.md` and `stale-tool-results.allium`):
   - preserve the persisted monotonic watermark and request-only cleared rendering as the source of the stable compacted prefix,
   - preserve maximal batched sweeps and the protected recent-round floor; do not introduce per-turn prefix mutation,
   - after a sweep, allow the newly compacted prefix to warm once and remain reusable while the watermark holds,
   - identify a valid cacheable OpenAI content block at or after the intended stable boundary rather than attempting to annotate a tool result,
   - define behavior for tool-loop requests where the trailing user-role item is solely a `function_call_output` and therefore cannot carry an OpenAI breakpoint,
   - keep `PromptCacheKey::stable(conv_id)` unchanged so later turns remain in the same cache cohort,
   - account for OpenAI's four new-write limit, implicit mode consuming one write slot, and reads considering up to the latest 50 historical breakpoints.
8. Add cross-feature tests around a compaction sweep: wire markers are valid, the compacted prefix is stable on later no-sweep turns, recent tool rounds remain verbatim, unsupported tool-result blocks are unmarked, and model-ineligible requests retain legacy automatic caching.
9. Update `specs/llm/*.allium` and the stale-tool-results cache-interaction specification so they describe OpenAI explicit breakpoints and no longer assume OpenAI is automatic-prefix-only or has no cache-creation concept. When touching the legacy `specs/stale-tool-results/design.md`, migrate normative behavior/rationale into the proper v2 artifact rather than adding new authority to the legacy design document.

## Acceptance Criteria

- [ ] OpenAI Responses usage parsing captures `cache_write_tokens`.
- [ ] Phoenix stores/report cache-write tokens as `Usage.cache_creation_tokens` for OpenAI Responses turns.
- [ ] Context-window accounting remains non-double-counted when OpenAI reports cached-read and cache-write detail buckets.
- [ ] Streaming and non-streaming OpenAI paths share the same cache accounting semantics.
- [ ] GPT-5.6 Sol/Terra/Luna usage cost estimates use parsed cache-write/read buckets and the configured model pricing.
- [ ] OpenAI Responses requests emit documented explicit breakpoints only on supported cacheable block types and emit no unsupported fields for older models.
- [ ] Breakpoint placement is compatible with stale tool-result clearing: a maximal sweep causes at most the intended one-time prefix rewrite, no-sweep turns preserve the compacted prefix bytes, and the protected recent rounds remain unmodified.
- [ ] Tool-loop histories ending in `function_call_output` have specified and tested behavior without placing an invalid breakpoint on that block.
- [ ] Tests cover request-wide mode/write-slot behavior, the four-write limit, and the interaction between implicit and explicit breakpoints.
- [ ] Specs/tests are updated so Responses cache creation is no longer documented as permanently zero or as automatic-prefix-only.
- [ ] `./dev.py check` passes.
