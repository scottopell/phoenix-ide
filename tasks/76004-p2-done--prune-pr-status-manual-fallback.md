# Prune dormant PR manual-fallback state and stale terminology

## Findings

This started as two dead hook fields, but the old manual-fallback concept left several fossils. The live UI has moved to a simpler model: `Clean up` is a single-click terminal verb; `gh_unavailable` adds a warning note; there is no click-to-enable fallback mode.

### 1. Dead hook API fields

`manualFallbackEnabled` and `enableManualFallback` on `useConversationPrStatus` are vestiges of the old two-step PR fallback UI. The UI path that consumed them is gone.

Current usage trace:

- `ui/src/hooks/useConversationPrStatus.ts` defines state for `manualFallbackEnabled`, resets it on scope changes, clears it after successful PR checks, and returns `enableManualFallback`.
- Production consumers (`ConversationPage`, `WorkControlBar`, `StateBar`, `ChainWorkIdentityBlock`) use the handle for `state` and/or `refresh`, not the fallback fields.
- Remaining references are mock shape padding in:
  - `ui/src/components/ChainWorkIdentityBlock.test.tsx`
  - `ui/src/components/WorkActions.test.tsx`

### 2. Unused derived fallback flag

`cleanUpIsManualFallback` in `ui/src/components/workDisposition.ts` is replacement-era residue of the same concept.

Current usage trace:

- `deriveWorkDisposition()` sets `cleanUpIsManualFallback: true` for gh-unavailable cleanup rows.
- `WorkControlBar` never reads `disposition.cleanUpIsManualFallback`.
- The actual rendered behavior is already represented by:
  - `showCleanUp: true` — renders the Clean up button
  - `note.kind === 'gh_unavailable'` — renders the warning note
- Remaining references are tests asserting the redundant flag in `workDisposition.test.ts`.

This is parallel representation: the same semantic fact (“cleanup is being offered while gh cannot verify PR state”) is carried both by `cleanUpIsManualFallback` and by the `gh_unavailable` note. Only the note affects production UI.

### 3. Stale “manual fallback” terminology in tests

Several tests still name the old concept even when asserting the new behavior:

- `ui/src/components/WorkActions.test.tsx`
  - suite name: `gh unavailable (single-click manual fallback)`
  - comment: `no two-step enable-then-mark fallback`
  - negative assertion includes old labels: `Use manual fallback`, `Clean up merged PR`, `Waiting for PR merge`
- `ui/src/components/workDisposition.test.ts`
  - test names assert `manual fallback`
  - invariant asserts `cleanUpIsManualFallback implies ...`

The negative assertions can keep guarding against old labels if useful, but the test names/comments should describe the current invariant: **gh unavailable cleanup is single-click and warning-noted**. Avoid calling it a “manual fallback” unless the UI actually has a distinct manual fallback state.

### 4. Stale “manual fallback / Clean up merged PR” terminology in specs and comments

`specs/work-actions-bar` already mostly describes the current model: no disabled-as-status, no two-step toggle, enabled single-click `Clean up` with a warning note when gh is unavailable.

But sibling specs still carry older wording that conflicts with the current UI vocabulary:

- `specs/work-lifecycle/requirements.md`
  - says a merged PR is presented as `Clean up merged PR`
  - says unmerged states let the user “opt into an explicit manual fallback”
  - says `Mark as merged` is permitted “as a manual fallback”
  - rationale says the button says `Clean up merged PR`
- `specs/work-lifecycle/design.md`
  - same `Clean up merged PR` and “manual fallback” language
- `specs/work-lifecycle/work-lifecycle.allium`
  - comments describe `Clean up merged PR` and “manual fallback available”
- `ui/src/pages/ConversationPage.tsx`
  - comment says `Clean up merged PR / Abandon` even though the rendered button is now `Clean up`

This is not harmless wording. These specs/comments are authoritative enough to make future work reintroduce a distinct fallback mode or old labels. The work-actions-bar spec should remain the source for presentation; work-lifecycle should describe lifecycle semantics without prescribing obsolete button labels.

## Plan

1. Remove `manualFallbackEnabled` and `enableManualFallback` from `ConversationPrStatusHandle`.
2. Remove the associated `useState(false)`, reset calls, and unavailable-reason clearing from `useConversationPrStatus`.
3. Keep the meaningful hook behavior unchanged:
   - disabled/loading/ready state transitions
   - stale result protection
   - 60s refresh polling
   - visibility refresh
   - explicit `refresh()` API
4. Update component test mocks to match the smaller handle shape.
5. Remove `cleanUpIsManualFallback` from `WorkDisposition` and `finish()` options.
6. Update `workDisposition` tests to assert the visible/semantic state instead:
   - `showCleanUp === true`
   - `primary === 'clean_up'` where applicable
   - `note.kind === 'gh_unavailable'`
7. Rename PR/work-action tests and comments away from “manual fallback” toward current behavior:
   - “gh unavailable cleanup”
   - “single-click cleanup”
   - “warning note”
   Keep old-label negative assertions only where they intentionally guard against regression.
8. Update stale presentation wording in `specs/work-lifecycle/*` and the nearby `ConversationPage` comment:
   - Replace `Clean up merged PR` with presentation-neutral lifecycle wording or current `Clean up` where the spec intentionally names UI.
   - Replace “manual fallback / opt into fallback” with “user-initiated cleanup without gh confirmation” or equivalent.
   - Preserve the normative behavior: PR state is advisory; cleanup is user initiated; gh unavailable does not block cleanup.
9. Run targeted UI tests for:
   - `useConversationPrStatus`
   - `WorkActions`
   - `workDisposition`
   - `ChainWorkIdentityBlock`
10. Run relevant spec validation/checks if the touched specs require them.

## YAGNI assessment

- **Removal value:** Shrinks two public-ish UI contracts (`ConversationPrStatusHandle`, `WorkDisposition`) and deletes state/fields that cannot change rendering.
- **Terminology value:** Removes old model vocabulary from tests/specs so future work does not resurrect the click-to-enable fallback design.
- **Risk:** Low for code. The hook fields have zero production consumers; `cleanUpIsManualFallback` also has zero production consumers and duplicates an already-rendered note discriminant.
- **Spec risk:** Medium-low. Work-lifecycle still owns terminal action semantics, but work-actions-bar owns presentation. The edit should preserve lifecycle behavior while removing stale labels/mode names.
- **Why not keep them “for clarity”:** They encode an obsolete manual fallback model and invite future code to branch on stale semantics instead of the current single-click cleanup + warning-note behavior.
- **Rollback:** Reintroduce an explicit field only if a future UI needs behavior that cannot be derived from `note.kind === 'gh_unavailable'` and `showCleanUp`.
