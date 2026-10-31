# Codex Rate-Limit Resets — Executive Summary

## Current Reality

Phoenix lists and manually redeems ChatGPT-account rate-limit reset credits
through two server endpoints that proxy the upstream Codex backend:

- `GET /api/codex/rate-limit-resets` returns `CodexResetCredits`
  (`account_id`, `available_count` as `u32`, and `credits`, with Unix-second
  times and explicit `unknown` reset types and statuses).
- `POST /api/codex/rate-limit-resets/consume` takes `CodexResetRedeemRequest`
  (`idempotency_key` as a UUID, an optional non-empty `CodexResetCreditId`, and
  the listing's `account_id`) and returns `CodexResetRedeemResponse`
  (`outcome`, `windows_reset`).

Both endpoints answer `409` with `error_type: "codex_signed_out"` only when no
ChatGPT credential is loaded. Any other failure, including a token refresh
failure, is `502` with `error_type: "codex_upstream_unavailable"`. The consume
endpoint answers `409` with `error_type: "codex_account_changed"`, without
contacting the upstream, when the token's account differs from the listing's.
Each upstream request takes its token and account identifier from one
credential snapshot. The endpoints share the authenticated usage read's
publication lock and its refresh-once-on-401 policy.

The quota-exhaustion error banner shows "N resets available · Use reset" when
the listing reports a positive count. `ConversationPage` keys the banner by
conversation, so a redemption in flight cannot resume a different conversation
after navigation. The settings Codex section always shows the reset count ("No
resets available" at zero), and shows "Resets unavailable" when the listing
fails. Both surfaces share one choose, confirm, and redeem flow. After a failed
redemption it offers only Try again (same key) or Close. A successful
redemption from the banner sends "continue" while the banner is still mounted.
Phoenix never redeems without a confirmation.

## Status

| Requirement | Status | Current evidence |
| --- | --- | --- |
| REQ-CRR-001 Listing | Complete | `codex_rate_limit_resets`, `reset_credits_from_upstream` tests in `phoenix_llm::codex_reset_credits` |
| REQ-CRR-002 Redemption | Complete | `codex_rate_limit_reset_consume`; `redeem_response_from_upstream` and `UpstreamRedeemRequest` tests; `CodexResetRedeemRequest` rejects a non-UUID key and an empty credit id (serde tests in `phoenix_core::domain::codex_reset_credits`). Axum answers the rejection as `422` with its own body, not the typed `ErrorResponse` |
| REQ-CRR-003 Refresh on 401 | Complete, untested | Shared `codex_backend_json` refresh-once loop; no handler-level test because the upstream URLs are fixed constants |
| REQ-CRR-004 No automatic redemption | Complete | Only the confirm and Try again actions in `CodexResetFlow` call `api.codexRedeemReset` |
| REQ-CRR-005 Error banner availability | Complete | `ErrorBanner` renders `CodexResetAction surface="error"`; `CodexResetAction.test.tsx` |
| REQ-CRR-006 Choose and confirm | Complete | `resetOptions`, `CodexResetFlow` tests (cancel paths, no go-back after failure) |
| REQ-CRR-007 Outcome handling | Complete | `CodexResetFlow` outcome, signed-out, account-changed, and notice-retention tests |
| REQ-CRR-008 Resume after redemption | Complete | Banner keyed by conversation in `ConversationPage`; `CodexResetFlow` calls `onSucceeded` only while mounted; keyed-remount and unmount tests. No end-to-end navigation test |
| REQ-CRR-009 Settings availability | Complete | `SettingsDropdown` Codex section renders `CodexResetAction surface="settings"` keyed by account |
| REQ-CRR-010 Account scope | Complete | `codex_backend_json` `required_account` refuses a mismatched redemption before sending (`reset_redemption_for_another_account_is_refused_before_sending`); quota refresh discards results after `clearCodexQuota`; publication lock shared with the usage read |

## Boundaries

- `specs/llm/` owns quota-exhaustion classification and the plan-aware message.
- The runtime's usage-limit sweep owns clearing an elapsed usage-limit error.
- This spec owns reset listing, manual redemption, and the post-redemption
  resume gate.
- Not in scope: automatic redemption, purchased resets, and Luna Reserve
  fallback.
