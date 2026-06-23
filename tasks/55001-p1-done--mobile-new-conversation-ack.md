# Improve mobile new-conversation submit acknowledgement

## Problem

On mobile, starting a new conversation can leave the user on `/new` for a long time with the composer disabled and the Send button morphed into a loading label. This is especially alarming on unreliable mobile networks: it is unclear whether the conversation was created, whether the first message was accepted, or whether it is safe to reload/lose connectivity.

Initial code reading shows the frontend waits for `api.createConversation()` to resolve before it clears the draft and navigates (`ui/src/hooks/useCreateConversation.ts`). The backend create endpoint performs several potentially slow operations before responding, including title generation, project detection, worktree creation for managed/branch modes, DB row creation, attachment storage, inline-reference expansion, and initial runtime dispatch (`crates/phoenix-ide/src/api/handlers.rs:create_conversation_with_id`). The store wrapper itself only upserts the returned conversation; it does not wait for SSE.

There is also a mobile visual issue: the same `.new-conv-send` button changes from `Send` to `Creating...`/`Creating folder...` while disabled, with no fixed width or stable mobile acknowledgement UI, which can produce a half-old/half-new-looking transition on narrow screens.

## Goal

Make mobile conversation creation feel durably acknowledged as soon as the server has enough information to identify the new conversation, and make the in-flight UI visually stable and reassuring.

## Plan

1. Reproduce/profile the create path on mobile-width UI:
   - Use a slow managed-workflow create path if possible.
   - Confirm whether the delay is before the POST response, before route navigation, or during conversation-page SSE init.
   - Capture current button/layout behavior at mobile width.

2. Short-term UX fix in the existing API shape:
   - Keep the Send button width/content stable while `creating` is true (e.g. fixed min-width and a non-janky inline spinner/status label).
   - Add a clear mobile acknowledgement message near the composer such as “Creating conversation…” / “Setting up worktree…” rather than only changing the button text.
   - Preserve the draft until a successful create response; on failure, restore the interactive composer with the existing inline error behavior.

3. Navigation/acknowledgement design check:
   - If measured delay is entirely inside `POST /api/conversations/new`, evaluate splitting create into an early durable acknowledgement plus asynchronous startup work.
   - The target shape should be: create and return the conversation row/slug as soon as the server has committed it, navigate immediately to `/c/:slug`, and show conversation-page state for any remaining startup/first-message dispatch work.
   - Do not introduce a state where the UI shows a conversation that can still disappear silently; any post-ack failure must be persisted/presented in the conversation.

4. Implement the safest scoped fix:
   - Prefer a frontend-only stabilization if backend semantics are not safe to split quickly.
   - If backend split is required, model the startup state explicitly rather than hiding it behind SSE timing.
   - Keep idempotency by `message_id` intact so mobile retries/reloads cannot create duplicate conversations.

5. Tests:
   - Add/adjust `NewConversationPage` tests for the creating state and failed-create recovery.
   - Add backend tests if create acknowledgement semantics change.
   - Run relevant UI tests and the normal project check lane as appropriate.

## Acceptance criteria

- On mobile, tapping Send immediately produces a stable, explicit acknowledgement state; no half-rendered old/new Send button text.
- The UI clearly communicates whether the submission is still being created vs. failed.
- Once a conversation slug exists, the user is navigated to the conversation promptly or receives a clear reason why not.
- Reload/retry behavior remains safe: no duplicate conversations for the same initial message id, and failed creates do not clear the user’s draft.
