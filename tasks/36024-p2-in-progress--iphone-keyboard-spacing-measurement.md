# iPhone keyboard spacing: bounded composer correction and acceptance

## Active scope and status

The measurement-only commission below was extended by the owner to publish the
reviewed composer cascade correction only: move the existing standalone/mobile
6px bottom-padding exception after the general safe-area default. The original
report is clarified as Home Screen PWA, based on source base
`7b0de589029bd842dab794599a3a69f77481fa3f`. Only the extra 34px **middle** band is
addressed (40 → 6px); StateBar outer 34px and primary controls 44px remain.
Browser and zero-inset paths are unchanged. No viewport/autosize/transcript or
native-client changes are in scope.

- [x] Freeze the two product/test files; approved patch SHA-256
  `f904b8046924c7af5f8a89a852ea60a6ad1e7f9085e540c50ff078477f17c2c4`.
- [x] Owner reports two independent reviews plus owner review with no findings.
- [x] Prove focused regression red (23 pass / 1 fail) before correction and
  green (24/24) afterward; cached UI lint/style and app/node typechecks pass.
- [x] Validate 36 paired synthetic browser samples; retain compact numeric
  results and selected synthetic-only screenshots in
  `docs/screenshots/iphone-keyboard-spacing/`.
- [ ] Parent qualifies published exact HEAD, hosted CI and fresh review.
- [ ] Physical Home Screen PWA / real keyboard acceptance: **unverified**.

Task remains **in progress**, not accepted/done. The actual-shell fixture,
mocked API/extensions, stories and exploratory capture harness remain uncommitted
local/private work and are excluded from publication. The matrix used Chromium's
native safe-area env override, TEST-ONLY standalone media substitution, and
WebKit TEST-ONLY env substitution; reduced layout and visual viewports changed
together. It is not a real device/keyboard test. No private original image or
production conversation text is published. No merge/deployment is authorized.

## Earlier measurement-only work (historical context)

Create a bounded, measurement-only iPhone keyboard/conversation-spacing fixture using real InputArea, WorkControlBar, and StateBar ancestor chains. Capture settled browser geometry and screenshots across viewport-height/width changes with cached WebKit and Chromium; document fixture fidelity gaps. Do not change product behavior. Keep results ignored/local and report unsupported conclusions.


Fidelity extension (owner review pending): opt-in real Ember and ordinary actual-wrapper stories mount DesktopLayout and the production route/provider/transcript/composer chain. No product viewport, CSS, or autosize changes. Existing synthetic stories and 48-sample baseline captures preserved.

Local evidence (this harness only, ignored): `qa-artifacts/iphone-keyboard-conversation-spacing/real-shell-captures-02/report.json` plus 48 PNGs. Cached WebKit 2311 first, Chromium 1228 control; Playwright 1.61.0. Empty / three-line / overcap drafts retained across 390×844 → 390×500 → restore → 844×390. One setup scroll to final real message; no post-resize scroll correction. Real-shell matrix passed. Focused fixture regression suite: 20 passing; app tsc and changed-file lint passing with target-owned caches.

Measured Ember below-StateBar gap is 0–0.015625 CSS px, internal primary bottom padding 0px, no document scroll/overlap. Final row remains visible; scroll owner remaining distance is 0px Chromium / 0–1px WebKit. These synthetic layout-resize observations do not reproduce the reported bottom gap and do not prove physical iPhone keyboard correctness. Actual browser visualViewport follows layout viewport here (offsetTop 0, scale 1); no independent keyboard-only shrink or shim. Width 844 crosses the desktop breakpoint. Original user image unavailable; no new product RCA inferred.
