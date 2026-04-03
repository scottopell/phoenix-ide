---
created: 2026-02-28
priority: p1
status: done
artifact: completed
---

# Exhaustive Error Enums

## Context

Read first:
- `specs/bedrock/design.md` — "Error Handling and Retry" section and Appendix A (FM-3)
- `specs/llm/design.md` — "Error Types (REQ-LLM-006)" section
- `specs/llm/requirements.md` — REQ-LLM-006

FM-3 from the architecture review: `ServerError` (5xx) hit a catch-all
`_ => ErrorKind::Unknown`. `Unknown` is not retryable. The SM treated transient server
errors as permanent failures. The fix: no `Unknown` variant, no `_ =>` match arms.

## What to Do

1. **Find `LlmErrorKind`** (or equivalent) in the Rust codebase. Remove the `Unknown`
   variant. Add any missing explicit variants needed to cover all HTTP status classes
   the provider adapters encounter. The spec calls for at minimum:
   - `Network` (timeout, connection reset)
   - `RateLimit` (429)
   - `ServerError` (5xx)
   - `Auth` (401, 403)
   - `InvalidRequest` (400)
   - `ContentFilter` (safety/content blocks)
   - `ContextWindowExceeded` (token limit)

2. **Find every match arm** on the error kind enum. Remove all `_ =>` catch-alls and
   `Unknown =>` arms. Replace with explicit handling for each variant. The compiler will
   find them all for you once `Unknown` is removed.

3. **Find the HTTP status → error kind mapping** in each provider adapter (Anthropic,
   OpenAI, Fireworks). Make the match exhaustive. Any status code that was previously
   mapped to `Unknown` needs an explicit, intentional classification. If genuinely
   uncertain, `ServerError` (retryable) is safer than `Unknown` (not retryable).

4. **Update `is_retryable()`** to cover all variants explicitly. No default arm.

5. **Find the `ErrorKind` enum** used for the SM's `Error` state (may be the same enum
   or a separate one). Apply the same treatment — remove `Unknown`, add missing variants
   per `specs/bedrock/design.md` ConversationStates section:
   - `Auth`, `RateLimit`, `Network`, `ServerError`, `InvalidRequest`
   - `ContextExhausted`, `TimedOut`, `Cancelled`

## Acceptance Criteria

- `Unknown` does not appear in any error enum in the codebase
- No `_ =>` match arms on error kind enums (grep for `_ =>` near error kind matches)
- `./dev.py check` passes (clippy, fmt, tests)
- Property tests still pass (`cargo test proptests`)

## Files Likely Involved

- `src/llm/` — error types, provider adapters
- `src/state_machine/` — ErrorKind, transition logic
- `src/runtime/` — error mapping in executor
