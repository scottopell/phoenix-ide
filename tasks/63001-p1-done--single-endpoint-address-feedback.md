# Single-endpoint Address Feedback submission

## Goal

Make the Work Actions **Address feedback** operation a single backend-owned API call that captures PR remediation context and submits the resulting message to the conversation, instead of relying on the browser to call `pr-auto-fix-context` and then separately post `/chat`.

This should support both the existing UI button and a future Phoenix-internal caller triggered by PR subscription events.

## Current behavior

`ui/src/components/WorkActions.tsx` currently performs a client-orchestrated sequence:

1. `POST /api/conversations/:id/pr-auto-fix-context`
2. read the returned `message`
3. call `onSendMessage(message)`, which posts `POST /api/conversations/:id/chat`
4. refresh PR status from the browser

This means the operation is not atomic from the client perspective: one successful HTTP request can capture context without sending the remediation message.

## Desired behavior

Add a backend-owned operation, for example:

```http
POST /api/conversations/:id/address-pr-feedback
```

The endpoint should:

1. Resolve the conversation and associated/primary PR using the same targeting semantics as the existing PR auto-fix context endpoint.
2. Capture the PR remediation context artifact using the existing `pr_monitoring` capture path.
3. Persist PR observations and feedback status exactly as the current context endpoint does.
4. Submit the generated auto-fix instruction as a conversation user message through the same semantics as `/chat`:
   - conversation-state validation,
   - steering queue behavior when the agent is busy,
   - message expansion semantics where applicable,
   - idempotency by message id,
   - baseline recording for the captured PR context.
5. Return a response that lets the UI update cleanly, such as:
   - `queued` / `steering` from chat submission,
   - `artifact_path`,
   - `pr_number`,
   - optional updated feedback status/freshness fields if already available.

Remove the old `pr-auto-fix-context` endpoint if no remaining caller needs context capture without message submission. Do not keep a parallel unused endpoint for speculative inspection/backward compatibility.

## Design constraints

- Do not duplicate `/chat` message-submission logic in a second handler. Extract a reusable backend helper for “submit this user message text to a conversation” and have both `/chat` and the new endpoint use it.
- The helper should be callable without browser-specific inputs so future internal PR subscription code can trigger the same operation.
- Preserve idempotency. The new endpoint needs a server-side message id strategy, or it should accept an optional caller-provided idempotency key/message id. The UI path should not be able to double-submit on retry.
- Preserve existing PR feedback freshness semantics from `specs/pr-association/requirements.md`: the baseline is recorded only for successfully captured agent-facing remediation context.
- Preserve Work Actions Bar behavior from `specs/work-actions-bar/requirements.md`: Address feedback is still only available when Phoenix can post an auto-fix message to the conversation.

## Implementation plan

1. Backend API
   - Add route `POST /api/conversations/:id/address-pr-feedback`.
   - Add request/response types in `api/types.rs`.
   - Delete the old `POST /api/conversations/:id/pr-auto-fix-context` route/API wrapper/tests if the new endpoint subsumes all current callers.
   - Extract common chat submission logic from `send_chat` into a reusable helper that accepts conversation id, text, empty attachments, message id, and a non-browser/internal user-agent marker.
   - Reuse the existing PR auto-fix capture code from `create_pr_auto_fix_context` rather than reimplementing GitHub/context capture.
   - Ensure observations, feedback status, and baseline writes remain consistent with the current two-step flow.

2. UI
   - Change `WorkActions.tsx` so `handleAddressFeedback` calls the new endpoint directly.
   - Remove the dependency on `onSendMessage` for Address feedback where practical, or keep only the “can post message” availability signal if the UI disposition still needs it.
   - Keep the PR status refresh after success unless the new response supplies enough fresh state to avoid it.

3. Tests
   - Add backend handler tests covering:
     - successful capture + message submission,
     - busy conversation queues a steering message,
     - invalid/terminal states reject consistently with `/chat`,
     - duplicate/idempotent request does not double-submit,
     - PR observations/status/baseline are updated as before.
   - Update `WorkActions` tests to assert one API call instead of `createPrAutoFixContext → onSendMessage`.
   - Run relevant UI tests and Rust API tests.

4. Specs/docs
   - Update `specs/pr-association/requirements.md` or executive/design notes if needed to say explicit remediation capture can be performed by a backend-owned operation that also submits the generated message.
   - Update `specs/work-actions-bar/requirements.md` if it currently describes the client-side two-step sequence.

## Acceptance criteria

- Clicking **Address feedback** from the UI requires only one browser-originated HTTP request to capture PR context and enqueue/send the auto-fix message.
- The generated message enters the conversation through the same state/idempotency/steering semantics as ordinary chat.
- Future server-internal callers can invoke the same operation without depending on browser code.
- Existing PR feedback freshness baselines and persisted PR observations remain correct.
- Tests cover the new one-shot path and the `/chat` path still behaves as before.
