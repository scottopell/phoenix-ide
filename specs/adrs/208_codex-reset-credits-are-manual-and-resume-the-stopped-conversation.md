# ADR-208: Codex reset credits are redeemed manually through a Phoenix proxy, and a redemption from the quota error resumes that conversation

- **Status:** Accepted
- **Date:** 2026-10-31
- **Affects:** REQ-CRR-001–010; REQ-LLM-006a (adjacent, unchanged)

## Context

ChatGPT-authenticated Codex accounts can hold rate-limit reset credits.
Redeeming one restores exhausted Codex usage windows. Phoenix's Codex bridge
already classifies quota exhaustion as a user-resumable `usage_limit_reached`
error, shows a plan-aware message, and clears the error once the window
elapses. It has no awareness of reset credits, so a user holding resets has to
switch to another Codex client to spend one.

The open-source Codex client (`codex-rs`) defines the upstream contract:
`GET /wham/rate-limit-reset-credits` to list,
`POST /wham/rate-limit-reset-credits/consume` with an idempotency key to redeem,
and four outcome codes. In that client, redemption is always manual: the only
redeem calls sit behind a picker and a "Use this reset?" confirmation. Nothing
in the client, its configuration, or the account settings response models an
automatic-redemption preference.

The ChatGPT bearer token never leaves the Phoenix server, so the browser cannot
call the upstream directly. Phoenix has exactly one loaded Codex credential at a
time, and that credential can change through sign-in, sign-out, or account
switching.

Decisions that were open:

1. Whether Phoenix ever redeems automatically.
2. Where the redemption request originates and who owns the follow-on resume.
3. What happens to the stopped conversation after a successful redemption.

## Options considered

1. **Automatic redemption on quota exhaustion.** Phoenix spends a reset as soon
   as a turn hits the limit. This removes all friction, but it spends a scarce
   entitlement without consent, and nothing upstream defines a preference to
   gate it. Rejected for this decision; a future opt-in would need its own
   requirement and ADR.
2. **Server-side "redeem and resume" conversation action.** One endpoint scoped
   to a conversation redeems and then injects the resume turn. This ties the
   resume to the redemption even if the browser disappears mid-flow. However,
   it couples an account-level operation to conversation state, needs a second
   endpoint shape for the settings surface (which must not resume anything),
   and duplicates the user-resume path that the error's Retry action already
   owns.
3. **Account-level proxy endpoints plus client-orchestrated resume.** The
   server exposes list and redeem as account operations with no conversation
   coupling. The error banner orchestrates choose, confirm, redeem, and then
   resume through the existing Retry path, but only while the conversation
   still shows the same error. The settings surface uses the same endpoints and
   never resumes.
4. **Manual redemption with no resume.** After a successful redemption the
   banner clears and the user clicks Retry. This is the simplest option and
   never surprises, but it asks for a second click when the confirmation
   already expressed the intent to continue.

## Decision

Option 3. Redemption is manual only (REQ-CRR-004). Phoenix exposes
`GET /api/codex/rate-limit-resets` and
`POST /api/codex/rate-limit-resets/consume` as thin, typed proxies over the
upstream contract. Each upstream request takes its token and account
identifier from one credential snapshot, and the endpoints hold the same
publication lock as the authenticated usage read, which orders them against
publishing a newly signed-in credential (REQ-CRR-010). The redeem body's
idempotency key is typed as a UUID, so a malformed key cannot be
represented.

A successful redemption started from a conversation's quota error resumes that
conversation through the error's existing Retry path (sending "continue"). The
user chose this behavior over a second click. It is gated on the conversation
still being in the same error state when the redemption returns, so a
redemption whose response arrives after the user retried, dismissed, or the
sweep cleared the error never starts an extra turn (REQ-CRR-008).

Option 3 beats option 2 because the account operation stays a single,
reusable contract for both surfaces, and the resume reuses the one user-resume
path instead of adding a server-side duplicate. Losing the resume when the tab
closes mid-redemption is acceptable: the reset is still applied, and the
conversation remains in a resumable error with Retry available.

## Consequences

- **Positive:** Users can see and spend resets without leaving Phoenix. The
  upstream idempotency key and the `nothing_to_reset` and `already_redeemed`
  outcomes make retries and stale banners harmless. Neither surface can spend a
  credit without a confirmation.
- **Negative:** The resume depends on the browser staying on the conversation
  until the redemption returns. If two conversations are stopped on the same
  account, redeeming from one resumes only that one; the others keep their
  error until the user retries them or the sweep clears them. The upstream
  endpoints are not a published OpenAI API, so they can change without notice,
  and Phoenix tracks them by following `codex-rs`.
- **Neutral:** If the account changes between the error and the redemption,
  the redemption applies to the newly loaded account. In the common case that
  account is not exhausted, so the outcome is `nothing_to_reset` and no credit
  is spent.

## References

- `specs/codex-rate-limit-resets/requirements.md`
- `specs/llm/requirements.md` REQ-LLM-006, REQ-LLM-006a (quota exhaustion
  classification and message)
- ADR-062 and ADR-085 (Codex account-bound routing and identity)
- Upstream: `codex-rs/backend-client/src/client/rate_limit_resets.rs`
  (`Client::list_rate_limit_reset_credits`,
  `Client::consume_rate_limit_reset_credit_request`),
  `codex-rs/backend-client/src/types.rs`
  (`ConsumeRateLimitResetCreditCode`),
  `codex-rs/tui/src/chatwidget/usage.rs` (`finish_rate_limit_reset_consume`),
  `codex-rs/tui/src/chatwidget/reset_credits.rs` (`reset_credit_options`)
- Code: `fetch_codex_quota`, `codex_quota` (the authenticated usage read whose
  locking and refresh policy these endpoints share)
