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

The exact installed, self-contained DOM probe selects the first fully visible
virtual row with a fully visible positive-size persisted-message descendant.
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
`checkedGatePending`. Existing `anchor()` and virtual-key/offset `anchorMatches`
remain in the acceptance/reconnect subsequence. Both checks additionally require
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
