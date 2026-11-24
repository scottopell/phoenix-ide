# Focus mobile Ember questions and qualify question readiness

Commissioned by Coordinator: long questions at 393x852 share viewport with live transcript chrome and nested scrolling. Provide focused mobile answering, one content scroller, reachable actions/custom input with background inert; non-destructive Return preserves answers and conversation/composer state. Preserve all request-bound submit/dismiss and desktop behavior. Measure pinned-base versus candidate cold/deep-link and warm notification opening with synthetic long transcript under identical CPU/network conditions; separate question usability from history readiness. Existing question feed is independent of transcript: dominant loading cost remains unmeasured. Hosted-only validation, no production snapshots/native builds/deploy; RC11 and broad performance work separately owned.

## Desktop placement follow-up — PR1156

PR1151 is merged. The release owner reported installed RC13 desktop top clipping; this is not a complete desktop usability pass. Disposable hosted RC13 (`1c125f0360251d823710762f474d35ff268fa648`) reproduced the metadata/dialog top at -17px and dock scrollTop 17 at 1280×900 before scripted repair (question QA run 45340421144). The transcript was scrolled; document scrollY was zero. That run later failed on an invalid tail-visibility expectation after the placement journey deliberately changed reading position; it is not candidate qualification.

PR1156 preserves scroll when automatically focusing the question on appearance/reactivation, retaining reveal-on-focus for explicit step changes. Expanded hosted regressions distinguish full clipped metadata from reachable options/actions, modal versus inline clip ancestors, 768/769 transitions, Next/Back, mounted answer/composer retention, and interior mobile reading position checked separately on reopen and Return. Qualification and final-head checks remain pending; do not treat historical receipts below as PR1156 acceptance. No merge/deployment in this scope.

Loading diagnosis is separate and paused awaiting the existing affected-client observation request. The bounded synthetic early-acquisition experiment did not demonstrate question speed improvement; do not restart it or ship speculative preloading. Physical device/OS keyboard and cross-viewport reading-anchor preservation remain unqualified.

## Integration correction and continuing loading investigation

- Integration review on `b9212938c` found a genuine desktop regression: `dialog.show()` matched the global `dialog[open]` guard and suppressed outside-dock transcript help/composer Escape. Earlier no-blocker reviews missed that boundary; their verdict is superseded.
- Merged fresh main `ad27c4ba0d9a2d25a6d182e12b9d0d75bd1c5455` into the owned branch. Corrected native-modal detection to `dialog:modal`, with hook regressions and actual hosted desktop outside-dock help/composer Escape/nested-confirmation checks alongside mobile modal exclusion/retention checks. No merge to main or deployment.
- Independent reviews of exact fix plus measurement delta found no concrete blocker. Current qualified source `09f3b07df186b2f01e764ba47cd5f89f94fff46d`; hosted question QA https://github.com/scottopell/phoenix-ide/actions/runs/45304420256 passed all 124 records (96 measured, eight interaction-free readiness observations, four actual in-flight held histories, 16 warmup/preflight). First fix run stopped on incidental missing file-list fixture after desktop assertions passed; real empty-list contract added.
- Four controls now prove option/custom interaction while history is already in flight and held, not merely before it was requested. All eight independent-readiness controls latch question and transcript layout before any scripted interaction. Document ResourceTiming permits same-clock correlation; rAF layout sampling adds measurement work and does not establish paint or unobstructed visibility.
- Candidate mobile cold/warm controls observe question layout ~2993/989ms and transcript layout ~5268/3502ms, not speed improvements. Waterfall plus source show transcript lazy-graph acquisition starts after coordinator resolution, before history acquisition; another ~0.8–0.9s history-response-to-layout interval has unattributed long tasks.
- **Historical loading handoff, superseded by the pause above:** the subsequent disposable early-acquisition experiment improved synthetic transcript layout but not question layout. No production cause, background-cancellation or speed claim is established. Further diagnosis awaits the existing affected-client observation request; release verification remains separately owned.
- No unknown fixture routes or page errors; one injected 409 diagnostic. 438/5953 request records lack completion and are censored/unclassified (not a clean network-completion claim). Aggregate results preserve 124 records but four preflight names overlap warmups. Physical iPhone limits remain.
- Exact resulting-head hosted checks and guarded integration handoff are required before review readiness. Historical receipts below remain version-bound, not final-head claims.

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
