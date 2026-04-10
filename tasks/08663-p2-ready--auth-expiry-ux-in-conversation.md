---
created: 2026-04-10
priority: p2
status: ready
artifact: ui/src/pages/ConversationPage.tsx
---

# Surface credential expiry inside an active conversation

## Problem

When a credential helper token expires while a conversation is open, the user sees
raw LLM failure errors (or a stalled conversation) with no indication of why or
what to do. The `AUTH ✗` chip that tells them to re-authenticate only appears on
the conversation list page — not visible from inside an active conversation.

Journey 3 is effectively: *something silently breaks, user doesn't know why*.

## What exists

- `GET /api/models` already returns `credential_status: "required" | "running" | "valid" | "failed" | "not_configured"`
- `GET /api/credential-helper/run` SSE endpoint + `CredentialHelperPanel` modal are fully implemented
- The conversation list page polls `/api/models` every 5s and shows the AUTH chip
- `credential_status` is in the `ModelsResponse` TypeScript type in `api.ts`

## What's needed

When `credential_status` transitions to `required` or `failed` while a conversation
is open, the conversation page should surface a clear, actionable prompt.

### Suggested approach

1. **Poll `/api/models` in `ConversationPage`** at the same 5s interval (or reuse
   an existing polling hook if one exists). Only do this when `LLM_API_KEY_HELPER`
   is in use — i.e. when `credential_status` is anything other than `not_configured`.

2. **Show an inline auth banner** when `credential_status` is `required` or `failed`
   and the conversation is in a state where it would need credentials (idle, error,
   or actively running). Banner placement: above the message input, below the message
   list. Style consistent with existing warning banners.

   Banner content:
   - `required`: "LLM credential expired. [Authenticate] to resume."
   - `failed`: "LLM authentication failed. [Retry] to resume."
   - `running`: "Authenticating..." (spinner, no action needed)

3. **[Authenticate] / [Retry] button** opens `CredentialHelperPanel` (already exists,
   just needs importing into `ConversationPage`).

4. **On panel close** (Done or Cancel), re-fetch models status and dismiss the banner
   if `credential_status` is now `valid`.

5. **No banner when `not_configured`** — in that case the existing `llm_configured`
   handling applies.

## Acceptance Criteria

- [ ] `ConversationPage` polls `/api/models` and tracks `credential_status`
- [ ] Banner appears when status is `required` or `failed`, with correct copy
- [ ] Spinner shown when status is `running` (another tab is mid-auth)
- [ ] Clicking [Authenticate] opens `CredentialHelperPanel`
- [ ] Banner dismisses automatically when status returns to `valid`
- [ ] No regression on conversations without a helper configured (`not_configured`)
- [ ] `./dev.py check` passes

## Notes

- `CredentialHelperPanel` is at `ui/src/components/CredentialHelperPanel.tsx` — just
  needs an import, no changes to the component itself.
- The conversation list page pattern (import `CredentialStatus`, poll `api.listModels()`,
  open panel on click) is the template. `ConversationPage` should follow the same shape.
- Test with `LLM_API_KEY_HELPER="bash scripts/dummy-auth-helper.sh" LLM_API_KEY_HELPER_TTL_MS=10000`
  to exercise expiry quickly.
