---
created: 2026-04-24
priority: p2
status: ready
artifact: src/llm/models.rs
---

## Problem

The exe.dev LLM gateway (`http://169.254.169.254/gateway/llm`) does not support
the `anthropic-beta: context-1m-2025-08-07` header required by the
`claude-sonnet-4-6-1m` and `claude-opus-4-6-1m` model variants. When Phoenix
sends a request with this header, the gateway returns HTTP 200 with an empty SSE
stream (`stop_reason=None`, `output_tokens=0`) rather than a 4xx error — causing
silent retries and a cryptic user-visible error.

Diagnosed from prod conversation `8f82c521` (2026-04-24). The empty-response
error was reclassified as retryable in commit `6114fb5`, but the root cause
(unsupported beta header reaching the gateway) is unaddressed.

## Goal

A mechanism to discover and encode gateway capability constraints so Phoenix can
avoid sending unsupported headers, or show a clear error instead of a retry loop.

## Options to evaluate

1. **Static config map** (simplest): keyed by gateway URL pattern. Encode which
   beta headers each gateway supports. The exe.dev gateway is always
   `http://169.254.169.254/gateway/llm` — map it to: no `context-1m` beta.
   Hide or downgrade 1M model variants at `/api/models` when this gateway is
   active.

2. **Gateway capabilities probe**: `GET /gateway/capabilities` on startup — if
   the endpoint exists, parse supported features; if not, fall back to safe
   defaults. Ask exe.dev team to implement.

3. **Header probe on first 1M use**: send the request, detect the empty-stream
   failure, cache "gateway does not support context-1m", retry with base model.

## Acceptance criteria

- Selecting `claude-sonnet-4-6-1m` via the exe.dev gateway either works or shows
  a clear "model not supported by this gateway" message — not a retry loop.
- Mechanism is extensible to future gateway capability gaps.
