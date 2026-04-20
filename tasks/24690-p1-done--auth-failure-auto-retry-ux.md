---
created: 2026-04-16
priority: p1
status: done
artifact: ui/src/components/CredentialHelperPanel.tsx
---

# Seamless auth: auto-retry on helper failure and clear stale output

## Summary

When the credential helper fails (e.g., ddtool Vault timeout), the auth strip
shows stale OIDC codes and no clear path to retry. The user must manually
refresh the page to trigger a new helper run. Auth should be seamless: failure
should auto-retry, and stale codes should be replaced immediately.

## Reproduction

1. Open /new or a conversation when credential is expired
2. Helper auto-triggers, shows Google device code
3. Complete Google auth, but ddtool fails (Vault timeout, exit code 23)
4. Auth strip still shows the old code -- user tries it at google.com/device,
   gets "invalid code"
5. Only a page refresh triggers a fresh helper run

## Expected Behavior

1. When HelperFailed fires, the auth strip should:
   - Clear the stale output (old device code)
   - Show "Authentication failed -- retrying..." 
   - Auto-trigger a new helper run after a brief delay (2-3 seconds)
   - Show the new device code when the fresh helper emits it

2. If the retry also fails, show "Authentication failed" with a manual
   "Retry" button. Don't loop indefinitely.

3. The /new page and conversation page should both handle this identically
   (both render the auth strip via LlmStatusBanner now).

## Design Notes

The auto-retry should happen in the CredentialHelperPanel component:
- On receiving the `error` SSE event, start a retry timer (2-3s)
- On retry, reconnect to `/api/credential-helper/run` (which auto-triggers
  a fresh helper if status is Failed)
- Cap retries at 2-3 attempts to prevent infinite loops
- Show attempt count: "Retrying (2/3)..."

The backend already supports this: `run_and_stream()` checks status and
spawns a fresh helper if Failed. The missing piece is the frontend
auto-reconnecting after failure.

## Context

Filed during QA of the AwaitingRecovery feature. The backend correctly
handles helper failure (CredentialHelperTerminatedWithoutCredential event),
but the frontend doesn't auto-retry. The user's ddtool failed due to a
Vault timeout -- a transient infrastructure issue that succeeds on retry.
