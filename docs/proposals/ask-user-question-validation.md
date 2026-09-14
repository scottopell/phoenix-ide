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

## Portable validation

- `./dev.py qa ask-user-question`: real components, deterministic local fixtures,
  native browser keyboard input, assertions, and screenshots. Nine scenarios:
  ordinary, preview, multi-question, multi-select, proven rejection, read-only,
  long wrapped preview, compact question headers, and the actual product page.
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

- Final browser matrix: **144/144 journeys passed**, 72 per browser. Two additional native-browser journeys verify Other deselection focus and draft restoration after the final focus correction.
- Native iOS: **39 tests passed** on an iOS 17.5 simulator; full app/test build passed.
- CLI: **6 tests passed**.
- Database: **6 atomic question tests passed**, including failure and concurrent-consumer cases.
- State machine: **173 tests passed**.
- Integrated runtime/router suite: **7 tests passed**, including real mock-model chat → question → answer/dismiss → settled turn and explicit-prose continuation. All seven passed again after the final async-helper extraction.
- Repository-wide `./dev.py check --compiler-cache none`: 18/19 gates passed initially; the remaining Clippy findings were corrected and Clippy/formatting reran successfully (2/2). The final frontend rerun also passed TypeScript, ESLint, Stylelint, and Vitest (4/4). Every repository gate has passed; no failing gate was waived.

## Release contract

Question response and dismissal now require the originating `tool_use_id`. Web,
CLI, and native iOS adaptations are included; older identity-free clients receive
an actionable rejection. Release the corresponding clients with the server;
there is no silent legacy fallback.

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
