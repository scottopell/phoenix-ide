Implement approved Ember conversation-list redesign across desktop/mobile web and native iOS. Establish pinned Ember entry and consistent journey naming, preserve ordinary conversation lifecycle separation, update requirements/executives and ADR, add focused regression tests and capture available visual evidence.


## Delivered

- Compact pinned Ember entry across desktop web (expanded/collapsed), mobile web, and native iOS, separate from ordinary list filtering and lifecycle controls.
- Ember identity in destination headers, loading/accessibility copy and return breadcrumbs; Global coordinator retained as role description. Existing singleton routes, in-flight guards and offline gating preserved.
- Requirements/executive updates and ADR-229; detailed approved brief archived in `docs/plans/ember-conversation-list-redesign.md`.

## Validation evidence

- Targeted web suites cover loading navigation, toolbar → Ember → Open/History ordering, router-driven active state, and destination Ember identity. Final focused run: 77 passed, 2 skipped.
- Native simulator suite passed 252 tests during initial implementation; final ModelCompatibilityTests passed 13 tests, including Coordinator identity-driven Ember titles.
- Parent visual review: `/tmp/ember-final-mobile.png`, `/tmp/ember-final-sticky.png`, `/tmp/ember-desktop-expanded.png`, `/tmp/ember-desktop-collapsed.png`. At 320px, document width is 320px. Scrolling the actual MAIN owner to 500px leaves pinned header top at 0 and Ember top at 47px. Fixed a flex-shrink containment bug found by this longer-scroll check.
- Final CSS/ESLint lane passed after that containment fix. No native screenshot, large-Dynamic-Type visual qualification, or live seeded native navigation journey was captured; these are remaining manual qualification gaps, not claimed verified.
- No deployment performed.
- Parent final `./dev.py check`: all 17 checks passed (181.4s); unaffected Rust/E2E lanes were incrementally skipped. After the final CSS containment fix, `./dev.py check --lanes ui-lint` passed both checks. `git diff --check` passed.
