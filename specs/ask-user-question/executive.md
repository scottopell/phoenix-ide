# Ask User Question Tool - Executive Summary

## Requirements Summary

The ask_user_question tool enables LLM agents to pause execution and ask the
user 1-4 structured multiple-choice questions when multiple valid approaches
exist. Users select from predefined options or provide free-text answers. For
questions comparing concrete artifacts (code, config), options can include rich
previews displayed side-by-side. Users can add notes to their selections for
additional context. Responses are delivered back to the agent as a formatted
tool result. The tool is excluded from sub-agents, which operate autonomously.

## Technical Summary

Follows the `AwaitingTaskApproval` state machine pattern: executor intercepts
the tool call, emits `AskUserQuestionPending`, state machine transitions to
`AwaitingUserResponse`, SSE notifies the UI, user responds via
`POST /conversations/{id}/respond`, `UserQuestionResponse` event resumes
execution. Each new pending request has a Phoenix-owned identity persisted in
conversation state; answer and dismiss mutations must match it while holding
conversation admission. Legacy identity-absent waits remain identity-absent and
answerable. Input validation enforces question/option count constraints and
uniqueness. The tool is registered with `defer_loading: true` for tool search
on supporting models. Tool result format is a human-readable string including
selected labels, preview content, and user notes.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AUQ-001:** Structured Question Presentation | ✅ Complete | `src/tools/ask_user_question.rs`, `AwaitingUserResponse` state, `QuestionPanel.tsx` |
| **REQ-AUQ-002:** Rich Option Previews | ✅ Complete | Side-by-side `question-preview-layout` in `QuestionPanel.tsx`/`.css` |
| **REQ-AUQ-003:** Flexible Response Collection | ✅ Complete | `OTHER_SENTINEL`, `otherTexts`, `multiSelections`, notes/annotations |
| **REQ-AUQ-004:** Response Delivery to Agent | ✅ Complete | `POST /api/conversations/:id/respond`, `UserQuestionResponse` event |
| **REQ-AUQ-005:** Prevent Ambiguous Question Responses | ✅ Complete | Schema constraints in tool; `AwaitingUserResponse` blocks `UserMessage` in `transition.rs` |
| **REQ-AUQ-006:** Parent Conversation Availability | ✅ Complete | Excluded from sub-agent `ToolRegistry` in `src/tools.rs` |
| **REQ-AUQ-007:** Real-Time Waiting Feedback | ✅ Complete | `awaiting_user_response` in `ConversationState` SSE union in `ui/src/api.ts` |
| **REQ-AUQ-008:** Low-Overhead Tool Availability | ✅ Complete | `defer_loading() -> bool { true }` in `ask_user_question.rs` |
| **REQ-AUQ-009:** Bind Actions to the Current Pending Question | ✅ Complete | Persisted request identity, admission-locked HTTP validation, transition recheck, and request-keyed web panel |
| **REQ-AUQ-010:** Coordinator Questions Do Not Block Coordination | 🔄 In review | `post_user_question` replaces the blocking tool for the Coordinator; `coordinator_user_questions` table (migration 168), keyed on the Coordinator ProductConversation; prompt wording. Not deployed. |
| **REQ-AUQ-011:** Answer Coordinator Questions Outside the Transcript | 🔄 In review | `GET /api/coordinator/questions`, answer/dismiss endpoints, retrying reply delivery in input discovery sent with the `coordinator_question_reply` input origin under a reserved `coordinator-question-reply:` message id, with delivery recorded by acceptance triggers on `durable_turns` and `steering_messages`; `CoordinatorQuestionDock` on Ember's page, Ember entry count, question notifications (5 s poll, which also runs in hidden tabs so notifications arrive; opens `/global`). Unit and runtime tests; not checked in a browser; not deployed. |
| **REQ-AUQ-012:** Withdraw and List Coordinator Questions | 🔄 In review | `withdraw_user_question`, `list_user_questions`; database and tool tests. Not deployed. |
| **REQ-AUQ-013:** Focused Mobile Coordinator Questions | 🔄 In review | Native mobile modal, one content scroller, non-destructive Return, retained request input, inactive keyboard scopes, visual viewport bounds. Hosted Chromium journeys and pinned-base comparison passed at `09f3b07df` in PR #1151, including outside-dock desktop shortcuts and actual in-flight history isolation; physical iPhone keyboard qualification remains unverified. |

**Progress:** 9 of 13 complete; REQ-AUQ-010 to REQ-AUQ-013 in review.

## Mobile surface qualification

The conversation stays mounted and continues loading behind the native modal.
The question feed already mounts independently of transcript history; the focused
surface is not a transcript-fetch cancellation or a proven loading optimization.
The hosted comparison uses production bundles, intercepted synthetic HTTP payloads,
4× CPU slowdown, 400 ms modeled API latency and throttled asset requests. A client
EventSource fixture excludes real SSE transport and server/database latency.

### Integration correction and independent readiness evidence

Integration review found that a desktop nonmodal `dialog.show()` still matched the
global `dialog[open]` shortcut guard. The guard now tests native `dialog:modal`:
background desktop transcript `?` and composer Escape work, while native mobile
modality still excludes global shortcuts. Nested confirmation ownership is retained.

[Hosted qualification 45304420256](https://github.com/scottopell/phoenix-ide/actions/runs/45304420256)
passed at `09f3b07df186b2f01e764ba47cd5f89f94fff46d`, including merged main
`ad27c4ba0d9a2d25a6d182e12b9d0d75bd1c5455`. All 124 records succeeded: 96 measured
samples, eight additional independent-readiness probes, four held-history controls,
and 16 warmup/preflight records. Each measured cell still has eight samples. The
first rerun at `b5fc0e66e` passed the new desktop interaction assertions but stopped
on a missing incidental `/api/files/list` fixture; its empty-directory response was
then modeled with the real `{ items: [] }` contract.

All four history controls now explicitly wait until the history handler is in
flight and blocked, then select an option and enter custom text before releasing
it. Both versions at both widths remain usable while that request is held, with
no transcript tail rendered before release. This supersedes the weaker earlier
history evidence below; it does not establish a speed improvement.

Eight cold/global and warm-navigation probes observe both question and transcript
layout signals **before any scripted answer interaction**. A rAF sampler latches
first layout presence, not paint, unobscured visibility or physical usability.
ResourceTiming and these signals share the document clock; warm setup resources
can precede the SPA boundary. Node request callbacks retain a separate trial clock.
The sampler can force layout and adds instrumented CPU work; its timings are not
uninstrumented performance baselines. Probe usability timings are excluded from
the measured matrix (`run >= 0 && !isolation && !readiness_only_probe`).

Candidate mobile cold probe: question layout 2993ms, transcript layout 5268ms;
warm probe: 989ms and 3502ms. These single controls are descriptive, not statistical
speed evidence. The cold ProductConversationPage dependency graph starts around
3022ms after coordinator resolution; history starts around 4073ms and ends 4483ms.
Warm equivalents are approximately 1408ms, 2274ms and 2683ms relative to navigation.
The next supported single causal measurement is a disposable, repeated alternating
control that starts the transcript module graph on Coordinator route entry rather
than after coordinator resolution, leaving question/history/content semantics
unchanged. Measure whether this overlaps module acquisition and advances history
start and independent transcript layout. Do not combine it with a render rewrite.
The separate roughly 0.8–0.9s history-response-to-layout interval contains observed
long tasks but lacks function attribution; no Markdown/React or production cause
is established. No preload optimization is included in this PR.

Independent source and artifact reviews found no concrete functional blocker in
the correction. Desktop hosted journeys now explicitly verify outside-dock help,
composer Escape and nested confirmation Escape. Mobile modal/global shortcut
exclusion and existing nested-confirmation/retention journeys also pass. There
are no unknown fixture routes or page errors; the intentionally injected 409 is
the only recorded diagnostic. Available request timestamps are monotonic, but 438
of 5953 request records lack completion; these are censored/unclassified terminal
states, not proven hangs or successful completion. Loading failures are not
collected. Aggregate results preserve all 124 records; four preflight names still
overlap warmups, so there are 120 distinct standalone trial names. Physical-device,
real transport and production-cause limits remain unchanged.

### Earlier qualification (before integration correction)

[Hosted qualification 45291125836](https://github.com/scottopell/phoenix-ide/actions/runs/45291125836)
compared baseline `8a685486413fc6517f61c548e3229a2864ce6703` with candidate
`e84976f940eef714801f7f3dd6aae52dfbe11f2d`. All 116 synthetic trials succeeded,
including 96 measured samples: eight per version × viewport (393×852, 1440×900)
× entry mode (cold/global, cold/canonical-link, warm SPA notification navigation).
The remainder are 12 warmups, four warm-route preflights and four history controls.
Full [CI 45291126141](https://github.com/scottopell/phoenix-ide/actions/runs/45291126141)
also passed. These receipts qualify that source revision, not subsequent commits.

Question usability includes scripted option selection and custom entry. Median
milliseconds (baseline → candidate; eight samples per cell):

| Entry mode | Mobile 393px | Desktop 1440px |
|------------|--------------|----------------|
| Cold/global | 3443 → 3450 | 3431 → 3434 |
| Cold/canonical-link | 3434 → 3440 | 3428 → 3444 |
| Warm SPA notification navigation | 1497 → 1499 | 1457 → 1456 |

These samples do not demonstrate faster loading. Captured request timestamps are
monotonic where present, with setup/navigation initiation phases and an immutable
trial clock; some requests remain incomplete at capture. Main-thread long tasks
use a separate document clock and extend beyond question usability. Transcript
visibility is observed after question interaction/Return, not as an independent
first-ready measurement. No unknown fixture endpoints or page errors occurred;
one console error is the intentionally injected answer-409/retry journey.

Both versions allowed selection/custom entry before history acquisition/reply in
all four cold/global controls. The history gate was armed, but the request began
after the usability checkpoint and gate release: this is not evidence of
responsiveness during an already in-flight stalled history request. The synthetic
comparison does not identify the user's production loading cause.

Candidate journeys cover retained answers plus mounted transcript scroll/composer
draft, native modal focus exclusion, one overflowing content scroller, confirmation
bounds/hit testing at 393×852 and 393×520, nested Escape, shortcut help, sequential
pending requests, custom/multi-select responses, injected 409/retry, confirmed
dismissal, withdrawal, reload and canonical-route query-variant Back/Forward.
Settled baseline/mobile, candidate/mobile, candidate/desktop and reduced-viewport
confirmation screenshots were inspected. Independent static source and artifact
reviews found no remaining concrete blocker.

Limits: reduced viewport is not an OS keyboard test; physical iPhone safe areas,
keyboard and focus restoration remain unverified. History coverage is not
close-first dialog Back behavior or draft persistence after route unmount/reload.
Feed loading/error presentation and desktop confirmation/help journeys are not
comprehensively browser-qualified. API interception models latency, not real API
throughput/server work; fake EventSource excludes real SSE. Four preflight artifact
names overlap warmup names; `results.json` retains all 116 records, while measured
samples have distinct artifact names. No production speed or loading-cause claim
is established.
