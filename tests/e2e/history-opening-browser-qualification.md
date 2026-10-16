# Instrumented opening browser qualification

This harness is **instrumented runtime evidence, not shipping-binary proof**.
`history_browser.py` and `qualify-history-realserver.mjs` share the corrected
wire identity helpers with this witness; the app source remains pinned unchanged. No new HTTP control endpoint exists.

## Inputs and hosted invocation

Supply an independent, tracked-clean app checkout at
`d4c2772a95aa46da608b356c8fe7be074de1b7aa`, its already-built `ui/dist`, installed
app-owned Vite/Playwright dependencies and Chromium, and the independently built
ignored libtest artifact. This script never compiles, installs, starts production,
or uses the shipping executable. Use the existing hosted preparation; workflow
integration belongs to the parent owner.

The existing `opening-instrumented-receipt.json` must additionally include:

- `ui_files`: every relative `ui/dist` file and its SHA-256, excluding `.gitkeep`;
- `ui_lock_sha256`: SHA-256 of `ui/pnpm-lock.yaml`.

The receipt's `harness_source` must equal the executing harness HEAD; app SHA/tree,
executable hash, Cargo/UI locks, build rustc attestation, selector, successful
focused-test exit, evidence class and ignored-driver name are checked before any
runtime starts. Source/toolchain attribution is the independent builder's
attestation; hashes alone cannot prove source provenance. Artifact identities are
checked again after the run.

```sh
uv run tests/e2e/history_opening_browser.py \
  --app-root /absolute/pinned-app \
  --instrumented-exe /absolute/opening-instrumented-libtest \
  --instrumented-receipt /absolute/opening-instrumented-receipt.json \
  --output /absolute/new/opening-browser-evidence --timeout 1100
```

Ordinary and Global each get a separate exact ignored-test process and one arm,
with a short owner-only system-temp IPC root, fresh driver-owned HOME/database,
mock models and loopback listener. The compiled startup JSON must match the
artifact/source and owned paths; readiness is an actual HTTP GET. Fixtures reuse
the shipping harness's exact `Fixture.build`, HTTP/SSE helpers, hashing and owned
process-group cleanup by import, not copied implementations.

After `Fixture.build`, the instrumented harness exhausts its final member and
GETs the persisted `ConvState` (`type: context_exhausted`, `summary: string`).
It POSTs that exact summary as the manual handoff, without performance tokens or
fabricated replacement text, and waits for actual SSE settlement and persistence
of the accepted opening identity. It exhausts the new successor for the later
one-shot armed continuation. Fresh member GETs and aggregate pages then replace
the fixture's members, latest row, boundary count, accepted manual-opening metadata
and occurrence coverage; the >201 occurrences and older anchor must survive.
The Node assertion still compares actual persisted boundary and opening content
before arming. This setup repair has compiler-free tests only, not a runtime pass.

## What an executed pass asserts

- Existing equal-text manual continuation has a completed separate-message
  opening and remains independently rendered; text does not authorize suppression.
- Exact predecessor/key arm precedes a real asynchronous continue POST. Entered
  IPC certifies committed intent before first-message persistence, not mere POST
  acknowledgment.
- While held: aggregate latest/opening, direct row GET, independent new SSE Init,
  browser aggregate response, deferred reserved identity, read-only composer and
  absence of browser chat POST are observed.
- A harness-only owner-file switch makes the Vite preview destroy only this
  aggregate's GET transport. The browser witnesses actual request failure and a
  correlated owned requestfailed outcome; no page-alert contract is inferred for
  background aggregate probes. No synthetic JSON, public headers, page routing or
  offline mode are used.
- One exact release precedes actual accepted message/event-cursor observation,
  enabled composer, exact successor chat request/response and persistence while
  roster refresh still fails. Historical source pin and physical anchor remain.
- Another owner-file token destroys only already-open exact successor SSE
  responses. The same page's new stream request and accepted Init are captured,
  with no navigation/offline substitute; transport receipts require a real drop.
  Accepted identity, composer, source pin and physical anchor survive reconnect.
  An explicit chapter click then renders exactly one persisted manual opening.

The pinned app has **no `/history-roster` route**: its actual roster refresh is
`GET /api/product-conversations/{id}` (or the exact Global history GET). The fault
path is taken from an actual browser aggregate response and constrained to the
owned aggregate, not a guessed nonexistent endpoint.

The pinned `ContinueConversationRequest` has **no `opening_authority` field**.
Normal manual POST selects `user_authorized_instruction`; generated context is
not selectable through this contract. Generated-authority browser acceptance is
explicitly **uncovered**, not fabricated or passed. Delayed response-race,
content-edit/replay-generation cases, shipping builds, real providers, production,
non-Chromium engines and physical devices are also not qualified by this harness.

## Compiler-free validation

```sh
python3 -B tests/e2e/history_opening_browser.test.py
node --test ui/scripts/qualify-history-opening-realserver.selftest.mjs
node --check ui/scripts/qualify-history-opening-realserver.mjs
```

These tests mock processes/proxy callbacks only inside selftests. They exercise
identity rejection, narrow transport targets, release-once failure cleanup and
owned stream tracking without starting any server/browser or invoking a compiler.
Syntax/selftest success is not runtime success. Runtime `result.json` starts
failed, includes per-case receipts, and records verified actual startup separately
from browser completion. Child failure logs and cleanup receipts remain retained.

## Recorder scope and retained failed attempt

Retained run 38506297019, artifact 11868396037, harness cf1940955929bb6460720d885789761c5fa4e1da
completed all eight ordinary behavioral assertions, then failed the final capture
assertion with five `Network.getResponseBody` no-data errors. Global did **not**
execute. The old async recorder saved response metadata only after body retrieval;
these five failures therefore have no saved URLs/methods/statuses. Their endpoint
and causal classification are unknown. They are not retroactively classified as
telemetry, navigation cancellation, application defects or harmless errors, and
this recorder repair does not turn that failed artifact into a qualification.
The application remains pinned d4c2772; a new executed artifact is required.

The instrumented recorder uses a discriminated capture policy:

- **Required aggregate JSON**: GET of the selected aggregate endpoint, or its
  exact known member/source aliases, including Global history. The committed
  successor identity registers an additional owned alias before navigation.
  Status 200, JSON content type, segments/boundaries arrays, exact canonical root
  and ProductConversation identity are required. Raw UTF-8 receipt (up to 2 MiB),
  byte count and parsed identity are retained. Invalid JSON/wrong identity/body
  unavailability/overlimit/disposal or the 11-second body deadline fail qualification.
- **Metadata-only API consumer**: all other API responses, including stream
  response headers and telemetry acknowledgments. No recorder response-body
  read is attempted. The actual successor chat ACK is separately read/retained
  by the behavioral driver in `assertions.jsonl` (URL, status, raw/parsed body,
  exact canonical message ID); a fresh randomized turn requires queued true,
  steering/already_persisted false when present (the pinned DTO omits false fields). Browser request payloads, including telemetry POSTs, remain in
  `browser-requests.jsonl`. Telemetry ack bodies are not owned history evidence;
  this harness imports no telemetry readiness or telemetry qualification claim.

Every API response saves immutable URL/method/status/content-type, request and
response times, request ordinal and navigation epochs **before** any body read.
Ordinals come from a WeakMap of actual Playwright Request objects and are labelled
local ordinals, never represented as CDP request IDs. Failures repeat their owning
metadata in `browser-capture-errors.jsonl` and response outcomes. Absent response
status/content type/time are explicitly null on missing-response/requestfailed
outcomes. Application `pageerror` events have a separate file and result array.

End-of-case seal freezes new request identities once. Response/requestfailed
listeners remain active only for already-known Request identities until all
required awaiting-response states and body tasks terminate. A shared absolute
11-second drain deadline caps all outstanding work; a body read is also bounded
by its own 11-second deadline, never reset by later arrivals. Unknown post-seal
responses/failures are typed `outside_capture` receipts, not new required work.
Page close fails both awaiting-response and body work immediately as `disposed`. `browser-capture-drain.jsonl`
records terminal outcomes and `partial:false`, even when qualification fails;
that field means receipt drain is complete, **not** that assertions passed.
Missing required responses fail at the shared deadline. A hard wall exit saves
`partial:true` and pending metadata/body-task inventory, then exits nonzero; it
never reports a complete drain. A real requestfailed transport outcome is not a
body-capture error; only an exact aggregate GET while the owned roster fault is
active is an expected exception, and the original failed-endpoint behavioral
assertion remains mandatory. A status200 body failure is never forgiven by that
exception. Both application errors and capture errors must be empty; cleanup
failure still forces failed status/exit. Controllable response mocks exist only
inside compiler-free selftests and invoke the actual installed producer.

## PRE-DISPATCH audit of parent 323 (offline correction; no runtime rerun)

**Disposition:** source/recorder correction and offline replay only. No hosted
push/dispatch, app build, install, production operation, browser or server was
performed. Independent parent review is the next gate. Roadmap was not refreshed
because network access was expressly excluded; retained domain/artifact context
was used. The original failed artifacts were not edited or relabeled.

### Audit matrix

| Surface/expectation | Finding and correction | Verification / remaining gate |
|---|---|---|
| Ordinary snapshot raw boundary shape | Default `ProductConversationSnapshotView` uses `segments[].handoff` (`null`, `historical`, `completed`), **not** top-level `boundaries`. Validate each handoff discriminant, adjacent full row IDs, continuation ID, summary type, completed authority and accepted successor ID. | Actual installed response handler captures all 25 retained ordinary full bodies; six handoff-field mutations fail closed. |
| Ordinary original-message history | `boundary_representation=original_messages` returns `ProductConversationHistorySnapshotView`: history segments without `handoff`, plus full boundary metadata. This is a distinct genuine wire projection, not a fallback to tolerate broken snapshots. | Both projections are exercised in the 25 bodies. Driver still requires the exact original boundary and null represented opening ID before arm. |
| Global history/root | Actual `CoordinatorConversationSnapshotView` **does** carry `canonical_root`, full `boundaries`, and history segments without handoff. Stable product ID differs from root/member IDs. | 13 actual browser Global bodies (first at line 272) + full driver body at line 22 = 14 captures. Root/product/member/ordinal mutations rejected. No fabricated instrumented Global fixture. |
| Fixture full-wire identity | Python original-message pages now check stable product, canonical root, ordered full roster and ordinals, requested root, latest row and exact endpoint identity; boundary metadata equals persisted predecessor continuations. | Compiler-free Python identity/member-edge regressions; original occurrence-count, ordering, cursor progress, no omission and older-anchor checks remain. |
| Pending / accepted union | Recorder validates full edge, exact committed key or accepted `successor:key`, manual authority and separate presentation against fixture or IPC evidence, not text or partial IDs. | Ordinary IPC/HTTP evidence supplies real successor registration. Global Pending is **synthetic mutation only**, explicitly not held-gate evidence. Wrong opening edge, owner, ID, authority and presentation fail. |
| Latest versus selected | Last roster row is latest; selected row must belong to the roster and match alias request identity. Stable ordinary product route selects root. Known historical responses may precede newly registered successors. | Authentic Global responses include selected middle/root/latest aliases and later accepted successor; selected/latest mutation tests reject. |
| Own Init / cursor | Acceptance cursor requires its own earlier CDP request Init, full edge, successor conversation identity, exact accepted message identity, valid incarnation and advancing JSON sequence. Transport `eventId` is retained, never treated as sequence. | Ordinary: 3 Init frames, 1 real acceptance-transition cursor, 2 later message cursors. Global: 8 accepted Init frames, 6 later message cursors, **0 acceptance-transition frames retained**. Foreign request and bad sequence mutations reject. |
| Failure probe/status/body policy | Required owned GET remains HTTP 200 + JSON + full body + parse + contract; metadata-only APIs never retrieve bodies. Expected transport failure is fixed at request time and scoped to exact active fault target. Failure witness excludes pre-release failures. | Existing deadline, disposed, over-limit, malformed, missing, status/type, concurrency, pageerror and expected/unexpected transport tests remain. No blanket ignore. Actual post-correction runtime failure scenario not rerun. |
| Drop/reconnect | New Init must have a different CDP request from acceptance and a CDP request URL matching the exact successor stream; proxy receipt must show token 1 destroyed at least one live response. | Source/selftest audit only. Real instrumented Global drop/reconnect remains unqualified. |
| Restored pin GET | Acquisition uses owned capture policy over every registered alias, not only the faulted path. Global chapter selection can issue a different owned alias GET. | Existing restored-before-click, enabled composer, exact DOM ID/count, source pin and physical-anchor assertions retained. Runtime revalidation remains required. |
| End-of-run 22 errors | Retained ordinary drain has 26 outcomes: 22 `capture_failed` (wrong aggregate), 3 captured original-history bodies, 1 expected request failure. The 25 raw bodies are all usable; the old rejection conflated snapshot projections. | Repaired handler accepts the 25 bodies offline; this does **not** retroactively pass the original runtime drain. New sealed drain still rejects every required body error and every application page error. |

### Inputs, provenance and transformation receipts

Local source evidence directory:
`.phoenix/evidence/continuous-history/pre-dispatch323-offline/`.
Final replay outputs are `ordinary-final/` and `global-final/`; `final/` has local
symlinks for the mutation runner. Earlier attempts/logs remain retained.

- Ordinary input: `instrumented323-artifact/opening-browser-evidence/ordinary/`
  fixture, `browser-responses.jsonl` full `body_received` records, IPC evidence and
  `browser-sse.jsonl`; its parent `identity.json` attributes the instrumented run.
- Global input: `shipping-d4-artifact/history-evidence/` fixture, all full Global
  bodies in `browser-api.jsonl`, `driver-api.jsonl`, `browser-sse.jsonl`, identity.
  The missing fixture `manual_opening` is appended **only to the replay copy**
  from the actual accepted aggregate at browser line 272. A later registered
  successor/key is derived from the actual accepted aggregate at line 406.
  `transformations.json` records source file/line/raw hashes, operation and value.
  These are retained **shipping facts**, not instrumented Global fixture truth.
- Source checkout is `target/history-hidden-geometry-repair`, verified read-only
  at `d4c2772a95aa46da608b356c8fe7be074de1b7aa`, tree
  `d9f0f0e1f49661ac0acc65ce70c7e7fd92f0dab9`, tracked-clean. Generated DTO hashes
  include both ordinary projection types and the actual Global snapshot type.
  Runtime source attribution remains the independent builder's attestation;
  hashes do not prove how a binary was compiled.
- Each output `manifest.json` hashes every supplied input, installed recorder,
  shared helper, replay CLI, generated DTOs and output receipts. `provenance.json`
  maps source JSONL line/hash/raw hash to replay request ordinal and retained
  request ordinal (where present). Driver records lack a transport content-type
  header; their JSON response adapter type is explicitly labeled inferred.
  Browser records lack a request-method field in shipping capture; GET inference
  is also labeled. Offline navigation epochs are not claimed as original epochs.
- The adapters feed authentic **unchanged full raw bytes** to
  `installAggregateRecorder`'s actual `request`/`response` listeners and drain.
  This is not merely a call to its validation helper. Explicit truncation, byte
  mismatch, over-bound inputs and unavailable/malformed required bodies reject.

### Reproducible compiler-free commands and results

```sh
E=.phoenix/evidence/continuous-history
F="$E/pre-dispatch323-offline"
O="$E/instrumented323-artifact/opening-browser-evidence"
G="$E/shipping-d4-artifact/history-evidence"
# Use NEW output directories when repeating; CLI refuses existing output dirs.
node ui/scripts/replay-history-opening-recorder.mjs \
  --fixture "$O/ordinary/fixture.json" --kind ordinary \
  --responses "$O/ordinary/browser-responses.jsonl" --identity "$O/identity.json" \
  --source-root target/history-hidden-geometry-repair \
  --sse "$O/ordinary/browser-sse.jsonl" --ipc "$O/ordinary/ipc.jsonl" \
  --output "$F/ordinary-final"
node ui/scripts/replay-history-opening-recorder.mjs \
  --fixture "$G/fixture.json" --kind global \
  --responses "$G/browser-api.jsonl" --responses "$G/driver-api.jsonl" \
  --identity "$G/identity.json" --source-root target/history-hidden-geometry-repair \
  --sse "$G/browser-sse.jsonl" --output "$F/global-final"
HISTORY_OFFLINE_EVIDENCE="$F/final" node --test \
  ui/scripts/replay-history-opening-recorder.selftest.mjs
node --test ui/scripts/qualify-history-realserver.selftest.mjs \
  ui/scripts/qualify-history-opening-realserver.selftest.mjs
python3 -B tests/e2e/history_opening_browser.test.py
uv run --offline tests/e2e/history_browser.test.py
UV_OFFLINE=1 ./dev.py check --lanes task
git diff --check
```

Results: 25 ordinary + 14 Global installed-handler captures; three authentic
replay/mutation test groups pass (no skips with supplied evidence), including 12
ownership/opening mutations per kind, malformed/truncated/foreign-request cases,
six ordinary handoff mutations, synthetic Global Pending key mutation and real
SSE cursor rejection cases. Node shared/opening selftests: 34 pass. Python
opening: 12 pass; shipping: 14 pass using existing offline uv environment (plain
system Python lacked httpx). Syntax checks of all three mjs drivers pass; task
lane and whitespace check pass. Full Rust/UI/E2E lanes were intentionally not
run under the no-build/no-new-runtime scope. Artifact-dependent mutation tests
skip when `HISTORY_OFFLINE_EVIDENCE` is absent; they are not standalone runtime
qualification. No dependencies or public configuration were changed.

**Residual gates:** independent parent review before any hosted push/dispatch;
new real ordinary/Global instrumented execution for held Pending, release,
actual roster transport fault, composer, physical anchor, stream drop/reconnect
and restored chapter acquisition. Global acceptance-transition cursor remains
unobserved in the retained shipping SSE (only accepted Init + later messages).
Generated-authority acceptance, shipping/provider/production and device/engine
claims remain outside this evidence. Offline success does not waive these gates.


## Remaining-findings audit of local 428 (pre-dispatch, no runtime)

This correction owns only the harness/recorder/tests/documentation. No push,
remote CI dispatch, build, dependency install, browser/server execution, runtime
change, production operation, app source or generated DTO edit occurred. The
roadmap body was read for orientation; no roadmap/remote record was written.
Earlier audit results above remain historical, not evidence of this source.

| Remaining finding | Disposition | Current validation / residual |
|---|---|---|
| Seal dropped pre-seal response arrivals | Corrected: freeze identities once, retain response/requestfailed handlers for known pre-seal objects; unknown arrivals get explicit `outside_capture`, never a synthesized identity or required state. | Deterministic installed-handler tests cover late known response success, unknown response/failure, known expected/unexpected failure and body rejection. |
| Drain was only a snapshot of body tasks | Corrected: awaiting-response states and dynamically arriving bodies both participate, with one absolute 11-second seal deadline plus each body's bound. Close aborts both states. | Adapter-controlled timers assert the exact absolute deadline, late-body noncapture and zero remaining timers/tasks. Real-clock test timeout is only a liveness guard, not the behavioral oracle. |
| Hard stop omitted incomplete capture state | Corrected: `partial:true` receipt and result inventory include pending metadata, outcomes, body task count and absolute deadline before nonzero exit. Complete drain is recorded only after all states/tasks terminate. | Partial-inventory regression plus source hard-stop wiring; no hard-stop child/runtime was executed. |
| Python could count forced SIGTERM as normal server completion | Corrected: after successful browser/Stop, wait at most 65 seconds **inside** `owned_process`, before its forced-cleanup finally. Require actual libtest returncode zero, removed IPC socket, removed driver-owned HOME from the validated startup receipt. | Fake-process success, timeout, positive/signal exit and resource-cleanup override tests. `server-completion.json` retains success/timeout/nonzero/signal/cleanup failure; failures before normal wait are `force_cleanup_required`, never success. Outer `result.json` includes each completion receipt. |
| Full manual edge and restored acquisition | Existing four-field manual fixture/Pending oracle retained; restored `fetchedLatestAccepted` body now independently validates ownership and full accepted edge against committed IPC evidence, not its own roundtrip. | Each of four edge fields mutated for fixture/Pending/accepted; Global acquisition uses all owned aliases. URL alone cannot qualify a foreign body. Python stable-product and persisted adjacent-boundary checks retained and endpoints asserted explicitly. |
| Actual chat ACK retained only as generic metadata | Corrected: driver saves real raw/parsed ACK, actual URL/status and exact canonical ID in assertions before validation. Fresh-key duplicate (`already_persisted:true`) and steering responses fail. Pinned false-field omission is accepted without fabricating keys in the retained body. | ACK field mutations fail; queued/steering/duplicate contract inspected in pinned/current `ChatResponse` implementation. Real ACK execution remains required. |
| Mock server can fail after Stop rejects a worker | No bypass added. Exact pinned ignored driver polls actual `run_server`, uses existing Stop/gate cleanup and drops its driver-owned TempDir before test process return. The new zero-exit gate exposes server/worker failure instead of forgiving it. | Read-only driver source audit only; a normal Stop returning nonzero is a genuine qualification failure requiring investigation, not a forced-cleanup pass. |

### Fresh authentic replay and source hashes

New local output, **not** replacement of any old artifact:
`.phoenix/evidence/continuous-history/pre-dispatch428-drain-audit/final-with-pinned-ack/`.
`ordinary/manifest.json` and `global/manifest.json` hash the corrected recorder,
shared helper, replay CLI, exact pinned generated DTOs, original inputs and new
outputs. The parent directory’s `source-audit.json` additionally hashes Python, Node tests, this document,
and the read-only pinned ignored driver. Same authentic source JSONL/IPC/SSE inputs
and CLI flags as above were rerun with `--output` changed to these fresh
`ordinary/` and `global/` directories; original full raw bodies remain unchanged.

Executed validation:

- Installed-handler authentic replay: **25 ordinary + 14 Global bodies**.
- Combined shared/opening/authentic replay Node tests: **43 passed, zero skips**.
- Compiler-free Python opening: **16 passed**; shipping: **14 passed** (offline uv).
- Three driver/CLI syntax checks, `UV_OFFLINE=1 ./dev.py check --lanes task`, and
  `git diff --check`: passed. Rust/UI/build/E2E lanes were deliberately not run.

The successful replay exercises corrected ordinary and Global wire projections;
it does not exercise newly authored normal-server completion, real browser drain
ordering, held gate, ACK, transport drop, composer or physical anchors at runtime.
Independent parent source review remains next. The retained Global SSE still has
**zero acceptance-transition frames** (eight accepted Init frames and six later
message cursors); generated authority, real providers, shipping/production,
physical devices and other engines remain unqualified. No new lifecycle or
public control surface was added.

## Global prefix-fault causality correction on parent 5fb (offline only)

Artifact **11870928672**, local
`.phoenix/evidence/continuous-history/instrumented5fb-artifact/opening-browser-evidence/`,
is retained unchanged. Ordinary completed **nine checks**, with no application or
capture errors, complete drain (`partial:false`, pending zero), normal server exit
zero and successful cleanup. This is ordinary instrumented evidence only.
Global completed two checks (manual opening and held Pending), released the gate,
and failed waiting for an actual failed owned GET. Its server completion is
**`force_cleanup_required`**, even though the eventual recorded returncode is zero;
it is not normal server-completion proof.

Raw Global observations, independently inspected:

- Request ordinal 42 / response lines 39–41: original-route Global history 200.
- Ordinal 95 / lines 104–106: mounted middle member history 200, Pending successor.
- Ordinal 100 / lines 109–111: same middle member history 200 with `before`.
- Ordinals 103, 109 and 110: `/api/product-conversations/<successor>` is **404**,
  metadata-only negative ownership probing, not a successful history response.
- No post-release Page history GET or preview destruction witness was retained.
  The old first-snapshot target and passive post-release assumption cannot prove
  this failure scenario. No 404 is reclassified as 200 or faulted as a substitute.

The new **Global-only** branch freezes the exact history pathname from the most
recent owned, registered, complete positive snapshot for the mounted URL's single
`source_transcript` pin. The route alias must also be registered but need not equal
that pin. Stable product, selected member, latest successor, full opening edge,
JSON/status and pagination cursor are revalidated. Ordinary retains its existing
post-release fault trigger/assertion pipeline.

While Pending, a DOM-only `#messages.scrollTop = 0` uses the pinned d4 rendered
prefix-top sentinel. No React callback/store access, fabricated event/API body,
page route interception, direct DB write, or application edit is added. The branch
requires the actual failed GET with message_limit 100 and the exact positive
page's next `before` cursor, matching owned preview destruction, and the existing
`Could not load earlier history:` prefix-error indicator **before release**.
It checks the gate remains entered, composer presence/read-only state and source
pin survive. A shared absolute 20-second prefix budget (per-wait cap five seconds)
is separate from the unchanged 60-second gate deadline. DOM scrolling restores
the saved viewport and exact persisted middle ID before taking the physical
anchor baseline, so deliberate reader movement is not acceptance movement.

The typed attribution kind is `global_history_prefix_acquisition`, retaining
source alias, stable product, known latest successor, positive URL/status/local
ordinal/cursor, actual failed request and destruction. Its scope is explicitly
**earlier-history acquisition failure, not full fresh-tail refresh**. Existing
accepted SSE, exact successor composer ACK/persistence, same-page reconnect,
physical-ID/count/anchor, fault-restore-before-chapter and drain assertions remain
mandatory. The prefix error is not cleared before that restoration step.

### Compiler-free validation performed

```sh
HISTORY_GLOBAL_5FB_EVIDENCE=.phoenix/evidence/continuous-history/instrumented5fb-artifact/opening-browser-evidence/global \
HISTORY_PINNED_SOURCE=target/history-hidden-geometry-repair \
HISTORY_OFFLINE_EVIDENCE=.phoenix/evidence/continuous-history/pre-dispatch428-drain-audit/final-with-pinned-ack \
node --test ui/scripts/qualify-history-realserver.selftest.mjs \
  ui/scripts/qualify-history-opening-realserver.selftest.mjs \
  ui/scripts/replay-history-opening-recorder.selftest.mjs \
  ui/scripts/global-history-prefix.selftest.mjs
python3 -B tests/e2e/history_opening_browser.test.py
```

46 Node tests passed, zero skips, including authentic 5fb request/response target
replay, 404/foreign/truncated/ambiguous-source rejection and literal checks against
the pinned d4 `MessageList` sentinel/error source. The old authentic ordinary and
Global installed-recorder replays remain included. Python: 16 passed. The new
artifact-dependent tests skip without their explicit evidence/source variables;
they do not fabricate substitute fixtures. Syntax and whitespace checks passed.

No hosted push/run, compiler/build, dependency install, browser/server, production
operation, app source or generated DTO edit was performed. Roadmap body 806 was
read for orientation only. **Independent next source review is required before
one hosted run.** Global's new user-scroll/failure/restore sequence, held-gate SSE
acceptance/composer/reconnect and normal server completion remain runtime gates,
not offline passes. Shipping/provider/device claims remain excluded.

## Response-before-IPC authority correction on parent 40 (offline only)

The actual ordinary `instrumented40-artifact` request **51** received five members,
latest successor `122a…`, and Pending key `13e2…` while the independent fixture
still knew four members. Full Entered IPC subsequently confirmed that edge.
The prior immediate roster rejection was a witness authority-ordering bug; the
failed runtime and forced cleanup are **not qualified** by this correction.
Prior 5fb ordinary COMPLETE evidence retains its original scope. App source
`d4c2772a95aa46da608b356c8fe7be074de1b7aa` remains unchanged.

Before POST, the installed recorder now receives a typed expected-manual-Pending
reservation from the existing manual product identity and independently armed
predecessor/key. The reservation is not committed membership. Exactly the known
roster prefix plus one future latest Pending successor can pass full structural
parse/projection/handoff validation and enter
`awaiting_independent_committed_evidence` (`structural_candidate`, not qualified).
Accepted, unreserved, foreign, unknown-prefix, two-extra-member, wrong-key and
wrong-authority bodies fail immediately. Full Entered IPC atomically registers
successor and opening only after matching reservation and every staged full
four-field edge/key. There is no member-only release method. CLI preexisting
facts use explicitly labeled `registerKnownOpening`, permitted only before all
requests/reservations; they cannot release staged authority.

Every staged receipt retains its original request identity, reservation epoch,
response metadata, raw bytes/hash, and original body deadline. Independent source
receipt provenance accompanies eventual captured evidence. Body read plus stage
share the original absolute **11s** response deadline, retain the **2MiB** body
bound, and are additionally capped by the shared **11s** seal deadline. Registration,
refresh, and seal never renew the original deadline; missing/mismatched/late IPC
fails qualification, including final drain. Up to 300 staged receipts are allowed.

### Actual compiler-free commands/results

```sh
E=.phoenix/evidence/continuous-history
# Output directories must be new. Both invocations were executed successfully.
for pair in '40 ordinary' '5fb global'; do
  set -- $pair
  d="$E/instrumented$1-artifact/opening-browser-evidence"
  node ui/scripts/replay-history-opening-recorder.mjs \
    --fixture "$d/$2/fixture.json" --kind "$2" \
    --responses "$d/$2/browser-responses.jsonl" --identity "$d/identity.json" \
    --source-root target/history-hidden-geometry-repair \
    --sse "$d/$2/browser-sse.jsonl" --ipc "$d/$2/ipc.jsonl" --race yes \
    --output "$E/authority-race40-offline/$2"
done
HISTORY_RACE_EVIDENCE="$E" \
HISTORY_GLOBAL_5FB_EVIDENCE="$E/instrumented5fb-artifact/opening-browser-evidence/global" \
HISTORY_PINNED_SOURCE=target/history-hidden-geometry-repair \
HISTORY_OFFLINE_EVIDENCE="$E/pre-dispatch428-drain-audit/final-with-pinned-ack" \
node --test ui/scripts/qualify-history-realserver.selftest.mjs \
  ui/scripts/qualify-history-opening-realserver.selftest.mjs \
  ui/scripts/replay-history-opening-recorder.selftest.mjs \
  ui/scripts/global-history-prefix.selftest.mjs \
  ui/scripts/history-opening-authority-race.selftest.mjs
python3 -B tests/e2e/history_opening_browser.test.py
PHOENIX_CHECK_WORKERS=1 ./dev.py check --lanes task,spec-shape,spec-anchors
git diff --check
```

| Executed check | Result and scope |
|---|---|
| Actual40 ordinary installed-handler race replay | **25 raw bodies captured**; request51 stages before full independent IPC registration and resolves with unchanged bytes/identity. |
| Actual5fb Global installed-handler race replay | **3 raw bodies captured**; actual coordinator-history Pending request95 stages with base-four fixture, then actual Entered IPC resolves. No renamed ordinary or shipping synthetic Pending. |
| Existing authentic old replays | **25 ordinary + 14 shipping Global = 39** raw bodies preserved through installed handler. Shipping Global still has no observed Pending transition. |
| Node regressions | **48 passed, zero skips**: existing46 guards plus two authentic race cases. Both domains cover immediate structural negatives, all four IPC edge fields/key/commit marker, missing/late evidence and deterministic original-deadline final-drain liveness without sleeps. |
| Python opening wrapper | **16 passed**. |
| Restricted dev.py check | Task, spec-shape, dev.py unit tests and spec-anchors **all passed**. No compiler lane. |

Manifest, input/output hashes, retained source-line/raw hashes, authority receipts,
staging/captured metadata, and drain inventory are retained under
`.phoenix/evidence/continuous-history/authority-race40-offline/`.
An initial CLI attempt pointed at `target/history-shipping-d4` and correctly
rejected its **8a273c** HEAD before producing output; the successful commands above
use the independently verified, tracked-clean d4 checkout. No authority check was
weakened to accept that mismatch.

**Independent exact-source review remains required before one successor run.**
No push, hosted dispatch, browser/server runtime, build, production, dependency,
public-control, or app/generated-DTO change was performed. Offline replay is not
normal runtime completion, Global fault/anchor/reconnect acceptance, shipping,
provider, or physical-device qualification. The current40 forced cleanup remains
failed/incomplete runtime evidence, not a retroactive pass.

## Explicit document navigation lifecycle correction on parent bd (offline only)

The actual retained bd ordinary request51 is an owned GET born at **2332.471986ms**,
navEpoch2, `net::ERR_ABORTED`, expected transport failure **false**. It is distinct
from actual request126 (expected owned proxy fault **true**) and the retained
`destroy-roster` preview receipt. The old logger has no failure time or explicit
goto/document confirmation receipts. Request51 is therefore a **trace candidate**,
not proven navigation cancellation; this change does not relabel the old failed
qualification or claim a proven old cause.

### Recorder lifecycle contract

Only the driver's existing `manual_identity` and `pending_historical` explicit
`page.goto` calls begin a typed document scope, immediately before goto. Requests
receive immutable birth scope, original metadata and navigation epoch. SPA pins,
chapter clicks, and arbitrary frame/hash events do not begin scopes. Confirmation
is called after awaited goto with its actual Playwright Response, never JSON or a
fabricated runtime Response. The installed handler must have observed that exact
Response/request object: HTTP200 navigation request, main-frame identity, exact
original/request/response target URL (strip fragment only), and a subsequent
main-frame event with matching frame/current URL. Intent, document receipt, frame
and confirmation URLs/times are logged in `browser-navigation.jsonl`.

A preexisting required request from the immediately replaced scope with exact
`net::ERR_ABORTED`, observed after intent, may enter
`awaiting_document_supersession_confirmation`. Its deadline is **11 seconds from
original failure**, never reset by confirmation or seal. Only actual matching
document confirmation permits terminal
`oldrequestCanceledAsSupersededNavigation`. A separate canceled inventory retains
original request/failure/time/context/intent/document proof; it is not a body
receipt and fabricates no missing semantics. `partial: false` means the sealed
inventory contains no pending operations; terminal cancellations and terminal
fatal failures are both drained, but fatal failures still reject qualification.
Seal imposes its own maximum11s hard wall. Current/same-context aborts, failures
before intent, foreign/wrong/failed/unconfirmed documents, non-navigation responses,
SPA-only events, other error strings and late confirmation remain fatal. HTTP200
unreadable old required bodies still fail the unchanged strict body validator.
The existing expected-owned-proxy-fault snapshot at request birth is unchanged.

After explicit navigation, qualification separately requires at least one
complete strict HTTP aggregate body born in the **current document scope**,
matching product/root/selected route and the latest registered roster/opening
identity. Old captured bodies, old canceled requests and current expected proxy
faults cannot satisfy that obligation. Current bodies must still resolve actual
independent committed IPC evidence where required. Global prefix fault, real SSE
body/cursor checks, normal server cleanup and all previous guards are preserved.

### Executed compiler-free audit matrix

| Check | Result / scope |
|---|---|
| Six Node selftest files | **92 passed, zero skipped** (all previous48 guards plus44 lifecycle guards). Ordinary and Global cover strict current-body positives; missing/wrong/failed/unconfirmed documents; foreign-frame/not-navigation/unobserved-response; current/no-intent/before-intent/other-error aborts; SPA-only, original11s deadline and seal cap; old-body-only, missing/stale/wrong-owner current body and unreadable old HTTP200 body negatives. |
| Retained bd installed-handler replay | Actual request51 and retained document-request metadata/full aggregate bytes, with preserved line/file hashes. **Synthetic controlled event ordering and navigation/frame adapters**, explicitly not actual old cause proof. Request51 is required for ordinary, metadata-only for Global. Retained request126 true fault classification/preview destroy are independently checked. |
| Authentic ordinary/shipping Global offline replays | **25 + 14** unchanged raw bodies, source and provenance hashes retained. Shipping Global still does not prove a Pending transition. |
| Authentic response-before-IPC staging replays | **25 ordinary + 3 Global** unchanged raw bodies captured; authentic40/5fb IPC sources preserved. |
| Python opening wrapper | **16 passed**. |
| Restricted dev.py lanes | Task, spec-shape, dev.py unit tests and spec-anchors passed; no compiler lane. |

Local output manifests, provenance, installed-handler receipts/drains and test logs
are under `.phoenix/evidence/continuous-history/navigationbd-offline/` (harness-local
pointers). Reproduce the added lifecycle tests with
`HISTORY_AUTHORITY_EVIDENCE=.phoenix/evidence/continuous-history node --test ui/scripts/history-opening-navigation.selftest.mjs`.
The optional `HISTORY_NAVIGATION_OUTPUT` path retains the explicitly labelled
trace-candidate replay receipts; it is only an offline test output, not a runtime
control. The first shipping Global CLI staging invocation omitted its actual
`browser-api.jsonl`/`driver-api.jsonl` inputs and rejected before replay; the corrected
invocation captured14 bodies. Neither invocation modifies retained inputs.

**Residual gates:** parent independent exact-head review before the one permitted
instrumented runtime attempt; actual owned main-document response/frame event
ordering and new intent/failure/confirmation receipts; real Global prefix fault,
accepted SSE/reconnect/send/anchor and normal cleanup. No runtime, hosted, build,
production, push, shipping/provider or physical-device action was executed here.
App source remains unchanged at d4; no app/framework/public controls were added.

## Global prefix reader-ownership correction on parent b1 (offline only)

Retained **b1e79db667a8f762a52ae2fd198a7ebf86170c6d** Global responses42,95,100
contain respectively `[0,20,76,4]`, `[0,20,76,4,0]`, `[44,56,0,0,0]`
segment message counts. All three report `has_older: true` with a nonempty
`before`; request100 is an already captured positive prefix page, not exhausted
history. Its next cursor is the target, not request95's earlier cursor. The
retained request inventory has no subsequent prefix GET, and preview.log has
no `destroy-roster` receipt. The old run remains **failed**, not qualified.

The offline causal probe loads the **actual pinned d4** import-free
`conversation/scrollMachine.ts` with Node26 `module.stripTypeScriptTypes` and a
data-URL import. It checks each input file byte-for-byte against d4 git objects,
extracts the actual `readerMovedViewport` function and `requestEarlierHistory`
early-return guard, and executes these extracted expressions without invoking
a load callback. Actual b1 raw bodies/cursors and independent Entered IPC remain
unchanged, with retained file/line/raw hashes and source hashes in the output.

**Synthetic offline causal scope:** navigation → measurement → DOM-style
`scrollerAttached` at top leaves `navigating/positioning`, so the actual guard
blocks. `upwardIntent` alone also leaves this navigation state ineligible.
The actual onWheel order is `interactionStarted` then `upwardIntent`; together
these transition to `navigating/user-returning`, making the **same retained
cursor** reader-owned and eligible. Loading, older-error, already-scheduled,
no-older and wrong-view cases still block. Geometry and event ordering here are
explicitly synthetic; the retained browser log does not contain machine state
or actual hardware gesture receipts. This is a demonstrated source-level causal
mechanism, **not proof of the old browser's exact internal state**. REQ-MLRU-014
intentionally prevents positioning commands acquiring reader ownership.

Only the Global runtime prefix action changes: hover the actual `#messages`
viewport, observe settled geometry/captured snapshots, then freeze the latest
strict positive cursor and activate the existing owned preview fault **before**
the first CDP hardware `page.mouse.wheel`. At most three negative wheels are
allowed, each derived from observed `scrollTop`, capped at16384px, with observed
viewport decrease/stability and the actual rendered `data-virtual-index` prefix
range (`startIndex <= 2`, `scrollTop <= 2`). There is no DOM-top trigger, React
callback or synthetic event. Missing sentinel/failure is a hard failure, not a
retry loop. The first failed owned prefix GET must match the frozen current
cursor; the exact existing proxy-destroy receipt, real earlier-history alert,
Pending composer/key/gate and pin guards still precede release. Wheel pre/post
geometry, selected request/cursor and restored premeasurement baseline are
retained. The original historical DOM restore is allowed **only before anchor
measurement**, not as evidence of user intent. Ordinary behavior is unchanged.

### Executed bounded audit

- Seven Node selftest files: **95 passed, zero skipped** = all previous48 +44
  navigation guards +3 causal/action groups (including all five state negatives).
- Actual b1 full raw response42/95/100 validation/selection replay and pinned d4
  executable reducer/predicate/guard probe passed. No bytes were regenerated.
- Python opening wrapper: **16 passed**. `git diff --check` passed.
- Compiler-free task/spec-shape/spec-anchors dev.py lanes checked separately.

Harness-local receipts/logs: `.phoenix/evidence/continuous-history/prefixb1-offline/`.
Reproduce the additional probe with
`HISTORY_PINNED_SOURCE=target/history-hidden-geometry-repair HISTORY_GLOBAL_B1_EVIDENCE=.phoenix/evidence/continuous-history/instrumentedb1-artifact/opening-browser-evidence/global node --test ui/scripts/global-history-prefix-causal.selftest.mjs`.
`HISTORY_PREFIX_CAUSAL_OUTPUT` optionally retains the labelled offline JSON receipt.

**Gate remains frozen:** parent independent exact-head review is mandatory before
one successor attempt. No runtime/browser/server, dependency install, compiler,
UI writer, build, hosted dispatch, push, provider request destruction, production
or physical-device action was performed. App d4 is unchanged. Actual hardware
wheel/sentinel timing, current-cursor fault, SSE/reconnect/send/anchor and normal
cleanup remain runtime obligations, not retroactive acceptance of b1.

## Scoped postfault physical baseline correction (parent 3ec063739)

The retained actual 3ec Global artifact is a **failed** runtime, not acceptance.
It certifies the hardware wheel/current-cursor acquisition fault, preview
transport destruction, visible prefix error and unreleased Pending gate. Its
browser result has two checks, then fails at the original middle-message
visibility wait (driver line 795). The actual wheel receipt changes scrollTop
7383→0, scrollHeight22616→21806 and clientHeight798→729. Restoring a pixel value
cannot certify the original logical location across that layout change. No
screenshot is used as persisted-identity proof; no old DOM snapshot is invented.

Retained harness-local evidence is under
`../../refresh/terminal-followthrough/witness3ec-artifact.zip`, relative to this
implementation worktree. SHA256:

- Artifact: `9862555fe097c583cce2f83e0c2eb5414423c61c67cd930da69e4b5687f882dc`
- Exact Global fixture bytes: `33d0d3518267a574271b426680d8cca944a9ddd107af9ddf262e5602d78e4e58`
- Exact Global wheel JSONL: `40b3952b383b07be67a339f5e43cb26e73cd1e654a731f89675b16acdb9b08df`

The successor removes savedTop/scrollIntoView restoration. Its declared baseline
is explicitly **settled POSTFAULT**, after visible error and while Pending, not
the prefault viewport, original middle anchor or URL-hash reconfirmation. The
unchanged `pinned_source_transcript` and separately measured `physical_source_row`
are both recorded; the latter may legitimately be a different historical member
following the user's upward hardware wheel.

The exact installed, self-contained DOM probe selects the first eligible fully
visible virtual row with an unambiguous, fully visible positive-size
persisted-message descendant in DOM order.
It resolves an exact `data-message-id` or a DOM ID with only the documented outer
`message-` wrapper removed against fixture occurrences. Source authority is the
occurrence ordinal's known historical member, **never an ID-prefix/text guess**.
Actual3ec has232 occurrences across four prior members and includes opaque IDs;
requiring every ID to embed its source would incorrectly reject this fixture.
The Pending successor, optimistic IDs, group-key-only identity and new opening
are not fixture members/occurrences and cannot certify a baseline. No generic
row fallback, private React state, extra chapter GET or new wheel is added.
Failure to certify is a hard failure, within the retained20s prefix budget.

Before release, the receipt includes virtual key/offset, exact persisted ID,
source row, element ID/data-message-id, row/message/viewport bounds, URL, source
pin and layout dimensions, four consecutive stable frames (≤2px), and
`checkedGatePending`. Virtual-key/offset `anchorMatches` remains in the
acceptance/reconnect subsequence; Global derives that key/offset from the same
physical observer evaluation as the selected child, not a separate `anchor()`
evaluation. Ordinary `anchor()` behavior is unchanged. Both checks additionally require
the declared exact persisted child/source to remain physically visible with
message offset≤2px; a surviving agent-group virtual key alone is insufficient.
The prefix error is checked again after both. Fault restoration still occurs
only before the existing explicit new-chapter acquisition phase. Ordinary
behavior, SSE/opening/chat ownership and normal-server guards are unchanged.

### Executed bounded validation

- Eight Node files: **117 passed, zero skipped** (retained95 +22 new DOM/mapping,
  stability and driver guards). The installed probe is serialized into a VM with
  **fabricated DOM/geometry UNIT ONLY**. These tests do not prove actual browser
  boxes. Exact3ec fixture mapping passed separately within that suite.
- Python opening wrapper: **16 passed**.
- `UV_OFFLINE=1 PHOENIX_CHECK_WORKERS=1 ./dev.py check --lanes task,spec-shape,spec-anchors`:
  task validation, spec shape, dev.py unit tests and spec anchors all passed.
- `git diff --check` passed. Logs remain harness-local under
  `.phoenix/evidence/continuous-history/postfault3ec-offline/`.

Reproduce the95 retained tests with the evidence environment recorded in the
preceding sections, adding `ui/scripts/history-postfault-baseline.selftest.mjs`
and `HISTORY_GLOBAL_3EC_FIXTURE` pointing to the exact extracted3ec Global fixture.

**Residual gate:** independent exact-HEAD review is mandatory before one
successor attempt. Actual DOM collection, virtualization/box stability, wheel
user-intent handling, acceptance/reconnect anchor preservation and cleanup
remain runtime obligations. No runtime, browser, server, compiler/build, hosted
dispatch, dependency change, UI writer, push or production action was performed.
Shipping app d4 is unchanged. This is a locally tested harness correction, not a
new runtime pass or retroactive qualification of3ec.


## Physical-baseline ambiguity blocker correction (parent 5540239f2)

Independent review of local554 was **not cleared**: the first provider attempt
returned HTTP500; the explicit GPT5.6 Sol retry found a blocking ambiguity.
The observer returned a first match without proving unique selected physical
identity/key, and Global followup separately evaluated a generic anchor and a
persisted child, potentially combining observations from recycled rows.

The corrective successor selects the first eligible known historical candidate
in DOM order among distinct visible messages. It does **not** require all known
visible candidates collectively to number one. For the selected candidate,
all document instances of its virtual key must be exactly one DOM row, and all
wrapped-ID/data-message-id matches of its persisted ID must be exactly one
physical element, that very descendant owned by the selected row. Element Sets
deduplicate two exact attributes on one element but never aliases on separate
nodes. Conflicting attributes still count against uniqueness. Hidden, offscreen,
zero-height and outside-scroller duplicates count; followup cannot substitute
another identity or accept the first of multiple matches.

Renderer inspection found `UserMessage` has one wrapped DOM ID; `AgentMessage`
puts its wrapped ID and data-message-id on the same element. Compact tool groups
can have an ID on a wrapper and data-message-id on tool descendants. Those
multiple physical nodes are deliberately ineligible, not false-green aliases;
the observer may select the next unambiguous historical message instead. No
renderer or shipping app d4 change, user-only restriction, runtime fixture fake,
private control, timer relaxation, or generic-row fallback is introduced.

Initial baseline and each of its four stable frames read key/row offset and
child/identity/source/message offset in one synchronous page evaluation. Global
acceptance/reconnect likewise use that one physical observer result for both
virtual and persisted-child comparisons. Ordinary followup retains its existing
generic anchor. The original four-frame/2px checks and prefix budget are intact.

Bounded offline validation: **139 Node tests passed, zero skipped** (117 retained,
22 added uniqueness/atomic-followup regressions), including exact retained3ec
fixture mapping. All fabricated geometry is explicitly **UNIT ONLY**, not real
browser-box proof. Three edited modules passed Node syntax checks; the Python
opening wrapper passed all16 tests. The restricted offline single-worker
`./dev.py check --lanes task,spec-shape,spec-anchors` passed task validation,
spec shape, dev.py unit tests and spec anchors. `git diff --check` passed. Logs are
harness-local under `.phoenix/evidence/continuous-history/physical-ambiguity554-offline/`.

**Independent final exact-HEAD rereview is still required.** This records a
blocking review followed by an implemented local correction, not reviewer
clearance or runtime acceptance. No push, build/compiler, server/browser run,
hosted dispatch, provider destruction, deployment or physical-device action.
Actual runtime DOM uniqueness/stability and the existing acceptance/reconnect/
cleanup obligations remain unperformed.

## Global known-chapter cache contract — pre-dispatch correction

Retained artifact `11877321061` (harness `0bac11fc87c15eda8bd8e208d8301c64e943be9f`, unchanged app `d4c2772a95aa46da608b356c8fe7be074de1b7aa`) contains nine actual Global behavioral checks, including Pending, native-wheel prefix failure, unique post-fault physical baseline, accepted full-edge publication/ACK/persistence and real same-page stream reconnect. The run is still failed: its final chapter assertion required a new successful browser aggregate fetch. No such fetch was observed; product-alias probes were 404. Neither the screenshot nor those probes prove final selected persisted identity or normal cleanup.

Pinned source inspection separates Global from ordinary acquisition. `App` renders `CoordinatorPage` directly for Global. Coordinator source-selection effects revalidate the route/pin with real APIs while preserving a known resolved coordinator ID; they do not promise no traffic. The product-owned snapshot and latest live opening can supply known member selection without a fresh aggregate fetch. The actual `ConversationNav`/`ConversationNavStack.selectChapterTarget` callback preserves the route, replaces `source_transcript`, removes `source_tool` and sets the exact encoded persisted-message hash. The offline test executes that pinned callback; it does not fabricate a browser request.

The ordinary branch retains its successful fresh browser-acquisition assertion. The Global branch instead requires: unchanged independent committed IPC edge and complete cached raw Pending roster; accepted and reconnect observations on the actual owning stream; durable opening/ACK/message bytes; exact selected Global alias, latest member pin and message hash; unchanged one-count/visible/physical persisted-ID checks; and an actual independent **driver** coordinator-history `original_messages` reference read after the click. That read is recorded as reference authority, never as a browser acquisition. Any browser aggregate fetch that actually occurs remains subject to the installed strict recorder. No force reload, fake JSON, relaxed membership check or universal no-fetch promise is introduced.

Cache raw receipt validation matches its producer: complete `body_received`, exact byte count/SHA256 and raw/parsed parity within 2 MiB; explicit truncation, foreign owners, incorrect root/latest/full edge, missing durable identity, steering/duplicate ACK and wrong selected physical ID remain failures. An authentic accepted original-message reference from shipping-d4 driver line 22 is replayed as a contract positive with distinct product/root/member identities; it is not an instrumented Global reference or DOM success. Offline stream URL associations and physical test boxes are explicitly adapters/UNIT ONLY, not new runtime proof.

Validation after the bounded correction: all **183 Node tests passed, zero skipped**, including the prior 139 and 44 cache/selection/reference/source-contract cases; opening Python **16 passed**; restricted task/spec-shape/spec-anchors/dev-unit checks, syntax and diff checks passed. Two initial negative-test message matchers were corrected to the actual `committed opening evidence` and `ordered full roster` guards only after genuine positives passed. Logs are harness-local under `.phoenix/evidence/continuous-history/global-chapter0bac-offline/`.

Independent exact-HEAD review is required before one successor. Actual Global final selection/physical identity, live reference response, current-context complete capture/drain and normal server exit/socket/HOME cleanup remain runtime-only gates. Source app, ordinary proof, provider/device exclusions and landing authority are unchanged.

The initial independent review of `c97f2ae519652f2302543d1fe82fbb38cdc76dd9` blocked dispatch: known-alias membership alone could falsely pass a real chapter callback that switched to another known Global pathname. The local correction retains the actual pre-click URL before the real click and requires strict post-click pathname equality in addition to exact pin/hash and physical identity; the receipt retains both URLs. Negative tests reject different-but-known historical/product/latest aliases, not merely foreign aliases. This is a strengthened real-click requirement, not proof from the pinned callback unit test. Final exact-HEAD rereview remains required.

The follow-up independent review of `7868a70ce01a5d15d4b8d38ccc79cfc4f6727e11` also blocked dispatch: a receipt-time URL read after asynchronous reference reads/drain could contradict the earlier validated URL while claiming preservation. The local correction now captures the final URL, DOM count, exact persisted identity and physical visibility in **one actual page evaluation** after the reference/drain, revalidates exact pre-click alias/pin/hash and requires full URL equality with the earlier measured checkpoint. The receipt uses those immutable measured values, not a later `page.url()` call. A source-extracted producer unit changes the URL between evaluations and rejects the changed known alias; duplicate physical-ID nodes also reject. This unit is fabricated DOM only, not runtime proof. Final independent rereview remains required.


## Global reconnect preserve authority — bounded harness correction

The retained instrumented832 Global run reached its real same-page reconnect on
CDP request `14288.127`. Its accepted opening was published at sequence 1 on
request `14288.113`, while the reconnect requested the same generation and
incarnation after sequence 14 and returned an empty-message `preserve` Init at
tip 14. The run remains **failed and is not qualification**: the old harness
incorrectly required that reconnect Init to be `complete`. The click and driver
reference reads had executed, but the cache authority guard rejected before the
final Global capture drain and atomic DOM receipt; normal Global cleanup was not
qualified.

The cache authority validator now has two explicit reconnect forms. A `complete`
or `tail` Init must carry exactly one full accepted opening message. A `preserve`
Init must carry no messages and must be rooted in the actual prior `complete` or
`tail` Pending Init on the accepted publication's request. Runtime selection of
that seed comes from captured SSE event order and the owning CDP request map; it
is not reconstructed from opening identity.

The preserve form additionally requires exact single canonical nonnegative safe
integer `after_event_sequence` and `transcript_generation` query values on the
owning reconnect stream URL; one conversation owner; one non-null safe generation
across seed, reconnect and durable row; one nonempty stream incarnation; safe
accepted/seed/tip/anchor cursors with accepted publication covered and
`anchor <= cursor <= tip`; `Init.sequence_id == last_sequence_id`; untruncated
pending events; and unique strictly increasing pending sequence IDs in
`(cursor, tip]`. Pending seed messages may be empty because the committed Pending
edge/key is proven directly by the seed Init opening. Exact accepted publication,
durable opening, composer ACK, cached Pending aggregate, IPC edge, SSE opening,
independent reference, capture/drain, and final atomic URL/DOM guards are unchanged.

The focused offline selftest replays the authentic832 seed and reconnect URL from
the retained event/assertion request slice through an explicitly labelled
retained-log adapter. That adapter is not CDP provenance and creates no positive
browser response. Authentic0bac complete reconnect coverage remains, with a unit
coverage variant for the identical message-bearing `tail` branch. Negative cases
cover missing/wrong seed owner, edge, and coverage; cross-generation seed/Init/
row/request; incarnation; missing, duplicate, malformed, and unsafe cursor/
generation values; regressed cursor; anchor/tip/truncation; nonempty preserve
messages; pending ordering/range; and foreign/missing/content-corrupt accepted,
opening, and durable values at responsible guards.

Bounded validation executed here: `node --test
ui/scripts/global-chapter-cache.selftest.mjs` — **83 passed, zero skipped**. No
runtime/browser/server, build/compiler, network, push, deployment, or qualification
run was performed. The immutable `target/history-hidden-geometry-repair` source
and retained evidence were not modified; real temporal stream destruction,
page-URL preservation, final selection/reference/capture drain, atomic DOM
identity, and normal cleanup remain runtime obligations.

Independent review of local `599a028a2` blocked dispatch on an omitted-replay-tail
false green: a preserve Init claiming tip15 after cursor14 with no replay passed.
The follow-up preserve-only guard requires its claimed tip to equal the last
replayed event sequence, or the request cursor when replay is empty, matching the
pinned producer. Two negative cases reject empty and nonempty replay short of the
claimed tip. This does not constrain message-bearing complete/tail snapshots.
Final exact-HEAD rereview is required before one hosted successor; no dispatch or
qualification follows from the offline correction alone.

Rereview of local `ed511e1ea` then blocked a distinct preserve regression:
request cursor1/tip1/anchor1 could cover the accepted publication but omit durable
composer rows already present before the drop. The correction retains the actual
successful composer-persistence poll snapshot **before** causing the transport
drop, records it as an independent driver reference, and requires the preserve
cursor to cover that owned, generation-matched durable lower bound. Both opening
and composer ACK must exist uniquely with exact full message bytes in that
snapshot. It is a pre-drop lower bound, **not an init-time exact DB watermark**;
a later driver row cannot replace it because persistence may advance after Init.
Authentic832 uses the first successful retained driver poll as an explicitly
offline call-order adapter. Missing/foreign/cross-generation/unsafe floor and
cursor-below-floor negatives reject, while a UNIT-only later-row extension does
not raise the earlier floor. Final rereview remains mandatory before dispatch.

## Integrated joint harness preparation — source only, not runtime qualification

The joint candidate app is `c825370814d22f0e03faf72d18f37df97bb59329`,
tree `ac4539bb02bff27c8094264322b6f5a8398e5011`, parent integrated
`85ab23b66c78e63fb12687918d00400edf67e6e8`, based on pinned main
`84fbebb0781067cbe843f61c53093dcc0f0d64f2`. Historical `d4`/`ee8`
receipts remain historical and do not qualify this candidate. The six new pure
Rust exporter-environment tests have not been compiled or executed locally.
The existing five instrumentation guards are retained in the future hosted
selector alongside those six; the actual-server driver remains ignored there.
The earlier hosted `0f` render-fixture failure is unresolved; isolated local
fixture passes are not a causal repair or broad-suite qualification.

The test-only app seam snapshots an optional all-or-none validated exporter
quartet before the full ambient scrub and restores only that quartet afterward.
Unrelated ambient variables are ignored/not projected, then scrubbed; they are
not parser-rejected. Endpoint validation proves literal loopback shape, **not
receiver ownership**. The joint Python owner must start and probe its receiver
before the app and bind the endpoint/service/run to that actual receiver.

Joint activation is explicit (`--joint-telemetry`). The harness verifies app
source/tree/base, binary/locks/UI receipt, and all three unchanged app-owned
module hashes before runtime imports. It does not copy donor code or infer app
identity from a donor checkout path. The receiver dependency is explicitly
`opentelemetry-proto==1.39.0`. Finite ordinary and Global fixtures and lifecycle
route/product identity come from the new isolated server; old artifact UUIDs
are never used as live fixture identities.

The existing history stage keeps its 170-second deadline. Only after its strict
capture seal/drain and exact physical message receipt may the joint telemetry
stage start on a separate context/page, leaving the original page unchanged.
The stock telemetry module retains natural 0.25 sampling, finite fresh-context
bounds, the actual exactly-one-visible SPA home-link guard, and its own main
285-second bound. The combined Node driver is bounded by 480 seconds and the
Python owner by 500 seconds, below the app's existing 600-second driver budget.
Stock current ready reports must retain accepted anchor results and all four
current marks matching their exported journey expectations. This is a
crosscheck of the installed read-only owning probe/report contract, **not** a
new telemetry-lifecycle physical-target witness from generic DOM checks. The
independent physical history receipt remains separately scoped.

The app must stop normally and close its listener/remove owned HOME/socket
before receiver shutdown/join. The owner records observed app/receiver stop
booleans and runs the final telemetry assertion only after teardown; forced
cleanup is a failed receipt. Browser capture drain is not substituted for
telemetry receiver drain. Global known-member history selection remains
cache-backed (possibly zero new owned aggregate GETs); telemetry cold contexts
do not turn that chapter action into a fresh browser acquisition. Stock
acceptance stays partial and overhead clearance stays false.

No push, CI build, server, telemetry module import, capture, merge, deployment,
or readiness was performed as part of this source-only wiring. Runtime and
Rust qualification require separate resource/host admission and exact-HEAD
source review. ADR numbering on frozen source is unchanged by speculative
installer/federation landing; integration renumbering must carry a new identity.

Exact review of local harness `1f3b9333` blocked source readiness on three
concrete gaps: original-history sampled exports were absent from the stock
module's exact inventory; the receiver's standalone 300-second timer could stop
before the joint app; and the executed joint helper was missing from the
before/after byte manifest. That checkpoint was never pushed or activated.

The repair rebinds to app `caa8138cad185530b4c5ddb17b5082502f21bb01`,
tree `94fd6cdc6f5e7518feb72f004402cd04e53f36cf`, parent `c8253708`.
Only the receiver donor changes: SHA-256
`aa3e704f4cf1ac356f130e8e64ed2733c7b253b9d927dd6544b301065b5790fd`.
The JS module and final-assertion hashes remain unchanged. The standalone
receiver keeps its 300-second default; joint ownership explicitly chooses its
validated 650-second maximum, covering setup, the existing 500-second driver
wait and normal app/drain cleanup without increasing those outer bounds.
Expiry persists a capture error. The harness checks actual `is_serving()` and
`expired` before startup and after normal app completion while still inside the
receiver context. The receiver's shared four-second join budget begins **after
synchronous `server.shutdown()`**, not as an end-to-end teardown guarantee.
The outer case deadline remains necessary.

An independent original-page listener attaches before any history navigation,
retains bounded actual report POST bodies and collector statuses, and records
actual finite/SSE request open IDs plus response trace headers. It remains
active through browser closure so late beacons cannot be silently excluded.
After closure and before app stop, privacy-validated reports and independently
matched request/trace/SSE correlations join the stock journey inventory using
the unchanged exported projection functions. The union is bounded to 64,
unique, browser-derived and explicitly scoped; no receiver output is used to
invent expected journeys. Missing reports/statuses/correlations, rejected
collector responses, duplicate identities and observer errors fail closed.
The final unchanged assertion still rejects any additional receiver view span
that was not independently captured. Natural sampling is unchanged.

The before/after manifest now covers both the joint helper and the existing
post-fault baseline helper as well as their importers and Python owner. The
joint receipt also hashes its own helper bytes. Helper mutation, receiver early
expiry, original-view inventory/correlation omissions, late capture and cleanup
ordering have compiler-free negative regressions. These are source/unit guards,
not executed app, receiver, Rust or lifecycle readiness receipts.

Rereview of local `e552ff5d` found a report-local trace provenance gap: a trace
observed for another history journey could satisfy the global captured-record
lookup. The follow-up requires a matching finite request record owned by the
current report whose request ID is also explicitly correlated in that report.
A two-request UNIT regression accepts the report's own trace and rejects both
borrowing another journey's trace and adding that foreign request ID alongside
it. The blocked checkpoint was not pushed or activated.

## Actual-main source advance for the single hosted attempt

The source candidate advances to `77ab7a42db1a74f6eb841bcf0c9c4669fb9ce4b0`,
tree `b1ed4d13258e94d43ac533731b0c5c28fcdeaf38`, with exact parents
`caa8138cad185530b4c5ddb17b5082502f21bb01` and actual pinned main
`7af06a4004b97b534d468b6ca2f42c145c8d4fb9`. The merge has no textual
conflicts. Four imported files are byte-identical to that main: retirement
cleanup, production Bash waiter/reader-drain extraction, and owned-home E2E
path canonicalization. Source audit verifies reader completion still precedes
terminal transition and waiter release; this is production code, not a test-only
change. No observer, receiver, opening, ADR or migration delta is introduced by
this base advance. All three donor hashes remain unchanged. ADR 101/102 remains
unchanged until an actual subsequent landing requires a separately identified
renumbering.

The existing Ubuntu-hosted instrumented job adds one full pinned-app UI lane
step: `PHOENIX_CHECK_ALL=1 PHOENIX_CHECK_WORKERS=2 ./dev.py check --lanes tsc,ui-lint,vitest`.
It uses standard full-suite scheduling/configuration and tsc/eslint, not a
render-fixture-only rerun or no-file-parallelism override. It gates capture,
retains the full console log even on failure, and binds command/exit status,
app/tree/base/harness plus Vitest-config/lock hashes in a separate receipt.
Counts are authoritative in the actual retained log, never invented by the
receipt. The standard harness UI job alone is not pinned-app qualification.
The previously failing return-navigation fixture and reaction tests remain in
the unfiltered suite; no causal repair is claimed before that hosted execution.
Genuine codegen, all eleven instrumentation guards with one ignored driver,
actual ordinary/Global joint capture/export/drain, and binary/build/module
receipts remain required by the same existing job. Existing wall budgets are
unchanged, and no retry-until-green behavior is added.

This packet is source preparation only. Local restricted dev.py checks,
rustfmt parsing, Python source compilation and compiler-free harness tests do
not execute the six Rust guards or provide full-app UI/runtime receipts. A
single exact-head hosted push/run requires coordinator admission; no local
build, capture or deployment was performed during base integration.

Exact review of preparation `646023e5` caught that `ui` is a CI group name,
not an accepted dev.py lane alias. The corrected command explicitly names
`tsc,ui-lint,vitest`: full app UI type/style checks and **all** Vitest tests,
not the ancillary ast-grep/Allium/spec group. A pure source-extracted regression
executes the exact app's lane resolver, accepts that explicit list and rejects
`ui`, without invoking lane setup/build/tests. The invalid preparation was never
pushed. Counts/log/exit and scope receipts use the corrected command.


## Causal diagnostic source integration after the joint telemetry failure

Retained failure identity: application `2f57bd1ba22569a01381d63117480cd946b6bd8d`,
harness `614b5d936332dc84649f588425941c805bfe3793`, run `38645213102`,
instrumented job `116130254846`, artifact `11933141719`. Source gates passed,
including genuine codegen, the actual one-test Coordinator DB fixture, eleven
instrumentation guards with one ignored driver, and the full pinned-app UI
suite. Ordinary history completed nine checks before the first telemetry trial
failed. Global did not execute; forced cleanup is not normal shutdown proof.
The separate standard Rust job is `116130356911`, terminal success at
2026-10-14 22:28:53 UTC; the previously transcribed `116130359816` is invalid.

The retained initial trial contains one correlated request, zero POSTs, no
reports, and generic visible-readiness latency 1168.533933 ms. Its primary
missing-report assertion was masked by a later finally POST-count assertion.
Request correlation IDs can exist independently of producer sampling, so the
old request-count sampling oracle is invalid. The original artifact lacks the
producer admission/marks and detailed request facts needed to establish the
cause of zero reports. It remains causal-unknown, not a demonstrated sampling,
readiness, or transport bug.

Telemetry donor delta `4d1c1009293e253bb562c8662afc38259fc2f79b` was integrated
as application `4e615ea3513bac5ee6f4cff797ec17a7bcb3dac0`, tree
`8535ae2edc92c4333e0e86cf85a7e1e5f89feaf0`, directly atop the retained `2f57bd1`
application. The pinned `7af06a4` main base and cfg-test initializer correction
remain present. The nine-file donor delta adds bounded content-free producer
admission/mark/transport diagnostics, request-object-associated facts, and
independent primary/cleanup error retention. Natural 0.25 sampling and readiness
semantics are unchanged. Close-time producer delivery remains `not_proven`.

The new relative-import dependency `ui/scripts/view-telemetry-diagnostics.mjs`
is included by the exact application checkout, explicitly required by the
workflow, included in the four-module codegen/runtime identity hashes, and
attested alongside the donor before import and after the stage/context drain.
Missing, symlinked, or changed helper bytes fail closed. Its focused test is
explicitly discovered; no separate donor checkout is imported at runtime.

Owned Python lifecycle glue preserves the original exception across nested
process/receiver exits, retains up to eight separate cleanup errors with
bounded messages, and writes distinct forced-stop observations. A forced
return code zero does not overwrite the normal-completion gate. Main-level
regressions cover partial receipt writes and final-summary publication errors:
invalid evidence is explicit and cannot mask the primary exception or promote
success. The shared Node stage also retains its primary error when context
cleanup fails and records the executed dependency manifest on failed stages.

Actual source-only checks on the integrated candidate: shipping Python 16,
base Node 14, opening Python 41, opening Node 29, joint Node 48, and focused
application Vitest 86 tests across seven files all passed; changed producer
TypeScript ESLint and Node syntax/diff checks passed. The first local pnpm
focused-test launch aborted before tests when it attempted dependency
maintenance (`ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`). It is retained
separately. The passing focused ring used existing installed binaries through
the verified dev.py Node environment without reinstalling or purging modules.
Pre-existing SQLite fixture ResourceWarnings remain visible in Python logs.

Exact integrated review additionally reproduced three boundary gaps: a failed
normal-completion receipt could replace a shutdown failure, the outer browser
cleanup could overwrite the primary browser error, and import/context admission
failures could omit the dependency manifest. The successor requires the shared
failure owner at normal completion, retains bounded outer cleanup facts without
rewriting the primary, and owns manifest publication before import/context
acquisition. Regressions exercise each boundary, including acquisition failure
without claiming module checks executed. One intermediate regression used an
incorrect type oracle (`TimeoutError` is an `OSError` subclass); its failed log
is retained and the corrected exact-type regression passes.

These are source/mock checks, not a new full UI, Rust/codegen, normal-shutdown,
export, Global, or overhead qualification. No hosted publication, rerun,
production mutation, or browser capture follows automatically from this packet.


## Authenticated disposable-fixture contract repair after run 38657260015

The admitted exact application `4e615ea` and harness `8447e887` were published
once. Run `38657260015`, instrumented job `116169860707`, failed at the isolated
collector authentication negative with actual status 204 rather than 401.
Artifact `11938370840` is retained separately from the earlier `614` failure.
Codegen, actual Coordinator DB fixture, instrumentation guards, and full pinned
UI passed. Ordinary history completed nine checks; the stock telemetry sweep
completed 48 trials (24 ordinary, 24 Global), with 16 sampled journeys and 16
POSTs and no per-trial failures. Those Global telemetry trials do not establish
execution of the outer Global-history case, which was not reached. Forced
cleanup remains failure evidence, not normal shutdown proof.

Source diagnosis establishes a fixture mismatch: the ignored cfg-test driver
scrubbed every environment variable, wrote empty configuration, and never set
`PHOENIX_PASSWORD`. The server's auth-disabled allow-all behavior is required
by `specs/auth/requirements.md` REQ-AUTH-001. The collector is inside the normal
auth layer; this result is not evidence of a product collector exemption.
The donor authentication-negative contract requires an authenticated server.

The source successor application `6e05de110b0cdcf075448d363f82be7a21fa26fc`,
tree `3f2ce53714f1f44dbc2d50d219e81692f07a6771`, retains `4e615ea` and applies
only donor preflight delta `107555e6b3ff451852a070ebbe3d542a0a5a1d40`
(cherry-picked as `fbd8b831`) plus the owned cfg-test authentication fixture.
After the full ambient environment scrub, the fixture generates its own random
UUID password, stores it using exclusive creation and mode 0600 under the owned
mode-0700 temporary HOME, and sets the server password from that synthetic
value. The startup receipt exposes only the credential file path, never its
contents. No production/inherited password or bearer credential is imported.

Parent HTTP, model discovery, and SSE use the same existing-login session
cookie. The parent first requires auth-required/unauthenticated status, logs in
through `/api/auth/login`, and requires authenticated status. Readiness uses the
public auth-status endpoint. Both the original browser context and the fresh
stock context mint sessions through the existing login endpoint before page
observers/navigation. Direct driver HTTP/SSE requests use the same context's
cookie; no credential or session value is written to capture receipts. Owned
file checks reject foreign paths, symlinks, wrong ownership/permissions and
invalid synthetic credential content. Shared shipping qualification is unchanged.

The donor checks the authenticated parent prerequisite before the first cohort.
The negative context remains empty-storage with no bearer/session injection and
must first prove auth-required/unauthenticated status, then receive the original
401 collector rejection. Auth-disabled fixtures fail early; 204 is not accepted
or skipped. Sampling, readiness, export assertions, and content-free diagnostic
bounds are unchanged. Close-time producer delivery remains `not_proven`.

Source-only checks: shipping Python 16, base Node 14, opening Python 45, opening
Node 30, joint Node 50, and exact successor focused application Vitest 90 tests
across seven files passed. Node syntax, Rust source-format parsing, and diff
checks passed. Initial local mock failures (missing imports/helper identifiers)
are retained separately from the corrected passing logs. No Rust compilation,
codegen, full UI, authenticated runtime, normal shutdown, Global-history,
export/drain, or overhead acceptance is transferred to this successor. It has
not been published or rerun merely by preparing this source packet.

Auth-successor exact review found two source-level blockers before publication:
the donor expected-hash constant did not match the actual pinned Git blob, and
Playwright authenticated auth-status transport errors could include the session
cookie in their call logs. The shared login boundary now sanitizes status-GET
transport failures. A new unmocked regression hashes the actual pinned Git blobs
and checkout bytes against both consumers, rather than trusting mocked hashes.
The corresponding donor transport boundary remains telemetry-owned. These
findings are not hosted runtime results and do not authorize another attempt.
The previous run's standard Rust job `116169938486` has independently completed
successfully; whole run `38657260015` is completed failure. Its watch terminated
with exit status 1, as expected for that overall failed run.

The donor privacy correction `6d6218f0ff08a19e774e0d92a552a2331990ca1c`
is integrated as application `d9f46a58c1c50ff83dbb5b2c2afe7ad5e15c7deb`,
tree `f24d93471e201d010f0657ce1712a3a04a02b6fd`, directly atop `6e05de1`.
It replaces raw Playwright transport/disposal/context errors at every donor
APIRequest boundary with fixed labels, preserving primary rejection assertions
and separate sanitized cleanup errors. The shared login helper also sanitizes
status transport and disposal failures and retains its separate disposal error
through the original browser result and fresh-context manifest. No original
transport message, stack, or cause is attached. Cookie/Authorization call-log
canaries cover these failure paths; the original actual-Playwright no-network
review reproduction remains separate evidence from mocked regressions.

Final source-only ring for this integrated privacy successor: 16 shipping
Python, 14 base Node, 46 opening Python (including actual pinned Git blob/checkout
hashes), 30 opening Node, 50 joint Node, and 93 focused application Vitest tests
across seven files passed. Syntax, Rust source-format parsing, and diff checks
passed. Actual authenticated runtime, full pinned UI, genuine codegen/DB fixture/
guards, export/drain and normal shutdown remain owed to a separately admitted
hosted attempt. The earlier failed artifacts and local failed regression logs
remain intact; there is no automatic retry/publication.


## Explicit private-HOME source repair after run 38660832205

One admitted app `d9f46a5` source-only publication and harness `925f75ce` push
created exact-head run `38660832205`. Instrumented job `116181400157` failed
before browser execution with `owned synthetic credential file identity mismatch`.
Artifact `11939500869` is retained. Genuine codegen, the actual Coordinator DB
fixture, guards, compiler-free checks and full pinned UI passed. There is no
browser/telemetry result and no outer Global-history execution in this attempt.
Normal shutdown was not proven; force-stop observations remain distinct.
The separate standard Rust job `116181483963` was still running at this source
packet's preparation; it must be refreshed rather than inferred from prior runs.

The failed artifact does not contain credential stat facts, so the actual
rejected predicate cannot be recovered from it. Source inspection identified a
producer omission: the locked `tempfile 3.27.0` directory creator uses default
`DirBuilder` permissions unless supplied, while the consumer requires HOME mode
0700. Existing Python fixtures explicitly created mode 0700 and did not expose
that omission. Application `06156cc05ce3636cd09b57c43e99dc3ff1f92724`, tree
`1b4f4ab7f79d1f37c8f6ee74a76185c1a04c62e9`, directly atop `d9f46a5`, adds exactly
one line requesting `Permissions::from_mode(0o700)` before `tempdir_in`. It does
not loosen the consumer, change authentication, inherit secrets, or alter the
telemetry modules and their hashes.

The Python consumer writes bounded content-free identity predicates, modes and
size to `credential-identity.json` before rejection. A mode-0755 fixture must
fail specifically `home_mode_0700` while all other predicates remain true; the
receipt must not contain the password. Valid mode-0700 and existing wrong-file,
symlink, external-path and invalid-content controls remain. A source assertion
requires the exact explicit private-directory builder sequence. This is source
and filesystem-mock coverage, not execution of the Rust builder under a hosted
umask or proof that permissions were the failed artifact's only mismatch.

Actual successor checks passed: shipping Python 16, base Node 14, opening Python
46 (including real pinned module-byte hashes), opening Node 30, joint Node 50,
Node syntax, Rust source-format parsing and diff checks. The app UI/telemetry
files are byte-unchanged from `d9f46a5`; prior source tests are not a new Rust or
runtime qualification. No successor hosted publication/rerun is automatic.

Separately, owner reconciliation found two stale Oct12 local Ladle services with
no consumer in this commission. Their exact node CLI PID/start/argv/PGID/cwd
identities were recorded before normal SIGINT to PIDs 20336 and 81788. All their
Node/esbuild descendants and port 61237 listener were absent afterward; idle
zsh wrappers 20330 and 81783 remained. No source/evidence was deleted and no
protected prototype or unrelated service was signalled. These local service
receipts do not qualify the hosted application's normal shutdown.


## Pagehide diagnostic successor after run 38779812212

The one exact app `06156cc` / harness `24398672` publication produced run
`38779812212`, terminal failure in witness `116576778604`; standard Rust
`116576973076` and other CI jobs passed. Artifact `11996596997` remains retained
remotely. Bounded ZIP range reads retained only selected small JSON receipts,
not the approximately 89 MB artifact archive. The identity receipt establishes
all nine predicates true, HOME mode 0700 and credential mode 0600. Ordinary
history completed nine checks. Stock telemetry completed 48 trials with eight
sampled journeys and eight POSTs; those are not outer Global-history execution.

The pagehide trial observed one incomplete report body and matching request
correlation but no page response status. Its original assertion was actual
statuses `[]` versus expected `[204]`. The donor finally then masked that
failure with `pagehide late capture failure`. A captured body does not prove
collector acceptance. The retained evidence cannot distinguish failed transport
from a missing observer response; no causal transport fix is claimed.

Donor `e75512a094734b294a6dc221b9552f1c18654d5e` is integrated atop `06156cc`
as app `debe25378d5a460c18d1ac2a01e3ebe309da236b`, tree
`c9e6eddf38044f8c59e496222d1c57a18a4c8e12`. The two-file delta preserves the
first Error through context close, cleanup assertions, callback and receipt
publication, and records independent bounded secondary failures. At most 32
content-free ordered page/context/CDP transport facts distinguish actual
Request-local ordinals and separate CDP-local ordinals without claiming
cross-channel identity. Raw CDP IDs, URLs, error text, headers and bodies are
not copied into those facts. Overflow fails closed. Observer phases and context
close return do not imply normal application shutdown.

The existing page-response `[204]`, request correlation, incomplete outcome,
null meaningful-transcript, sampling, timing and export assertions are unchanged.
The successor is diagnostic: context/CDP observations cannot replace missing
page-response acceptance evidence. Actual cached offline checks passed: shipping
Python 16, base Node 14, opening Python 46, opening Node 30, joint Node 50 and
focused donor Vitest 54 across two files. No local compile, install, application
server, browser capture or large artifact copy was performed. New-source full
UI/codegen/Rust/runtime/normal drain/export remain hosted obligations, not
inferred from those source and mock checks.


## Producer-admitted pagehide classification after run 38807478274

Diagnostic app `debe253` / harness `532112bc` produced witness `116671957805`
failure in run `38807478274`, artifact `12007392732`. Selected small receipts
were retained using 53,289 bytes of ZIP range reads. The original assertion now
survives: `sampled pagehide must report once`, actual zero versus one expected.
This trial has no report, no response status and only page/context-close facts;
that does not prove an attempted or failed transport. Stock telemetry completed
48 trials, 13 sampled/13 POSTs, and all seven collector negatives including 401.
Those observations remain distinct from whole-case acceptance and the earlier
243 trial with a report body but missing response 204.

Exact donor `50c8fbbab918b15dcf5c149c5cb091c108396567` is integrated as app
`e84f78faa572da6156d541360ce6ccb96bb11eb8`, tree
`6a5fa5017bf2315892ca06cea7446678c237123b`, directly atop `debe253`. Pagehide
installs the existing bounded read-only producer diagnostic contract before page
creation and classifies sampling from its explicit admission. A sampled finite
request UUID must match the producer request-association event, and the report
must match that admission. Missing, invalid, overflowing or foreign admission
fails closed. A genuinely unsampled correlated GET remains inconclusive, with
no forced pagehide or report; a sampled journey without a POST still fails.
The 64-event cap and existing 32-fact transport cap are retained. Observer phases
do not promise complete close-time delivery or normal application shutdown.

The old correlated-GET sampling assignment is removed. Other lifecycle probes
use finite GETs only to establish an actual pending request and still demand a
correlated report and 204; absence-only labels now say `sampling_unknown` rather
than imply an unobserved sampling decision. No sampling force, readiness change,
timeout increase, response-status relaxation or export exemption is introduced.
The earlier report-body-without-204 case remains causally unresolved.

Actual cached offline checks passed: shipping Python 16, base Node 14, opening
Python 46, opening Node 30, joint Node 50 and donor Vitest 59 across two files,
plus syntax and diff checks. These include genuine-unsampled correlated GET,
missing admission, foreign request UUID, overflow, sampled/no-POST and missing
response controls. No local compile/install/server/capture or large copy was
performed. New-source full UI/Rust/runtime/drain/export acceptance remains owed.

Exact review of the admission successor found one bounded-lifecycle gap: newly
installed binding/init-script awaits preceded the case's absolute-deadline
operation. Donor correction `af5d0ba5d15aaab7131995ce53ab8e15c3381435`, integrated
as app `89a9e8ec5bf7779bb2dff4c145e76844803a47ff`, tree
`542bbc877ae5cd3fdbfefb13b762d660220e78ae`, starts that same operation before
installation, without increasing any deadline, and rejects late installation
completion before page admission. Fake-clock controls stall each install await:
the original timeout survives, the context closes once, the trial receipt is
emitted, and late release cannot create a page. Actual cached offline checks
passed again: 16 shipping Python, 14 base Node, 46 opening Python, 30 opening
Node, 50 joint Node and 61 donor tests across two files. The prior uncorrected
admission packet was not published; these remain source/mock checks, not runtime
acceptance or proof of the earlier missing response's cause.


## Synchronous keepalive dispatch successor after run 38810704870

Harness `95707d666b4529bbaf849430788bbf7eb099cd7d` / app `89a9e8e`
completed run `38810704870` with witness `116682903480` failure. Standard Rust
`116683030501` and all other standard check jobs succeeded separately; this is
not joint acceptance. Artifact `12009500583` remains remote and preserved.
Selected receipts were inspected in memory with 54,735 bytes of ZIP range reads,
not a full artifact audit. Trial zero explicitly admitted unsampled and correctly
remained inconclusive. Trial one explicitly admitted sampled, bound its request
and incomplete report to the same journey, and produced one report with no page
response status. Producer attempted then failed submission; page/context request
failure occurred during pagehide navigation before context cleanup. A separate
CDP-local POST observation recorded canceled=true. Channel-local ordinals are
not cross-channel identity. These facts establish actual pre-cleanup transport
failure, not which actor canceled it. The strict `[] != [204]` assertion remains
primary, with late cleanup capture failure separate.

Source already used keepalive. Lifecycle cleanup does not abort fetch; the
reporter's three-second timer is its sole explicit abort. Donor `05513cb` removes
promise-microtask deferral of fetch initiation: dispatch starts inside report,
while completion stays asynchronous. Synchronous exceptions become handled,
sanitized rejection. A finite `timeout_abort` diagnostic immediately precedes
the explicit abort, and the existing helper parser admits that fixed event.
No retry, timeout increase, forced sampling, readiness relaxation, status
substitution, session change or normal-shutdown exemption was introduced.
Removing the deferral is a causal-order correction/hypothesis, not a proven
explanation or native-browser delivery fix for the failed run.

Exact donor deltas `05513cb0638e9969928cf41919bd505fe87cdca6`,
`8eac8010d15e7a799e5caef9b67b1a273bb49c98`, and
`10eaa7273dd5e778ccf01f8148793e1998a391a9` are integrated atop app `89a9e8e`
as `f9355ade31138f06229808d8d5ca5a0d0f1df74b`, tree
`39b3306f93d1d68807c4e0f77f1f0223383a906b`. The latter two are test-only.
Independent donor review found no introduced production defect, but caught
assertions inside the injected send that the reporter could swallow; the final
test correction asserts options outside that catch before any await. Injected
controls prove dispatch before handler return, handled synchronous throws and
already-rejected promises, unchanged busy-drop and failure counters, reusable
inflight slot, cleared timers on settlement, no false timeout after ten seconds,
no private failure text and exact timeout-event-before-abort ordering. They do
not simulate native teardown delivery.

Actual cached offline validation: shipping Python 16, base Node 14, opening
Python 46, opening Node 30, joint Node 50, and focused telemetry Vitest 105 across
six files passed. A pre-pin opening check correctly failed its actual-byte hash
regression when the helper changed; its failed log is retained. Rebinding the
actual app HEAD/tree and helper SHA made the final check pass. Syntax/diff checks
passed. No local compile, install, application server, browser capture, cache
cleanup or large artifact copy was performed. Full new-source hosted codegen,
UI/Rust, ordinary and outer Global history, export inventory, normal drain and
overhead qualification remain owed; no joint acceptance is claimed.


## Fixture settlement successor after run 38832404299

Exact published harness `e4e9166303914b8b025f88c757920512d8d9ecac` / app
`f9355ade31138f06229808d8d5ca5a0d0f1df74b` completed run `38832404299`
with witness `116753898790` failure; standard Rust `116753978935` and all
other standard jobs succeeded. Browser/pagehide qualification was not reached.
Artifact `12018217312` (89,009,915 bytes) remains remote and preserved. Two
bounded ZIP reads fetched 20,241 then 343,715 bytes; selected final-five API/SSE
records, runtime log, lifecycle and completion receipts were retained, not the
DB, binary, credential file or complete artifact. The completed job log was
also retained. Python failed at `other = fixture.build(other_kind, other_cwd)`:
Global chapter-one turn 20 returned `{queued: true, steering: true}` and the
strict non-steering assertion failed. The preceding persisted GET was Idle
with the prior canonical user and agent. Runtime's fatal-authority warning
followed the failed POST and forced-cleanup request, so is not evidence of an
earlier application failure causing steering. Forced cleanup is not normal
shutdown evidence.

Source identifies a missing fixture completion boundary, without uniquely
attributing the actual steering OR-branch: direct send admission checks live
effective state, queued steering and active Runtime ownership. GET persisted
Idle plus agent evidence does not establish those predicates. This failure
supplies no new native evidence about synchronous keepalive delivery.

The successor strengthens the existing bounded condition wait with an optional
qualification-owned completion predicate; shipping's default wait behavior and
strict non-steering assertion remain unchanged. It adds no fixed delay, elapsed
sleep, resend attempt or second wait loop. The existing wait's polling/backoff
and deadline remain unchanged. Before returning successful completion, the
opening qualifier requires a read-only transaction on the verified isolated DB:
matching instance identity, the exact persisted canonical user's Runtime turn
terminal and ownership released, no active Runtime owner and no queued steering.
Normal completion additionally requires the existing authenticated message
reconciliation endpoint to confirm that exact message is persisted and live
`conversation_idle` is true. This public boolean shares chat's effective-state
source but is not alone sufficient; the DB checks cover the other admission
fences. Context-exhausted completion requires the existing persisted exhausted
state and exact Failed terminal/released ownership, not live Idle. Exact app
`ExecutorRuntime::terminal_obligation_for_state` and continuation settlement
classify ContextExhausted as Failed even when summary generation succeeds.
Opening/continuation waits without a caller-supplied chat ID resolve the actual
persisted user ID. Direct-turn openings require the same exact terminal lookup.
Ordinary creation instead has its own authority: exact app
`materialize_conversation_creation_runtime` / `settle_conversation_creation_runtime`
use `conversation_creation_jobs`, not a Runtime durable-turn row. Its correlated
message must have a ready job with worker/token/lease cleared; the persisted
Idle and public effective Idle conditions still apply. Missing user, foreign
identity, retained claim, failed job or non-completed direct opening cannot pass. This is an isolated single
fixture writer's quiescence observer, not an atomic public admission guarantee
under unrelated concurrent producers. No product endpoint, producer, cfg-test
control signal or production database change is introduced.

Deterministic mocked-runtime/owned-DB regressions hold durable ownership after
persisted Idle/agent evidence, then independently release ownership and live
state. Corrected wait returns only after both; original e4e wait fails the same
test with one read instead of three. Mutation evidence is retained, using the
original wait in memory without editing source; elapsed sleeps are mocked out.
Other controls cover absent/foreign exact canonical identity, terminal failure,
a new active owner, backlog, missing/schema-error/changed-identity DB, and
opening/exhausted paths. Actual offline ring: shipping Python 16, opening Python
53, base Node 14, opening Node 30, joint Node 50 passed. An initial bypassed-
constructor shipping test failed on the new member; its test fixture now sets
the constructor-owned default explicitly. Failed intermediate logs remain
retained. These are bounded readiness tests, not hosted or joint acceptance.

Before publication, review left real creation-ID tracing explicitly unverified.
Parent source follow-through caught that ordinary creation uses its separate job
authority, so the initially frozen uniform-Runtime observer `140da572b` was not
published. The correction adds only the correlated ready-job branch for waits
without a supplied chat ID and tests raw/canonical opening IDs and cleared
claims. An explicit chat ID never falls back to a creation job. Earlier 52-test
logs and mutation proof remain retained separately from the final 53-test ring.


## Product-creation authority correction after run 38835967561

Harness `11cdc0f7c5afab85c5ae358ec105568532edadf6` / unchanged app `f9355ade`
produced witness `116765054229` failure in run `38835967561`, timing out on the
very first ordinary `Fixture.build` opening, before browser qualification. At
inspection, standard Rust `116765139606` remained running and uncancelled; other
standard jobs passed. Artifact `12019671980` (88,607,696 bytes) remains remote.
Two bounded 42,330-byte range reads retained first/final-five API/SSE records and
runtime facts, not the DB, executable, credential or entire artifact. The API
records show POST `/api/product-conversations/new` with `request_id` equal to
the actual raw persisted user message ID. Idle plus agent then repeated without
reaching reconciliation.

This is an introduced harness bug: the preceding parent/reviewer source trace
followed the **legacy** conversation creation endpoint and its
`conversation_creation_jobs` authority, not the actual shipping fixture's
product-creation endpoint. That earlier conclusion is explicitly retracted for
this fixture. Its failed run, reviewed commits and receipts are preserved.

The correction **replaces**, rather than supplements, the wrong branch with
`product_creation_jobs`: exact `request_id` equals the persisted opening user ID,
exact `published_conversation_id` equals the fixture row, status `published`, and
worker/token/`claim_lease_until_unix_micros` all cleared. Explicit chat IDs still
cannot fall back to creation. `create_product_conversation` dispatches through
`process_product_creation_request`; the creation worker delivers a SteerMessage
whose message ID is `job.request_id`. Product creation DB delivery completion
then atomically changes delivery_pending to published and clears the claim
under its live claim CAS. Public effective Idle and no Runtime owner/backlog are
still required; published alone does not establish runtime completion. No
legacy-table fallback, timeout increase, added sleep, loop or retry exists.
A bounded latest-evaluation `fixture-completion.json` records only opaque IDs,
boolean ownership/backlog/publication facts, finite terminal kind and nullable
live Idle, never claim tokens, errors, bodies or credentials.

The corrected regression executes the SAME shipping `Fixture.build('ordinary')`
POST and response path, using the complete product-creation table DDL read from
the exact pinned app migration, not hand-selected substitute columns. It holds
delivery claim, rejects wrong request and wrong published row, releases the
matching publication and separately holds live runtime Idle before advancing
to the first fixture send. One creation request is asserted. The actual schema
rejects a published row with retained claim by CHECK; the observer also demands
all claim fields be null. The original 11cdc observer fails this same-path test
on its nonexistent legacy table in memory, without source edits. Current ring:
54 opening Python, 16 shipping Python and 94 Node tests passed. Initial test
import/name errors are retained separately. These checks establish the fixture
correction, not native pagehide or joint acceptance.


## Reviewed collector/export acceptance implementation

Run `38838103037` at harness `1c7b24a` / app `f9355ade` completed with witness
`116771663445` failure and standard Rust `116771738701` success. Setup reached
browser qualification: nine history checks, 41 stock trials, 17 sampled/17 POSTs,
and all seven collector-negative statuses matched. The sampled pagehide again
reported once with no browser 204, producer rejection and pre-cleanup CDP
cancellation. There was no captured timeout-abort event. This falsifies
synchronous dispatch as a sufficient remedy, not the exact transport actor.
Artifact `12020760357` remains remote; selected receipts used 53,236 range bytes.
No outer Global/export/overhead acceptance followed that failure.

A separate push-only synthetic experiment at source
`4ee8d17613a0da0eac4ef322fe51877fd90341ad` completed run `38842011920`,
job `116784022706`, operation success. All four cells (CDP 0/200 ms × blank/
same-origin departure) had server body completion/204 response finish while the
browser reported rejection/request failure/CDP cancellation. No cell had a
browser 204 or captured own timeout abort. The 2,052-byte artifact `12021448843`
was fully inspected; this was not application acceptance or export evidence.
The probe's two late-operation lifecycle findings were fixed before execution.

The user explicitly selected **Collector + export proof**, with contract review
before a gate change or rerun. The original departing-browser 204 oracle came
from donor `22e73bffa` and `3726c1d42`, not a normative reliable-delivery promise:
REQ-CVT-004 requires best-effort nonblocking keepalive without retries/queues.
The reviewed REQ-CVT-006 / ADR-107 distinguishes that transport contract from
candidate delivery proof. Exact contract packets `2dba157b` and `047b089f` were
reviewed independently; the latter fixes ADR-103 collision and preserves exact
browser-reported correlations, observed provenance and caps/drop accounting.
They were imported as docs-only commits before executable gate changes.

Donor implementation `a001fd275eeef688e3ec6b6690003151d3f039e9` is imported
as app `194e42b7e7730ae163aa494193a55c81ff914b87`, tree
`6d09383d282006b34f8c8aedb4fbb871d9dea67d`, atop frozen `7af06a4`.
A real authenticated, validated handler completion emits the fixed six-field
INFO acceptance witness after span/children creation, immediately before 204.
The pagehide browser stage is provisional; an observed non-204 or duplicate
response still fails, and nondeparting browser-204 requirements stay unchanged.
Source review found no introduced issue. A real-router test covers 401/400/422
with no witness and repeated successful POSTs with two witnesses (no dedupe).
This Rust test is source-reviewed but **not locally compiled or executed**; the
existing hosted selected libtest guard explicitly executes its exact name,
retains a separate log and records success in the build receipt. Harness source
admission rejects a missing/failed/wrong-name collector-test receipt.

Shared collector projection rejects malformed claimed events, unknown fields,
noncanonical identities, boolean version/status, duplicates, cross-source or
instance witnesses and byte/line/count overflow. It preserves only content-free
allowlisted facts, not JSON envelope spans or arbitrary runtime log content.
After normal app-first/receiver-second drain and mandatory raw-protobuf/index
reconciliation plus semantic retrieval, the independent full browser inventory
must exactly equal both collector and actual-export occurrence inventories.
Missing either half, foreign/extra/duplicate identity, failed drain or wrong
outcome/trace prevents the separate `telemetry-delivery.json` seal. It never
changes browser failures to success or grants joint/overhead acceptance.

Actual cached offline ring on this integration: 65 opening Python, 16 shipping
Python, 94 Node selftests and **111 focused telemetry tests across seven files**
passed, with syntax/YAML/diff checks. The log named `focused115` contains the
actual 111 count, not 115. Serialized-protobuf/corrupt-retained-raw controls prove
export proof cannot be replaced by a collector log. Earlier intermediate wrong
new-test function-name and patch-placement errors were corrected before this
ring; failed logs remain retained. No local Rust build, server/listener/browser,
installation, cache cleanup, deployment or production action was performed.
Full new-source hosted qualification and actual collector/export join remain
owed; no current candidate/joint or overhead acceptance is claimed.


## Required-body navigation ordering after run 38844570559

Harness `42508a870a2a357c993e9c3f978ef239358ee04d` / app `194e42b7`
produced witness `116792049706` failure in run `38844570559` before stock or
pagehide qualification. The new real-router collector witness unit actually
compiled and executed successfully (one test); codegen, the existing opening
guards and pinned full UI also passed. This is not collector/export acceptance.
At inspection standard Rust `116792137738` was running and uncancelled.
Artifact `12022663186` (89,658,923 bytes) remains remote and protected.

The first required-capture failure was ordinal 58, scope one: GET request at
2051.115 ms, next pending-historical navigation intent 2118.116, response headers
HTTP 200 at 2120.248, then CDP body retrieval failed with no resource identifier.
The new document confirmed at 2136.240. A later successful document/body does
not repair the missing old actual body. Primary capture error remains distinct
from cleanup. Selected result/runtime/unit receipts used 119,573 range bytes;
additional bounded request/response/navigation records retain the exact order.

The successor changes ordering, not evidence policy: a recorder-owned bounded
live drain waits for already-admitted required headers, actual body reads and
validation/commit tasks before synchronously starting the explicit navigation.
It recomputes live pending work and includes new admissions during the wait.
Both explicit driver navigations use this wrapper. One absolute existing
11-second cap applies without reset; close, deadline, wrong status/identity or
body failure rejects without invoking goto. Listeners stay installed; final
seal/drain is mutually exclusive. Actual document confirmation and strict
current-document aggregate capture remain required. A request arriving at the
handoff remains required and its body loss still fails. This reduces the
observed race; it does not prove the renderer cannot create future requests.
No No-resource/reading-body supersession exception, retry, alternate fetch,
quiet-period sleep, sealed admission or successor-body substitution was added.

Deterministic controls hold old headers/body, admit a second required request,
and require both exact raw/hash captures before goto; they cover timeout with
non-sliding arrivals, page close, late completion, unavailable-body failure,
new handoff requests and both real driver callsites. All 35 opening Node tests
passed, including existing unavailable-body rejection controls. This ordering
repair is independent of the reviewed pagehide collector/export contract.


## Mounted SPA-control correction after run 38846448530

Harness `55b48731` / app `194e42b7` completed run `38846448530` with witness
`116797958302` failure and standard Rust `116798020980` success; all standard
jobs passed. History's nine checks had no capture/page errors. Stock telemetry
completed 48 trials, nine sampled/nine POSTs, and all seven negative probes.
The real owned collector log contained ten distinct success witnesses from the
actual candidate/instance, but final export/inventory/drain acceptance was not
reached. Both pagehide attempts were explicitly unsampled and remained
inconclusive, not delivery proof. Artifact `12024086061` remains remote;
207,113 range bytes retrieved selected receipts/runtime evidence.

The first failure was the SPA-cancel fixture's mounted anchor count zero. Source
tracing shows this is structural: ProductConversationPage renders its loading
skeleton before EmbeddedConversationPage/StateBar exists, while the fixture
must click during that initial pending snapshot. Waiting for the StateBar link
would finish the request and invalidate cancellation. The existing desktop
shell/sidebar mounts independently and already has a real home-navigation
button in expanded and collapsed layouts. No injected DOM, history mutation or
new product UI is needed.

Exact donor `5e15ac8242fc9b5838561d0aa083d3fb331c3aa0` is integrated as
app `e494c38c3ca164607236949aa240961f0a02931c`, tree
`474a84048f2250c1f121272dea13634945c4d77b`. The fixture replaces the unusable
anchor selector with a discriminated `spaControl: {kind: 'sidebar_home'}`;
there is no legacy-selector fallback or arbitrary button destination. Donor
owns the fixed existing expanded/collapsed selectors. Exactly one visible,
enabled, mounted BUTTON is required, followed by a real browser click, changed
same-origin home URL and unchanged document time origin. Pending correlated
request is rechecked immediately before click; actual canceled report and
nondeparting browser 204 remain mandatory. Pagehide's reviewed collector/export
contract, natural sampling and provisional status are unchanged.

Actual cached ring: 65 opening Python, 16 shipping Python, 99 Node selftests and
123 focused telemetry tests across seven files passed. Missing/duplicate/hidden/
disabled/wrong-element controls, cross-document navigation, wrong destination,
completed request, wrong ready outcome and rejected report remain failures.
This source/mock result does not prove the actual mounted path or candidate
export join; those remain hosted qualification obligations.


## Explicit lifecycle admission correction after run 38848525587

Harness `2d790f9a` / app `e494c38c` completed run `38848525587` with witness
`116804361665` failure and standard Rust `116804432524` success; all standard
jobs passed. Nine history checks and stock 48 trials (six sampled/six POSTs)
passed, as did seven negative controls. A naturally sampled pagehide emitted one
incomplete report and exactly one matching real collector acceptance witness;
no browser 204 was observed. It remains provisional: final semantic export join
and normal drain were not reached. Artifact `12023839353` remains remote, with
203,220 range bytes of selected evidence retained.

The first failure was a missing SPA-canceled report after real sidebar navigation.
The earlier interpretation as a potentially defective cancellation emission is
not established: those lifecycle trials recorded no explicit producer sampling
admission. A finite legacy GET open ID exists independently of sampling, so it
cannot prove a sampled journey. Source cleanup does invoke canceled finish on
real unmount, and explicit unsampled journeys correctly emit no report. Actual
prior sampling therefore remains unknown, not retroactively unsampled or passed.

Exact donors `33c8d9a50ba6e4a4f471ef8bbb8afcd5ddfa6496` and
`0a0a7c64cfd373ae2199381fcd1cc9ec8d731532` integrate as app
`9782694428ee7f9749de9ee514be59738663cab6`, tree
`85ed064ff77440d0592d712d57b54a867bd36046`. All lifecycle contexts install
bounded producer diagnostics inside the absolute case deadline before page
creation. Primary admission is explicit before actions: unsampled trials take no
sampled-only action and expect zero primary reports; sampled captures require
exact producer request association and bind any report to the admitted UUID.
Missing/foreign/overflow admission fails, as does a missing report after an
exercised sampled action. Successor/resume reports remain distinct from primary
admission and are retained. No producer behavior or sampling rate was changed.

Review caught readiness-first capture identity/mandatory-report inconsistency
and a late-CDP-emulation cancellation gap before import. The immutable correction
binds identity before branch selection; a readiness-first unexercised case may
have zero reports but any observed report must match, be unique and receive204.
Both final capture checks enforce that same policy. A canceled emulation cannot
install listeners or navigate after its deadline. Primary errors survive cleanup,
callback and receipt persistence errors. An independent reviewer also ran
in-memory full-stage readiness mocks covering late valid, foreign, duplicate,
missing-response and non-204 POSTs, and late emulation; no native run was claimed.

Actual cached integration ring: 65 opening Python, 16 shipping Python, 99 Node
selftests and 137 focused telemetry tests across seven files passed. Source/hash
admission is refreshed; no timeout, retry, forced sampling, capture exemption or
collector/export acceptance relaxation was added. The reviewed REQ-CVT-006 join
still requires the full independent report inventory, real collector witnesses,
actual semantic exports and normal app/receiver drain. New hosted evidence is
owed; previous unsampled/inconclusive observations are not delivery success.


## Full-UI disclosure test synchronization after run 38850583279

Exact harness `fa6140ec` / app `97826944` run `38850583279` completed with
witness `116810677154` failure and standard Rust `116810759426` success; all
other standard jobs passed. The witness stopped at the full pinned UI gate,
before application/browser qualification: 167 files/3,521 tests passed, two tests
skipped, and `MessageComponents.test.tsx` disclosure integration timed out at
5,000ms. Real collector unit, codegen and opening guards had passed. Artifact
`12025266655` (88,615,960 bytes) remains remote; 52,173 range bytes of selected
receipts were inspected. There is no new native sampling/delivery evidence.

The exact failing test and its entire 147-test file passed independently on the
unchanged source using cached Vitest with one worker. Thus the external scheduler
cause of the hosted timeout is not established. Source does establish an
unnecessary timing boundary: MermaidDiagram synchronously renders its figure,
but this disclosure test awaited `waitFor` merely for that figure's presence.
Testing Library's async wrapper adds a timer turn even when that assertion is
already satisfied. The test was not awaiting asynchronous SVG completion.

App `a7b39c0985bbd1419b1f09d599b1d79a5d1fe0fa`, tree
`429a7cf4cd59a76b7cde182d6c7f82b5641c2a9a`, changes only that test: it now
asserts the same figure synchronously and removes the async callback. Visibility,
manual close and fragment-switch assertions are preserved; actual asynchronous
Mermaid rendering tests are unchanged. No timeout increase, skip, weaker
assertion, product or telemetry behavior change was made. The patched full
owning file passes 147/147 locally; shared cached opening Python 65, shipping
Python 16 and Node 99 pass with refreshed exact source/tree pins. This is a
source-supported synchronization repair, not a claim that local testing
reproduced or conclusively explained the external hosted scheduling stall.
The strict full UI and native collector/export gates remain mandatory.


## Receiver rejection predicate after run 38901479005

Harness `5963ba82` / app `a7b39c09` completed run `38901479005` with witness
`116974533255` failure; standard Rust `116974658821` and all other standard
jobs succeeded. The repaired full UI gate passed. The actual browser stage
completed nine history checks and 49 stock trials, 14 sampled. Both pagehide
trials were explicitly unsampled; lifecycle admissions likewise retained
inconclusive/unsupported distinctions, with one BFCache primary ready report.
The independent browser inventory had 15 journeys, not final acceptance.

The first failure occurred at receiver.assert_receipt/final_index: retained
capture.errors was `['ValueError']`, so export qualification correctly failed.
The successful prefix had 1,257,499 wire bytes, ten batches and 2,362 spans;
its retained index contained no browser view spans. The failed batch was never
written. Those facts cannot recover the erased predicate or justify claiming
an overflow/unknown attribute cause. Actual logged view fields matched the
receiver's existing allowlist. Artifact `12046602577` (89,651,261 bytes) remains
remote; selected bounded receipts used 302,117 range bytes. No final semantic
collector/export join, normal-drain proof, outer Global or overhead acceptance
was obtained.

Reviewed donor `5f227d0778d34fe422d142d2f4ce300326346d4b` integrates as app
`ea2d362b33a2558564d61911e156f7a6562e3b07`, tree
`04d2ff2c2c99f336fe43f969b072c61a46841051`. It is diagnostic-only: each existing
rejection predicate records its finite fixed code instead of the generic
exception class. Bounds, allowlists, predicate comparisons, raw retention,
fail-closed final retrieval and first-error preservation remain unchanged.
Unknown internal exceptions map to a fixed internal-failure code without raw
exception text, rejected payload, unknown key names or values. It cannot
repair or retroactively explain the prior failed export. A new exact-source
run is needed specifically to identify the rejection boundary; this is not an
unchanged retry or a claim that the error was fixed.

Actual cached validation: 16 no-network receiver semantic/reconciliation/
rejection/privacy controls, 65 opening Python, 16 shipping Python and 99 Node
selftests passed. No listeners, browser, install or heavy local build were run.
Review confirmed unchanged predicates and finite literals; tests are not claimed
exhaustive over every code. Acceptance still requires zero receiver errors,
actual semantic exports, exact real collector inventory and normal owned drain.


## Typed SDK projection and independently owned history correlations

Run `38904633015`, harness `42c8bd2730c0d0befe26d0172f62a352aadc71fe` /
app `ea2d362b33a2558564d61911e156f7a6562e3b07`, is terminal failure.
Witness `116985013660` failed; Rust `116985121090` and the other standard jobs
succeeded. The bounded receiver receipt proves `attribute_unknown`, after a
successful prefix of ten batches, 2,094 spans and 1,144,558 wire bytes. The
rejected batch/key was not retained; the exact offending key is unknown.
Separately, browser `cleanup_failures` records `appendObservedHistory` rejecting
an independently observed finite request after nine history checks and a
`passed_assertions` donor browser stage. Journey UUID and finite transport UUID
are different namespaces. The former equality check was incorrect; merely
trusting reported correlations against any observed request would also be wrong.
Artifact `12048202108` remains remote (89,665,822 bytes); the retained bounded
facts used 304,042 range bytes. No final join or normal-drain acceptance resulted.

Reviewed immutable telemetry deltas `df931bb4d65a6e12f429125aa812e972a342d1f6`
and `01fb489b254a91b120113c5b243708d025c9686e` integrate directly above `ea2d362`
as app `7a36dd28191a1f32f65f68b037543ab924526189`, tree
`5e908a223e26e3cf8f264c65cd82ed33080d7ccf`. The receiver intentionally consumes
exactly eight typed default SDK metadata keys before semantic projection:
`code.file.path`, `code.module.name`, `thread.name`, `target` (bounded strings),
`code.line.number`, `thread.id`, `busy_ns`, `idle_ns` (nonnegative signed-i64,
not booleans). `level` and unknown application attributes remain rejected.
Count, duplicate, scalar, finite and size checks precede consumption. The raw
protobuf remains separately bounded and retained; semantic capture excludes
metadata and final re-decoding uses the identical projection. This explains a
real schema mismatch in the pinned default layer, not the unrecoverable exact
key from the failed run. Production export configuration is unchanged.

The shared history collector now observes independent producer admission and
journey-to-request edges, plus accepted journey-to-SSE-attempt edges. The latter
is emitted only after actual correlation acceptance; it is not SSE connection
success. Context binding/init installation precedes page creation and navigation;
only the designated original page's main frame may contribute. All reported
request and SSE IDs must have same-journey producer edges and actual original-page
network observations. Trace headers must belong to an owned, reported finite
request. Foreign pages/frames cannot supply admissions or consume bounds.
The combined ownership cap is 512 and diagnostic event cap is 2,048. Setup has
a ten-second deadline, cancellation checkpoints, sanitized phase-coded failures
and deterministic unit-only timer/parser seams that cannot enter runtime config.
Observer errors are latched to a bounded fixed code, not private exception text.

The hosted selected-libtest stage additionally names, executes and retains
`api::view_journey::tests::pinned_default_layer_emits_typed_sdk_metadata_separately_from_journey_fields`.
The build receipt records its exact command, zero exit and exactly-one-pass
witness; shared admission rejects missing, boolean, zero-test or wrong-command
attestations. This actual default-layer test has not been compiled or run locally.
The original eleven instrumentation guards and real collector test stay intact.

Actual cached offline checks passed: shipping Python 16, opening Python 66,
base Node 14, opening Node 35, joint Node 65, receiver no-network subset 18 and
focused UI 140 across seven files. A stale opening source-order assertion first
failed and was updated to check authentication, ownership install and observation
in order; that failed log is retained separately. No local native capture,
listener, install, Cargo build or large artifact copy was performed. Full UI,
codegen, real default-layer execution and actual joint qualification remain hosted
obligations. Collector/export inventory, normal app-first/receiver-second drain,
outer Global physical history and overhead gates are not yet accepted.


## Exact efce successor: SDK transport accepted, readiness export truncated

The single reviewed successor run `38909280148` binds harness
`efce1fa84df13268827dbd892c68477a6b102a4b` and app
`7a36dd28191a1f32f65f68b037543ab924526189`. It is terminal failure:
witness `117000716935` failed; Rust `117000846117` and all other standard
jobs succeeded. Genuine codegen, original eleven instrumentation guards, real
collector test, full UI gate and the new actual default-layer test passed.
The latter executed exactly one test, not a zero-test successful process.

The browser completed nine ordinary history checks with `passed_assertions`;
its cleanup failures are absent. The original history page produced zero sampled
reports in this run, so the new ownership join was not positively exercised by
an original-history report. The donor browser inventory contained eight journeys.
Receiver capture now has no errors: 34 batches, 5,357 spans, 2,549,532 wire bytes
and eight real `browser.conversation_view` exports. However, every ready view
lacks `browser.meaningful_transcript_ms`, `browser.breadcrumb_presented_ms`,
`browser.breadcrumb_enabled_ms` and `browser.anchor_settled_ms`; total duration
and anchor result are present. `receiver.assert_receipt` correctly fails at
`validate_view` with `missing ready evidence`. No collector/export delivery
certificate was sealed. `server-completion.json` says `force_cleanup_required`,
not normal shutdown. Outer Global and overhead acceptance remain unproved.

Artifact `12049943021` remains remote (89,965,825 bytes). Selected receipts,
capture and runtime log used 376,921 ZIP range bytes and are retained under
`.phoenix/evidence/continuous-history/integration7af-source-only/runefce-bounded-facts`
on this harness. The new default-layer test had used SDK defaults rather than
Phoenix's production span limit; its pass did not cover production capacity.
Source investigation found 44 semantic view attributes plus up to eight pinned
SDK attributes versus production `phoenix_span_limits()` capacity 32. A proposed
capacity correction must be independently reviewed and exercised through the
real collector with the actual shared production limits, zero dropped attributes,
typed complete readiness fields and measured serialized OTLP bytes. Merely
reordering fields, removing the semantic gate or claiming normal drain would
conceal rather than fix the loss. No unchanged rerun was dispatched.


## Reviewed production capacity correction and mandatory hosted proof

Immutable donor commits `9b0808aa8c6db08f070f5bee0e3fcd5880a7e539` and additive
fixture correction `e619d5754e8543873de320628b2ed71b20143a73` integrate above
`7a36dd281` as app `78f5a75e470c02fd967c160aaa5527820f5c28d8`, tree
`faf6bed6849ace0ea0d6abd0323a887b5b80dc23`. Independent exact-object source
review approved the corrected packet for hosted compilation/execution, not
acceptance. The first source review caught invented fixture fields, invalid
counter values and an absent correlation child; the additive correction uses
only the actual validated report schema, consistent saturated counters at 10,000
and one actual typed request correlation. Report validation is unchanged.

ADR-108 and REQ-CVT-007 make the capacity decision explicit. Both production
exporters use shared `phoenix_span_limits()` with 52 attributes: exactly 44
semantic view fields plus at most eight pinned default SDK fields. This is a
provider-wide ceiling, not signal-specific isolation. Original PR #519's 32 was
a conservative defensive backstop; content-free span allowlisting is the privacy
boundary. Disabling SDK metadata still leaves 44 fields; reordering or discarding
fields conceals loss; separate providers or decomposed spans would change
lifecycle/routing or the schema/join contract. Sixty-four adds unnecessary headroom.
The 52 decision permits 20 more slots per span (62.5% count capacity), not a
proven memory/traffic/timing increase. At the SDK's default queue of 2,048 that is
40,960 potential additional attribute slots, not a byte or process-memory bound.
Other allowed emitters can retain pre-existing fields formerly truncated: the
LLM schema can reach 29 semantic fields plus SDK metadata. Direct-turn settlement
has 14 ordinary declared fields plus special SDK-consumed status metadata and
still fits 32. Existing error strings are not claimed to have universal size
bounds. Zero-event capacities, four-link/four-link-attribute limits, exact span
allowlist, receiver caps and overhead acceptance gates are unchanged.

The hosted selected-libtest stage must run these exact tests individually,
retaining commands, strict zero exits and exactly-one-pass receipts:

- `api::view_journey::tests::production_attribute_capacity_preserves_complete_ready_collector_schema`
- `logging::tests::otel_limits_are_conservative_and_event_free`
- `logging::tests::otel_filter_is_a_spans_only_allowlist`

The first invokes the actual validated collector handler through a real default
layer (authentication remains covered by the separate collector guard), compares
old 32 against the actual production limits, and asserts
all 44 typed semantic values, required readiness fields, zero production drops,
no events and correlation-child identity/parentage. It prints actual official
OTLP-protobuf encoded Span and semantic/SDK KeyValue byte measurements. These
are fixture measurements, not whole-wire or universal byte bounds. The already
locked proto/prost crates gain direct dev-dependency edges only. Existing real
collector/default-metadata tests and original eleven guards remain mandatory.
Shared admission rejects missing, reordered, duplicate, wrong-command, failed,
boolean or zero-test capacity/privacy attestations. Actual cached offline checks
passed: Python 16 and 67; Node 14, 35 and 65. Production Rust execution and byte
receipts are still owed to the hosted successor. No new native capture, install,
Cargo compilation, listener or large artifact copy ran locally.


## Production-capacity execution exposed unsigned counter stringification

Run `38914624547`, exact harness `0212fad79dd5c8b5cb98d16462fb37836a20b0da` /
app `78f5a75e470c02fd967c160aaa5527820f5c28d8`, is fully terminal failure.
Witness `117018808475` failed; Rust `117018938843` and all other standard jobs
succeeded. The new real-collector capacity test actually compiled and executed
one test; it failed after printing these pre-correction fixture measurements:

| Capacity | Retained attributes | Dropped attributes | OTLP Span bytes | Semantic KeyValue bytes | SDK KeyValue bytes |
|---|---:|---:|---:|---:|---:|
| 32 | 32 | 20 | 1,497 | 1,051 | 295 |
| 52 | 52 | 0 | 2,272 | 1,758 | 325 |

It then rejected `browser.admitted_count` with `wrong type`. Pinned tracing-core
records `u32` through `record_u64`; pinned tracing-opentelemetry's span visitor
has no unsigned override, so the default debug path yields strings. This is a
real producer type loss, not justification to weaken the all-typed-field test.
The capacity/drop observations are valid failed-run facts, not a passing schema
receipt or corrected measurements. Original eleven guards, separate collector
and default-SDK tests passed. The capacity guard failed first, so logging guard
executions, full UI and native/browser qualification were not reached in this
witness. Artifact `12051334489` remains remote (88,689,483 bytes); selected unit
logs/receipt required 53,454 ZIP range bytes. No unchanged rerun was dispatched.

Immutable reviewed correction `ae127d11f2b7dcf5ce9e9ee861f4ef47cb1ff1b1` integrates
as app `c466cfb40c6b3ff1428c7611b48d48f005bb7bc9`, tree
`e3f4d726df1d31e6f68e386945fe69e052920b38`. All eight validated `u32` counter
emissions use lossless `i64::from` and the real collector test additionally
requires all eight to be `I64(10000)`. Report input/validation, 44-field schema,
52 capacity, metadata projection, privacy/event/link/receiver budgets and normal
drain/export gates are unchanged. Four telemetry module hashes are unchanged.
Offline Python 16/67 and Node 14/35/65 passed at the new pin. Corrected actual
protobuf metrics, three exact capacity/privacy guards, native joint inventory,
normal drain, outer Global history and overhead acceptance remain hosted gates.


## Typed-counter guard passed; full UI reaction restoration blocked native capture

Run `38918233402`, harness `aa2283a962bb9f512534c82860ec2cc242d6759d` /
app `c466cfb40c6b3ff1428c7611b48d48f005bb7bc9`, is terminal failure. Witness
`117031150642` failed; Rust `117031295430` and all other standard jobs succeeded.
The selected-libtest step succeeded with its unchanged mandatory one-pass guards:
real complete production schema/capacity, logging limits, spans-only filter,
collector/authentication and default SDK metadata. Full UI then failed one of
3,527 tests (3,524 passed, two skipped): the retained-reaction older-history
fixture could not find `Your reaction` at `renderFixture.test.tsx` after returning
to the passage. Native/browser qualification was skipped, not failed or passed.

Artifact `12053773873` remains remote (88,666,655 bytes). A selected ZIP-range
read was rejected with HTTP 503 `Egress is over the account limit` before any
selected receipt was retained; no retries or full download followed. Corrected
serialized-byte numbers therefore are not claimed from this artifact. A bounded
workflow console projection now prints only the six fixed numeric
`collector_capacity` fields already emitted by the real test; full logs and exact
receipts remain retained, and all assertions/admission checks are unchanged.

The unchanged owning fixture module passed 15 cached local tests; an initial
`-t draft` attempt matched zero tests and is not pass evidence. Source tracing
identified async history loading, React source-row commit and pill animation-frame
positioning as distinct phases, but did not prove CI starvation or a product race.
Test-only app commit `1ef9a5773446d62f10c60a71eeef69f235653c8a`, tree
`49907e8c10205f5675291fe031bcc3170807ebe9`, wraps the actual return click in async
`act`, observes the source row commit, then services positioning through an actual
animation frame before the existing textbox/body/no-focus/selection/API assertions.
No timeout, skip, assertion removal, mock return callback or production behavior
changes. Cached exact named test passed (one), full owning module passed (15),
and changed-file ESLint passed. Affected Python admission 67 and joint Node 65
passed at this new pin. The source-backed synchronization change is not claimed
to prove the cause of the prior hosted failure. Full UI and native qualification
remain required; no acceptance or overhead clearance follows from these passes.


## Immutable natural baseline and separate controlled-functional packet

Run `38921946475` is terminal success at harness
`42a2d078b7f73ea71f3e850acb3546c2a6cde526` and app
`1ef9a5773446d62f10c60a71eeef69f235653c8a` (tree
`49907e8c10205f5675291fe031bcc3170807ebe9`). All ten jobs passed,
including witness `117043819200` and Rust `117043953760`. The ordinary
collector/export inventory was 15/15/15; Global was 11/11/11. All reports
were ready. Independent source/instance/value joins, same-trace HTTP ancestry,
40 ordinary and 21 Global correlation children, normal app-first/receiver-second
drain, and physical history ordinary9/Global11 passed. The real capacity test
measured cap32 attributes32/drop20, Span1489B/semantic1043B/SDK295B, and cap52
attributes52/drop0, Span2240B/semantic1726B/SDK325B. Those are fixture encoding
sizes, not whole-wire, memory or timing clearance. Artifact `12056176368`
remains remote; the retained bounded selection was 802,027 bytes.

This baseline is partial: four actual pagehide attempts were naturally unsampled,
and both history pages had zero additional-history reports. Native resume is
unsupported; overhead clearance and full acceptance remain false. Controlled
positives do not retrospectively change that stock evidence.

The separate source packet pins app `d760aad0573f949e61a04fd95bd905257fec4721`,
tree `9477fd6afb59a333ac6590a6e3b324886dc6fbda`, integrating the reviewed
factory/graph guard and additive actual-config/output-directory verification.
The governing controlled exception is the app's ADR-109 and REQ-CVT-001/005/006;
no shipping sampler, collector schema, privacy limit or SDK capacity changes are
introduced by this harness packet. Both stock and controlled artifacts must be
built separately by the hosted workflow. Exact mode-specific manifest digests
and output inventories are sealed immediately in the independent build-owner
receipt, threaded into libtest provenance, and checked before and after capture.
Git-pinned factory/config/guard/module hashes, lockfile identity, exclusive graph,
contained non-symlink asset inventory and independently fetched served bytes are
additional gates, not substitutes for that build attestation. Stock/overhead
admission structurally rejects a controlled artifact.

The native command selects only `--joint-telemetry --controlled-functional
--timeout 300`: one original-page retained historical direct-open, one real
pending-GET cross-document pagehide, and one real pending-GET Sidebar-home SPA
cancellation per ordinary/Global route. Admission is truthfully sample_rate1.
Exactly six reports must join six real collector acceptances and six semantic
exports (ready2/incomplete2/canceled2); one unauthenticated POST must return401
and add zero acceptance/export. Original-page producer and finite-network
ownership, actual report/producer/scenario route equality, unique UUIDs, physical
historical source identity and current writable composer owner are checked.
Observers survive context closure so late reports/admissions cannot disappear.
Pagehide waits for the departing transport to settle before destroying its
context, but that event is not delivery proof: only the final real collector
and export join after normal drain can seal it. A failed transport cannot be
promoted by the provisional browser receipt.

The scenario clock is absolute195s, within a browser lifecycle bound205s plus
bounded4s shutdown and15s asset verification; the Python child cap is230s and
whole qualification cap300s. Deadline/stalled setup, late completion, primary
versus cleanup failure, failed auth-negative receipt retention, changed report
routes, rehashed replacement assets and delayed departure transport are offline
regression targets. These ceilings are liveness bounds, not performance budgets.
The original real libtest guards, collector/auth/default-SDK and capacity/privacy
guards, codegen and full UI source qualification remain mandatory hosted gates.
No native resume simulation, unchanged sampling rerun, overhead capture or
numerical overhead tolerance is included. No controlled artifact build or native
execution is claimed by the source/unit packet. Hosted qualification may proceed
only after cohesive exact-source contract/artifact/workflow review.

The source-only cached ring passed: artifact8, Python lifecycle68, controlled
Node16, existing Python16 and Node14/35/65, and pinned-app25 across four focused
Vitest files. Diff whitespace and workflow YAML parsing passed; actionlint was
not installed, and no dependency was installed for it. This is not a full check
or artifact build. Cohesive independent review found an execution blocker at the
pinned app: `writeBundle` inventories only Rollup bundle entries, omitting Vite's
copied public files and the stock `closeBundle` `.gitkeep`. Exact asset admission
correctly rejects that mismatch. The telemetry source owner has the focused
correction request. Hosted execution and publication remain held until corrected
source is imported, re-pinned, tested and independently reviewed; the exact
inventory gate must not be weakened to bypass the defect.

The focused correction donor `c060d77989d9bcad0e0491f471fe6595697671b1`
integrates as app `c2344bc1387e073de83e164b086a7260e713d4af`, tree
`f3c8a59de1daf4860c418ebc73d6126bbb867bb6`. Manifest sealing now runs in
post-order `closeBundle`, after the normal `.gitkeep` hook. Emitted-byte equality
remains mandatory; the final exact inventory additionally proves recursive public
source bytes and an empty output-local `.gitkeep`, rejecting extra/missing/changed
files and symlinks. The `.gitkeep` writer resolves the selected output rather
than modifying stock output during a controlled build. Shared exact admission
is unchanged. Read-only independent review checked the cached Vite8/Rolldown
close ordering and approved source import/progression to hosted validation,
not artifact or runtime acceptance. Cached parent tests at this pin passed
artifact8, lifecycle68, controlled Node16, joint Node65 and app29 across four
focused files. All executable workflow/Python/Node app pins now select this exact
integrated SHA/tree; the earlier blocked pin above is retained as history.
The next gate is the actual hosted two-build inventory and controlled six-report
collector/export/drain witness. No local heavy build or native run was performed.

### First controlled hosted packet: terminal failure and causal successor

Run `38932424750` at harness `4838341db8bbb6033b8eb76d572bb6b574ba3cd6`
is terminal failure: witness `117079492200` failed full UI at TS6307; Rust
`117079632867` and all other jobs succeeded. Both actual canonical Vite builds,
strict initial artifact admission (454 files per mode), codegen, libtest guards,
collector/authentication/default-SDK and capacity/privacy passed. Full UI lint,
stylelint and Vitest passed, but composite `tsc -b --noEmit` rejected the missing
explicit guard-module include. Native capture was skipped, not failed or passed.
Remote artifact `12059059498` is 95,559,021B; bounded selected reads were289,072B.
Independent final-manifest comparison also caught the full-UI production-CSS
fixture overwriting the stock canonical manifest with its temporary-output
manifest (457 files), while controlled454 stayed exactly build-owner bound.
The initial stock454 build-owner/libtest bindings remain failed-run evidence;
the later manifest is not resealed or promoted. Durable evidence:
https://github.com/scottopell/phoenix-ide/pull/908#issuecomment-6220316723.

Donors `2c4bbcc9e601270d2434467b7cb2716a2b171f74` and
`9cb1db2d55ad61814573d9f294712d628a262302` integrate as app
`cdcbc6e103044c2f7e00d7b0a85cb2475a5c7768`, tree
`dfbee24aa0886265e43905fffd8bc91a54fc135a`. The first explicitly lists the
build guard in the strict composite Node TypeScript project. The second limits
canonical stock-manifest publication to canonical output: ordinary custom-output
CSS builds retain graph exclusion but cannot read/hash or publish a canonical
artifact manifest. Controlled exact config/mode/output and all canonical byte/
complete-inventory gates are unchanged. Independent additive source review
approved one causal hosted successor; it did not claim actual successor build
or native results. Genuine parent `tsc -b --noEmit`, focused app30, artifact8,
lifecycle68, controlled Node16 and joint Node65 passed; YAML parsing/diff checks
passed. No local bundle/server/install/native work occurred. The next hosted gate
must prove full UI and preservation of both canonical manifest bindings, followed
by the controlled exact-six collector/export/normal-drain witness. No overhead,
full acceptance or native resume claim follows from this source packet.

### Controlled module-resolution successor

Run `38935238132` at harness `0a085a31d5d7885114b9c575061aad6f1fd5c2dc`
reached actual app startup after successful two-build admission, all guards,
codegen and full UI source qualification (full UI receipt exit0). Witness
`117088967913` failed before Chromium launch because the new controlled CLI
required undeclared `@playwright/test`; the app declares `playwright`, also used
by the existing real-server qualifiers. No controlled browser reports or positive
collector/export results are claimed. The run was followed to terminal failure,
including the standard Rust job. Remote artifact `12060558409` is96,415,396B;
selected bounded reads290,368B. Independent final receipt joins verified BOTH
stock/controlled454-file manifest digests and inventories still equal the original
build-owner and instrumented receipt bindings after full UI. The prior canonical
manifest overwrite and composite TypeScript failures are cleared in this run.

The narrow shared CLI correction resolves Chromium through the actual pinned
app's declared `playwright` dependency with the same app-relative `createRequire`
boundary. A no-launch regression checks the actual package declaration and real
module resolution; controlled Node17 passed. Independent narrow review approved
this source fix only. Local dependencies remain the existing shared cache, not
independent frozen-install provenance; hosted frozen dependencies remain required.
No app pin, asset gate, collector/export inventory, outcome or timing policy was
changed for this correction. Instrumented server startup evidence is not browser
launch/report acceptance; no native, resume, overhead or full acceptance is inferred.

### First entered controlled scenario: failed, exact cause not retained

Run `38938107186` at harness `a05512b901bd3bd5e35eb109fcd1756da9ef41e0`
is terminal failure: witness `117098592669` failed the controlled step; Rust
`117098722795` and all other standard checks passed. The browser dependency,
canonical builds/admission, guards, codegen and full UI gates cleared. The browser
receipt contains trials[] and failed_trial:null, proving entry into the first
ordinary/history scenario but no completed collected scenario. The outer wrapper
replaced the scenario label with controlled_driver_failed; no exact failed check
can be recovered. This is not evidence of missing browser launch or any particular
auth, navigation, readiness or physical-history defect. Artifact `12063320113`
remains remote (96,430,302B); first selected bounded reads99,983B preserved the
browser log, sanitized failure and controlled artifact admission.

A diagnostic-only correction preserves the original scenario failure label and
adds a static last-entered phase, validated route/scenario kind, assertion versus
operation classification and capped report/admission/request/pending/error counts
plus at most8 numeric response statuses. No raw errors, stacks, URLs, credentials,
paths or new report bodies are retained. Independent review approved this bounded
diagnostic boundary; Node19 tests pass, including label preservation and private
setup-error sanitation. No scenario assertion, timeout, retry, outcome or exact
collector/export criterion changes. A successor may localize the failed stage;
it cannot retroactively establish the previous cause or any acceptance.
