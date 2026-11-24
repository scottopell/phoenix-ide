## Bounded question attribution and preload cost decision

Further read-only source and artifact reviews used exact merged source `0f6e3627` and the eight measured **control** trials per viewport/mode from diagnostic run45311433061. All times below are document-clock navigation-relative medians, not production observations. No additional experiment or product-source change was made.

| Event | 393 cold | 1440 cold | 393 warm | 1440 warm |
|---|---:|---:|---:|---:|
| Shell index JS response end | 1014.7 | 1008.6 | -1796.9 | -1913.6 |
| Auth response end | 1455.8 | 1443.8 | -1338.1 | -1452.2 |
| CoordinatorPage JS response end | 1913.5 | 1907.7 | 430.6 | 438.9 |
| QuestionPanel JS response end | 1912.2 | 1906.4 | 428.9 | 434.6 |
| Shared lib JS response end | 2333.5 | 2329.8 | 461.8 | 939.3 |
| Shared arrow-left JS response end | 2306.5 | 2302.4 | 825.0 | 844.0 |
| Identity request start / end | 2359.4 / 2769.7 | 2362.6 / 2772.2 | 918.6 / 1321.0 | 1031.3 / 1432.9 |
| Question feed response end | 2770.6 | 2772.5 | -56.6 | -69.7 |
| Independent question layout | 2875.7 | 2882.7 | 919.8 | 1032.6 |

CoordinatorPage and QuestionPanel acquisition start together: there is no separate sequential QuestionPanel-download waterfall. Shared dependency acquisition continues after their own chunks finish. Cold feed and identity requests overlap (feed starts ~4–6ms later), and paired feed-response-to-layout residual is ~112ms. Warm feed has already completed; question layout occurs ~400ms **before** identity response in all16 warm control trials. Source agrees: DesktopLayout starts the shared question feed, CoordinatorQuestionDock is outside the identity-gated transcript branch, and its store subscription does not require identity. Identity correlation is not identity causality.

Resource response end is not module evaluation/React commit. Last acquisition-wave response to identity-request start is ~25–32ms cold/~92–94ms warm; these and the ~112ms cold feed-to-layout residual mix execution, effects/store publication, React work, dialog/layout and rAF observation. No exact QuestionPanel commit mark exists, so the residual must not be called measured mount cost. Existing `phoenix:view-diagnostic` events expose journey admission/shell/snapshot/hydration/transcript/correlation evidence but not pre-attachment module evaluation, question-ready or exact React commit; journey sampling is not forced here.

**Simplest preload cost review:** no existing application-owned preload/cache helper was found. A scoped best-effort existing-module import is simpler than new history ownership/cache/dedup machinery, but the diagnostic `void import()` is not production-ready failure handling. The import cannot be aborted on departure, executes modules and inserts global CSS, and can waste work if initial ensure/source validation fails. The existing Vite preload-error recovery can display an attachment-safe reload notice even for a speculative failure; catching the promise does not necessarily suppress that event. Do not add automatic reload or a sticky rejected-promise singleton. React.lazy and speculative import rejection/retry behavior are not interchangeable.

One representative early mobile cold trial acquires12 transcript assets (7JS/5CSS),219,701 ResourceTiming transfer bytes (~214.6KiB). Matching successful control eventually acquires the same graph later: advanced work, not extra total successful-navigation traffic. The filename-filter receipt captures only69,926bytes and understates graph cost. Actual failed/unused-route waste, style-order effects and composer input responsiveness were **not measured**. Independent question layout did not improve and does not establish no contention/regression. These costs and the lack of affected-client evidence do not justify shipping a preload yet. No new history owner/cache machinery or optimization PR is proposed.

**Precise missing observation / minimal nonmutating capture proposal:** one actual affected foreground Ember entry, on the affected client's ordinary cache state (do not clear it), identifying deployed version/asset hashes and global versus historical route. Collect one bounded client timeline from before navigation through question/transcript/composer readiness (stop at readiness or45s): document/SPA boundary, sanitized ResourceTiming for shell/route/shared graph, auth/identity/route/question/history request start/end/status and request correlation IDs, timestamped existing `phoenix:view-diagnostic` events without forcing sampling, and independent rAF question/transcript layout. Record manual input/visible readiness separately, with no synthetic answer submission. If module-evaluation-versus-commit attribution remains necessary, one short same-entry main-thread recording can resolve the coarse residual; exact React commit still requires explicit instrumentation, not network inference.

This is a **proposal, not a performed capture**. Never export raw HAR, credentials, auth headers, message/question bodies, attachments, screenshots of private content or production database snapshots; retain only sanitized timing/route templates and correlation identifiers locally. No cache/service-worker override, API interception, live duplicate question, forced sampling, state mutation or performance optimization. Match request IDs to narrow server traces/collector warnings only after that real affected entry exists. Existing synthetic fixtures establish frontend ordering under imposed conditions, not reproduction of the remaining deployed complaint. A production cause/fix needs this missing observation; no broad profiling campaign or deployment/restart was started.

---

## Postmerge qualification and isolated causal experiment

PR #1151 was squash merged as `0f6e3627a24777364406c3f3718efe2431ccf16d`; parent `7dc7372a5231d34a6f1e7f846e03db88744b9e77`, tree `e3bd8d5c82ced901207f895bc5da732791f66754`. Exact-source rendered QA [45311375874](https://github.com/scottopell/phoenix-ide/actions/runs/45311375874) passed: 124 aggregate/96 measured, eight per cell, four pending-history controls, eight preinteraction readiness probes, desktop shortcut receipt and mobile journeys. All records passed; no unknown routes/page exceptions. Full postmerge CI [45310777440](https://github.com/scottopell/phoenix-ide/actions/runs/45310777440) also passed, including all four Rust test shards, build/musl/clippy, codegen, UI/specs, e2e and repository checks. Exact-commit status rollup was verified SUCCESS.

Diagnostic branch `diagnostic/question-module-acquisition`, harness commit `68b347e1479b00254e2d0405cf4a3de971f737e8`, is **not a production optimization or merge candidate**. Its manual-only workflow replaces automatic question QA only on this disposable branch. Both production bundles use the exact merged source above; only the early copy gets an asserted, artifact-retained mount-effect import of the existing ProductConversationPage graph. No app-source change is committed. Hosted run [45311433061](https://github.com/scottopell/phoenix-ide/actions/runs/45311433061) passed, artifact `question-module-diagnostic-45311433061`.

Independent source and artifact audits verified 88 unique/matching aggregate and standalone trials: 64 measured (8 pairs per viewport/mode), 8 warmups, 16 held-history controls. All were successful with no unknown routes or diagnostics. Every measured trial observed question and transcript layout independently before any scripted interaction; held controls observed question layout and accepted input while history remained in flight/unreplied. Control/early order alternated and reversed on odd runs. Conditions were hosted Chromium, 4x CPU, 400ms modeled latency, 1MiB/s, synthetic intercepted HTTP/fake SSE; fresh context per trial, shell/feed setup for warm navigation.

Median **paired early minus control** differences, milliseconds:

| Viewport / route | Question layout | Transcript-tail layout |
|---|---:|---:|
| 393 cold-global | +22.4 | -479.0 |
| 393 warm-notification | +3.5 | -662.8 |
| 1440 cold-global | +7.1 | -466.9 |
| 1440 warm-notification | +3.3 | -193.5 |

All eight transcript pairs were earlier in each cell (nominal two-sided exact sign/sign-flip p=0.0078125 per cell; small n, unadjusted, same-host paired trials, not independent machines). Question layout did not demonstrate improvement; lack of significance is not equivalence or proof of no regression. `question_usable_ms` is **not** a causal endpoint here: the scripted interaction deliberately waits for transcript layout. Use only independent navigation-relative rAF layout markers, not postinteraction transcript timers or variable-window CPU metrics. Layout is not paint, unobscured visibility through the mobile modal, or physical input responsiveness.

Manipulation check: ProductConversationPage acquisition started before the navigation-phase Coordinator response in all early measured trials (~407–425ms before), versus after it in every control (~12–130ms after), using the same document clock. History request initiation moved earlier too (median paired changes ~-507/-645/-462/-192ms respectively on its own request clock). Module completion was not necessarily before the Coordinator response. Full resource audit, not the incomplete filename regex, found downstream JS/CSS/font overlap and viewport-specific ordering. Thus this supports the **total early-import intervention effect on synthetic transcript layout**, not a download-only mechanism, question-speed improvement, or production effect size. Import also moves evaluation, CSS and scheduling.

Concrete remaining dependency: history fetch is owned by an effect in the large lazy ProductConversationPage renderer graph; it waits for acquisition/evaluation/mount after identity resolution. A next bounded diagnostic could isolate typed history acquisition in a lightweight resolved-coordinator owner, retaining selection, deduplication, cancellation/freshness and telemetry semantics. That experiment/design has **not** been implemented or approved for shipping here. Question layout instead needs separate upstream shell/QuestionPanel attribution. Production cause remains unresolved; no production snapshots, live questions, local native builds, deployment/restart, or broad performance takeover occurred.

---

# Focus mobile Ember questions and qualify question readiness

Commissioned by Coordinator: long questions at 393x852 share viewport with live transcript chrome and nested scrolling. Provide focused mobile answering, one content scroller, reachable actions/custom input with background inert; non-destructive Return preserves answers and conversation/composer state. Preserve all request-bound submit/dismiss and desktop behavior. Measure pinned-base versus candidate cold/deep-link and warm notification opening with synthetic long transcript under identical CPU/network conditions; separate question usability from history readiness. Existing question feed is independent of transcript: dominant loading cost remains unmeasured. Hosted-only validation, no production snapshots/native builds/deploy; RC11 and broad performance work separately owned.

## Integration correction and continuing loading investigation

- Integration review on `b9212938c` found a genuine desktop regression: `dialog.show()` matched the global `dialog[open]` guard and suppressed outside-dock transcript help/composer Escape. Earlier no-blocker reviews missed that boundary; their verdict is superseded.
- Merged fresh main `ad27c4ba0d9a2d25a6d182e12b9d0d75bd1c5455` into the owned branch. Corrected native-modal detection to `dialog:modal`, with hook regressions and actual hosted desktop outside-dock help/composer Escape/nested-confirmation checks alongside mobile modal exclusion/retention checks. No merge to main or deployment.
- Independent reviews of exact fix plus measurement delta found no concrete blocker. Current qualified source `09f3b07df186b2f01e764ba47cd5f89f94fff46d`; hosted question QA https://github.com/scottopell/phoenix-ide/actions/runs/45304420256 passed all 124 records (96 measured, eight interaction-free readiness observations, four actual in-flight held histories, 16 warmup/preflight). First fix run stopped on incidental missing file-list fixture after desktop assertions passed; real empty-list contract added.
- Four controls now prove option/custom interaction while history is already in flight and held, not merely before it was requested. All eight independent-readiness controls latch question and transcript layout before any scripted interaction. Document ResourceTiming permits same-clock correlation; rAF layout sampling adds measurement work and does not establish paint or unobstructed visibility.
- Candidate mobile cold/warm controls observe question layout ~2993/989ms and transcript layout ~5268/3502ms, not speed improvements. Waterfall plus source show transcript lazy-graph acquisition starts after coordinator resolution, before history acquisition; another ~0.8–0.9s history-response-to-layout interval has unattributed long tasks.
- **Loading remains commissioned and unresolved. Next supported causal measurement:** disposable repeated alternating early transcript-module acquisition vs existing serialized acquisition, holding question/history/content semantics fixed; compare module/history start and independent transcript layout. Do not ship a preload or render rewrite based solely on correlation. No production cause, background-cancellation or speed claim. RC11 verification remains separate and unblocked by this work.
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
