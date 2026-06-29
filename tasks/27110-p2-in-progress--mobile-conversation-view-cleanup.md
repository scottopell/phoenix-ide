# Mobile conversation view cleanup

## Context

The mobile conversation list already has a QA fixture under `ui/src/fixtures/mobileConversationList/` and a Ladle story in `ui/src/stories/mobile-conversation-list.stories.tsx`. It is not currently exposed through an in-app `__qa` route; the only dev QA route registered in `App.tsx` is `/__qa/grounding-panel`.

Two mobile UI regressions need cleanup:

1. The relative “last activity” timestamp on mobile conversation cards can collide with/overlap slug, state, mode, or PR badge text.
2. The mobile conversation status/footer bar can appear like a floating footer in the middle of the viewport instead of being visually anchored to the conversation chrome.

## Plan

1. Expose/reuse the existing mobile conversation list fixture through a dev-only QA route, e.g. `/__qa/mobile-conversation-list?scenario=active-dark`, mirroring the existing grounding panel QA route pattern.
2. Adjust the mobile conversation row layout so the timestamp cannot overlap other card content:
   - reserve a stable trailing slot for the time, or allow it to wrap/stack predictably on narrow widths;
   - ensure slug/title text truncates instead of painting under badges/time;
   - preserve existing mobile density and tap targets.
3. Clean up the mobile conversation page status/footer placement:
   - inspect the `ConversationPage`/`StateBar` mobile stack and CSS around `conversation-column`, `#main-area.chat-main-area`, and `statebar-mobile`;
   - make the status bar either anchored as a bottom chrome element or part of the flex stack without mid-screen floating;
   - verify offline banner/expanded statebar still behave sensibly.
4. Add/adjust tests where practical:
   - component tests for mobile conversation row structure and timestamp placement semantics;
   - StateBar/mobile layout regression coverage if the fix changes structure;
   - use the QA fixture for visual validation at phone viewport widths.
5. Run focused UI tests and a relevant check lane via `./dev.py`.

## Acceptance criteria

- A developer can open a mobile conversation list QA fixture without running Storybook/Ladle.
- On narrow mobile widths, the conversation card timestamp never overlaps slug, badges, state chips, or action menu controls.
- The mobile conversation status/footer bar no longer appears stranded in the middle of the screen.
- Existing desktop/sidebar conversation list layout is unchanged.
- Relevant tests pass.
