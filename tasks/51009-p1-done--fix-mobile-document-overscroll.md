# Fix mobile document overscroll on `/new` and conversation views

## Problem

On iOS Safari, focusing the message composer allows the whole page/document to overscroll. The issue is visible on both `/new` and `/c/:slug`; a captured `/new` case shows a large blank region between the composer and the software keyboard/browser chrome after the page has moved.

This work is deliberately separate from the in-progress conversation scroll-state-machine rewrite. `/new` does not render `MessageList`, Virtuoso, or the conversation scroll reducer, so the shared failure must be investigated at the document/root/app viewport boundary rather than in message-list policy.

## Evidence and likely root cause

Current shared viewport CSS leaves the browser document scrollable:

- `html`, `body`, and `#root` have heights but no overflow containment.
- `#app` is `100vh` followed by `100dvh`, but has no overflow containment.
- conversation pages render their flex shell as `#app`.
- `/new` does not render `#app`; its `.new-conv-page` instead has `min-height: 100vh` inside the mobile `DesktopLayout` wrappers. On iOS, `100vh` can be taller than the visible/dynamic viewport when browser chrome or the keyboard is present.
- `ui/src/hooks/useIOSKeyboardFix.ts` exists but has no caller anywhere in the current tree. It listens to `visualViewport`, mutates `#app` height, calls `scrollTo`, and schedules delayed `scrollIntoView`; do not reconnect it blindly.

Relevant history:

- `a432b65f` introduced fixed/overflow-contained `html` and `body`, an overflow-contained `#app`, and an iOS keyboard hook to address document movement on focus.
- `dd31bcc3` removed fixed positioning from `html`/`body` but retained overflow containment.
- `6fefe51d` (“Clean up layout: remove all iOS keyboard hacks”) removed overflow containment from `html`/`body`, removed containment/positioning from `#app`, and simplified to flex layout. The unused hook remained.
- No recent commit changing the conversation scroll machine can explain `/new`; recent mobile conversation commits also do not directly change this shared viewport chain. Treat “recently became visible” as unproven until the repro is bisected or attributed to browser/layout conditions.

A Chromium mobile viewport without the software keyboard is insufficient to reproduce iOS visual-vs-layout viewport behavior. Native iOS automation was attempted during investigation but the Explore-mode sandbox could not create agent-browser’s socket directory. Reproduction on iOS Safari or an iOS Simulator is the first implementation step.

## Plan

1. **Reproduce on `/new` first**
   - Use iOS Safari or an iOS Simulator at a phone viewport.
   - Focus `.new-conv-textarea-mobile`, then drag/scroll the page around the composer as in the report.
   - Record before/focused/overscrolled values for:
     - `window.scrollY` and `window.innerHeight`;
     - `visualViewport.height`, `offsetTop`, and `pageTop`;
     - `documentElement` and `body` `clientHeight`/`scrollHeight`;
     - bounding boxes and computed height/overflow for `#root`, `.new-conv-page`, and the focused textarea.
   - Confirm whether the blank area is document scroll, visual viewport offset, child overflow, or a combination.

2. **Confirm the same boundary on one conversation**
   - Repeat with a conversation composer.
   - Measure the document, `#app`, `.conversation-column`, `#main-area`, and `#input-area`.
   - Do not modify `scrollMachine.ts`, `MessageList` auto-follow policy, or the in-progress ownership rewrite unless measurements contradict the `/new` isolation evidence.

3. **Identify the regression/exposure point**
   - With a deterministic native-Safari repro, compare the current build against likely historical boundaries, especially before/after `6fefe51d` and recent mobile shell changes.
   - Distinguish a Phoenix regression from a latent layout flaw newly exposed by browser behavior or changed page geometry. Record the exact first bad commit only if the repro proves one.

4. **Establish one viewport owner**
   - Prevent the document/root chain from becoming the scroll container for app-shell routes.
   - Keep scrolling in intentional inner owners: the `/new` content area where needed and the conversation message scroller.
   - Replace `.new-conv-page { min-height: 100vh }` with sizing inherited from, or explicitly aligned to, the dynamic app viewport; avoid nested `100vh`/`100dvh` owners.
   - Prefer CSS containment and a structurally clear flex chain (`height`/`min-height: 0`/owned overflow) over JS scroll resets and delayed `scrollIntoView` calls.
   - If `visualViewport` JS is still necessary after CSS containment, implement the smallest measured adapter at the shared shell boundary. Do not revive the dead hook unchanged; either replace it with covered behavior or delete it as vestigial code.

5. **Add regression coverage**
   - Add a layout-level assertion or browser test that `/new` and conversation shells do not make the document vertically scrollable at mobile viewport sizes.
   - Cover focused-composer/visual-viewport behavior with the most native Safari-capable test available; if automation cannot summon the keyboard reliably, retain a documented manual iOS QA case alongside deterministic structural assertions.
   - Ensure intentionally document-scrollable non-chat routes are not accidentally broken by a global rule.

## Acceptance criteria

- On iOS Safari, focusing and interacting with the composer on `/new` cannot move the app shell away from the visible viewport or reveal blank document space.
- The same holds for conversation views, independently of message-list scroll-policy behavior.
- `window.scrollY` remains zero (or otherwise demonstrably non-user-scrollable) for these app-shell routes while the keyboard is open; intentional inner scrollers continue to work.
- `/new` no longer mixes a `100vh` minimum with a dynamic viewport-owned ancestor in a way that can enlarge the document.
- The solution has one clear viewport owner and does not rely on recurring timers or competing scroll resets.
- The unused `useIOSKeyboardFix` is either replaced by a tested shared-shell adapter with a real caller or removed.
- Mobile Safari manual QA and relevant automated UI tests pass.
- `./dev.py check` passes.

## Manual QA matrix

- `/new`: focus composer, type, drag upward/downward around the composer, dismiss and reopen keyboard.
- Conversation at newest: focus composer and repeat; composer remains above keyboard and message list remains the only content scroller.
- Long conversation away from newest: keyboard focus does not force document movement or regress user-owned message scrolling.
- Rotate portrait/landscape and exercise Safari URL-bar expansion/collapse.
- Desktop and Android/Chromium mobile: no new clipping; intentional page routes remain scrollable.

## Implementation result

- `/new` and `/c/:slug` now opt into a route-scoped dynamic viewport owner at `DesktopLayout`; `html`, `body`, and `#root` are contained only while that owner is active.
- The mobile and desktop wrapper chain has explicit flex sizing and `min-height: 0`. `/new` inherits the owned viewport instead of creating a nested `100vh` minimum, and `.new-conv-main` remains its intentional inner scroller.
- Conversation `#app` inherits the shared owner while `#messages` remains the content scroller.
- The unused timer- and scroll-reset-based `useIOSKeyboardFix` hook was removed.
- Regression coverage checks route scope, global-class lifecycle across ownership changes, viewport/flex containment, and the absence of a nested `/new` viewport minimum.

## Verification

- Chromium at 390×844: `/new` and `/c/fixture-turn-one` both reported document `scrollHeight === clientHeight`, `window.scrollY === 0` after a forced scroll attempt, and focused composers remained inside the shell. The conversation message scroller remained independently scrollable.
- Chromium at 1440×900: conversation shell remained viewport-sized. Navigating to `/about` removed document containment and restored the route's normal overflow ownership.
- iOS 17.4 Simulator, iPhone 15 Pro: Safari rendered `/new` with the page and composer fitted to the visible browser area in portrait. This environment has no Simulator input driver and macOS accessibility automation is blocked, so software-keyboard drag gestures still require reviewer/device manual QA using the matrix above.
- `./dev.py check` passes.
