OpenAI provider hardcodes cache_creation_tokens/cache_read_tokens to 0 while the sibling Anthropic provider threads them correctly — silent token/cost-accounting data loss with no log and no typed sink.

## Verified locations
- crates/phoenix-ide/src/llm/openai.rs:779-780 — non-streaming `normalize`: `cache_creation_tokens: 0, cache_read_tokens: 0` hardcoded into `Usage`
- crates/phoenix-ide/src/llm/openai.rs:339-354 — streaming accumulator reads only `input_tokens`/`output_tokens` from `/response/usage`
- crates/phoenix-ide/src/llm/openai.rs:429-431 + 1026-1029 — `ResponsesApiUsage` is defined with only `input_tokens`/`output_tokens` (two fields), no comment explaining the omission
- crates/phoenix-ide/src/llm/types.rs:283-294 — shared `Usage` has all four fields and `context_window_used()` (types.rs:293) sums all four (`input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens`)

## Why egregious
The OpenAI Responses API reports `usage.input_tokens_details.cached_tokens` on the wire; it is silently discarded. There is no `tracing::debug!` recording the gap, and `ResponsesApiUsage`'s two-field shape is not a typed sink (no comment, no provider-capability type) — so "OpenAI doesn't report this" is structurally indistinguishable from "we forgot to parse it" (violates omission-is-data-loss and capability-gaps-are-logged). It silently corrupts token/cost accounting for every OpenAI-backed conversation.

## Correct sibling pattern
Anthropic provider threads all four: crates/phoenix-ide/src/llm/anthropic.rs:74-79 parses `cache_creation_input_tokens`/`cache_read_input_tokens` in the streaming accumulator and anthropic.rs:292-293 emits them; the non-streaming path threads them too.

## Fix direction
Add `cached_tokens` to `ResponsesApiUsage`, parse `usage.input_tokens_details.cached_tokens` in both the streaming accumulator (openai.rs ~339) and non-streaming `normalize` (openai.rs ~776).

IMPORTANT — avoid double counting: OpenAI's `input_tokens` already *includes* `cached_tokens` (cached is a detail/subset of input), whereas `Usage::context_window_used()` (types.rs:293) sums `input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens`. So naively setting `cache_read_tokens = cached_tokens` while leaving `input_tokens` as-is double-counts the cached portion. The mapping must either (a) set `input_tokens = openai.input_tokens - cached_tokens` and `cache_read_tokens = cached_tokens`, or (b) leave `input_tokens` whole and keep `cache_read_tokens = 0`, surfacing the cached count through a separate non-summed field. Pick one explicitly and add a test pinning `context_window_used()` for a known OpenAI usage payload.

The Responses API exposes no cache-*creation* concept; make `cache_creation_tokens` a typed/commented sink rather than a bare `0`, and emit a `tracing::debug!` where a capability gap is hit.

## Related tasks
- None found tracking OpenAI usage/cache-token accounting.
