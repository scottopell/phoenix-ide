# Restore mobile web liveness across published continuations

## Ownership and reported journey

New focused owner for the user-reported MOBILE WEB defect: active Global Coordinator (and possibly ordinary ProductConversation) remains on the predecessor after automatic continuation, showing **Open continuation**; clicking it effectively refreshes the view. Deliver a minimal repair, bounded lifecycle audit, production-path regressions, realistic mobile browser evidence, independent adversarial review, and an exact-final-head hosted-CI integration packet. Do not release, deploy, merge, or restart shared servers.

## Grounded initial findings

Read-only starting checkout: `06e2c74ae019b613ebee54182a146093106e1b5d` (detached; also locally cached origin/main). Local `main` is stale at `e74290876a587a8c1b795f91d623825f92d0cbde`; do not treat it as remote current main. RC5 tag resolves to `fae542129a6e2d4eacd1b4e37d5ddabfe58d4529`. Confirm deployed source identity separately rather than assuming tag equals deployed binary. GitHub/roadmap access was blocked by the Explore sandbox; remote freshness is unverified.

Verified source journey:
- `ProductConversationPageInner` owns aggregate history/latest-member snapshot. `currentLatestProjection.conversation.continued_in_conv_id` triggers `setSnapshotRetry`; successful readback adopts `latest_transcript_row_id` and merges acquired predecessor messages.
- `EmbeddedConversationPage` uses the latest member slug with `suppressCanonicalization=true`; `ConversationPage` publishes a projection and owns `useConnection`. Store subscriptions are slug-keyed. This is an existing stable-route following mechanism, not a blank-slate routing feature.
- The continuation observer is edge-triggered on member/continuation/latest IDs. Failed background snapshot reads retain the old snapshot, while unchanged continuation IDs do not re-trigger that observer. A stale successful snapshot has a similar potential gap. **This is a candidate reproduction, not the established incident root cause.** Test it against metadata delivery, publication timing and reconnect recovery before selecting a repair.
- Several aggregate tests mock the embedded conversation or emit `phoenix:automatic-continuation-updated`; they do not establish full runtime/SSE/store/projection/subscription behavior.
- `commit_continuation_tx`, admitted publication, `persisted_opening_message`, SSE `prepare_sse_delivery`/`enrich_update_opening`, and snapshot opening fences already exist in RC5. Do not credit those as new fixes or assume missing SSE is the cause.
- `specs/sse_wire/executive.md` explicitly records unimplemented aggregate lifecycle/boundary/root-ledger targets. Record this architectural gap without implementing a lifecycle redesign for this assignment.

## Interaction map to reproduce and diagnose

Authoritative continuation admission/settlement and successor opening persistence → admitted runtime publication and predecessor metadata → row SSE live delivery/replay/init → `useConnection` and conversation store reducer → embedded projection → aggregate snapshot invalidation/readback → authoritative latest member → subscription rebind and visible composer/status/history controls.

Trace failure/retry, cancellation/Stop, Close/history, reconnect, mobile foreground and network loss through the same map. Distinguish admission, publishing, accepted/ready/current, failed, explicitly paused/stopped, and natural completion. Trace the exact **Open continuation** callback and route identity rather than inferring a document reload from appearance.

## Implementation sequence

1. Verify current task/PR ownership, remote current main, RC5 and deployed identity using permitted read-only source/deployment records. Create isolated owned branch/PR; retain exact source identities. Read relevant ProductConversation, Coordinator authority, continuation, SSE, conversation atom/connection and Close specs before edits. Respect existing owners of PR #1076 and native task approval/unified Close #1058; coordinate only on actual shared-file/dependency conflicts.
2. Build isolated synthetic reproduction for active stable Coordinator and ordinary product routes. Use real production client composition and transport handling, plus isolated runtime/server fixtures where available; never rotate/exhaust/stop/Close live user conversations or directly mutate the database. Capture the first failing boundary across persistence → publication → transport → subscription → projection → route/control state. Include first readback failure/stale result as falsifiable candidates.
3. Add trustworthy failing-before/passing-after tests before the minimal repair. Prefer existing event/readback/reconnect mechanisms; no polling, reload bandaid, blind redirects, or broad aggregate-ledger redesign. Fix proven same-path lifecycle defects; record unrelated gaps separately. Retain an honest actionable fallback if auto-follow cannot complete.
4. Qualify route-stable successor streaming and disconnect/background recovery. Ensure old subscriptions clean up and exactly one successor subscription owns writable controls; no duplicate messages, lost events or stale predecessor destination. Preserve intentional exact-transcript/history navigation, reading position, draft/attachments, focus and mobile keyboard applicability. Do not infer permission to resume a stopped/paused conversation from discovering a successor.
5. Use realistic mobile-width browser fixtures and inspect screenshots plus interactive assertions. Clearly label synthetic browser results; physical iOS/PWA keyboard and OS Accessibility acceptance are deferred, not passed.
6. Obtain independent adversarial review of the exact candidate diff, resolve findings, run final hosted checks on the exact final SHA, and return a merge-ready packet with PR, source identities, reproduction/root cause, minimal repair, audit matrix, review disposition, CI/artifact links and evidence limits. Any final source change invalidates earlier exact-head qualification.

## Bounded audit matrix deliverable

For **each** Coordinator and ordinary mobile web surface, record expected visible state/controls, authoritative persisted source/event, delivery/subscription/readback path, test/evidence and any precise gap:

| Lifecycle edge | Required distinction / evidence |
|---|---|
| Automatic/manual continuation; admission/publishing/ready | Stable active route follows only published authority; successor streams and accepts controls without redundant Open continuation. |
| Continuation failure/retry | No false readiness; honest retry/fallback; failed/stale snapshot recovery exercised. |
| Waiting/running/idle/error | Status and writable destination follow authoritative member state. |
| Question/task approval wait and resolution | Wait remains actionable and resolution updates; no gate bypass. |
| Stop/cancel versus natural completion | Explicit stop/pause is not auto-resumed; no false completion/recovery controls. |
| Close/history and intentional transcript browsing | History/Close gates preserved; exact transcript not blindly redirected. |
| SSE disconnect/reconnect | Init/replay recovers missed events and successor ownership with one subscription. |
| Duplicate/out-of-order delivery | Existing sequence/incarnation and authority guards prevent duplicates/stale control regression. |
| Mobile background/foreground and network loss | Existing resume/readback mechanisms recover live state; deterministic synthetic evidence and device limits stated. |

## Resource and safety contract

Existing local native Cargo/rustc/full-build hold remains. Read-only `df` showed roughly 14 GiB available during exploration. No new local dependency installation, browser environment or test environment without fresh explicit resource admission. Use existing hosted mechanisms; `.github/workflows/ci.yml` already has UI/spec checks, hosted browser/e2e paths and Ladle screenshot artifact infrastructure. Minimal scoped fixture/workflow wiring and routine command repairs/reruns are in scope; no new spending, dependencies or trust changes. If required hosted capability is unavailable, report its exact target and missing capability rather than waiving regression/visual acceptance. No production captures or sensitive transcript exports. No deployment/shared-server restart, direct DB mutation, approval/Stop/pause/Close bypass. Progress reports do not pause execution; do not repeatedly message the Coordinator chain.

## Acceptance

- Reproduced production-path failure with exact baseline identity and failing-before evidence, not merely a mocked notification test.
- Active stable Coordinator and ordinary ProductConversation follow successfully published continuations and resume live messages/controls without reload or redundant action.
- Intentional history, user input/reading state, cancellation and lifecycle authority remain intact; actionable fallback retained.
- Completed bounded lifecycle audit with precise test-backed results or explicit gaps.
- Mobile-width interactive/screenshot evidence inspected; no physical-device claims.
- Independent review resolved and final hosted CI names the exact final commit; integration packet is ready for Global Coordinator review, with no release/deployment performed.
- Source/production identity, GitHub freshness and any restricted validation gaps reported honestly.

## Explicit non-goals

Native task approval/Close executor ownership, PR1076 author/reviewer work, aggregate SSE/root-ledger redesign, unrelated lifecycle architecture, production reproduction by manipulating real conversations, physical-device/Accessibility permissions, local resource-heavy qualification without admission, and deployment/release.

## Qualification checkpoint

Draft PR: https://github.com/scottopell/phoenix-ide/pull/1080. Implementation includes committed-link publication/recovery, stable aggregate follow/composer ownership, retired draft-writer fencing, explicit transcript upload targets, persisted-prefix attachment authorization, and payload-free saved-opening retry through shared lifecycle/rearm orchestration with generated reserved-capacity review. Main dependency migration was integrated at `6316b31a06e8768c25e25565e0c8c844b2f521c7`.

**Not merge-ready.** No merge/deploy/restart performed. The detailed executed-versus-authored ledger is `docs/qa/mobile-web-continuation-liveness.md`; earlier green lanes do not qualify later corrections. Independent reviews found and corrected the undefined close gate, missing automatic retry rearm path, reserved-capacity bypass, cold-hydration upload scope fallback, route syntax integration error, and newly authored test API/type errors. Latest narrow upload review's excess-property finding is corrected at `a8011ab8752c0d7ff83f1840a714fea7cc521c6c`; its production prop migration had no additional source finding.

Remaining gates:
- Exact final-source hosted CI, including publication mutation witness and the native automatic reserved-recovery regression.
- Final ordinary/Coordinator mobile captures inspected (before/followed/reconnected), with same native editor/focus/selection and new post-reconnect message assertions.
- Final source-review packet with explicit coverage and no unresolved actionable findings.
- Exact terminal/unmaterialized failed-automatic service-level retry is now authored using real supported DB/workflow/send/retry APIs, with unchanged durable identity and separate cancellation/rearm generation assertions; hosted execution remains required.
- Physical iOS/PWA keyboard/OS Accessibility coverage remains deferred. Historical RC5/deployed identity reports are not fresh independent production verification.

The local build/test/browser/server/install resource hold remains respected. Hosted normal PR CI is the qualification path; no duplicate manual dispatch.

### Generated-metadata and recovery follow-up

Run 43643901904 at `3e4c64ace628d1553d4c76ee2fa65f948b8f944e` passed UI Vitest/type/spec, visual, E2E, clippy, musl and Rust build including the expected-failure/restored-pass publication mutation witness. Six ordinary/Global captures were inspected in the predecessor session; exact identities and limits are recorded in the QA ledger. Overall CI failed: our automatic reserved-recovery test incorrectly expected a capacity error instead of accepted dispatch (corrected), and an unchanged Federation multi-case fixture hit a unique peer-address constraint (unrelated blocking TODO captured in the QA ledger).

A final backend review found saved generated-opening retry incorrectly rejects persisted user-agent metadata. The guard is corrected without altering generated authority, saved intent, successor identity, or Idle admission. A real configured-provider regression covers retry and one generated opening; the new terminal/unmaterialized failed-automatic regression covers the shared rearm branch. Both require hosted execution at the corrective head. Source-only rustfmt (edition 2021) and diff checks pass; no local builds/tests/browser/server/install and no merge/deploy.
