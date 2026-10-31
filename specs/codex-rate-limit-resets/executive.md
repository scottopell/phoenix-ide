# Codex Rate-Limit Resets — Executive Summary

## Current Reality

Phoenix lists and manually redeems ChatGPT-account rate-limit reset credits
through two server endpoints that proxy the upstream Codex backend:

- `GET /api/codex/rate-limit-resets` returns `CodexResetCredits`
  (`available_count` and `credits`, with Unix-second times and explicit
  `unknown` reset types and statuses).
- `POST /api/codex/rate-limit-resets/consume` takes `CodexResetRedeemRequest`
  (`idempotency_key` as a UUID, plus an optional `credit_id`) and returns
  `CodexResetRedeemResponse` (`outcome`, `windows_reset`).

Both endpoints answer `409` with `error_type: "codex_signed_out"` when no
ChatGPT credential is loaded, and `502` with `error_type:
"codex_upstream_unavailable"` for upstream failures. Each upstream request
takes its token and account identifier from one credential snapshot. The
endpoints share the authenticated usage read's publication lock and its
refresh-once-on-401 policy.

The quota-exhaustion error banner shows "N resets available · Use reset" when
the listing reports a positive count. The settings Codex section always shows the
reset count ("No resets available" at zero), and shows "Resets unavailable"
when the listing fails. Both
surfaces share one choose, confirm, and redeem flow. A successful redemption
from the banner sends "continue" if the conversation is still in its
usage-limit error. Phoenix never redeems without a confirmation.

## Status

| Requirement | Status | Current evidence |
| --- | --- | --- |
| REQ-CRR-001 Listing | Complete | `codex_rate_limit_resets`, `reset_credits_from_upstream`, unit tests in `phoenix_llm::codex_reset_credits` |
| REQ-CRR-002 Redemption | Complete | `codex_rate_limit_reset_consume`, `redeem_response_from_upstream` tests (known codes, unknown code rejected), `UpstreamRedeemRequest` body test |
| REQ-CRR-003 Refresh on 401 | Complete | Shared `codex_backend_json` refresh-once loop (no handler-level test: upstream URLs are fixed constants) |
| REQ-CRR-004 No automatic redemption | Complete | Only the confirm action calls `api.codexRedeemReset` |
| REQ-CRR-005 Error banner availability | Complete | `ErrorBanner` renders `CodexResetAction surface="error"`; `CodexResetAction.test.tsx` |
| REQ-CRR-006 Choose and confirm | Complete | `resetOptions`, `CodexResetFlow` tests |
| REQ-CRR-007 Outcome handling | Complete | `CodexResetFlow` outcome tests |
| REQ-CRR-008 Resume after redemption | Complete | `ErrorBanner` passes its Retry action as `onRedeemed`; `CodexResetFlow` calls it only while mounted; banner integration and unmount tests |
| REQ-CRR-009 Settings availability | Complete | `SettingsDropdown` Codex section renders `CodexResetAction surface="settings"` beside `CodexQuotaBlock` |
| REQ-CRR-010 Account scope | Complete | Endpoints hold `CodexLoginManager::lock_publication` |

## Boundaries

- `specs/llm/` owns quota-exhaustion classification and the plan-aware message.
- The runtime's usage-limit sweep owns clearing an elapsed usage-limit error.
- This spec owns reset listing, manual redemption, and the post-redemption
  resume gate.
- Not in scope: automatic redemption, purchased resets, and Luna Reserve
  fallback.
