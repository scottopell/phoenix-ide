---
created: 2026-05-10
priority: p2
status: ready
artifact: crates/phoenix-ide/src/llm/openai.rs
---

# Codex/ChatGPT quota awareness — parse structured 429 payload, plan-aware messages

## Problem

In prod (codex backend, `chatgpt.com/backend-api/codex`) Phoenix
surfaces opaque "Rate limit exceeded: {body}" strings on 429 (see
`crates/phoenix-ide/src/llm/openai.rs:101`). The user can't tell
whether this is:

- a transient per-minute throttle (retry in 30s)
- a weekly usage cap on their ChatGPT Plus plan (retry next Sunday)
- a credits-depleted state (no retry will help; need to buy credits)
- a workspace-member limit (admin action required)

`OpenAIErrorResponse` at `openai.rs:97` is `{ error: { message } }` —
that schema drops the structured `type` / `plan_type` / `resets_at`
fields *before* we ever read them. Fix is to add a second
deserialization attempt against a Codex-shaped struct, plus parse the
response headers, before falling through to the generic message.

## Decided scope

This task ships **429 parsing + structured terminal error variants only**.

Explicitly **out of scope** for this task (filed as follow-ups):

- Mid-stream `codex.rate_limits` SSE event (pre-429 awareness path)
  → separate task; needs new `TokenChunk` variant + runtime threading
  + UI surface to be useful
- UI rendering of quota state (status row, badges)
- Persisting `RateLimitSnapshot` across turns

## Decisions

| Decision | Choice |
|---|---|
| Error model | Split — add new `UsageLimitReached` + `ServerOverloaded` variants to `LlmErrorKind` (and matching `LlmOutcome` / `db::ErrorKind`). Transient `RateLimit` stays retryable; `UsageLimitReached` is terminal. |
| Wording | Copy codex CLI's `UsageLimitReachedError::fmt` strings verbatim (`/tmp/codex/codex-rs/protocol/src/error.rs:453-517`). End-user text, no contract. |
| Spec format | spEARS only — extend REQ-LLM-006, add new REQ for plan-aware quota. No Allium (no state machine / lifecycle). |
| Codex SSE event | Deferred — file as separate task. |
| Mid-stream snapshot persistence | Deferred. |

## Codex CLI reference (clone via `git clone --depth=1 https://github.com/openai/codex.git /tmp/codex`)

All paths under `/tmp/codex/codex-rs/`.

### 429 body parser — canonical

`codex-api/src/api_bridge.rs:80-106`. On `HTTP 429`:

```json
{
  "error": {
    "type": "usage_limit_reached" | "usage_not_included",
    "plan_type": "plus" | "pro" | "free" | "team" | "business" | ...,
    "resets_at": 1709568000   // unix seconds
  }
}
```

Branch logic:
- `type == "usage_limit_reached"` → `CodexErr::UsageLimitReached` with
  plan + reset + headers + promo_message
- `type == "usage_not_included"` → `CodexErr::UsageNotIncluded`
  (upgrade required, not retryable)
- Otherwise → fall through to transient `RetryLimit` (the per-minute
  throttle case)

Also from the same file:
- HTTP 503 + `error.code in {server_is_overloaded, slow_down}` →
  `ServerOverloaded` (terminal-ish, distinct from generic 5xx)
- HTTP 400 + `error.code == "cyber_policy"` → policy block
  (skip for now; we don't need a separate variant yet)

### Header parser

`codex-api/src/rate_limits.rs:56-98` (`parse_rate_limit_for_limit`).
Response headers carry the structured snapshot:

- `x-codex-active-limit: codex` — which limit family was hit
- `x-codex-{limit}-primary-used-percent: 87.4`
- `x-codex-{limit}-primary-window-minutes: 10080` (weekly = 7d×24h×60)
- `x-codex-{limit}-primary-reset-at: 1709568000`
- `x-codex-{limit}-secondary-used-percent: ...`
- `x-codex-{limit}-secondary-window-minutes: ...`
- `x-codex-{limit}-secondary-reset-at: ...`
- `x-codex-{limit}-limit-name: gpt-5.2-codex-sonic`
- `x-codex-credits-has-credits: true|false`
- `x-codex-credits-unlimited: true|false`
- `x-codex-credits-balance: "$3.42"`
- `x-codex-promo-message: "Upgrade to Pro at chatgpt.com/explore/pro"`

### Error type to mirror

`protocol/src/error.rs:446-517`:

```rust
pub struct UsageLimitReachedError {
    pub plan_type: Option<PlanType>,
    pub resets_at: Option<DateTime<Utc>>,
    pub rate_limits: Option<Box<RateLimitSnapshot>>,
    pub promo_message: Option<String>,
}
```

`Display` impl produces the plan-aware strings — copy wording verbatim:

- **Plus** → "You've hit your usage limit. Upgrade to Pro
  (https://chatgpt.com/explore/pro), visit
  https://chatgpt.com/codex/settings/usage to purchase more credits.
  Try again at 3:42 PM."
- **Pro / ProLite** → "Visit https://chatgpt.com/codex/settings/usage
  to purchase more credits…"
- **Team / Business / Enterprise-CBP** → "send a request to your admin…"
- **Free / Go** → "Upgrade to Plus…"
- **Enterprise / Edu** → "You've hit your usage limit. Try again at $time."
- **Unknown plan / None** → "You've hit your usage limit. Try again later."
- **Promo message present** → "You've hit your usage limit. $promo,
  Try again at $time."
- **`limit_name` present and != "codex"** → "You've hit your usage
  limit for $limit_name. Switch to another model now, or try again
  at $time."

Reset time formatter: `protocol/src/error.rs:537-560`
(`format_retry_timestamp`, `day_suffix`). Renders as `HH:MM AM/PM` for
same-day, `Mon DDth, YYYY HH:MM AM/PM` otherwise.

`RateLimitSnapshot` / `RateLimitWindow` shape:
`protocol/src/protocol.rs:2070-2101`.

Plan enum:
`codex-backend-openapi-models/src/models/rate_limit_status_payload.rs:89-125`.

## Phoenix implementation

### Layer 1 — `LlmErrorKind` (`crates/phoenix-ide/src/llm/error.rs`)

Add two new variants:

```rust
pub enum LlmErrorKind {
    Network,
    RateLimit,                  // transient, retryable (unchanged)
    UsageLimitReached,          // NEW — terminal, structured payload
    ServerError,
    ServerOverloaded,           // NEW — terminal, distinct from 5xx
    Auth,
    InvalidRequest,
    ContentFilter,
    ContextWindowExceeded,
}
```

`is_retryable()`:
- `RateLimit` retryable (unchanged) — transient throttle
- `UsageLimitReached` **not** retryable — quota window
- `ServerOverloaded` **not** retryable — try different model

Carry the structured payload alongside the `LlmError` message:

```rust
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,        // pre-rendered plan-aware string for display
    pub recovery_in_progress: bool,
    pub quota: Option<QuotaDetails>,  // NEW — present iff UsageLimitReached
}

pub struct QuotaDetails {
    pub plan_type: Option<String>,        // raw string from backend; don't enum
    pub resets_at: Option<DateTime<Utc>>,
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub credits: Option<CreditsSnapshot>,
    pub promo_message: Option<String>,
}
```

Define `RateLimitWindow` / `CreditsSnapshot` minimally in a new
`crates/phoenix-ide/src/llm/rate_limit.rs` mirroring codex's
`protocol/src/protocol.rs:2070-2101`. **Don't** depend on
`codex_protocol` / `codex_api` — they're heavy crates and we only
need three structs.

Treat `plan_type` as `String` for now (we display it, not match on it).
The plan→message branches use string equality on lowercase. Codex CLI
has a full enum (`KnownPlan`) — we don't need it until we have product
reasons to.

### Layer 2 — parser (`crates/phoenix-ide/src/llm/openai.rs`)

Gate **on `use_codex_backend == true` only** (lines 42, 253). Platform
Responses API stays generic — OpenAI's platform doesn't send these
headers.

Update both call sites:

- **Non-streaming**: `openai.rs:96-110`. Before the existing
  `OpenAIErrorResponse` parse, try Codex-shaped deserialization.
- **Streaming**: `openai.rs:288-295`. Currently falls through to
  `LlmError::from_http_status` — same parser, same gate.

Decision tree (mirror `api_bridge.rs:42-121`):

```
status == 429:
  body.error.type == "usage_limit_reached":
    → LlmError::usage_limit_reached(QuotaDetails {plan, reset, headers, promo})
       with pre-rendered plan-aware message
  body.error.type == "usage_not_included":
    → LlmError::auth("Upgrade required: …")     // 403-like terminal
  else:
    → LlmError::rate_limit(message)             // transient, retryable
status == 503 && body.error.code in {server_is_overloaded, slow_down}:
  → LlmError::server_overloaded("Selected model is at capacity. Try a different model.")
status == 500:
  → LlmError::server_error("We're currently experiencing high demand…")
otherwise:
  → existing from_http_status path
```

### Layer 3 — executor → state machine

Add `LlmOutcome::UsageLimitReached { details: QuotaDetails, message: String }`
and `LlmOutcome::ServerOverloaded { message: String }` to
`crates/phoenix-ide/src/state_machine/outcome.rs:21-51`.

Map in `runtime/executor.rs:3162-3209` (the `LlmErrorKind` → `db::ErrorKind`
and `LlmOutcome` mappers).

### Layer 4 — `db::ErrorKind` (`crates/phoenix-ide/src/db/schema.rs:470`)

Add `UsageLimitReached` and `ServerOverloaded` variants. Both
`is_retryable() == false`. Serde rename = snake_case (existing
convention).

### Layer 5 — state machine transition (`state_machine/transition.rs:2033-2049`)

Add branches to `llm_outcome_to_event` for the two new outcomes. Pass
the pre-rendered message and `error_kind` through to `Event::LlmError`.

The structured `QuotaDetails` payload is **not** persisted yet — for
this task we only need the message to display. A follow-up can add a
JSON column to `messages` for the snapshot if/when the UI grows badges.
(Recording the decision so we don't drift into "save it because we
have it.")

### Layer 6 — TS codegen

Re-run `./dev.py codegen` after touching `ErrorKind`. Frontend just
displays `message` today, so no UI work needed — the message field
now carries the plan-aware string.

## Acceptance criteria

- [ ] 429 from codex backend with `error.type == "usage_limit_reached"`
      produces `LlmError { kind: UsageLimitReached, message: <plan-aware
      string>, quota: Some(_) }` — wording matches codex CLI's
      `UsageLimitReachedError::fmt` for each plan variant
- [ ] 429 from codex backend without `usage_limit_reached` body still
      maps to retryable `LlmErrorKind::RateLimit` (the per-minute throttle)
- [ ] 429 with `usage_not_included` body → terminal `Auth`-variant
      error with upgrade message
- [ ] HTTP 503 + `server_is_overloaded` / `slow_down` body →
      `ServerOverloaded`, not generic 5xx
- [ ] Parsing **only** runs when `use_codex_backend == true`
- [ ] Both non-streaming (`openai.rs:80-110`) and streaming
      (`openai.rs:278-295`) paths parse identically
- [ ] `LlmOutcome::UsageLimitReached` / `ServerOverloaded` thread
      through executor → state machine → `db::ErrorKind`
- [ ] `is_retryable()` returns false for both new variants in every
      layer (LlmErrorKind, db::ErrorKind)
- [ ] Unit tests built from `codex-api/src/api_bridge_tests.rs` cover:
      Plus/Pro/Team/Free/Unknown plan, `resets_at` present/absent,
      `promo_message` present/absent, secondary window only,
      `usage_not_included`, `server_is_overloaded` 503, plain 429
      fallthrough (still RateLimit)
- [ ] Reset-time formatter matches codex CLI: same-day `"3:42 PM"`,
      cross-day `"Mar 3rd, 2026 3:42 PM"` (`protocol/src/error.rs:537-560`)
- [ ] No new heavy deps: `chrono` already in tree; do **not** pull
      `codex_protocol` or `codex_api`
- [ ] `./dev.py check` clean (codegen regenerated for new `ErrorKind`
      variants)

## Out of scope (filed as follow-ups)

- Mid-stream `codex.rate_limits` SSE event ingestion — separate task
- UI rendering of `QuotaDetails` (status row, percent badge,
  resets-in indicator) — separate task
- Persisting `QuotaDetails` to `messages` JSON for replay — separate
- Multi-account rotation when one hits its cap — won't do
- Translating same patterns to direct `auth.openai.com` /
  platform-OpenAI quotas — different wire format, lower priority

## Notes

- The reason we see opaque 429s right now: `OpenAIErrorResponse` at
  `openai.rs:97` strips structured fields before we read them. Fix is
  the second deserialization attempt against the Codex-shaped struct.
- `x-codex-active-limit` (limit_id) matters on multi-model plans —
  different models have independent quotas. Preserve it in
  `QuotaDetails` so a future UI can show "gpt-5.2-codex-sonic: 87%"
  rather than a single global number.
- Codex CLI persists snapshots in session state
  (`core/src/state/session.rs:127`) and renders via
  `tui/src/status/rate_limits.rs`. That's the model for Phase 2 (SSE
  event) + UI work, not this task.
