# Qualify AskUserQuestion on physical keyboards and assistive technology

The approved AUQ redesign is implemented in task 10005. Automated Chromium/WebKit viewport journeys and native iOS simulator tests do not establish real-device or screen-reader usability. This is the remaining release qualification gate, not additional design or feature scope.

## Required evidence

- Physical iOS Safari and Android Chrome: ordinary, long-preview, and multi-question forms; open Other/notes with software keyboard; type multiline text; rotate; reach Back/Next/Send and dismissal; ensure focused caret and actions remain reachable and drafts persist.
- VoiceOver on macOS/Safari and iOS/Safari, plus TalkBack on Android/Chrome: concise choice names, one description announcement, native radio/checkbox groups, unanswered/completed progress, errors/status, preview disclosure, and modal focus isolation/restoration.
- Manual desktop browser zoom 200% and 400%, including narrow split panes and short viewports: no horizontal overflow, stationary choices, usable body scrolling and footer, full preview access.
- Confirm help/command palette preserve answer drafts and return focus appropriately.
- Record device/browser/OS versions, exact steps, screenshots or recordings, and pass/fail for each surface. Fix reproduced problems, then update specs/ask-user-question/executive.md and docs/proposals/ask-user-question-validation.md.

Start from the committed AskUserQuestion Ladle fixtures (`./dev.py qa ask-user-question`); use the actual product-layout fixture as well as a live request for keyboard/focus lifecycle checks. No production deployment is authorized by this task.
