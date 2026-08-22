# Compact the mobile Work Actions fallback into a two-row status and action dock

## Observed journey

- In a narrow mobile conversation, when there is no actionable PR rail—especially dirty work with no PR, or a merged PR ready for cleanup—the Work Actions fallback sits immediately above the composer.
- The fallback currently renders every available action and the full disposition guidance as siblings in one wrapping flex row. At phone width, `Review workspace changes`, `Abandon`, and guidance such as `Uncommitted changes found. Review, commit, and push before opening a PR.` wrap independently and consume several lines of transcript space.
- The same fallback owns terminal-PR cleanup, where the desired persistent context is much smaller: a merged status and the primary `Clean up` action.
- Product choice: replace this fallback with a compact two-row treatment:
  - first row: concise status plus an info affordance for full guidance;
  - second row: one full-width primary action plus a compact overflow control for secondary/destructive actions.

Example:

```text
⚠ Uncommitted changes                                      ⓘ
[ Review changes                                      ] [•••]

✓ PR merged
[ Clean up                                           ] [•••]
```

## Verified findings

- `WorkControlBar` in `ui/src/components/WorkActions.tsx` enters `mobile-work-fallback` when compact layout cannot represent an active PR selection. This includes no-actionable-PR and terminal-active-PR cases.
- The fallback renders primary, cleanup, Abandon, disposition note, and error as direct siblings. `ui/src/components/WorkActions.css` defines the container as a wrapping-prone flex row and gives each button a 44px touch target, while the note is another flexible sibling. This combination produces the screenshot’s excessive height.
- `deriveWorkDisposition` correctly determines action priority and lifecycle semantics. Dirty no-PR work makes review primary and retains Abandon; merged work makes cleanup primary and retains Abandon. The layout fix must consume this existing typed disposition rather than re-derive lifecycle state.
- Existing component tests cover no-PR fallback, terminal-active fallback, dirty-work review, and merged cleanup, but assert action presence/priority rather than bounded mobile geometry or disclosure behavior.
- Ladle fixtures already include `no-pr-dirty-review` and `merged-clean-up`, providing deterministic visual QA states.
- `specs/work-actions-bar/requirements.md` currently requires no-PR, loading, and terminal-PR states to use one thin horizontally scrollable rail with shared geometry. `work-actions-bar.allium` likewise names this presentation `compact_rail`. The selected two-row status/action dock intentionally changes that normative presentation and must update those artifacts rather than silently contradict them.

## Interaction map

- PR/work-change status from `ConversationPrStatusHandle` → `deriveWorkDisposition` → typed primary, secondary/terminal actions, and note.
- `derivePrRailAvailability` → actionable PR rail when representable; otherwise → compact mobile fallback dock.
- Compact dock primary → existing diff/GitHub/cleanup handlers.
- Overflow secondary action → existing Abandon/cleanup handlers, preserving confirmation, snapshot, and cleanup semantics.
- Info affordance → presentation-only disclosure of the existing disposition guidance and terminal hint; it must not alter action state or create a second lifecycle authority.
- Error state remains visible and accessible after failed actions.

## Proposed scope

### Responsive contract

- Update REQ-WAB-002 and the corresponding Allium presentation rule to permit/require the two-row compact mobile fallback when no actionable PR rail can represent the state.
- Preserve these governing invariants:
  - the transcript remains dominant;
  - exactly one primary action is visually emphasized;
  - the REVIEW action remains available whenever the bar is visible;
  - terminal intent/explanations remain accessible;
  - PR selection authority and the existing actionable-PR rail behavior do not change.
- Keep timeless specs free of task/status language and run the spec authoring pre-flight plus `allium check`.

### UI implementation

- Refactor only the compact `mobile-work-fallback` branch of `WorkControlBar`; do not change `deriveWorkDisposition` or backend state semantics.
- Add a concise, state-derived status label for the first row, including at minimum:
  - dirty no-PR review: `Uncommitted changes` (or an equivalent reason-specific short label);
  - merged PR: `PR merged`;
  - other existing fallback dispositions (loading, closed, GitHub unavailable, create-PR, stuck, continuation) must receive honest compact labels without dropping their existing guidance.
- Put the full existing disposition note and relevant terminal explanation behind an accessible info disclosure that works with touch, keyboard, and screen readers. Do not rely on hover/title alone on mobile.
- Render the single primary action in the second row. Shorten the visible mobile label from `Review workspace changes` to `Review changes` while retaining a precise accessible name.
- Move non-primary terminal actions such as `Abandon` into a compact overflow menu. Preserve 44px touch targets, destructive styling, disabled/loading behavior, confirmation behavior, and click-away/Escape/focus behavior. Do not bury the selected primary action in overflow.
- Keep action failures as an inline `role="alert"` without allowing normal guidance to expand the dock by default.
- Bound text overflow and prevent dynamic metadata refresh from causing the several-line layout shift shown in the report.

### Regression and journey validation

- Extend `WorkActions.test.tsx` to verify semantic grouping, exactly one primary, compact status copy, info disclosure, overflow access to Abandon, accessible names, and unchanged handler behavior for:
  - dirty no-PR review;
  - merged-PR cleanup;
  - clean no-PR cleanup;
  - closed/unavailable/loading fallback states;
  - action error display.
- Preserve existing tests for actionable mobile PR rails and desktop controls; those layouts are explicit non-regression surfaces.
- Use/update the existing `no-pr-dirty-review` and `merged-clean-up` Ladle fixtures and capture both at a representative narrow iPhone viewport. Confirm the collapsed dock is approximately two compact rows (target about 76–88px, excluding a transient error/disclosed detail) and does not overlap the composer or safe area.
- Run focused UI tests/typecheck, Allium validation, spec pre-flight, and the repository check lanes required by the touched files.

## Risks

- An overflow menu can reduce discoverability of Abandon; mitigate with conventional ellipsis presentation, an explicit accessible label such as `More work actions`, and preservation of the primary action outside the menu.
- Native `<details>` is easy but may not provide robust menu focus/dismissal semantics. Prefer an existing shared menu/popover primitive if one exists; otherwise implement the smallest accessible disclosure appropriate to the content.
- Short status labels must remain derived from the authoritative disposition/work-change reason and must not imply that uncommitted work is safe to discard.

## Non-goals

- No changes to PR discovery, PR association/selection, `WorkDisposition` action semantics, lifecycle endpoints, cleanup/abandon behavior, or persistence.
- No redesign of the actionable-PR expandable rail or desktop Work Actions rail.
- No move of Work Actions into the composer or StateBar.
- No reduction of the required 44px mobile touch targets.
