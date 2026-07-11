# Refine conversation composer controls and mobile steering UX

## Problem

The conversation message composer has several related interaction and layout defects:

- Send and Stop labels/icons are not reliably centered within their controls.
- The microphone control is absolutely overlaid on the textarea. The textarea reserves only 80px on the right, while the 44px microphone target, gap, and Send button require more space. Text and text-selection gestures can therefore enter the microphone hit area and accidentally start voice input.
- While the agent is cancellably busy, the composer replaces Send with Stop. This removes the visible way to submit a steering message, especially on mobile where users cannot rely on a hardware keyboard.
- Enter without Shift is specified to submit, but mobile use is no longer reliably taking that path.

The existing send logic and backend already support steering messages while the agent is working; this task should expose that capability clearly rather than introduce another queue representation.

## Scope and approach

### 1. Give composer actions explicit, non-overlapping layout ownership

Refine `InputArea` markup and its colocated styling so the textarea's editable/selectable region never sits beneath an interactive control.

- Move the microphone and submit/cancel controls into a dedicated action region whose dimensions drive or are matched by the textarea inset; do not rely on the current fixed 80px guess.
- Preserve the compact integrated-composer appearance, but ensure text, caret, and drag-selection remain outside every button hit target at all textarea heights.
- Use explicit flex/grid centering and consistent control heights so Send, Queue, Stop, and their labels are visually centered.
- Keep the microphone's accessible 44×44px mobile target required by REQ-VOICE-007 without letting that target cover editable text.
- Preserve safe-area padding and textarea auto-resize behavior.
- Follow CSS ownership guidance by colocating newly bounded InputArea-specific styles with `InputArea.tsx`; avoid broad unrelated cleanup of `index.css`.

### 2. Expose steering and cancellation as separate busy-state actions

When the agent is in a state that permits both cancellation and steering:

- Render Stop and the message submission action at the same time instead of using the current mutually exclusive ternary.
- Label the busy-state submission action clearly (for example, `Queue`) and provide an accurate accessible name/title explaining that it queues a follow-up/steering message.
- Keep submission disabled when there is no content, an expansion error exists, attachments are uploading, cancellation is pending, continuation compaction is active, or the initial optimistic send is still in `awaiting_llm`.
- Preserve Stop behavior and disabled `Stopping…` feedback.
- Do not add a second frontend queue or alter steering-message persistence/reconciliation semantics.
- Keep idle behavior concise (`Send`) and avoid duplicating status already shown elsewhere.

The responsive layout must retain both controls at narrow mobile widths without covering text or creating undersized touch targets. If horizontal room is constrained, prefer a deliberate compact action treatment over hiding Queue.

### 3. Restore reliable keyboard submission

Audit and correct the textarea submission event path so Enter without Shift submits on supported mobile and desktop browsers, while:

- Shift+Enter inserts a newline.
- Active autocomplete receives Enter first.
- IME composition/confirmation does not accidentally submit.
- The same `handleSend` eligibility rules govern keyboard and button submission.
- Busy-state Enter queues a steering message when steering is allowed.
- Blocked states (`awaiting_llm`, `awaiting_continuation`, cancelling, upload/error conditions) remain blocked.

Prefer standard form/submission semantics where they improve mobile soft-keyboard behavior, without causing duplicate sends from key and submit events. Set an appropriate textarea enter-key hint if browser support makes it useful, but keep the visible Queue button as the dependable mobile affordance.

### 4. Update normative UX requirements and status documentation

Update the conversation UI requirements to state that:

- A busy composer visibly supports submitting a steering message when that operation is allowed.
- Cancellation and steering remain independently accessible when both are valid.
- Composer controls must not overlap the editable/selectable text region across supported responsive widths.

Keep the requirements timeless and update the relevant executive status/verification notes after implementation. Update voice-input status only where this work changes the verified mobile control layout; do not perpetuate stale line-number references or unrelated legacy design content.

## Verification

### Automated component tests

Extend `InputArea` tests to cover:

- Idle state renders Send and, when speech is supported, microphone access.
- Cancellable working state renders both Stop and Queue; Queue invokes `onSend` with the draft and Stop invokes `onCancel` independently.
- Busy-state Enter queues a draft.
- Empty content and all existing blocked states disable/prevent Queue consistently.
- Enter submits, Shift+Enter does not submit, autocomplete still consumes Enter, and composing/IME Enter does not submit.
- Cancelling shows disabled `Stopping…` and no enabled queue path.
- Accessible names/titles describe Send, Queue, Stop, and voice actions correctly.

Add a focused responsive/layout regression test where practical (class/state contract or browser-computed geometry) rather than asserting fragile pixel-perfect CSS in jsdom.

### Browser QA

Run the app and verify the real composer at desktop and mobile viewports, including at least a narrow phone width (~375px):

1. Type enough text to wrap and auto-resize; select text near the right edge without activating the microphone.
2. Confirm Send and Stop/Queue content is centered and all controls remain fully visible.
3. Start/stop voice input and confirm its 44px target remains reachable without covering text.
4. During an active LLM/tool turn, enter a follow-up and submit it using the visible Queue action.
5. During an active turn, submit another follow-up with Enter where the keyboard/browser emits Enter; confirm Shift+Enter remains a newline and IME confirmation does not send.
6. Confirm Stop remains independently operable and transitions to disabled `Stopping…` feedback.
7. Check console output for new errors and capture before/after screenshots for desktop idle, mobile idle, and mobile busy states.

Run focused UI tests, typecheck/lint gates, and `./dev.py check` before committing.

## Acceptance criteria

- Editable text and selection gestures never occupy the microphone, Send/Queue, or Stop hit regions at supported widths.
- Send, Queue, and Stop content is visibly centered with consistent sizing.
- Mobile users always have a visible Queue action while the agent is steerable and busy.
- Stop and Queue are simultaneously available when both operations are valid.
- Enter without Shift follows the same valid send/queue path as the visible action; Shift+Enter and IME composition remain safe.
- Existing attachment, autocomplete, voice, optimistic delivery, steering reconciliation, cancellation, and continuation behavior does not regress.
