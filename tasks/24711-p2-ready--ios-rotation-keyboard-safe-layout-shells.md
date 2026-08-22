# Deliver rotation-safe, keyboard-safe iPhone and iPad layouts

## User outcome

Make Phoenix dependable as an installed iPhone and iPad work app rather than merely applying isolated notch-padding fixes. Users must be able to rotate or resize the app, open the software keyboard, read and compose conversations, and use full-screen tools without content jumping, controls becoming obscured, or safe-area spacing being applied twice.

PR #701 stabilizes Home Screen identity and the current viewport edges. This follow-up turns that stabilization into a reusable product capability; it is not a selector-renaming refactor.

## Scope

Deliver the work incrementally through explicit viewport and surface contracts:

1. **Conversation shell** — navigation, transcript, composer, StateBar, offline banner, and their compact/desktop transition.
2. **Full-screen surface shell** — viewer, terminal, task/review approval, and process inspector, with consistent edge ownership, focus, and internal scrolling.
3. **Route shells** — conversation list, chains, Coordinator, settings/about/usage, and shared conversations.

Migrate a surface only when its user journey is covered. Remove route-specific safe-area exceptions as their owning shell replaces them; do not perform a flag-day stylesheet rewrite.

## Acceptance criteria

- [ ] On supported iPhone and iPad viewports, portrait and landscape layouts keep text and interactive controls outside notches, rounded-corner exclusion zones, status areas, and the Home indicator.
- [ ] Rotating or resizing across the compact/desktop breakpoint preserves the active list or transcript reading position and does not produce duplicated safe-area gaps.
- [ ] Opening, resizing, and dismissing the software keyboard keeps composer actions reachable and preserves the transcript reading position; hardware-keyboard mode remains usable.
- [ ] Conversation navigation, transcript, composer, StateBar, and offline state have one structurally explicit owner for each exposed viewport edge.
- [ ] Full-screen viewer, terminal, approval, and inspector surfaces keep close/primary controls reachable, confine focus where applicable, and scroll internally rather than behind the surface.
- [ ] iPad desktop/split layouts and narrow Stage Manager-style windows can transition between layout modes without clipping, double insets, or stale dimensions.
- [ ] New routes can select a documented compact, desktop, or overlay shell instead of adding unrelated global safe-area selectors.
- [ ] Automated layout-contract coverage includes iPhone portrait, iPhone landscape, iPad portrait, iPad landscape, compact/desktop breakpoint transitions, standalone mode, ordinary Safari mode, and software-keyboard viewport changes.
- [ ] Physical-device verification is recorded for at least one notched iPhone and one iPad before claiming the corresponding device support.
- [ ] Existing Home Screen metadata behavior remains intact; this task adds no install prompt, service worker, caching, or offline-capability claim.

## Design constraints

- Edge ownership must be structural: a shell or typed surface variant determines which component consumes top, right, bottom, and left insets.
- Parent and child surfaces must not consume the same physical edge in the same layout mode.
- Preserve each component's ordinary spacing in addition to safe-area insets.
- Browser and standalone behavior must remain distinct where the physical viewport contract differs.
- Prefer colocated component CSS for owned surfaces; keep global CSS limited to viewport primitives and cross-route shells.

## Verification

Exercise real journeys rather than testing only computed selector presence:

- rotate while deeply scrolled in Active and Archived conversation lists;
- rotate while reading a long transcript;
- compose and send with the software keyboard open;
- open and close viewer, terminal, approval, and inspector surfaces in portrait and landscape;
- resize an iPad-width layout across the compact breakpoint;
- compare standalone Home Screen mode with ordinary Safari.
