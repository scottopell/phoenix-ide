# Fix merged PR cleanup badge styling and StateBar PR link behavior

## Findings

- The Work Actions cleanup control uses `.work-actions-complete`, which is always green. When `prStatus.display_state === 'merged'`, the label becomes “Clean up merged PR”, but the visual treatment does not match the traditional GitHub merged purple used by `.pr-badge--merged` elsewhere.
- The StateBar PR badge is currently a `<button>` wired to toggle an inline CI popover, but in the actual StateBar layout the user-visible result is that clicking the badge appears to do nothing. It also does not directly open the PR, which is the expected behavior for a PR badge/link.
- StateBar tests currently assert the badge is a button and that clicking it opens the popover, so tests need to be updated to match the intended direct-link behavior.

## Plan

1. Update `WorkActions.tsx` so the cleanup button receives a merged-state modifier class when GitHub reports `display_state === 'merged'`.
2. Add CSS for that modifier in `index.css`, matching the merged PR badge purple (`#a855f7`/`#c084fc`) instead of green, including hover state.
3. Refactor the duplicated StateBar PR badge render into a small helper/component so both branch-display variants stay consistent.
4. Change the StateBar PR badge from the current popover-toggle button into an anchor to `prStatus.url` with:
   - `target="_blank"` / `rel="noreferrer"`
   - `title={prTooltip(prStatus)}` for the native tooltip
   - the existing `prBadgeClass` / `prBadgeLabel` styling
   - event propagation stopped so mobile/collapsed StateBar interactions are not accidentally triggered.
5. Remove or stop wiring the now-unused StateBar PR popover state/component if no longer needed.
6. Update tests:
   - assert merged Work Actions cleanup control has the purple merged modifier class.
   - assert StateBar PR badge renders as a link with the PR `href`, tooltip/title, label, and merged badge class.
   - replace the old popover-click assertion with direct-link behavior.
7. Run targeted UI tests for `StateBar` and `WorkActions`, then the appropriate project check lane if time permits.
