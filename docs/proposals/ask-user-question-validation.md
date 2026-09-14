# AskUserQuestion implementation validation

Implementation worktree: `phoenix-auq-ux`, branch `codex/auq-ux-bug-hunt`.
The user approved the audited revision 3 proposal on September 14, 2026.
Approval covers implementation, not merge or deployment.

## Audit findings and corrections

| Finding | Correction | Verification |
| --- | --- | --- |
| Custom multi-select text omitted or checkbox toggled by editing | Separate labeled choice from editor; explicit retained custom inclusion and focus on deselection | QuestionPanel interaction tests; Chromium/WebKit journeys |
| Previously selected preview survives absent preview | Derive preview and submitted annotation from current single selection | Mixed-preview tests and browser journeys |
| Keyboard focus can disappear below answer viewport | Native input focus and nearest scrolling; body owns scrolling | Native arrow/Tab/Escape browser journeys |
| Notes missing from ordinary/multiple answers | Per-question notes for every answer mode; collapsing retains inclusion | Payload, navigation, and collapsed-notes tests |
| Controls lack names or repeat descriptions | Explicit concise label and separate description association | Testing Library roles and browser accessible-name locators |
| Long choices lose reading context | 840 px container columns, bounded content width, mobile stacking, full-region expansion | Eight-size matrix including actual ProductConversationPage |
| Stale or late operation targets another question | Required originating identity across API, runtime, web, CLI, and iOS; guarded callbacks | Admission/transition tests, web late-callback test, iOS request tests |
| Transport failure invites a different second answer | Frozen operation with authoritative status check and identical explicit retry | Unknown-outcome, retry-rejection, malformed-state tests |
| Modal clips or leaves background interactive | Native modal dialog with explicit cancel/send-chat decision | Browser top-layer and viewport assertions |
| Modal obstructs global help/palette | Close only the confirmation before handing off to global UI | Help hook and command-palette handoff tests |
| Long wrapped preview evades disclosure | Bound displayed preview height to eight lines, measure overflow | Long single-line preview browser scenario |
| Choice positions shift on narrow preview selection | Stable preview action after the choice list | Browser layout-position assertion |
| Expanded panel shrinks from measuring its own content | Measure stable page owner minus reserved chrome; observe wrapping/insertion | Actual embedded product-page geometry; short-content expansion test |
| Failure between message and state saves permits partial consumption | Atomic persisted identity/message/state operation before publication/resume | Database fault/concurrency and runtime tests |
| Dismissal can drain queued work | Preserve idle waiting through dismissal and restart; settle active turn without draining | Runtime active-turn/queued-work/restart regressions |
| Native retry remains available after failed status check | Separate status-check eligibility from retry eligibility; only a matching authoritative snapshot permits retry | Native reconciliation regressions |
| Unknown browser status is mistaken for resolution | Parse reconciliation through the shared state decoder and reject unknown state shapes | Malformed/unknown-state interaction regressions |
| Lost SQLite commit result leaves stale runtime authority | Classify exact durable message/state/turn evidence once; close the existing fatal-authority fence when the result remains unknown | Transaction response-loss and runtime fail-stop regressions |
| Provider reuses a tool ID for a later question | Bind mutations and drafts to a durable server-generated question incarnation | Reused-provider-ID, migration, and client identity regressions |
| Successful native POST waits indefinitely for SSE | Resolve only the originating attempt and restart reconciliation after confirmed success | Actual URLSession success and delayed-response tests |
| Escape bypasses open notes from another control | Close notes and restore disclosure focus before offering dismissal | Keyboard interaction regression |
| Code preview wraps long code into unreadable fragments | Render fenced code distinctly with horizontal scrolling inside the preview | Long-code Chromium/WebKit geometry assertions |
| Dismissal leaves queued chat paused or permits reordered continuation | Persist dismissal pause separately; explicit chat acceptance releases it atomically and drains the FIFO batch | Real chat API, runtime reconstruction, prompt-order, and single-dispatch regression |

## Portable validation

- `./dev.py qa ask-user-question`: real components, deterministic local fixtures,
  native browser keyboard input, assertions, and screenshots. Ten scenarios:
  ordinary, preview, multi-question, multi-select, proven rejection, read-only,
  long wrapped preview, fenced code, compact question headers, and the actual product page.
- Viewports: 320×568, 390×844, 600×568, 839×900, 840×900, 1024×900,
  1440×900, and 390×320. Chromium and WebKit run the same journeys.
- Focused web tests cover content preservation, explicit selection, custom
  inclusion, no implicit send, IME, overlap protection, dismissal confirmation,
  frozen retries, malformed reconciliation, and late completion isolation.
- CLI request tests exercise required identity. Native app build and 39 iOS
  simulator tests cover request bodies, missing identity, identical-text request
  replacement, frozen uncertainty, and callback isolation.
- Rust tests cover endpoint admission, consuming transition identity, database
  rollback/concurrent consumption, and runtime persistence/resumption boundaries.

## Results

- Initial implementation browser matrix: **144/144 journeys passed**, 72 per browser. Two additional native-browser journeys verified Other deselection focus and draft restoration after the initial focus correction.
- PR review browser matrix: **160/160 journeys passed**, 80 per browser, including fenced-code scrolling and the server-generated request identity contract.
- Four further native-browser keyboard journeys passed at 390/1440 px in Chromium/WebKit after the final focusable-code correction, exercising arrows and Home/End. The code renderer retains DOM identity across draft changes.
- Native iOS: **39 tests passed** on an iOS 17.5 simulator; full app/test build passed.
- PR review follow-up: **45 focused native tests passed**, including failed/malformed reconciliation, unsupported SSE state, confirmed success without SSE, and late old success; **159 focused web tests passed**, including normalized recovery state and unknown-state reconciliation.
- Backend review follow-up: **23 focused tests passed** (9 database, 6 state-machine, 8 runtime/API), covering request incarnations, migration preservation, response-loss classification, fail-stop, durable dismissal pause, and FIFO continuation after restart.
- CLI: **6 tests passed**.
- Database: **6 atomic question tests passed**, including failure and concurrent-consumer cases.
- State machine: **173 tests passed**.
- Integrated runtime/router suite: **7 tests passed**, including real mock-model chat → question → answer/dismiss → settled turn and explicit-prose continuation. All seven passed again after the final async-helper extraction.
- Repository-wide `./dev.py check --compiler-cache none`: 18/19 gates passed initially; the remaining Clippy findings were corrected and Clippy/formatting reran successfully (2/2). The final frontend rerun also passed TypeScript, ESLint, Stylelint, and Vitest (4/4). Every repository gate has passed; no failing gate was waived.

## Release contract

Question response and dismissal now require the originating server-generated `request_id`. Web,
CLI, and native iOS adaptations are included; older identity-free clients receive
an actionable rejection. Release the corresponding clients with the server;
there is no silent legacy fallback.

Migration 097 assigns durable identities to existing pending questions, preserves
legacy queued-input eligibility, and pauses empty legacy dismissed queues.
The provider's `tool_use_id` remains
provenance; it does not authorize an answer to a later request that reuses it.

## PR review evidence

PR #770's initial frozen target was
`dd54a4fb04588ceb06e328082b66a6638cd6d1fa..c882dfe5d4859e0b1f214e546f50b99736c060c3`.
An isolated local review sealed its SQLite authority-loss finding before Codex
feedback was read. Codex reviewed the same target and returned seven findings:
one overlapping authority-loss defect and six additional findings. All seven
were validated; the queued-message finding's actual API path could remain paused
indefinitely, stronger than the reordered-turn symptom suggested by the review.
The corrections and regressions are recorded above. Follow-up review must cover
the updated PR commit; the initial review is not approval of the changes.

A second isolated local pass targeted
`dd54a4fb04588ceb06e328082b66a6638cd6d1fa..ea46b58a95ad67207d6b9a9134b77cb59fff9708`.
It found that the question commit returned an admission token without restoring
it to the enclosing terminal operation. The correction keeps dismissal tracked
through its final state publication. A focused regression reserves an SSE range,
dismisses an active question, closes the authority fence, and verifies that the
queued message and state notification retain ownership until publication.
All nine focused runtime/API question-mutation tests passed with this correction.

The full check on that review target passed 18/19 gates. An unchanged database
telemetry test, `two_connections_record_overlapping_native_reads`, failed its
concurrency-peak assertion and stopped the Rust suite early. The remaining Rust
coverage and the final correction require a follow-up run; this result is not
reported as a clean full check.

The follow-up on `8e8941073c10e962d58e259cae5a52cfb51afe33` passed all 19 local
checks. The telemetry test also passed individually and with its 50-test module.
Remote Rust CI encountered an unchanged terminal-server liveness-probe failure;
the failed job was retried without changing the code.

Codex reviewed that commit and returned five further findings. The migration
finding exposed absent historical evidence: legacy queue rows record neither
admission source nor acceptance time. Migration preserves their established FIFO
restart eligibility instead of inferring an unprovable pause; empty legacy
dismissed queues gain pause ownership, and all new dismissals use the exact
transactional policy. The other corrections enable Other's narrow-layout preview
shortcut and fetch authoritative web/native state after success or stale
rejection. Failed refresh keeps the consumed question closed with status checking
and no mutation retry. Regressions cover recovery errors after successful sends,
composer eligibility without SSE, late refresh isolation, and legacy queues on
both sides of dismissal. The focused follow-up passed 25 web/fixture tests,
47 native simulator tests, three migration tests, and six frontend/spec gates.
The updated Chromium/WebKit matrix passed all 160 journeys, including Other's
narrow-layout preview focus and successful submission with authoritative refresh.

Review moves added from these findings: inspect missing historical information
before designing a migration predicate; follow a successful mutation through the
next required control without SSE; and test every selected-state variant of
secondary navigation actions.

A bounded local follow-up found two display projections omitted by status
refresh: native presentation/activity/list metadata and the web phase timestamp.
Native status refresh now adopts a typed status-only snapshot without claiming
transcript coverage; web refresh carries the server timestamp into the phase
owner. The native unsupported continuation-failure example was disproved because
that state remains unclassifiable; recognized error and working states reproduce
the metadata defect. The corrections passed 48 native simulator tests and 234
focused web tests, including a refreshed working clock advancing without SSE.

## Qualification limits

Browser automation and the native iOS simulator suite do not establish physical
mobile software-keyboard behavior or screen-reader usability. VoiceOver, TalkBack,
and physical iOS/Android keyboard qualification remain a release qualification
gap tracked in [task 10006](../../tasks/10006-p1-ready--qualify-auq-physical-keyboards-and-scree.md). Equivalent reduced CSS viewports at increased device scale exercise reflow;
they do not substitute for a manual browser-zoom audit. No production deployment
or merge was performed.

Screenshots and logs are local ignored evidence under
`ui/dogfood-output/implementation`, `ui/dogfood-output/implementation-webkit`,
and `ui/dogfood-output/screenshots`; committed fixtures and tests reproduce them.
