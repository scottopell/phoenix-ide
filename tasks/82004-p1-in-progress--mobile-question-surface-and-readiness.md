# Focus mobile Ember questions and qualify question readiness

Commissioned by Coordinator: long questions at 393x852 share viewport with live transcript chrome and nested scrolling. Provide focused mobile answering, one content scroller, reachable actions/custom input with background inert; non-destructive Return preserves answers and conversation/composer state. Preserve all request-bound submit/dismiss and desktop behavior. Measure pinned-base versus candidate cold/deep-link and warm notification opening with synthetic long transcript under identical CPU/network conditions; separate question usability from history readiness. Existing question feed is independent of transcript: dominant loading cost remains unmeasured. Hosted-only validation, no production snapshots/native builds/deploy; RC11 and broad performance work separately owned.

## Delivery

- PR: https://github.com/scottopell/phoenix-ide/pull/1151 (`fix/mobile-question-surface`).
- Implementation and independent source/artifact reviews complete; awaiting integration review. No merge, deployment or restart performed.
- Mobile uses a native modal and one content scroller, with safe-area/visual viewport bounds and nondestructive Return. The request-keyed question panel stays mounted; inactive focus/keyboard scopes are released. Conversation loading continues behind it. Desktop stays nonmodal.
- Fixed nested confirmation coverage and focus, plus shortcut-help modality. Tests cover answer retention and confirmation/breakpoint focus races.

## Verified evidence

Qualified candidate: `e84976f940eef714801f7f3dd6aae52dfbe11f2d`.
Pinned baseline: `8a685486413fc6517f61c548e3229a2864ce6703`.

- Hosted CI: https://github.com/scottopell/phoenix-ide/actions/runs/45291126141 — success, including full UI tests/lint and other repository checks.
- Hosted question qualification: https://github.com/scottopell/phoenix-ide/actions/runs/45291125836 — success; artifact `global-question-loading-45291125836`, SHA receipts, raw `results.json`, request/CPU observations, journey evidence and settled screenshots.
- All 116 synthetic trials succeeded, including 96 measured samples (runs 0–7, eight per version × viewport × entry mode). 393×852 and 1440×900; cold/global, cold/canonical-link, warm SPA notification navigation; 4× CPU, modeled 400ms API latency, 1MiB/s asset conditions. Production bundles, mocked API/EventSource, no production data.
- Corrected mutable warm-setup request clock before accepting evidence. All available final request timestamps are monotonic; initiation phases are separated. Some requests are incomplete at capture. Document-clock long tasks are not exactly aligned to Node request timing and include observation after question interaction.
- No unknown fixture routes or page errors; one expected injected 409 console diagnostic.
- Browser journeys: answer/custom retention on Return/reopen, real mounted composer draft/transcript scroll, modal focus exclusion, one content scroller, full-surface confirmation geometry/hit testing at normal and 393×520 viewports, nested Escape, shortcut help, multiple requests/custom/multi-select, 409/retry, dismissal, withdrawal, reload, canonical query-variant Back/Forward.
- Inspected baseline/candidate mobile, candidate desktop and reduced-height confirmation screenshots. Final independent source review and artifact audit reported no concrete blocker for bounded claims.

## Findings and remaining limits

No speed improvement demonstrated: median mobile usability was baseline→candidate 3443→3450ms cold/global, 3434→3440ms cold/link, 1497→1499ms warm. Desktop medians likewise unchanged (3431→3434, 3428→3444, 1457→1456ms). Usability includes scripted selection/custom entry; transcript visibility is observed after interaction, not independently timed first readiness.

Both versions supported question interaction before history acquisition/reply in four cold/global controls. The history request did not begin until after usability and gate release, so this does not demonstrate responsiveness during an already in-flight stalled request. Source confirms feed mounting outside transcript Suspense; no background-fetch cancellation or production loading-cause claim. Bounded production trace lookup found no relevant waterfall; backend responsibility remains unproven either way.

Physical iPhone keyboard/safe-area/focus behavior remains unverified (reduced viewport only). Back/Forward covers query variants, not close-first modal history or draft retention after route unmount/reload. Feed loading/error and desktop confirmation/help are not comprehensively browser-qualified. Real SSE, server/database work, notification delivery and API throughput are unmeasured. Four preflight filenames overlap warmup filenames, but aggregate results retain every record and all 96 measured samples have distinct artifacts.

Detailed medians, verification receipts and limits: `specs/ask-user-question/executive.md`. Scope excludes deployment, RC11 gating and broad performance optimization. Final documentation updates require their own hosted checks; do not present earlier source receipts as exact-head results for later commits.
