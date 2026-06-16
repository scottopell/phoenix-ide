Phase 3 of codex quota awareness (depends on 67002 + 67003).

Once 67002 lands structured `UsageLimitReached` errors with `QuotaDetails`, and 67003 surfaces continuous `RateLimitSnapshot` chunks, the UI can grow:

1. Inline status row near the model picker: "Weekly: 87% used, resets Sun 8 PM" (the most-urgent of primary/secondary windows)
2. Error display for `UsageLimitReached`: render plan-aware message + clickable promo_message link if present
3. Credits-depleted badge when `credits.has_credits == false`
4. Per-limit display when `limit_id != "codex"` (multi-model plans show "gpt-5.2-codex-sonic: 87%" instead of a single global number)

Reference: codex CLI TUI implementation — `/tmp/codex/codex-rs/tui/src/status/rate_limits.rs` (`rate_limit_snapshot_display`, `rate_limit_snapshot_display_for_limit`), `tui/src/chatwidget.rs:2693+` (snapshot consumption + display selection).

Follow UI Design Philosophy in AGENTS.md: inline status, progressive disclosure, no extra chrome.
