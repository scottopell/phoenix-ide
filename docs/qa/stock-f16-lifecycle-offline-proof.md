# Exact-f16 lifecycle: conditional offline close-drain model and coverage boundary

## Scope / identities

This packet starts from harness `023713e0054ecf1531ccece441d1bd80b765c061`,
tree `cc515d1e2473bf7db65ec60e5d0ff43695c278cc`. The app remains
`f16afb227d9336a863c0b0345960ea9a6d62b7c7`, tree
`369305dcd94862d4ee12e7af9fd463d5da14d808`.

Only repository documentation, a proposed patch **as data**, an offline
Node selftest and actual-reporter Vitest unit controls are added. Neither
production app source nor a live harness module is repaired by this packet.
The four `TELEMETRY_MODULES` pins, app/tree pins, diagnostic trust maps, workflow,
sampling, transport, readiness and admission bounds are unchanged.
No push, CI launch, natural-sampling rerun, browser, service, HTTP/GET, SQL,
installation or heavy build was performed. Roadmap/network context was not
reverified under the explicit offline restriction.

## Actual-023 non-reproduction is a coverage gap

The parent reports that the actual-023 observation had initial sampled reports
but all sixteen terminal/lifecycle trials were `sampled:false`. This child did
not independently read an actual-023 artifact; no run/artifact identity or
per-trial producer ledger was supplied here. That report is not a new locally
measured runtime result. The earlier failed run's primary cause remains unknown;
see [the retained diagnostic packet](stock-f16-primary-diagnostic.md).

The sixteen count is consistent with the wired bounds, not evidence of delivery:
for each of two route invocations, `runTelemetryChecks` admits at most two
pagehide trials and six lifecycle trials (two attempts for each of three kinds).
Explicit unsampled lifecycle trials skip background/SPA/BFCache actions and
continue the attempts. Thus an all-explicit-unsampled run does **not** exercise
the sampled lifecycle cleanup response assertion. Initial sampled cohorts do not
cover subsequent contexts. Non-reproduction cannot clear that branch.

### Source checks before choosing the reproducer

The reporter, lifecycle, hook, stock factory, App/Coordinator/Product route
owners, UUID helper, build selector and two telemetry JS modules were compared
with exact f16 using `git diff --exit-code f16afb227... -- <paths>`; there was no
diff. The opening wrapper and publisher were also read in this harness checkout;
this does not claim that those harness files are byte-identical to f16.

| Question | Exact source finding | Limit of the conclusion |
|---|---|---|
| Actual admission predicate | `ViewJourneyReporter` constructor captures `options.random ?? Math.random` and `options.sampleRate ?? 0.25`. In exact-f16 `begin` (lines 99–110), admission count increments first; `random >= sampleRate` emits `{kind:'admission', sampled:false, open_id:null}` and returns null **before** `cleanIdentity`, per-open UUID generation, measurement construction or visibility initialization. Values below `0.25` enter the sampled branch; exactly `0.25` is false. | A validated false producer admission maps to this early return, not a visibility/identity filter or feature gate. Constructor reporter-ID UUID work already happened. No actual-run random draws/seed were measured. |
| Seed / stock override | `conversationViewReporterFactory` supplies only `send`. `tests/e2e/history_opening_browser.py` selects `ui/scripts/qualify-history-opening-realserver.mjs`; that entry calls `chromium.launch({ headless: true })`, with no explicit `--js-flags` / `--random-seed`. Inspected `addInitScript` callbacks in the qualification and joint modules install diagnostic bridges/drains, not a seeded PRNG or `Math.random` override. | This is source-only launch/config evidence, not universal behavior across operating systems or unseen environments. Actual PRNG draws, context seeds and consumption remain unknown; counts do not reveal them. |
| Context / first admission | Initial, pagehide and lifecycle stages create fresh owned contexts from storage state. The hook singleton is per loaded document; lifecycle `ensure` calls `begin` once while the attempt is undefined and retains a null result. `openId` access can admit before root attachment; later request/mark calls do not resample that attempt. | Initial reports do not prime, force or certify later decisions. New reporters also create an identity; UUID generation has a `Math.random` fallback when native crypto UUIDs are absent. No exact PRNG consumption/ordering was measured. |
| Feature disable / route presence | `ProductConversationAliasRedirect` admits ordinary journeys; `CoordinatorPage` admits Global journeys; `ProductConversationPageInner` uses the inherited lifecycle. No feature-disable branch was found in this view-reporter path. The stock build plugin rejects the separate controlled reporter graph. | A missing route/hook would give missing admission evidence, not an explicit false producer event. This is source evidence, not runtime bundle/route verification. |
| Admission selection | `samplingDecision` requires **exactly one** producer admission, the expected route, and matching ownership of other events. It does not select the first admission or infer sampling from GET/POST presence. Missing, duplicate or foreign admissions fail closed. | The pagehide trial initially defaults to false before validation; false alone on a failed/early trial is not enough. Require its validated ledger/status. The lifecycle trial has no initial sampled default. |
| Publication | `history_runtime_publication.trial` retains an existing boolean `sampled` and producer events; it does not fabricate a false boolean for a missing field. | Inspect the actual retained ledgers to separate a confirmed false admission from partial/default state. That input was unavailable to this child. |

**Source cause boundary:** if all sixteen ledgers each contain exactly one
validated false admission, the source establishes actual unsampled decisions,
not a feature-disable or missing-route fallback. Source alone does not explain
why every value fell on that side of the predicate. No probability calculation,
cohort refill, speculative seed fix or production `Math.random` replacement is
used to explain the observation. If those ledgers are absent, the stronger
claim is not established. B/C speculative fixes are out of scope.

### Deterministic actual-reporter controls (not an actual-run explanation)

`ui/src/hooks/conversationViewTelemetry.test.ts` imports the actual local
`ViewJourneyReporter`, unchanged from f16, and injects **only unit-level**
`options.random` plus a mocked `send`. One finite sequence supplies `0`,
`0.25 - Number.EPSILON`, then `0.25` and fifteen `0.9` values, then `0`.
The actual reporter emits two sampled, sixteen explicit false, then one sampled
admission, with exactly one random call and one admission per `begin`. Every
false attempt is null with a null open ID. No transport is attempted. This
proves the default boundary and that prior sampled attempts do not latch later
admissions on; it neither seeds Chromium nor explains the reported sixteen.

A separate exact-boundary control guards identity-cleaning and visible getters
and spies on visibility initialization, clock and UUID generation. The false
admission returns without touching any of them, after accounting separately
for the constructor's reporter-ID UUID call. No production override is added.
The existing `lifecycle-close-drain.selftest.mjs` test named `sampling selector
cannot turn missing/ambiguous/foreign evidence into false or pick the first
admission` independently rejects missing and multiple admissions, route mismatch
and foreign evidence; the source branch map applies only after that validation.

## Concrete A: modeled-schedule RED / GREEN on the real function

`ui/scripts/lifecycle-close-drain.selftest.mjs` imports the **unchanged real**
`runLifecycleQualificationStage` for RED. Its dedicated fake browser has separate
POST and response emission. It settles readiness before admitting the finite
request, emits the first report during `owned.close()`, then emits the matching
204 on the next fake task. It models event timing only: reports/diagnostics are
manufactured UNIT data, **not the actual reporter, fetch, collector or export**.
The older `lifecycleBrowserFixture` emits POST and 204 synchronously and does not
exercise this gap. With identical synchronous timing our baseline control passes.

RED retains the actual Node assertion at
`qualify-view-telemetry.mjs:784:20` (`final report-response drain`):

1. Sampled readiness-first branch is inconclusive; before close both report and
   response inventories are empty, so the pre-close drain does no work.
2. Close emits one owned POST; close returns before its response event.
3. Immediate final comparison fails with one report / zero responses. The trial
   snapshot retains the failure and cleanup failure; the next fake task really
   emits the matching 204 **after** failure.

This **demonstrates immediate comparison failure under the modeled schedule**
using the exact-source harness function; it does **not** validate a harness bug.
The fake context does not implement Playwright's close-channel, close-event or
dispatcher-disposal barriers. Its next-task response after close has not been
shown API-reachable; the installed-source review below gives evidence against
that simple schedule on the normal in-process close path. This is **not
historical-incident causal closure** and does not identify the old incident's
actual assertion. `BrowserContext.close()` may cancel an in-flight request:
keepalive does not guarantee a 204 or an observable response after close. The
proposal only observes for a bounded interval and then fails if responses remain
missing; it cannot force delivery, preserve a canceled request, or turn a non204
into success.

GREEN applies only `stock-f16-lifecycle-close-drain.proposed.patch` to the full
source **in memory**, with one exact hunk and a checksum guard. Only the relative
diagnostic import is relocated for data-URL loading. There is no duplicated or
simplified lifecycle implementation. The proposed three added lines, after close:

- reuse the five-second response observation cap, clipped to the existing absolute
  lifecycle deadline, and wait only while observed POSTs lack responses;
- retain the existing final count, unique-open-ID, BFCache count, owner/correlation
  and capture assertions; additionally require final observed statuses all204.

All six fake readiness-first trials complete without the modeled timing failure,
**remaining inconclusive**, never claiming lifecycle/delivery/overhead acceptance.
Negative controls still reject never204, duplicate report, wrong journey owner,
wrong response request object and non204. Never204 is bounded at the existing
five-second cap; a 100ms stage deadline is not extended. No-POST and explicit
unsampled trials remain inconclusive. Both original and proposal preserve a
primary Error's identity while retaining a distinct context-close error.
All fake-clock loops are explicitly bounded; there is no wall-clock sleep.

## Offline installed-Playwright contract review

### Source provenance (not an actual-run installation attestation)

This read-only follow-up starts at commit
`f626bada79a954db37010840fd7bfc4019129671`, tree
`f2f8c5fec51b60a3fd7990148d92078a9d56be28`. From this worktree, the existing cache
package root is the relative path:

```text
../../../.phoenix/current-task-attribution/ui/node_modules/.pnpm/playwright-core@1.61.0/node_modules/playwright-core
```

Its `package.json` declares **1.61.0**. Exact-f16 and this checkout's
`ui/pnpm-lock.yaml` both resolve `playwright` and `playwright-core` to **1.61.0**;
`ui/package.json` requests `playwright: ^1.57.0`, not an installed 1.57.0 pin.
The lockfile has no diff against exact-f16. Thus the inspected cache version
matches the pinned resolution, but no actual-023 host dependency manifest or
installed-package hash was supplied. This is not proof of which bytes that run
loaded, nor of its browser build or launch environment.

SHA256 identities of the inspected/read files:

| File | SHA256 |
|---|---|
| Cache package `lib/coreBundle.js` | `6bc8db3d485266f1e0b62d18494849dce89c03479220b1784d96625ab8eb9cc0` |
| Cache package `package.json` | `eb05f2eefbe2bc56258cc44cdc117c90ed3c9086835f3cdb729df77476f29a17` |
| Checkout `ui/pnpm-lock.yaml` | `42552207bd7922e4bc35f75f18fc3091c3f8917d8d996a6206e533f4514dedd0` |

The package's `index.js` selects `require('./lib/coreBundle').inprocess.playwright`.
The client/server JavaScript is bundled, not a separate `lib/client/` tree. The
anchors below are source-section comments **inside `lib/coreBundle.js`**, relative
to `packages/playwright-core/src/`, followed by symbols; they are not claims that
those TypeScript files are installed separately. The emitter section is named
`client/eventEmitter.ts`, not `asyncEventEmitter.ts` in these bytes.

### Close, dispatch and listener ordering

| Bundled section / symbol anchor | Source finding |
|---|---|
| `client/browserContext.ts`: `BrowserContext2` / `_BrowserContext`, constructor, `close`, `_onClose` | On the normal not-already-closed path, `close` awaits request-context disposal, before-close instrumentation, HAR export, **`this._channel.close(options)` and then `this._closedPromise`**. The constructor's once-close listener resolves `_closedPromise`; `_onClose` marks the context closed and emits that event. It is not just a fire-and-forget close request. |
| `client/connection.ts`: `Connection.dispatch`; `client/browserContext.ts`: `_onResponse` | Command replies resolve/reject their stored callback. An event instead validates and immediately calls `object._channel.emit(...)`. The context's response channel listener calls `_onResponse`, which emits the context response and then the page response in the same call stack. |
| `client/eventEmitter.ts`: `EventEmitter3.emit`, `_callHandler`, `removeAllListeners`, `_waitFor` | Listener invocation uses `Reflect.apply` immediately, in listener order; `emit` does not await returned promises. Returned `Promise`s are tracked for explicit `removeAllListeners(..., {behavior:'wait'})`. `BrowserContext.close` does not call that wait. Asynchronous listener continuations can outlive invocation, but the lifecycle stage's page-response listener is synchronous: it checks `posts`, calls `request()` / `status()` and pushes directly into `trial.statuses`, with no `await`. Tracking promises does not postpone that push to a later task. |
| `server/browserContext.ts`: `BrowserContext.close`, `_didCloseInternal`; `server/chromium/crBrowser.ts`: `CRBrowserContext.doClose` | Server close awaits tracing/screencast cleanup and its custom handler or `doClose`, deletes downloads/temp dirs, and awaits `_closePromise`. The normal Chromium owned-context path awaits `Target.disposeBrowserContext`. `_didCloseInternal` fulfills the server close promise and emits context Close. These are shutdown barriers, **not a wait for every telemetry POST to receive 204**. |
| `server/dispatchers/browserContextDispatcher.ts`: constructor Close/Response listeners, `close`; `server/dispatchers/dispatcher.ts`: `Dispatcher._dispatchEvent`, `_disposeRecursively`, `DispatcherConnection.dispatch` | The close method awaits server context close. On context Close, the dispatcher sends `close` and immediately disposes itself, removing object listeners recursively. A disposed dispatcher refuses event dispatch. The command reply is sent only after the awaited command and after-call instrumentation finish. A later server response is not simply forwarded through that disposed context dispatcher. |
| `inprocess.ts`: `createInProcessPlaywright`; `client/connection.ts`: `Connection.dispatch` | After initialization, each direction schedules dispatch with `setImmediate`. Response events sent before context Close are queued before close/dispose and the close-command reply. With this ordinary ordered in-process delivery, their synchronous page listeners run before a successful `context.close()` returns. A response dispatched earlier does not acquire a separate delayed callback turn through this emitter. |

**Reachability conclusion:** this source review does not establish a real
next-task response after successful context close for this synchronous listener.
The normal in-process close/dispose ordering instead argues against the fake
schedule: a response forwarded before shutdown is processed before close returns;
a response arriving only after dispatcher disposal is not newly forwarded by
that dispatcher. This is a bounded source conclusion, not browser execution or a
universal assertion about all transports, exceptional close paths, custom close
handlers or other versions. No actual bundled Connection/EventEmitter execution
was needed or performed; no replacement model is offered as proof of reachability.

The earlier offline RED/GREEN remains **conditional only** on its modeled event
schedule. Any correction for Global or ordinary lifecycle qualification still
needs a demonstrated real API contract/reproducer permitting the missing response
to become observable after close, plus parent admission review. A missing response
could instead reflect cancellation/non-delivery; waiting after close cannot
promise recovery or a guaranteed 204. The proposal remains unapplied, is not a
validated fix, and does not close the historical cause, which remains unknown.

## Pin / policy gate: proposal not admitted

- Unchanged live module SHA256:
  `1c702c78bb06f74f3dae4c8f97d1932fc308959596f3602375bc65d7d99fb9e1`.
- Proposed source bytes before test-only import relocation:
  `aa494cdc2a6c61bd2e96609bec49f662ecc6d0f6d42d570b518f5a3678294a3d`.
- Proposed patch SHA256:
  `5a4a3818c86e025f11c27f3187ebb82b80c1f59d9626b83e9ee5584096273a45`.

Do not silently apply the patch to the pinned app or relabel the modified module
as exact f16. The existing runtime imports the module **from the pinned app**;
merely changing a harness pin would not install this proposal. Admission requires
parent/owner review and an explicitly owned harness override with its source
hash, exact diff, dependency manifest and diagnostic-site trust review (the
insertion shifts later assertion positions), or a separately admitted app
successor. None is implemented or authorized for execution by this packet. No
pin guard is relaxed. The conditional unit reproducer and unvalidated proposal
are retained as data; real API reachability and any runtime adoption remain
separate review/integration work.

## Validation actually run

Node v26.8.2; Python 3.12.13. From the repository root:

```sh
node --test ui/scripts/lifecycle-close-drain.selftest.mjs
node --test ui/scripts/history-joint-telemetry.selftest.mjs \
  ui/scripts/qualify-history-opening-realserver.selftest.mjs
python3 -B tests/e2e/history_stock_prepare.test.py
python3 -B tests/e2e/history_runtime_publication.test.py
python3 -B tests/e2e/history_controlled_artifact.test.py
git apply --check docs/qa/stock-f16-lifecycle-close-drain.proposed.patch
git diff --check
```

Results: **17 new offline controls**, **170 existing Node tests**, and
**18 preparation + 12 publication + 8 artifact-admission Python tests pass**.
All four Python module pins were independently rehashed and matched.
`git apply --check` validates applicability only; it does not apply the patch.
No full `./dev.py check`, app/receiver build, runtime rerun, production tracing,
hosted/physical-device acceptance or performance clearance was attempted.
The untouched live harness still contains the immediate comparison exercised by
the conditional model; no API-reachable scheduling defect has been established.
Green refers only to the explicitly isolated proposal under the modeled schedule.

### Actual-reporter follow-up validation

The follow-up added only the two unit controls and this documentation. With
Node v26.8.2 and cached Vitest v4.1.7, the targeted actual-source file passes
**6 tests** (4 existing + 2 new). Dependencies were reused from the original
worktree's `.phoenix/current-task-attribution/ui/node_modules`, using a temporary
`ui/node_modules` symlink removed after validation. CWD, `--root` and `--config`
all point at **this** worktree's `ui`, not the cached checkout's source:

```sh
# From this worktree's ui, with CACHE pointing at the existing dependency cache:
node "$CACHE/vitest/vitest.mjs" run --root "$PWD" \
  --config "$PWD/vitest.config.ts" src/hooks/conversationViewTelemetry.test.ts \
  --maxWorkers 1
```

The **17** offline close-drain Node controls and **18** static-preparation Python
controls were rerun and pass; patch applicability and `git diff --check` pass.
All four runtime module files were independently rehashed against the unchanged
`TELEMETRY_MODULES` literals and match. Reporter, lifecycle, factory, hook and
UUID production sources still have no diff against f16. The three-line proposal
is still unapplied. The earlier 170/12/8 results above belong to the original
packet, not new follow-up executions. No install, network, browser, build,
server, GET, SQL, CI or universal-platform runtime claim accompanies these tests.

### Documentation / installed-source clarification validation

The follow-up from `f626bada79a954db37010840fd7bfc4019129671` changes only this
Markdown packet and its diagnostic-packet cross-reference. Offline static checks
verified the three SHA256 identities above, installed/locked version 1.61.0 and
the eight bundled source-section anchors. `git diff --check` passes; the lockfile
still has no diff against exact-f16, and no tracked non-document file changed.
No unit suites were rerun for this documentation-only clarification; the counts
above are retained prior results, not new execution evidence. No Playwright
module was executed, and no browser, network, service, install, build, SQL, CI or
push was used. Roadmap context remains unverified under the offline restriction.

## Offline publication CI boundary (follow-up from ca7469)

The existing `qualify-history-realserver` job now additionally requires
`!contains(github.event.pull_request.labels.*.name, 'offline-regression-only')`.
Its existing PR-event, same-repository and three-branch allowlist conditions are
unchanged. With that exact label present in the PR event, the **entire job is
skipped**, including its builds, browser/server runtime and artifact publication;
it is **not a passed runtime qualification**. Without the label, the historical
job conditions and commands are unchanged. This protects against an unintended
measurement run, not a coverage waiver, validated fix or overhead clearance.
The conditional fake-context model and installed-Playwright contract review above
remain unchanged; no Global or ordinary runtime result is added or upgraded.

**Parent publication ordering:** set/verify the PR label **before pushing** the
publication commit. This child neither sets labels nor pushes. The existing
`pull_request` trigger has no explicit activity types, so `labeled`/`unlabeled`
events do not themselves launch this workflow. The subsequent `synchronize`
event reads its PR-label snapshot. Adding a label after a run starts does not
retroactively stop that run; this guard is not a cancellation mechanism.

Normal check-plan/check jobs are not label-gated. In the existing `check-ui` job,
a new step after Node setup and before `./dev.py check` runs, with no step-level
condition or failure suppression:

```sh
node --test ui/scripts/lifecycle-close-drain.selftest.mjs
python3 -B tests/e2e/history_stock_prepare.test.py
python3 -B tests/e2e/history_runtime_publication.test.py
python3 -B tests/e2e/history_controlled_artifact.test.py
```

That job retains its ordinary `plan_check.outputs.ui == 'true'` gate; the workflow
and UI changes in this publication select that lane. Its existing full Vitest
lane also discovers the actual-reporter file (six tests) normally. The Python
source guards above explicitly check the unchanged historical runtime predicate
plus label exclusion, unchanged default PR trigger, absence of label gating
outside the runtime job, and unconditional offline commands in `check-ui`.
They run there explicitly rather than depending on E2E scenario discovery or the
skipped runtime job. No new job, runner framework or dependency is introduced.

### Follow-up validation actually run

Offline: **17/17 lifecycle Node controls**, **170/170 existing joint/opening Node
controls**, **6/6 actual-reporter Vitest tests**, **20/20 stock-preparation tests**
(including the two new CI source guards), **12/12 publication tests**, and
**8/8 artifact-admission tests** pass. Vitest reused the existing dependency cache
with this worktree's root/config and a temporary symlink removed afterwards.
CI YAML parsed with Ruby YAML; all **36** inline run blocks passed `bash -n`
with GitHub expressions substituted; `git diff --check` passes. Full
`./dev.py check` and hosted CI were not run. No network/roadmap refresh, install,
app build, browser, server, measurement, label change or push was performed.

## Historical evidence correction: terminal delivery is not lifecycle attribution

The Python `LifecycleFailures` object tracks the **whole outer case**, not just
`runLifecycleQualificationStage`. The old publication's
`lifecycle_failure_present: true` therefore establishes a retained outer-case
primary exception, not a lifecycle-module throw. All six retained old lifecycle
attempts were explicitly unsampled and inconclusive. The modeled sampled
close/response schedule is a poor fit for that incident and is not its cause.

Artifact `12122620564` retains a different concrete fact: one sampled terminal
pagehide attempt captured an incomplete report and producer submission states
`attempted` then `failed`, both without an HTTP status. These are distinct
boundaries: capturing a POST/report is not receiving an accepted 204, and neither
alone proves collector acceptance or completed export. The evidence does not
identify a network cause, browser abort, or server rejection.

The pinned `appendTerminalInventory` iterates **all captured trial reports** and
adds `journeyExpectation` records regardless of submission success. Consequently,
a failed submission alone does not imply a missing journey, nor prove that the
`validateTelemetryResult` report/journey-count assertion rejected this run.
The lost original stack and incomplete published journey inventory prevent that
assertion-level attribution. The historical failure phase may be module checks,
result validation, or later history finalization; it remains unknown.

The new primary diagnostic catches errors inside `runJointQualification`.
Its returned `finalizeHistory` callback executes later, outside that catch. An
error there is a specific remaining capture boundary, not evidence that it threw
in the historical run. No capture expansion, production fix, module-pin change,
or natural-sampling retry is included in this offline publication.

A further source boundary prevents assuming that a primary Node assertion even
existed: `retainCleanupFailure` in the opening driver sets `result.status` to
`failed` without requiring `result.error`. A later `finalizeHistory` exception is
retained as `joint_browser_cleanup`; the Python outer-case return-code assertion
can then become its own primary error. Thus the old failed browser status and
nine completed checks are also compatible with a cleanup-only failure. The old
safe artifact omitted both Node error presence and the cleanup inventory. The
missing causal datum is **a primary Node error, or the first cleanup-only error,
with its phase**—not necessarily a primary assertion. The newer diagnostic's
fixed cleanup stages narrow that boundary when retained, but its clean
non-reproduction does not reconstruct the old event or close the incident.
