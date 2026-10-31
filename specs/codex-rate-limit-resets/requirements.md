# Codex Rate-Limit Resets

## User Story

As a user whose ChatGPT plan grants usage-limit reset credits, I need to see how
many resets I hold and to spend one deliberately from Phoenix, so a conversation
stopped by Codex quota exhaustion can continue without waiting for the window to
roll over and without leaving Phoenix for another Codex client.

## Background

The Codex backend that serves ChatGPT-authenticated traffic grants accounts
**rate-limit reset credits**. Redeeming one restores the account's exhausted
Codex usage windows (for example the five-hour and weekly windows). The
upstream contract, as exercised by the open-source Codex client
(`codex-rs/backend-client`, `Client::list_rate_limit_reset_credits` and
`Client::consume_rate_limit_reset_credit_request`), is:

| Operation | Upstream request | Upstream response |
| --- | --- | --- |
| List | `GET https://chatgpt.com/backend-api/wham/rate-limit-reset-credits` | `{ credits: [{ id, reset_type, status, granted_at, expires_at, title, description, … }], available_count, … }` |
| Redeem | `POST https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume` with JSON `{ redeem_request_id, credit_id? }` | `{ code, windows_reset, … }` |

- `granted_at` is an RFC3339 string; `expires_at` is an RFC3339 string or
  `null` (the credit does not expire).
- `reset_type` is `codex_rate_limits` for every credit Phoenix understands;
  other values are unknown types.
- `status` is one of `available`, `redeeming`, `redeemed`; other values are
  unknown statuses.
- `code` is one of `reset`, `nothing_to_reset`, `no_credit`,
  `already_redeemed`. `windows_reset` counts restored windows and is absent on
  some responses (absent means zero).
- `redeem_request_id` is an idempotency key for one logical redemption. The
  backend answers a replay of a key that already redeemed with
  `already_redeemed` and does not spend a second credit.
- An omitted `credit_id` lets the backend choose the credit.
- The backend answers `nothing_to_reset` without spending a credit when the
  account has no exhausted window to restore.
- Both requests authenticate with the ChatGPT bearer token and the
  `chatgpt-account-id` header, exactly like the authenticated usage read
  (`/wham/usage`). An API-key credential cannot use either operation.

Unknown extra response fields are ignored. This spec covers manual redemption
only; Phoenix never redeems a reset credit without an explicit user
confirmation for that redemption.

Error classification for quota exhaustion and the plan-aware exhaustion
message belong to `specs/llm/requirements.md` (REQ-LLM-006, REQ-LLM-006a). The
clear-to-ready sweep that returns a usage-limit error to Idle after its window
elapses belongs to the runtime and is unchanged by this spec.

## Requirements

### REQ-CRR-001: Reset Credit Listing

WHEN the user interface requests the signed-in account's reset credits
THE SYSTEM SHALL request the upstream list operation using the loaded
ChatGPT credential and its account identifier
AND SHALL return the available count and every listed credit with its
identifier, reset type, status, grant time, expiry time, title, and description
AND SHALL represent grant and expiry times as Unix seconds, with a
non-expiring credit represented by an absent expiry
AND SHALL map unrecognised reset types and statuses to an explicit unknown
value rather than failing the whole listing
AND SHALL report a negative upstream available count as zero

WHEN a listed credit carries a grant or expiry time that is not valid RFC3339
THE SYSTEM SHALL fail the listing as an upstream failure rather than invent a
time

WHEN no ChatGPT credential is loaded
THE SYSTEM SHALL answer the listing with a signed-out response distinct from an
upstream failure, without contacting the upstream

WHEN the upstream request fails, times out, is rejected, or returns a body that
does not decode
THE SYSTEM SHALL answer with an upstream-failure response
AND SHALL log the failure cause at debug level or above

**Rationale:** Visibility is the precondition for any deliberate redemption.
The signed-out and upstream-failure answers are distinct because the first
means "there is nothing to show" while the second means "Phoenix could not
find out"; a client must not render either as "zero resets".

---

### REQ-CRR-002: Explicit Reset Redemption

WHEN the user interface submits a redemption with an idempotency key and an
optional credit identifier
THE SYSTEM SHALL send one upstream redeem request carrying that key as
`redeem_request_id` and the credit identifier when one was supplied
AND SHALL return the upstream outcome (`reset`, `nothing_to_reset`,
`no_credit`, `already_redeemed`) and the number of windows reset
AND SHALL require the idempotency key to be a UUID, rejecting any other value
as an invalid request before contacting the upstream

WHEN the upstream answers with an outcome code outside the four known codes
THE SYSTEM SHALL answer with an upstream-failure response and SHALL NOT
report the redemption as successful

WHEN no ChatGPT credential is loaded
THE SYSTEM SHALL answer with the signed-out response without contacting the
upstream

WHEN the redemption completes with outcome `reset` or `already_redeemed`
THE SYSTEM SHALL log the redemption at info level with the outcome and the
number of windows reset, and SHALL NOT log the bearer token

**Rationale:** Redemption spends a scarce, account-level entitlement. The
idempotency key makes a retried request after a lost response safe, so the key
must be a well-formed unique value chosen by the client for one logical
redemption.

---

### REQ-CRR-003: Credential Refresh on Rejection

WHEN the upstream rejects a list or redeem request as unauthorized
THE SYSTEM SHALL invalidate the cached access token, obtain a fresh one, and
retry the request once with the same idempotency key
AND SHALL answer with an upstream-failure response when the retry is also
rejected

**Rationale:** The same refresh-once policy governs the authenticated usage
read. Retrying a redemption is safe only because the key is reused.

---

### REQ-CRR-004: No Automatic Redemption

THE SYSTEM SHALL redeem a reset credit only as the direct result of the user
confirming that specific redemption in the confirmation step of REQ-CRR-006
AND SHALL NOT redeem a reset credit in response to quota exhaustion, a
retry, a restart, a conversation resume, or any background process

**Rationale:** Reset credits are scarce. Spending one is the user's decision,
including the decision to wait for the window instead.

---

### REQ-CRR-005: Reset Availability on the Quota-Exhaustion Error

WHEN a conversation is displayed in a user-resumable `usage_limit_reached`
error state, is neither read-only nor archived, and a ChatGPT credential is
loaded
THE SYSTEM SHALL fetch the reset credit listing for that error display
AND SHALL show the available reset count inline with the error, with an action
to use a reset, when the available count is greater than zero
AND SHALL show nothing about resets when the available count is zero, when the
account is signed out, or when the listing fails

**Rationale:** The quota-exhaustion error is where the user decides between
waiting, upgrading, and spending a reset; the option belongs beside the
existing plan-aware message. Failures stay silent here because the existing
Retry and Dismiss affordances remain the recovery path.

---

### REQ-CRR-006: Choose-and-Confirm Redemption Flow

WHEN the user chooses to use a reset
THE SYSTEM SHALL fetch a fresh reset listing before presenting the choice
AND SHALL present a choice of reset options built from that listing:
every credit whose status is `available`, ordered by soonest expiry with
non-expiring credits last, limited to the available count
AND SHALL label each option with the credit title (or "Full reset" when the
title is absent or blank) and its expiry ("Expires <local date and time>" or
"Does not expire")
AND SHALL offer a single unnamed option that lets the backend choose the
credit when the listing reports a positive available count but no `available`
credit details

WHEN the user selects an option
THE SYSTEM SHALL ask for confirmation, naming the chosen reset and showing its
description (or "Reset your current usage limits" when the description is
absent or blank)
AND SHALL allow the user to go back to the choice or cancel without redeeming

WHEN the user confirms
THE SYSTEM SHALL submit exactly one redemption for the chosen option under an
idempotency key created for that confirmation
AND SHALL disable further confirmation while the redemption is in flight

**Rationale:** Mirrors the upstream Codex client's list, choose, confirm flow so
users see the same choices and wording across Codex surfaces, while keeping the
spend gated behind a deliberate second step.

---

### REQ-CRR-007: Redemption Outcome Handling

WHEN a redemption answers `reset` or `already_redeemed`
THE SYSTEM SHALL treat the redemption as successful
AND SHALL refresh the displayed account quota and reset listing

WHEN a redemption answers `nothing_to_reset`
THE SYSTEM SHALL tell the user that their usage does not need a reset right
now, SHALL report that no reset was spent, and SHALL NOT resume any
conversation

WHEN a redemption of a chosen credit answers `no_credit`
THE SYSTEM SHALL tell the user that the reset is no longer available and
SHALL refresh the reset listing

WHEN a redemption without a chosen credit answers `no_credit`
THE SYSTEM SHALL tell the user that no usage limit resets are available

WHEN a redemption fails without an outcome (transport failure, upstream
failure, or timeout)
THE SYSTEM SHALL tell the user the reset could not be applied
AND SHALL offer to try again with the same idempotency key and credit choice,
so a redemption whose response was lost cannot spend a second credit

**Rationale:** The four outcome codes and their user-facing meanings are the
upstream contract. Reusing the key on retry turns a lost success into
`already_redeemed` instead of a second spend.

---

### REQ-CRR-008: Resume After Redemption From the Error

WHEN a redemption started from a conversation's quota-exhaustion error succeeds
AND the error display that started it is still shown, meaning the conversation
has not left that `usage_limit_reached` error state since the redemption began
THE SYSTEM SHALL resume the conversation through the same user-resume path as
the error's Retry action, without a further user action

WHEN that conversation has left the error state before the redemption
succeeds (for example the window elapsed and the error cleared, or the user
retried or dismissed it)
THE SYSTEM SHALL NOT resume the conversation on the redemption's behalf

WHEN a redemption started from a conversation's error does not succeed
THE SYSTEM SHALL leave the conversation in its error state with its existing
Retry and Dismiss actions available

**Rationale:** The confirmation is the user's statement of intent to continue
the stopped work; asking for another click would add friction without adding
consent. Resuming is gated on the error still being displayed so a redemption
never triggers a turn the user did not ask for.

---

### REQ-CRR-009: Reset Availability in Account Settings

WHEN the settings view shows the signed-in Codex account's quota
THE SYSTEM SHALL fetch the reset credit listing and show the available reset
count inline with the quota
AND SHALL offer the choose-and-confirm flow of REQ-CRR-006 when the available
count is greater than zero
AND SHALL show that resets are unavailable, distinct from zero, when the
listing fails

WHEN a redemption started from the settings view succeeds
THE SYSTEM SHALL refresh the displayed quota and reset listing
AND SHALL NOT resume any conversation

**Rationale:** Resets are an account resource; the user may want to spend one
before a conversation hits the limit, and needs to see the count without
waiting for an error.

---

### REQ-CRR-010: Account Scope

THE SYSTEM SHALL list and redeem reset credits only for the ChatGPT account of
the credential loaded when the request starts
AND SHALL take the access token and account identifier for one upstream
request from that single credential, so a request never pairs one account's
token with another account's identifier
AND SHALL order list and redeem requests against the publication of a newly
signed-in credential, as the authenticated usage read does

WHEN the credential is signed out or replaced while a request is in flight
THE SYSTEM SHALL let the in-flight request finish or fail against the
credential it started with, and SHALL NOT retry it against the replacement

**Rationale:** Reset credits belong to an account. Pairing the token and
account identifier from one credential is what prevents a cross-account
request. Ordering against sign-in publication matches the usage read. A
replacement credential must not inherit a redemption the user confirmed for
the previous account.
