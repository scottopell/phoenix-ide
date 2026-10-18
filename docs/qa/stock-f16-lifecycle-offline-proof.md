# Exact-f16 lifecycle: offline close-drain proof and coverage boundary

## Scope / identities

This packet starts from harness `023713e0054ecf1531ccece441d1bd80b765c061`,
tree `cc515d1e2473bf7db65ec60e5d0ff43695c278cc`. The app remains
`f16afb227d9336a863c0b0345960ea9a6d62b7c7`, tree
`369305dcd94862d4ee12e7af9fd463d5da14d808`.

Only repository documentation, a proposed patch **as data**, and an offline
Node selftest are added. Neither the app nor a live harness module is repaired
by this packet. The four `TELEMETRY_MODULES` pins, app/tree pins, diagnostic trust
maps, workflow, sampling, transport, readiness and admission bounds are unchanged.
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
| Actual admission predicate | `ViewJourneyReporter.begin` increments admission and tests its injected/default random value against the default `0.25`. Its explicit false diagnostic is emitted only by the `random >= sampleRate` branch. | A real false producer admission establishes the branch, not why the random value had that value. No random draws/seed were measured. |
| Seed / stock override | `conversationViewReporterFactory` supplies only `send`; reporter captures `Math.random`, not a seeded generator or runtime sampling control. The opening driver launches Chromium with `{headless:true}`; inspected qualification init scripts install diagnostic bridges, not PRNG overrides. | No seed or per-context PRNG state can be inferred from counts. This does not attest an unseen runtime's environment/bundle. |
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

## Concrete A: deterministic RED / GREEN on the real function

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

This proves an exact-source harness scheduling defect; it does **not** identify
that site as the historical incident's actual assertion or prove that Playwright
will deliver a response event after a real context closes.

GREEN applies only `stock-f16-lifecycle-close-drain.proposed.patch` to the full
source **in memory**, with one exact hunk and a checksum guard. Only the relative
diagnostic import is relocated for data-URL loading. There is no duplicated or
simplified lifecycle implementation. The proposed three added lines, after close:

- reuse the five-second response observation cap, clipped to the existing absolute
  lifecycle deadline, and wait only while observed POSTs lack responses;
- retain the existing final count, unique-open-ID, BFCache count, owner/correlation
  and capture assertions; additionally require final observed statuses all204.

All six fake readiness-first trials complete without the timing false failure,
**remaining inconclusive**, never claiming lifecycle/delivery/overhead acceptance.
Negative controls still reject never204, duplicate report, wrong journey owner,
wrong response request object and non204. Never204 is bounded at the existing
five-second cap; a 100ms stage deadline is not extended. No-POST and explicit
unsampled trials remain inconclusive. Both original and proposal preserve a
primary Error's identity while retaining a distinct context-close error.
All fake-clock loops are explicitly bounded; there is no wall-clock sleep.

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
pin guard is relaxed. The bounded reproducer plus minimal diff is the deliverable;
a deployed fix and candidate requalification remain separate integration work.

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
No full `./dev.py check`, Vitest, app/receiver build, runtime rerun, production
tracing, hosted/physical-device acceptance or performance clearance was attempted.
The untouched live harness still contains the reproduced defect; green refers
only to the explicitly isolated proposal.
