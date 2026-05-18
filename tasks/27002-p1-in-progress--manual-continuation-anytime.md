# Allow manual continuation at any context size

## Problem

The backend already supports `UserTriggerContinuation` from `Idle`, but the UI hides the manual continuation action until the context warning threshold is reached (currently 80% of the model window). On 1M-token models that means the action does not appear until roughly 800k tokens, while quality can degrade earlier (around 500–600k tokens for GPT 5.5 1M).

Users should be able to deliberately end the current conversation, summarize it, and continue into the next conversation in the same continuation chain whenever they decide the current context is no longer useful — not only near the hard limit.

## Goal

Make continuation a user-controlled action that is available any time the conversation is idle, while preserving the existing automatic threshold behavior and single-continuation chain semantics.

## Plan

1. **Update the UI gating**
   - Change `ContextIndicator` so the menu action is available whenever `onTriggerContinuation` is provided (idle conversation), not only after `WARNING_THRESHOLD`.
   - Keep warning/critical styling thresholds for visual status only.
   - Adjust the tooltip/menu copy so the action is understandable below warning threshold (for example, “End & summarize now” / “Continue in a new conversation”).

2. **Preserve backend semantics**
   - Reuse the existing `POST /api/conversations/:id/trigger-continuation` endpoint and `Idle + UserTriggerContinuation` state-machine path.
   - Do not relax `continue_conversation` directly; it should still create the next chain member only after the parent has entered `ContextExhausted` with a continuation summary.
   - Preserve idempotency and the single-continuation policy for already-continued parents.

3. **Update specs to match the new UX contract**
   - Revise REQ-BED-023 so the warning indicator still appears at 80%, but the manual continuation action is available from idle regardless of threshold.
   - Update the bedrock design section and Allium rule notes if needed so the UI no longer contradicts the existing state-machine rule (`UserTriggersContinuation` already only requires `idle`).

4. **Add/adjust tests**
   - Add a UI test for `ContextIndicator` or `StateBar` showing the manual continuation action below 80% usage when idle.
   - Keep/verify behavior that the action is unavailable while the conversation is busy.
   - Keep existing warning/critical visual threshold tests, if present, unchanged in intent.

5. **Verify**
   - Run the relevant UI tests/typecheck.
   - Run the project check path appropriate for the touched files (`./dev.py check` if feasible).
   - Since this is UI-affecting, ensure the dev server reflects the change and report the URL for verification.

## Non-goals

- Do not change the automatic continuation threshold.
- Do not create multiple continuations from the same parent.
- Do not make continuation available while the agent is actively working; the current idle-only constraint avoids interrupting in-flight LLM/tool state.
