# Fix AskUserQuestion answer loss and interaction gaps

Bug hunt baseline: ce66022db. Dedicated branch: codex/auq-ux-bug-hunt. Reusable scenarios: ui/src/fixtures/askUserQuestion; run ./dev.py qa ask-user-question for desktop/mobile captures.

## Verified problems and reproduction

1. High: multi-select story → select Current conversation → click Other textarea → type Only archived conversations → Submit. Other remains unchecked and outgoing answers omit the custom text. Clicking the focused textarea toggles the checkbox. Editing custom text must include it without accidental toggle.
2. Medium: preview story → ArrowDown → Space selects Include ancestors (no preview), but scope: current from the previous option stays visible. Absence of a preview must clear the pane.
3. Medium: preview at 1440×900 → ArrowDown three times. Other has keyboard focus below the visible scroll area (top 973 px, viewport bottom 803 px). Focus changes must keep the option visible.
4. Medium: plain and multi-select stories provide no Add notes; n does nothing. REQ-AUQ-003 permits notes without a preview prerequisite.
5. Medium: predefined radio/checkbox inputs have no accessible names. Associate visible option labels and descriptions with the controls; validate browser accessibility tree and a screen reader.
6. Low: preview options are constrained to 320 px beside an oversized preview pane; long descriptions push the question/preview out of view while the user reaches notes. Explore a wider reading column and retained context with the existing long-content fixture.

## Boundary and likely owners

QuestionPanel QuestionItem (Other click/focus callbacks, notes condition, radio/checkbox labels), focusedPreviews update, focusedIndex navigation, and QuestionPanel.css own these failures. The outgoing api.respondToQuestion arguments prove item 1 before the backend boundary. REQ-AUQ-002/003 govern previews/answer collection; keyboard-interaction requirements govern focus behavior.

## Acceptance and validation

- Regression for clicking, typing, editing, and submitting Other alongside predefined multi-select choices; assert outgoing answer payload.
- Mixed preview/no-preview choice changes clear stale content.
- Keyboard traversal scrolls focused items into view at 1440×900 and 390×844; named inputs expose their labels.
- Plain/multi-select annotations survive navigation and submission; determine consistent Other+notes presentation.
- Retain passing navigation, multiline custom answers, notes+preview submission, failure retention, and dismissal behavior.
- Re-run fixture captures, focused tests, and ./dev.py check.
- After UI fixes, mock-runtime integration: actual waiting → response → resumed turn; dismissal remains waiting for explicit prose. Reconnect, delayed submission, and real iOS keyboard remain additional validation coverage.

Local evidence: ui/dogfood-output/report.md, screenshots/, videos/ in the dedicated worktree. Evidence artifacts are ignored; exact reproduction steps above and committed fixture scenarios are the portable source.

Scope: AUQ user interaction and fixtures. No provider changes, production edits/deploy, or conversation lifecycle redesign.

## Approved implementation

The user approved the [revision 3 design proposal](../docs/proposals/ask-user-question.md) for full implementation on September 14, 2026. The redesigned browser panel and cross-client request contract are implemented. Independent audits also closed partial persistence, queued dismissal, active-turn settlement, restart, modal, focus, and responsive-layout gaps.

[Validation record](../docs/proposals/ask-user-question-validation.md) contains the correction matrix and reproducible checks. Physical-device keyboards, screen readers, and manual browser zoom remain the release qualification gate in [task 10006](10006-p1-ready--qualify-auq-physical-keyboards-and-scree.md). Merge and deployment are not authorized.

Implementation complete. Every repository check passed, with final lint, frontend, and runtime reruns after audit corrections. Browser validation passed 144 matrix journeys plus two final focus journeys; 39 native iOS simulator tests passed. Manual release qualification remains explicitly tracked separately.
