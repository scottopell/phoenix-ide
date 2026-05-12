Phase 2 of codex quota awareness (depends on 67002).

Codex backend emits a structured `type: "codex.rate_limits"` SSE event mid-stream on every turn carrying `used_percent`, `window_minutes`, `reset_at` for primary + secondary windows, plus credits + plan_type. Phoenix currently drops this event in `ResponsesStreamAccumulator::process_event` (openai.rs:143-222).

Parsing it into a new `TokenChunk::RateLimitSnapshot(QuotaDetails)` variant gives the UI a pre-429 awareness channel — codex CLI's TUI uses this for its weekly quota status row.

Reference: `/tmp/codex/codex-rs/codex-api/src/rate_limits.rs:131-162` (`parse_rate_limit_event`). Reuses the `QuotaDetails` / `RateLimitWindow` / `CreditsSnapshot` types from 67002.

Scope:
1. New `TokenChunk::RateLimitSnapshot` variant
2. Branch in `process_event` for `dispatch_type == "codex.rate_limits"`
3. Wire through to runtime broadcast → SSE → frontend (codegen)
4. Gated on `use_codex_backend == true`

Out of scope: UI rendering (separate task), session-state persistence across turns.
