# PREPARE Stage 2: actual reporter/lifecycle pagehide functional probe

Prepared from `404651fbc` (Stage 1 observations retained) and its cleanup-only
predecessor `0b4bc78ae`. **No Stage 2 hosted result is claimed.** Parent review is
required before one hosted execution, after the current CI invocation terminates.
This worker neither sets labels nor pushes/runs CI. Roadmap/network context was
not queried under the explicit no-network preparation boundary.

## Source and runtime scope

The actual lifecycle is `ViewJourneyLifecycle` in
`ui/src/hooks/conversationViewLifecycle.ts`. It accepts the actual
`ViewJourneyReporter` from `ui/src/hooks/conversationViewTelemetry.ts`.
The tiny test entry **imports both**; it does not reimplement either class.
All five runtime source dependencies are hashed against unchanged f16 bytes;
all four telemetry module hashes remain fixed. The f16 app pin is
`f16afb227d9336a863c0b0345960ea9a6d62b7c7`.

A hosted-only isolated **Vite library build** uses the existing pinned UI
Vite/esbuild dependencies, not the full app build. The single IIFE's exact module
inventory must be the entry plus the five pinned runtime sources. No copied
production modules, mocked Playwright objects, reporter factory substitution,
fetch override, global `Math.random` override, dependency change, app/Cargo build,
production server or history database is involved.

The synthetic page creates one real reporter and one real lifecycle. Its existing
`random` constructor option chooses 0.1 for three sampled controls and 0.9 for the
unsampled control; the default sample rate remains 0.25. Its existing `send`
option transparently delegates `(input, init) => fetch(input, init)`, matching the
production factory's native-fetch invocation semantics without substituting or
overriding that factory. The reporter's full request/options/signal pass unchanged
to real browser fetch; no fake transport is installed.
It calls the real `requestStarted` **before a real browser fetch** carrying a fresh
request UUID on the finite API-shaped path inspected by the unchanged qualifier.
The owned server holds that GET unfinished. The sampled page never presents or
hydrates a transcript, so pagehide remains incomplete before presentation.
The unsampled page has a minimal visible transcript/breadcrumb fallback for the
qualifier's existing readiness race, but does not fabricate a ready report.

The entry's small pagehide adapter calls real lifecycle `visibility(false)` and
`finish('incomplete')`. It is **not** the React `useConversationViewJourney` hook,
a router/authenticated Phoenix page, a production conversation identity, or a
physical-history witness. No fake Phoenix login, API snapshot, span, collector
export, ready report, acceptance result or history physical receipt is created.
The actual reporter serializes and sends its real fetch with `keepalive: true`
and its real three-second `AbortController` behavior. This does not establish
that its abort timer runs after the old document is destroyed.

Each hosted control passes the **actual Chromium Browser** to the unchanged
`runPendingPagehideCheck`, with **attempts: 1** and a 20-second absolute case
budget. Its existing diagnostic binding, admission/request attribution,
cross-document navigation, bounded observation and context drain are retained.
The 120-second process envelope is a kill fence, not an expanded qualifier
cohort/deadline or benchmark. There are exactly four controls, each owning a
separate browser and loopback synthetic collector:

| Control | Collector schedule |
|---|---|
| `normal204` | Receive full actual telemetry POST body, then attempt 204 immediately |
| `deferred204` | Await **both** that actual POST body and the entry's explicit real pagehide-trigger POST, then attempt 204; no blind sleep |
| `never204` | Receive body but never end the telemetry response; socket teardown is cleanup only |
| `unsampled` | Existing reporter admission chooses unsampled; no telemetry report/POST allowed |

The synthetic collector is an **owned delivery oracle**, not Phoenix's
schema/auth/export acceptance. It receives only this synthetic bundle's traffic.
Body bytes are capped at 8192; parsed reports are compared with the qualifier's
actual captured report and never published. It does not read or write SQL.

## Independent observations, not invented qualification

The safe JSON separates:

- qualifier-captured report versus complete collector body receipt;
- exact parsed body equality versus merely receiving some bytes;
- server 204 **attempt** versus server `finish` (neither proves client receipt);
- page 204 versus context 204 joined by the qualifier to the original Request
  object (context telemetry ordinal zero); `client204` is their union;
- `deliveryConfirmed`: matched body + attempted/finished server 204 + observed
  page/context client 204. This is synthetic transport delivery, **not** Phoenix
  collector/export acceptance;
- the unchanged qualifier's returned stage status **and** its separate trial
  status. A single unsampled attempt returns stage
  `inconclusive_sampling_budget`, with trial `unsampled`; it is not a sampled
  qualifier pass. Sampled terminal cases can return
  `provisional_incomplete_collector_export_pending`.

**`never204` may legitimately return qualifier-provisional with zero page
statuses.** That is not a qualifier negative. Its independent delivery flag must
remain false. Normal/deferred functional controls require matched body and
attempted/finished server 204, but missing browser receipt stays explicitly
`deliveryConfirmed: false`; a green functional job does not assert every delivery
succeeded. Unsampled requires no report/body/response. All controls require the
qualifier's context closure and no primary/cleanup error. No stock module is
changed to tighten its zero-status rule.

The surfaced qualifier primary is projected using the existing independently
hashed trusted assertion-site map. Unknown Playwright/nonassertion errors stay
unknown; raw messages/stacks/URLs are not published. Stage 2-owned errors use only
two fixed causes (`collector_contract`, `observation_timeout`) and a fixed module
name/current source hash, **not** an invented assertion stack location. Primary
projection happens before outer browser/collector cleanup; cleanup diagnostics
are separate. If the immutable qualifier promotes a cleanup-only error, the
wrapper does not manufacture a primary. Matching frozen trial primary/cleanup
strings cannot prove which original Error surfaced: the diagnostic is retained
as `ambiguousQualifierFailure`, primary remains null, and independently known
cleanup failure is recorded separately with unknown cause. Identical-message
primary/cleanup collisions therefore cannot erase the surfaced diagnostic or
claim a known cleanup-only role. Discarded internal Error objects cannot be
reconstructed. Setup failures before
the first browser case can exit without a receipt and are not functional passes.

`safeJSON` is the executable allowlist: exact keys, fixed enums/booleans, bounded
counts and approved version/hash fields only. Receipt provenance binds Node,
Playwright 1.61.0, Chromium revision/version, PR head SHA, lock SHA256, f16 source
hashes, all four fixed module hashes and the actual built bundle SHA256. The
upload allowlist contains only `reporter-pagehide-evidence/result.json` and this
document. No raw report/body, request identity, filesystem path, auth, stack,
trace, screenshot, HAR, database, server log or bundle is uploaded.

## Joint contract coverage is UNIT ONLY

The existing `history-joint-telemetry.selftest.mjs` uses its already labelled
`UNIT_ONLY` fixture and import seam to call the **actual**
`validateTelemetryResult` and the **actual returned** `finalizeHistory` closure:

- terminal-only incomplete inventory cannot pass the required ready-report gate;
- wrong-owner/duplicate inventories are rejected, and failed finalization also
  consumes the one-shot closure;
- valid existing unit inventory finalizes once; reentry is rejected;
- primary-plus-cleanup (existing controls) and cleanup-only failure preserve
  distinct diagnostic roles.

Those tests are not hosted Phoenix joint integration. Stage 2 never invokes
`runTelemetryChecks`/`runJointTelemetryStage`, never manufactures their required
ready/export/history receipts, and never claims collector/export/drain or
physical-target integration. Duplicate/wrong-owner negatives are UNIT ONLY, not
extra hosted cases. Full React lifecycle integration, Phoenix authentication,
real collector schema/export acceptance, joint-history finalization with a real
physical target, PWA/BFCache/platform behavior and overhead remain untested here.

## Independent opt-ins and parent handoff

Both jobs require the exact owned same-repository PR branch
`prepare/exact-f16-stock-admission` and `offline-regression-only`, plus:

- Stage 1: `context-close-probe`;
- Stage 2: `reporter-pagehide-probe`.

Neither opt-in label was present in the supplied parent context; this preparation
does not add either. Stage 2 has no dependency on/rerun of Stage 1. The stock job's
negative `offline-regression-only` predicate is byte-for-byte unchanged, so it
remains **SKIPPED**, not passed. The existing frozen pnpm lock and Playwright
1.61.0 Chromium installation pattern is reused only in the hosted opt-in job.
Default PR activity types do not start a run on label addition; the parent must
review the exact head, wait for current CI to terminate, verify the event label
snapshot, then publish for the authorized single execution. No rerun, natural
sampling, 48-trial matrix, benchmark tuning or code-policy change is authorized.

## Compiler-free preparation validation

Allowed offline checks: Node unit tests and `node --check` syntax, Python static
regressions, YAML parsing, inline `bash -n`, source/hash inventory and
`git diff --check`. Full `dev.py check`, Vite build, Playwright, local listeners,
network/API calls, installs and SQL are deliberately **not run** locally.
The hosted bundle configuration and real browser/HTTP behavior remain pending
parent review and the single authorized hosted execution.

### Validation actually run in preparation

On Node `v26.8.2`, **163/163** compiler-free Node controls pass: 23 Stage 2,
82 joint UNIT ONLY, 41 Stage 1, and 17 lifecycle-close-drain controls. The five
added joint controls exercise the existing fixtures/actual validator and returned
finalization closure; they do not add hosted integration credit.

Python static regressions: stock preparation **20/20**, safe publication
**12/12**, artifact admission **8/8** pass. All three new `.mjs` files plus the
entry/config `.ts` files pass syntax-only `node --check`. Ruby parses CI YAML;
all **42** inline `run` blocks pass `bash -n` with GitHub expressions replaced by
a syntax placeholder. `git diff --check` passes. The entire stock runtime job is
unchanged from `404651fbc`, SHA256
`c6d1b37992d3c05b0ef2b058ec15e7a37d0b8e958a7bdb56628350d4f2fec210`.
Production source, package/lock files and the retained Stage 1 README/result/
provenance are unchanged; all four immutable module hashes and five imported
runtime source hashes match their f16 bytes.

An additional attempt to run the existing Vitest-based
`qualify-view-telemetry.test.mjs` with Node could not load the absent `vitest`
package. That suite is **not counted as passing**; no dependencies were installed
to enable it. The allowed compiler-free suites were then rerun independently and
passed. The first Ruby shell-check helper required adjustment for this host's old
Ruby (`filter_map` unavailable); the corrected helper passed all 42 blocks.
No local build/browser/listener/network/install/runtime/SQL or hosted execution
was performed. These are preparation checks, not a Stage 2 runtime result.
