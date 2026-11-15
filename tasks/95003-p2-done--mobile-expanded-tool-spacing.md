# Mobile expanded-tool spacing

## Delivered implementation

PR: https://github.com/scottopell/phoenix-ide/pull/1081

The original private screenshot was recovered through the authenticated message-range API and inspected. The synthetic fixture reproduces its collapsed-send → expanded-preference-read → database ordering without publishing private content.

The embedded selected-tool message rendered an empty continuation toolbar despite suppressed message copy. Omitting that empty chrome removes 44px on mobile; ordinary continuation/streaming chrome, first-message metadata, owner-copy rows and full tool details remain intact.

## Evidence

- Hosted same-fixture before/after run: https://github.com/scottopell/phoenix-ide/actions/runs/43608760260 — success at source `024c3eec2defc775ad2b3f883742b65267be8f63`.
- 393px and 320px content inset: 58→14px; desktop: 14→14px. No horizontal page overflow.
- Send input copy, preference/database result copy, collapse/reopen height, and retained following cards exercised.
- Independent source review at that SHA: no actionable regressions after fixture and geometry-test corrections.
- Durable synthetic screenshots, exact geometry, provenance and limits: `docs/screenshots/mobile-expanded-tools/README.md`.

## Integration gate

Implementation and synthetic qualification are complete. PR readiness still requires all applicable hosted CI and fresh external/independent review at its final head; the PR remains draft until those gates pass. No merge or deployment is authorized by this task.

The evidence is component-level Chromium with touch viewports, not physical iPhone, iOS Safari/PWA, keyboard, full-app scroll anchoring or live automatic-continuation acceptance. Existing owner-copy rows remain intentionally unchanged; this fixes one demonstrated empty-toolbar gap rather than every mobile whitespace region. The separate iPhone keyboard qualification remains deferred.
