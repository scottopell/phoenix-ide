# Stabilize Work Actions PR feedback primary action

## Findings

- The Work Actions interface is implemented by `ui/src/components/WorkActions.tsx` and pure derivation lives in `ui/src/components/workDisposition.ts`.
- There is **no Ladle fixture/story for Work Actions** today. Existing Ladle stories cover message list, task approval, grounding panel, conversation panel, mobile conversation list, and meta viewer only.
- Test coverage exists in `ui/src/components/WorkActions.test.tsx` and `ui/src/components/workDisposition.test.ts`, including Address feedback and secondary Merge/Open PR behavior, but it is not visually fixtureable.
- The primary-action shift is real in the current design:
  - `useConversationPrStatus` seeds from `Conversation.cached_pr` when available.
  - `CachedPrSummary` only carries PR identity/state fields, not refresh freshness, checks, feedback freshness, feedback coverage, or work-change state.
  - `cachedPrToStatus()` marks the seed as `refresh.state = 'unavailable'` and `work_change = { kind: 'loading' }`.
  - `deriveWorkDisposition()` treats an open PR with unavailable refresh as an honest `Open PR #N ↗` link, then flips to `Address feedback` once the async `GET /api/conversations/:id/pr-status` returns a fresh open PR with a message channel.
  - That is exactly the dangerous late primary-button shift described in the UX complaint.

## Goal

Make the Work Actions primary action stable enough that users can flip through conversations to see what needs attention without buttons changing under the pointer, while still ensuring PR feedback data is fresh before auto-fix context capture.

## Plan

1. **Add a Ladle fixture for Work Actions**
   - Create `ui/src/fixtures/workActions/` with scenario data and a renderer for `WorkControlBar`.
   - Create `ui/src/stories/work-actions.stories.tsx`.
   - Include scenarios for at least:
     - cached open PR seed / pre-refresh state,
     - fresh open PR with Address feedback,
     - passing PR with Address feedback plus secondary Merge link,
     - merged PR → Clean up,
     - no PR dirty work → View Diff or Create PR,
     - gh unavailable → manual-cleanup note,
     - stuck/error phase suppressing RESOLVE.
   - Include a specific scenario that demonstrates the current/target “no late shift under pointer” behavior.

2. **Move PR feedback freshness syncing into a background poll**
   - Add backend-owned polling for work-scope PR status/feedback on a roughly 5 minute cadence with ±90s jitter.
   - Poll work scopes with active/cached PR associations rather than only the currently opened conversation.
   - Persist enough poll output that list/conversation payloads can seed the UI with a richer, already-reconciled PR status view.
   - Keep the poll best-effort: failures should update explicit freshness/availability metadata, not block conversation loading.

3. **Live-refresh clients when background poll results change**
   - When persisted PR/feedback state changes, notify connected clients via the existing live update mechanism so visible conversation/list state refreshes without navigation.
   - Ensure conversation list items expose the attention signal needed for fast scanning.

4. **Keep open-time fresh fetch, but prevent unsafe primary-action jumps**
   - Retain an on-open or visibility refresh so the visible conversation can still converge to freshest data.
   - Use the background-polled cached state as the initial UI source so the normal case is already stable.
   - For the rare residual race — last background poll saw no actionable feedback, user opens conversation, immediate fresh fetch discovers feedback — avoid changing the clickable primary button under the pointer. Prefer one of:
     - show a transient non-clickable “Refreshing PR…”/stabilizing state before rendering the RESOLVE primary, or
     - reserve the RESOLVE slot and update label only after a short safe window / no pointer hover, or
     - structurally keep the primary as Address feedback for open addressable PRs and put GitHub link-out as secondary, with feedback freshness only changing badge text.
   - Choose the option that best matches `specs/work-actions-bar` after review; update the spec if the intended behavior changes.

5. **Backend/API shape**
   - Extend the cached PR/list payload deliberately; do not overload `CachedPrSummary` if the data is no longer just a lightweight identity summary.
   - Model any persisted feedback/refresh state structurally in the database where it is queried or independently updated; avoid JSON-in-TEXT blobs for child/field-wise data.
   - Preserve `create_pr_auto_fix_context` as the final fresh capture path before sending the auto-fix message, so the generated artifact remains authoritative.

6. **Tests**
   - Add hook tests or component tests proving initial cached state no longer produces `Open PR` then flips to `Address feedback` in the common background-polled case.
   - Add backend tests for background polling persistence, jitter scheduling boundaries, and failure behavior.
   - Update Work Actions tests for any intentional derivation/spec changes.
   - Add fixture scenarios for visual review of the primary-action states.

## Acceptance criteria

- A Work Actions Ladle fixture exists and can be used to visually inspect primary-action states.
- Conversation/list payloads include enough background-polled PR/feedback state to seed Work Actions without the current identity-only cached seed shift.
- Background PR/feedback sync runs approximately every 5 minutes with ±90s jitter and updates connected UI without manual navigation.
- Opening a conversation still triggers/uses a fresh fetch, but the primary action does not switch under the pointer in the common path.
- The final Address feedback click still captures a fresh PR auto-fix context artifact before sending the message.
- Tests cover the old `Open PR` → `Address feedback` race and the new stabilized behavior.
- `specs/work-actions-bar` remains aligned with the implemented derivation.

## Notes

This should be treated as a UX-safety fix, not just polish: the current shift can cause accidental GitHub navigation or the wrong primary action when a user is moving quickly through conversations.
