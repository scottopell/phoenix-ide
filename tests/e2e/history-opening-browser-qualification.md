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

The bounded diagnostic successor `20f7b399bdbcb2c7714ac0cf08d448f4d5e2ae88`
ran as `38940681380`, terminal failure. Witness `117107221064` failed the
controlled scenario; Rust `117107339361` and all standard jobs passed. The
retained typed location is ordinary/history/physical_history, assertion, with
one report, one204 response, one admission, four observed requests, pending0
and capture errors0. There are no completed trials. This localizes the failure
to physical target validation after report observation; it does not distinguish
a null fully-visible oracle from the subsequent identity/source assertions, and
it does not establish independent collector/export joining or normal final drain.
No causal claim about app geometry, sampler or transport is inferred. Artifact
`12063282692` is96,430,001B; selected bounded reads75,314B retain the failure,
browser log and startup identity. The next step is source/target/physical-oracle
diagnosis, not another unchanged execution or weakening of the fully-visible
persisted-identity assertion. No additional run is dispatched by recording this
result. Resumeunsupported, overheadfalse and fullacceptancefalse remain explicit.

### Source-only physical-oracle failure distinction

A source-only diagnostic delta splits the prior physical_history stage into
observation presence, fixture identity binding, and source-pin equality. The
existing serialized oracle optionally returns its observation plus bounded
static rejection-predicate counts. Default callers still receive the identical
observation-or-null contract. Both the virtual row and persisted child must still
be fully contained on all four clip edges, finite/positive, visible and unique;
all ownership/key/alias/identity checks and existing assertions remain intact.
Diagnostic counters saturate at512 and contain no raw keys, IDs, source strings,
URL, geometry, DOM text or errors. Predicate counts describe rejected candidates
encountered during traversal, not necessarily the target's unique root cause.

The returned sourceRow is the caller's known persisted-ID→fixture-source mapping,
not independently observed DOM source provenance. Accordingly the new diagnostic
is named fixture_identity_matches; sourcepin_matches separately compares the URL
pin. Neither is promoted into independent DOM source attribution. Independent
read-only review approved source-only freeze of this delta. Cached controlled20,
physical-oracle44 (one external retained-fixture test skipped), existing opening35
and Python68 passed; diff checks passed. Tests use fabricated DOM/geometry and
existing cached dependencies, not a local or hosted browser qualification. No
app source, pin, admission, sampler, timeout or acceptance rule changed. No push
or hosted execution is part of this source-only boundary; the exact prior hosted
physical failure remains unresolved until actual bounded diagnostic evidence.

Release-critical follow-through retains the frozen app baseline rather than
unreviewedly rebasing to merged main. Source inspection confirms the production
source-link syntax and keyed virtual wrapper are consistent. A local serialized-
DOM equivalent reproduces strict null for a fully visible exact target inside
an oversized virtual row, including an oversized standalone first-user message;
this proves an oracle rejection class, not the prior hosted dimensions. Optional
target-ID prepass counters now distinguish a seen exact fixture attribute, row
height exceeding the clip, row rejection and child visibility/rejection, even
when the row is rejected before normal child selection. These are diagnostics,
not accepted-identity evidence; conflicting aliases and duplicates still fail.
Independent additive review approved one bounded actual hosted attempt. Combined
cached tests65 passed with1 external-fixture skip; no local native/build/server
was run. This publication supersedes the prior source-only execution hold solely
to collect the reviewed target-specific actual distinction, without thresholds,
timeout, retry, source-pin, app-pin or acceptance changes.

### Actual target is present but clipped; local readiness/containment distinction

The reviewed bounded run `38966536941` at harness
`ca8df845ddfb2f4e24ab84bbee1e67e318b89ffd` is terminal failure. Witness
`117194278348` failed controlled ordinary/history at physical_history_present;
Rust `117194382117` and all other standard jobs passed. Actual diagnostics show
known_target_seen1, known_target_row_clipped1, known_target_message_clipped1,
row_clipped8, unknown_or_unexpected_identity3, with NO target-row-taller-than-clip
counter. The target is physically present and rejected by containment; oversized
height is not supported at this observation. One report/204, one admission,
four requests, pending0/errors0 precede the assertion. Source-pin equality is
not reached, so no claim about its actual match is made. Remote artifact
`12075382939` is96,430,484B; selected bounded reads75,055B retain exact receipt
and browser log. No completed trial or final collector/export acceptance follows.

A local no-browser equivalent now executes the actual pinned app
`isCommitVisible` function (AST-selected and transpiled with cached TypeScript),
not a rewritten predicate, alongside the exact serialized physical oracle. A
short target clipped1px on top/bottom/left/right, and one retaining only1px overlap,
passes the app's positive-intersection predicate but fails strict whole-row and
message containment. A fully contained control passes both. Combined cached
Node66 passed with1 external-fixture skip. This proves the source predicate
relationship, not the complete lifecycle gate, actual clipping axis or timing
cause. The app readiness path additionally requires targetApplied, transcript,
breadcrumb and child visibility. It cannot be equated to strict full containment.

The controlled harness still uses an unpinned newContext viewport and a single
physical read immediately after report204, unlike the existing explicit1360x900
realserver geometry/physical stability path. These are concrete local seams to
regress before another actual attempt: original-page physical settlement versus
report readiness, and explicit qualification geometry. No tolerance, scroll
mutation, alternate target, sampler change or relaxed oracle is adopted. No
further diagnostic-only hosted attempt is started by this record. Merged #926
is not integrated speculatively into this frozen evidence baseline, and is not
claimed deployed. Native source-link acceptance remains release-blocking; stock
baseline, controlled proof, overheadfalse, resumeunsupported and fullacceptancefalse
remain separate.

The installed app-relative Playwright1.61.0 core `validateBrowserContextOptions`
confirms omitted viewport defaults to1280x720; this is a concrete harness mismatch
from the old explicit1360x900 qualifier. A cohesive driver correction now pins
1360x900 for all six scenario contexts and observes strict original-target
containment independently of report204. Acquisition and the unchanged shared
four-animation-frame/2px hold run within a5s stage inside the existing30s history
and195s absolute budgets. All reads retain exact persisted-ID/fixture-source,
URL source-pin, original route and acquired virtual-key identity. Null after
acquisition, identity/pin/URL changes or >2px instability fail without restarting
the hold. No scroll mutation, alternate target, retry, app change or weaker
containment is introduced. Python receipt admission requires the declared
viewport and4 stability frames. Cached controlled22 and lifecycle68 passed,
including delayed first physical observation, permanent clipping, unstable
identity/URL/pin/offset, and wrong receipt viewport/frame-count negatives.
Independent cohesive source review approved one causal hosted successor. This
corrects two source-demonstrated driver discrepancies; it does not assert which
caused the actual earlier clipping. Same-artifact local browser reproduction is
not claimed: the hosted output is remote and no local heavy build/server/browser
was admitted or started. Actual successor strict containment and exact-six real
collector/export/drain remain the next execution gate.

The cohesive correction ran as `38970376574` at harness
`5e16c608bd46a28b768f5c770326897770804562`, terminal failure. Witness
`117206992051` failed controlled ordinary/history; Rust `117207113852` and all
other standard jobs succeeded. Explicit1360x900 and bounded physical acquisition
did not clear containment: the final probe still has target_seen1, target row/
message clipped1 each, no target-row-taller counter, row_clipped10, unknown4,
report1/204, admission1, requests4, pending0/errors0. The failure occurs during
strict acquisition before any successful baseline/four-frame hold; no completed
trial exists. It does not show a source-pin mismatch because that assertion is
not reached. This disproves the sufficiency of viewport matching and physical
waiting, not the need for those driver contracts. No oracle relaxation or product
predicate change follows. Artifact `12077047082` is96,448,691B; selected bounded
reads75,334B retain the exact sanitized failure/browser log.

No more diagnostic-only hosted attempts are planned. The minimal missing actual
evidence is the offending clip axis and target/virtual-row/scroller/window
rectangles from the same artifact/runtime, needed to distinguish app positioning/
layout from a strict-oracle boundary mistake. The retained generic clipped count
cannot recover that geometry after context closure. Cached source/VM tests cannot
substitute for it. Same-artifact local browser reproduction was not performed:
only remote build artifacts and source/cache are available here, and no local
heavy build, native server/browser or artifact copy was started. The v0.13 blocker
is unresolved and explicitly geometric, with zero completed controlled scenarios;
collector/export/full/native acceptance is not claimed. Frozen stock evidence,
controlled class, resumeunsupported, overheadfalse and fullacceptancefalse remain
separate. Further execution is held pending an admitted same-artifact bounded
geometry observation, not an unchanged rerun.

### Fractional start regression and narrow product correction

The approved five-minute artifact-reuse reproduction ran as `38973904057`, job
`117218596370`, terminal diagnostic SUCCESS in about100s, source
`d1a18b983ab37ac269964ab1fca759a2a5b82d7d`. Exact old executable/UI hashes
were verified; no Cargo/Vite/codegen/full-suite build occurred. Recreated ordinary
fixture recipe is not the originalDB. All five actual observations at1360x900,
DPR1, Chromium148.0.7778.96 show cliptop93, row/message top92.5, sole top-edge
violation0.5CSSpx; horizontal/bottom edges fit. Row75px/message67px, scrollTop43.
Exact identity unique1, alias/sourcepin/originalroute true throughout. Strict
containment remains false across four following rAF. Screenshot viewed; normal
app exit0/HOME+socket removed/receiver closed. Artifact `12078106197` is56,986B;
only bounded private-safe diagnostic evidence retained. This proves actual
fractional displacement, not the historical setter argument or acceptance.
https://github.com/scottopell/phoenix-ide/pull/908#issuecomment-6225670473

Donor test-only `6f992c42f6f547982cda6ade977688a806c37b34` integrates as
`e434785fb5fc61182ef9d48a2010324f551eddef`. Parent independently reproduced
57pass/2expectedred: real immediate and hidden/reveal start paths request42.5,
modeled native nearest-integer setter applies43 and physical edges are-0.5.
Integer/end/tail/restore controls pass. This reproduces the modeled mechanism,
not the exact earlier browser setter trace.

Authorized correction donor `dd7f7ef5205cc1772b12b09e909fe10dfc3fadee`
integrates as app `c115db652817815e3e3d3c13fa9dcdd91355404b`, tree
`9454eccd3d0e7884f50ea393bf5e9b1519aa2998`. Shared immediate/deferred
start execution reads native actual position; only directional start overshoot
above the clamped ceiling by<=1CSSpx triggers one conservative ceiling-1 request.
Final readback updates viewport/anchor and own-write bookkeeping only when a write
actually occurred. No generic rounding/tolerance/quantizer guarantee is added.
Restore explicitly selects continuity; end/tail and readiness/oracle stay intact.
Hidden semantic targets retain intent until visible and existing authorization/
supersession gates remain enforced. Minimal REQ-VT-006/executive coverage is in
the app source; reducer Allium is unchanged. Independent exact donor review
approved import and one actual-browser qualification, not release acceptance.
Parent genuine TypeScript passed and235focused tests across8files passed, including
both red→green cases and existing/integer/accepted-fraction/own-write/clamp/resize/
continuity controls. Harness lifecycle68/controlled22/joint65 also passed. The
existing MessageList test fixture setState-in-render warning remains recorded,
not silently treated as a new product defect or removed. No local bundle/native
work occurred. Actual newly-built strict containment and exact-six collector/
export/drain remain required; old artifact geometry is not promoted to fixed
artifact evidence. The dedicated geometry workflow is not triggered by this PR
branch publication, so no duplicate diagnostic runner is created.

### Corrected-artifact result and observed later-header writer

Qualification run `38975198473` at harness
`c47c678ecc2e0f8121889fe2bd7e3787eafbc526`, appc115, is terminal FAILURE.
Witness `117222852207` still fails first ordinary/history strict acquisition;
Rust `117222933323` and all standard jobs pass. Corrected artifact
`12078389483` is96,401,787B; bounded failure selection75,052B retained. Target
is present with row/message clipped, report1/204/admission1/request4/pending0/
errors0, zero completed trials. The modeled immediate/deferred fix is not an
actual-renderer resolution and no acceptance is claimed.

The subsequently approved same-corrected-artifact write trace ran as
`38977500891`, job `117230360829`, terminal diagnostic SUCCESS in about90s,
source `b439a83b434860af63e57825eaec239701bf15b4`. No build/fullsuite or
product mutation; exact retained executable/UI provenance verified; new ordinary
fixture recipe is not originalDB. Observer forwards original assignments once,
preserves method returns/native errors/RO callback receiver, and retains at most128
safe finite events. Four cached VM tests and independent narrow source review
passed before this run. Trace health installed=true, dropped0/failures0,34events.

Observed event32 is start-helper write0→actual0 while header/target are absent.
LAST assignment event33 requests42.5, before0, immediate native readback43;
contemporaneous header BCR height42.5, target row/message top92.5 versus clip93.
The next event is header RO content height42.5; no later write occurs. All five
final observations remain scrollTop43 and sole top-edge delta-0.5. Exact target,
sourcepin and original route match. This is now observed setter history, not an
inference from final geometry.

Safe compiled callsite coordinates map event33 to native setter K at line1 col15503,
applyAnchor q col17837, applyPhysicalChange kt col18441, header-ref callback
col21890 in `MessageList-DFfKfDYj.js`. Its SHA256
`79e0554708297f8397ff48d987e3817130b9e1967e643fa71403695485fc98ca`
was independently matched to the exact controlled manifest. Event32 maps through
applyAlignedPosition xt and positionAtIndex wt; event33 does NOT use that helper.
Thus the late header measurement/anchor write bypasses the directional start
correction after initial start at header0. The generic native rounding mechanism
is observed here, while the next policy/source fix must preserve continuity and
not globally floor anchor writes.

Diagnostic artifact `12079985807` is58,903B, containing only safe geometry/write
trace/screenshot/provenance/completion. Screenshot viewed. Independent trace and
compiled-hash join passed; normal app exit0/HOME+socket removed/receiver closed.
Telemetry owns the next bounded local late-header regression/source correction;
this writer retains runner/integration ownership. No speculative second product
patch, full qualification retry, oracle relaxation or full/overhead/native
acceptance is introduced by recording this cause.

### Grounded late-header anchor intent correction

Test-only donor `92da8ec138ce351694695be2631e4cc1406f4d2a` integrates as
`268e8d3755fc2a34dc809d343d6929545cd8f9e0`. Parent independently reproduced
73pass/1red via actual React late-header ref mount with stable callbacks, requested
42.5/native43/-0.5 physical edges; continuity and user-interrupted controls pass.
Independent review confirmed fidelity to observed event33, not a fabricated helper.

Correction donor `fb613818ddcbdeb1bda26dd66731534f0a0db85b` integrates as app
`f542a268f795821e00a118cf348bf9584dd1d58d`, tree
`d25915d3cffe37c8c363776d3f57486a8438de2d`. Internal typed physical-anchor
intent distinguishes directional start from continuity. Same-key late-header
capture preserves live directional intent; generic captures/interruption/cancel/
retirement/replacement demote to continuity. Directional applyAnchor uses the
existing bounded readback executor and RETURNS its final actual anchor;
applyPhysicalChange retains that returned anchor instead of stale input. Public
capture exports only geometry, so restore/end/tail semantics remain continuity.
No generic floor, extra retry loop, oracle/readiness/tolerance change or arbitrary
quantizer guarantee is introduced. Minimal normative/executive wording follows
the same scoped lifecycle. Independent exact4file review approved source import
and one actual-browser qualification, not acceptance. Parent genuine TypeScript
and241focused8file tests passed, including the retained late-header red→green,
accepted fractions, second fractional resize/item update and interruption/cancel/
replacement controls. Harness lifecycle68/controlled22/joint65 passed. Existing
MessageList fixture warning remains recorded. No local build/native run occurred.
Exact pinned corrected-artifact strict containment/collector/export/drain remain
required; the old observed write trace remains failed-artifact evidence only.

### Actual typed-anchor failure and representative parent completion regression

Qualification `38978290925` at harness `e7a03e028908719c610a18863a8aff6c7cd2f5e3`
is terminal FAIL: witness `117232892530` still fails first strict historical
acquisition, Rust `117232971215` and all standard jobs pass. Zero controlled
trials complete. Artifact `12079698600` is96,431,157B; bounded failure reads75,193B.
Typed-anchor unit success is not actual browser resolution.

Approved passive same-artifact trace `38981645495`, job `117243634289`, source
`cbc66dd22616f27b7b849341e9ec00f46019f9b2`, is terminal diagnostic SUCCESS.
Exact f542 executable/UI provenance reused, no builds/fullsuite, new recipeDB
not originalDB. Healthy trace installed/drops0/errors0 again shows initial start0
before header, then LAST native write42.5→43 from late header, row/message top92.5
versus clip93. All five final frames remain clipped. Callsite in exact
`MessageList-DE8weF38.js` maps to setter15622, applyAnchor CONTINUITY branch18222,
applyPhysicalChange18838, header-ref22288, line1. Manifest-verified SHA256 is
`eb3912046f401161a88bf85f41aac7e9449a9b8624c5d704f1b249c466c5ad1e`.
This proves the later write uses continuity; the trace does not expose the parent
authority value or independently prove why intent was demoted. Artifact
`12081153181` is56,898B; original source-pin/identity checks and cleanup remain
private-safe diagnostic evidence, not acceptance.

Test-only donor `db9aed7e9f809f38546efb19dca1d6069e1a919d` integrates as
`b6fd1c69a70b483706bf3cb6618a8697fba60a10`. Parent independently reproduced
79pass/1red with real reducer plus real VirtualTranscript parent: positioning→
owned native command→measured physical snapshot→physical_observed→finish applied
exactly once→input idle/authority NONE→late header. It requests42.5/applies43 and
clips-0.5. User interruption and replacement controls retain continuity. This
covers the missing success-lifecycle seam; full MessageList integration/callbacks
and actual gesture delivery are not claimed. Next correction must distinguish
settled semantic geometry from expired command capability without retaining
command authority indefinitely, restoring cancelled intent or changing continuity
policy. Telemetry owns that representative source fix; parent retains integration/
actual-browser execution. No further unchanged qualification is dispatched.

### Successful completion settles geometry without retaining command capability

Donor `a9581e81502ed59913de398173d7cd204cf1b23a` integrates after the retained
db9 parent red as app `302a3733e9013bb09beb1e83c92b29b616ab0706`, tree
`afd4a76e7b86a7a784d8070b97cc9873d0fca057`. Matching authorized success
explicitly hands command ownership to typed same-view settled geometry BEFORE
parent notification; pending target intent is discarded and no new position or
finish is emitted. Same-view idle preserves only this geometry, not command
capability. View generation changes, replacement/cancel, user takeover even
without native scrolling, and detach retire it. Missing/superseded/stale finishes
do not settle. Restore/end/tail remain continuity; no oracle/readiness/native
correction tolerance changes. ADR115 records this distinction; requirements and
Allium executor guarantee are aligned. Fresh remote ADR115 collision check found
none; pre-existing107/108/109 conflicts with main remain an explicit landing gate.

Independent exact10file review approved import and one actual-browser qualification.
Parent genuine TypeScript and262focused9file tests passed, including real-reducer
parent79pass/1red→green, exactly-once handoff, takeover-without-scroll, cancellation,
replacement, view-reset and stale controls. Lifecycle68/controlled22/joint65 passed.
Allium validation returned diagnostics[]/findings[]; spec preflight reviewed the
executor contract/symbols and no changed wire schema applies. Existing fixture/
mock localhost warnings remain recorded. No local build/browser/native execution.
The observed f542 continuity writer remains actual failed-artifact evidence; the
new lifecycle regression is source evidence, not direct observation of the old
parent authority. Actual newly pinned strict containment and exact-six collector/
export/drain remain required before any controlled acceptance claim.

The first success-settlement artifact ran as `38982699457` at harnessd97c,
terminal FAIL: witness `117246983254` passed ordinary strict physical acquisition,
four-frame stability, exact persisted identity/fixture binding and source-pin
checks, then failed NEXT at composer_owner. The retained physical diagnostics
show observation=true, fixture_identity=true, sourcepin=true and exact target
fully visible. This execution clears the prior ordinary physical gate only;
zero scenarios complete and Global/departure/collector/export/final-drain claims
remain unproved. Rust `117247051296` and all standard jobs succeeded. Artifact
`12082485923` is96,385,215B; selected bounded reads74,610B retained failure/log.

The next failure is a source-proven harness selector mismatch: controlled capture
hardcodes textarea placeholder Message..., while actual InputArea uses dynamic
mode/state/hint placeholder text and stable textarea#message-input. The narrow
shared correction imports existing COMPOSER_SELECTOR from the realserver qualifier,
retaining exactly-one and editable assertions. The imported module has a guarded
CLI and no browser startup on import. Controlled23 tests pass including placeholder
rejection and missing/readonly negatives; independent narrow review approved one
causal harness successor. No app pin/product/oracle/physical/timing rule changed.
Unit selector correction is not actual successor acceptance.

### Actual physical fix confirmed; controlled writable fixture prerequisite repaired

Composer-selector successor `38984649048` at harnessb962 is terminal FAIL at
composer_owner; witness `117253514945`, Rust `117253592241` and all standard
jobs pass. Exact artifact `12082193993` is96,431,697B; bounded failure reads74,612B.
A reviewed <=5min same-artifact diagnostic `38986434292`, job `117259416719`,
is terminal SUCCESS, sourceed3eea096. Artifact `12084337181` is64,110B and
retains safe observations/screenshot/provenance/completion. In ALL five actual
frames strict containment=true, target top93.5 versus clip93 (+0.5 margin), exact
identity/sourcepin/route true. Composer count is0, not disabled/readOnly. This
confirms actual corrected ordinary geometry, not exact-six qualification.

Source diagnosis proves a fixture prerequisite mismatch: equal_text_fixture
exhausts the newest accepted successor, and ConversationPage correctly excludes
ordinary composer for context_exhausted. Controlled capture requires a writable
latest owner. The narrow fixture change adds writable=False by default, preserving
stock exhaustion byte-for-byte; only controlled ordinary/Global passTrue, retain
the existing accepted-opening persisted-idle wait and explicitly assert idle,
then omit the final synthetic exhaustion. All member messages/pages/identities/
boundaries are refetched and validated afterward; predecessor summary and separate
accepted opening remain exact. No target/oracle/composer/sampler rule is weakened.
Python70 tests pass including stock exhaustion, controlled no-exhaustion and
nonidle rejection. Independent narrow review approved one causal successor;
actual six-scenario collector/export/drain remains required. No app pin changed.

### Additional-history ownership diagnosis after writable latest fixture

Shared `98718c5c05b8191a1cfbf418652a1d08ec401c6c`, app
`302a3733e9013bb09beb1e83c92b29b616ab0706`, run `38986752552`:
ordinary controlled history cleared strict physical acquisition, four-frame hold,
fixture/source-pin identity and the canonical editable composer. The witness
failed at `additional_history_owner`; the retained receipt does not distinguish
its two assertions. Rust remained in progress at this entry, so this is not a
whole-run terminal claim. All other standard lanes were successful.

Source inspection found that the shared correlated-request observer excludes
GETs lacking URL `open_id`, while the app older-snapshot helper supplies only
limit/cursor for the initial older GET, cursor-refresh GET and retry. The initial
snapshot path separately registers journey request ownership. Coordinator API
also suppresses `open_id` on `before` requests. The app owner independently
confirmed this instrumentation gap; actual pagination and its ordering relative
to the report still require observation before naming the runtime cause.

The shared diagnostic now observes up to 128 actual older GETs at the exact
fixture endpoint independently of correlation. Retained data contains only
`has_open_id`, `before_report` (request event before report POST event), response
status and `in_report`; no URL, cursor, ID, response body or raw error is retained.
It cannot grant acceptance. Normal failure phases distinguish missing correlated
older requests from missing report ownership; the positive checks are unchanged.

A five-minute hosted artifact-reuse diagnostic is pinned to original artifact
`12084070461` (96,401,945 bytes), run `38986752552`, exact shared source `98718`.
It uses the same controlled writable-latest fixture recipe in a newly generated
DB, not the original DB. No rebuild, emulation or full-suite rerun is requested.
Stock `.25`, unsupported resume, overhead clearance false and full acceptance
false remain unchanged.

Cached tests: 93 Node tests pass (joint66, controlled23, diagnostic4); Python
lifecycle70 passes with cached `opentelemetry-proto==1.39.0`. An initial Python
invocation omitted that dependency and had one import error; the corrected
cached invocation passes. No local native/browser/build execution occurred.

### Exact-artifact pagination result and whole-run terminal state

Original run `38986752552` is terminal **failure**: isolated controlled witness
failed, Rust and every other standard lane passed. This is not an incomplete
Rust check or a successful six-report controlled qualification.

Diagnostic run `38988280900`, job `117264708678`, source
`191dacb3e7bcd671990298be8896d6cdcdb2015c`, completed successfully. Its diagnostic
artifact `12083668021` is 87,890 bytes. The exact `98718` retained executable/UI
and `302a` app source were reused with the writable-latest fixture recipe in a
**new DB**, not the original run DB. No build or emulation occurred.

Observed actual older-history GETs: **two**, both response **200**, both request
events **before the report POST**, both **without `open_id`**, both **absent from
report request correlations**. Diagnostic errors and report-capture errors were
zero; exactly one report received 204. Strict containment held for all five
observations; exactly one current composer was connected, visible, enabled and
not readonly. Normal drain: app exit0, owned home/socket removed, receiver closed.

This proves actual pagination exists before the report in the reproduced recipe,
but is omitted by the correlated-request observer because the app does not attach
request ownership. The app-source owner was commissioned to fix that seam, with
separate ownership per actual GET on the captured original journey, no new
admission, no mutation after terminal, and no expansion of backend initial-open
stages. One causal qualification will follow source regression and review; no
unchanged normal qualification was rerun for this diagnosis.

### Complete older-request inventory through final browser drain

Independent read-only source review found that the additional-history check could
miss an uncorrelated older GET arriving after its first snapshot, and the small
diagnostic saved pagination before its geometry frames rather than on every
save. Shared qualification now requires every independently observed older GET
to match the correlated inventory and original report, and rechecks that same
bounded inventory after context closure. New negative regressions reject an
extra uncorrelated older GET both before collection and during context close.
Diagnostic saves refresh pagination/report counters through frames, error paths
and final drain. No additional hosted diagnostic was run for these changes.
The existing `191dac` result above remains limited to its actual report-time
observation; later source changes do not retrospectively strengthen it.
Cached shared regressions: **130 pass** (joint66, controlled25, diagnostic4,
existing opening35); diff check passes. Positive additional-history, original
journey, strict geometry, stock and acceptance constraints are unchanged.

Before admitting paginated requests into the correlated observer, its retained
request schema was projected from raw URL to `pathname` plus `has_before` and
already-allowed opaque request/trace IDs. Actual endpoint matching remains exact;
cursors, arbitrary query values and origin are not retained in controlled
ownership receipts. A regression includes sentinel cursor/unknown query values
and proves neither enters the snapshot. This does not rewrite immutable stock
baseline evidence. All130 shared cached Node regressions still pass.

### Recovery request identity versus initial-open stage attribution

The app-source owner identified an attribution constraint during repair: a
fresh-tail recovery GET has no `before`, so sending `open_id` would qualify it
for an existing initial-open backend stage. The source repair therefore needs
an exclusive finite `request_id` query identity for that recovery request, not
an expanded backend stage. Initial and actual older-page GETs retain their
explicit admitted `open_id` contract. The shared network observer recognizes
one nonempty single-valued correlation key (`open_id` or finite `request_id`),
rejects mixed/duplicate keys, and normalizes it into its existing opaque network
request-identity slot for producer/network/report joins. Neither query string
nor arbitrary values are retained. SSE still requires its existing `open_id`.
Cached shared131 Node tests pass, including exclusive recovery-key and query
privacy regressions. This is shared integration preparation; the app repair
packet has not yet been imported or qualified at this entry.

### Original-journey pagination repair admitted for causal qualification

Imported donor `a25ac59bc0b15403871115ac5d4f2df06a7cf29b` as the exact eight-file
delta atop app `302a`, producing clean app
`2ad41de0b681528d65d8a534b251c82d53a4df04`, tree
`c6822541c58a7d1db7fe7c442c107a44e3e200ab`. No donor ancestry rebase occurred.
The clean source is published as `qualification/history-joint-source-2ad41de0b`.

The load-older operation captures the original lifecycle **attempt**, not just
its mutable lifecycle object. Each actual older/recovery/retry GET registers a
fresh UUID synchronously before dispatch. Terminal or replaced/resumed attempts
admit no subsequent recovery registration or response observation. The typed
API excludes simultaneous `open_id` and `request_id`; actual cursor reads keep
`open_id`, fresh-tail recovery uses `request_id` without `open_id`, preserving
backend initial-open stage scope. No Rust/backend/collector/schema/capacity or
sampling change is included. Normative REQ-CVT-003 and executive coverage match.

Parent independently verified source hashes, genuine composite `tsc -b --noEmit`,
**553 focused tests across12 app files**, shared **131 Node**, lifecycle **70
Python**, artifact admission **8 Python**, workflow YAML parsing and diff check.
Read-only independent exact-commit review found no blockers; a separate shared
follow-up review found no remaining concrete issues. Owner reported an existing
alias-delete test transient failure, isolated pass and final full553 pass; the
parent full553 run also passed. Existing fixture warnings remain in logs.

Source/spec preflight reviewed the exact API query names, mutually exclusive TS
shape, unchanged backend initial span predicate, current requirements/executive
agreement and timeless requirement text. No Allium file or generated wire type
changed. Local native/browser/build execution was not performed.

Workflow/native/browser source pins and source-tree admission are updated
atomically for **one causal normal qualification**. The dedicated diagnostic
branch remains frozen at `191dac` and its original artifact/app pins; it is not
pushed or rerun by this candidate. Controlled six-report acceptance, stock
comparison, overhead and full acceptance remain unproven until actual results.

### Full-source gate exposed before controlled browser execution

Run `38989132913`, shared `b099397b499a31474e7dab731de8e9dc73c19c16`, app
`2ad41de0b681528d65d8a534b251c82d53a4df04`, is **fully terminal failure**.
Rust (`117267398630`) and all standard lanes passed. The witness
`117267336253` passed both separate UI builds/admission, codegen, owned Coordinator
fixture and libtest capture, but failed full pinned-app UI qualification:
tsc/eslint/stylelint passed; Vitest had1 failure,3586 pass,2 skip across170 files.
The failure was `InlineMessageReaction.test.tsx`, `uses touch presentation when
reselecting a tokenless source`, expecting `Docked reaction` but observing the
floating `React to selected text` region. Controlled Chromium preparation and
all six controlled journeys were **skipped**. No pagination runtime acceptance
can be inferred from this run.

Original artifact `12084009960` is95,560,135 bytes. Bounded index/log/receipt
reads were retained locally; selected UI log and receipt fetched73,266 bytes.
The absent browser result was checked against the ZIP index, not manufactured.
No unchanged run or full-suite retry was performed.

The sole app-source owner ran the unchanged named test (1 pass) and whole owning
file (30 pass); this did not reproduce the hosted failure or establish a product
root cause. Source inspection shows pointerup schedules/cancels the owning
animation frame and `read` applies presentation there. A test-only delta aligns
this tokenless test with adjacent frame-driven regressions: fake only rAF and
cancel-rAF; assert no selection before first frame; advance owning frame to the
floating pill; assert floating remains before touch frame; advance to docked
presentation and require exact retained tokenless source equality. No product
logic, timeout, retry, expected touch result or source identity was weakened.

Imported donor `b17af62db296930ab77b0cd9f72147b0ace7fea8` as exact one-file delta
atop `2ad`, producing clean app `35f7cfbc8dcab2d454d5bb07ad3f808dfa31b39f`,
tree `a548be3b0ad35596434018068e297bf3f5066af5`, published as
`qualification/history-joint-source-35f7cfbc8`. Parent reviewed the exact diff,
independently passed named1/owning30 and genuine composite tsc-b; shared131 Node
and70 Python also pass at the new pin. The earlier553 app/8 artifact tests apply
to its unchanged pagination-product parent. Owner reported independent read-only
approval, changed-file lint pass, and retained transient warning logs without
claiming a cleanup/product fix.

One changed-candidate normal qualification is authorized to exercise this
completion-boundary correction and then the still-unqualified pagination repair.
Stock sampling/unsupported resume/overhead false/full acceptance false unchanged.

### Global canonical source target and failure-receipt masking

Run `38990862125`, shared `d01e0d04b4cd90342abe74f03d3bdadd4a9c4076`, app35f,
passed the complete pinned UI gate (retained exit0 receipt) and entered controlled
browser execution. Witness `117272743090` failed after45s. Rust `117272797999`
was still running at this entry; no whole-run terminal claim is made yet.
Original artifact `12085511636` is96,383,480 bytes. Bounded browser receipt reports
only outer lifecycle/capture/operation; no scenario or trials survived.

Independent retained OTLP receiver capture has2190 spans across9 batches with
zero receiver errors, but only two `browser.conversation_view` exports: ordinary
ready/sample1 and Global canceled/sample1. The Global canceled export lacks
resolved product identity and snapshot/meaningful timestamps. These are actual
partial facts, not proof of six reports or the underlying masked failure phase.
The failed browser path did not establish normal app-first drain; retained drain
reports app_stopped=false/receiver_stopped=true. No acceptance is inferred.

Source investigation found two shared issues:

1. The controlled target factory combined Global **latest** route with the
   **oldest** source pin. CoordinatorPage intentionally replaces that entry with
   `/global/<source>` preserving query/hash, changing location.key and canceling
   the unresolved admission. The factory now enters the canonical source slug
   directly, preserving the same exact pin, encoded message, independently
   selected anchor and latest composer owner. Ordinary route is unchanged.
   This avoids rewrite; it does not accept cancellation as historical ready or
   relax original-route/single-admission assertions.
2. Failure construction called `getReports` again, repeating fixture assertions
   that could throw for an unresolved Global canceled report and erase the
   original scenario evidence. Failed receipts now project through the actual
   bounded allowlist parser without fixture assertions, count/reject invalid
   bodies, retain completed trials and label context-creation/final-inventory
   phases. Diagnostic-only reports cannot grant acceptance. A regression uses
   the actual pinned captureReport parser to reject an unknown private field
   without retaining its body.

The earlier references here/handoff to a production `sourceLinks` file were
inaccurate for the frozen app: no such file exists. Actual authorities are
CoordinatorPage.tsx/tests, App.tsx/alias tests, and MessageComponents.tsx origin
links. The source owner independently confirmed canonical direct Global entry;
parent independently ran both actual route suites (43 tests pass).

Global product identity is unknown at admission and supplied only after snapshot
resolution. Thus only expected pending Global incomplete/canceled scenarios may
truthfully omit it; any supplied identity must match the fixture. Historical
ready and all ordinary identities remain required. No identity is fabricated;
actual pending finite request, producer/network owner, report route, frontend,
exact outcomes, unique admission and six-report collector/export checks remain.

Cached validation:136 shared Node tests,71 Python lifecycle tests,43 actual
CoordinatorPage/App alias tests pass; diff check passes. An intermediate new
report-route assertion moved an existing negative failure earlier and broke its
expected failure-label check; that unnecessary assertion was removed, retaining
the unchanged final strict route check and original negative regression. Source
app remains clean35f; no product edit, local browser/native/build or unchanged
normal qualification rerun occurred for this diagnosis. Independent scoped
read-only review found no blockers in the target/identity/diagnostic changes.

Run `38990862125` is now fully terminal **failure**: isolated witness failed;
Rust and all standard lanes succeeded. The canonical-source/failure-projection
candidate is committed only after that complete result, with unchanged app35f.
A single causal normal qualification will exercise the corrected Global entry;
there was no repeated diagnostic or unchanged full run in this interval.

### Both historical routes clear; pending ordinary identity is unresolved

Run `38992494875`, shared90ad/app35f, witness `117277723793`, failed at
**ordinary/pagehide/departure_report_wait**. Rust `117277778971` was still running
at this entry. Original artifact `12085831605` is96,394,534 bytes; bounded
browser receipt/log fetch78,376 bytes.

Unlike the previous masked receipt, this actual receipt retains **both** prior
historical trials: ordinary and Global ready, each one report, each exactly two
older GETs owned by its original journey. Both completed strict physical target
acquisition/four-frame stability, exact fixture/source pin/original canonical
route, current editable composer, original request/stream ownership and final
context-drain inventory checks. These are partial controlled scenario facts,
not six-report collector/export/drain acceptance.

The ordinary pagehide failure retained one actual incomplete report with one
request correlation, one admission, zero network errors, no collector response
(as permitted for departure), no remaining pending request, and **no product
identity**. Source ProductConversationPage constructs its local journey with
only `enabled` and enriches product identity from the accepted snapshot, just as
Global does. The previous Global-only absence exception was too narrow.
Expected pending incomplete/canceled reports on either route may now truthfully
omit unresolved product identity; every supplied identity must still match, and
historical-ready identity remains mandatory. No value is invented. Actual finite
pending request, original producer/network/report join, frontend, route, exact
outcome, collector/export and strict historical checks are retained.

Source review also found the adjacent pending Global route/API mismatch before
running into it: fixture endpoint/fault filter identifies the owning root, but
an unpinned `/global/<latest>` entry resolves ProductPage's API reference as that
latest slug, not the root. Controlled pending Global now enters unpinned
`/global/<root>` with no source query/hash and asserts that exact original URL
after observing the pending root GET. The source owner independently confirmed
this valid same-owner route retains ensure/resolve prerequisites and calls the
exact root API endpoint. Stock aggregate fixtures/routes are not changed.
Historical pinned Global entry and pending unpinned root entry remain distinct.

Cached shared138 Node tests pass, including both-route unresolved pending
identity with exact finite correlation and root-entry/no-query/no-hash checks.
An intermediate new test assumed the retained request-ID strings were objects;
its expectation was corrected to the actual string schema without product or
oracle change. Existing71 Python/43 actual route tests apply to unchanged target
factory/production source. No local native/browser/build, no app-source edit,
no unchanged normal rerun or diagnostic runner was performed.

Run `38992494875` is now fully terminal **failure**: isolated witness failed at
ordinary pagehide; Rust and every standard lane succeeded. App35f remains clean
and unchanged. The changed pending-identity/root-reference candidate may now
receive one causal normal qualification; no unchanged run was retried.

### Exact nullable field correction, replayed against retained report

Run `38993749887`, shared3d97/app35f, witness `117281557850`, retains both history
ready trials but fails at the same ordinary/pagehide report-wait phase. Artifact
`12086502244` is96,402,250 bytes; bounded receipt/log fetch78,429 bytes. Rust
`117281628754` was still running at this entry. This is not an unchanged rerun:
the candidate changed pending identity handling/root entry, but its nullable
field interpretation was wrong.

**Correction to earlier prose:** the actual incomplete reports in both90ad and
3d97 do **not** omit `product_conversation_id`; the key is present with explicit
JSON **null**. Earlier summaries used Python `.get(...) is not None` as a
presence proxy and incorrectly described null as omission. The pinned app type
is `string | null`, with total serialization. The source emits null until the
snapshot enriches identity. Frontend source matches the historical ready report.
The preceding patch tested undefined instead of null, so it still asserted
`null == fixture ID` and failed. This was a shared boundary/test-fixture bug,
not a product data-loss or transport failure.

The check now accepts **explicit null only** for the expected pending terminal
outcome; absent/undefined is not treated as an alternative encoding. Ready and
any nonnull supplied identity remain exact fixture matches. Positive unit
reports serialize the same explicit null; the actual pinned parser is exercised
through a complete six-scenario fake transport with four null pending reports.
No ID, response or acceptance evidence is fabricated.

An independent compiler-free replay extracts the actual controlledObserver
function via the cached TypeScript AST, imports the actual pinned captureReport
parser, and feeds the **exact retained3d97 report**: old undefined check fails
red with AssertionError; fixed null check passes green. The replay prints only
static labels, not private IDs/URLs/bodies. Evidence and code remain under the
local evidence directory. All138 shared Node regressions pass; diff check passes.
No additional native/browser/build/runner was used to establish this causal fix.

Run `38993749887` is fully terminal **failure**: isolated witness failed; Rust
and all standard lanes succeeded. One causal normal qualification of the exact
retained-report-proven null correction follows this complete state, with app35f
unchanged. Stock baseline/resume/overhead/full acceptance constraints remain.

### Six browser scenarios and original raw semantic exports pass; wrapper path blocks drain

Run `38995018348`, sharedca514/app35f, witness `117285561573`, produced actual
browser status **passed_assertions** for all six distinct contexts: ordinary and
Global history ready; ordinary and Global pagehide incomplete; ordinary and
Global same-document Sidebar cancellation canceled. Both histories retain two
original-owner older GETs and all strict physical/composer/route checks. The
unauthenticated negative made one POST and received401, expected acceptances0.
This is actual controlled browser evidence, not a mocked or diagnostic recipe.

Native qualification then failed **after** driver success with FileNotFoundError
at its duplicate write to `ordinary/telemetry/telemetry-browser-result.json`.
Controlled mode owns `ordinary/browser-result.json`; unlike the stock child,
it never creates a telemetry subdirectory. The wrapper attempted a redundant
copy before collector join/normal server stop. Retained drain shows
app_stopped=false, receiver_stopped=true. Thus normal app-first drain and the
complete controlled acceptance packet are still **not** established.

Original artifact `12087087150` is96,476,200 bytes. Bounded browser receipt/log
read81,221 bytes; native result/lifecycle/drain read82,537 bytes. To falsify later
semantic-join blockers without another live run, selected the original capture
index and all16 raw protobuf batches using320,458 range bytes. The unchanged,
source-hash-checked pinned `final_index` validated exact raw inventory/bytes and
decoded equality (1,317,489 raw bytes); its actual retrieve/validate_view/
validate_correlations functions passed all6 authoritative journey expectations,
with exactly6 exports and zero receiver errors. This was an **offline raw-export
semantic check**, not assertPhoenixTelemetry with a fabricated successful drain.
The actual complete drain assertion remains unavailable for this failed run.

A further bounded148,410-byte range read retained the original runtime log.
The actual collector_acceptances parser independently matched exactly6 unique
browser reports to6 acceptances; no extra accepted journey or export exists.
Together: **6 browser reports = 6 actual collector acceptances = 6 actual semantic
exports**, with the six expected outcomes and negative401, but **without normal
app-first drain**, so no full controlled qualification claim is made.

The structural fix removes the redundant controlled copy. All collector,
post-drain semantic assertion and final delivery consumers select the **one
existing controlled browser receipt**; stock still selects its unchanged nested
telemetry receipt. An AST-based regression executes the actual qualifier path
expression in a temporary case containing only browser-result.json and no
telemetry directory, then verifies the same path threads to all three consumers.
No directory-creation fallback, fake receipt, weaker join or drain bypass exists.

All72 Python lifecycle tests pass; an intermediate new test omitted local ast
and pathlib imports, corrected to import ast and use already-imported Path.
Existing138 shared Node tests apply to the unchanged browser driver. No local
native/browser/build or hosted diagnostic rerun was used. Rust `117285608206`
was still running at this entry; next normal push waits for the whole terminal.

Run `38995018348` is fully terminal **failure**: witness failed at the redundant
controlled receipt write, Rust and all standard lanes succeeded. The single-
receipt correction receives one causal normal qualification after this complete
result; app35f and the six-scenario driver are unchanged. No fabricated drain or
unchanged full run is used to promote the preceding partial evidence.

### Normal app drain requires the owned stop command before waiting

Run `38996536868`, sharedc4ea/app35f, witness `117290100319`, passed all6 browser
scenarios and the pre-drain collector join, then failed normal server completion
with the existing65-second timeout. Original artifact `12087702470`
(96,479,038 bytes) retains status timeout/returncode null and owned home/socket
not yet removed; app_stopped=false, receiver_stopped=true after forced cleanup.
Bounded receipt/result/drain read83,650 bytes and runtime log read153,586 bytes
were retained. No normal-drain acceptance is claimed. Rust `117290170705` was
still running at this entry.

Exact shared source inspection identified the omission: normal_server_completion
only called server.wait. The owned libtest does not exit until its gate receives
`{"command":"stop"}`. Only forced_server_shutdown sent that command, after the
normal wait had already failed. Runtime logging shows real shutdown beginning
then. The successful earlier artifact-reuse diagnostics explicitly sent stop
and received its acknowledgement before waiting, on this same source protocol.
This is a missing lifecycle transition, not evidence that65 seconds is too low.

Normal completion now sends the owned AF_UNIX stop request while the receiver
is still live, requires the compiled protocol's exact acknowledged stopped
response, then performs the **unchanged65-second** wait, exit0 and owned
home/socket-removal checks. Control exchange uses one absolute2-second budget,
bounded4096-byte framing, partial-read support and no raw acknowledgement/error
publication. Failure remains primary and cannot claim app stopped/drain success.
Already-exited processes still undergo the same exit/cleanup validation; forced
cleanup remains unchanged and classified separately.

All76 Python lifecycle tests pass. New tests mock sockets entirely (no local
server): exact/fragmented acknowledgement, malformed/missing/oversized replies,
one absolute deadline, stop-before-wait order and sanitized primary failure.
Independent scoped source-only review found no blockers and verified the exact
frozen Command::Stop/ack protocol, receiver lifetime and strict exit/cleanup
checks. Existing138 browser selftests and immutable app35f source are unchanged.
No local native/browser/build, raised timeout, fabricated drain or unchanged
qualification rerun was used. Next push waits for the entire current run.

Run `38996536868` is fully terminal **failure**: witness timed out waiting for
unsignaled normal stop; Rust and every standard lane succeeded. The acknowledged
normal-stop candidate is published only after this complete result. App35f,
six-scenario browser driver, raw semantic validators and deadlines are unchanged.

### Current-main landing successor: preserved baseline, stronger executable gates

The exact green baseline remains immutable: PR908 sharededac/app35f, whole run
38997749429, with durable accepted evidence in PR comment6229102693. It is not
replaced, rebased or retrospectively reclassified by the landing work.

A separate integration/v013-history-current-main successor incorporates pinned
main d07417325246d1ad1787af468b168bdae397c067. Live PR898 is the common product
source (3a7ea), PR908 targets its older qualification base8a273; the successor
will target main rather than pretending the old stack is directly mergeable.
The product writer composed original-boundary/coordinator semantics with main's
unpublished-creation filter, MATERIALIZED selected IDs, async event presentation,
SSE and typed wire/schema contracts. Independent backend/UI static reviews found
no demonstrated composition blockers; hosted Rust/codegen remain owed.

ADR namespace repair preserves every main ADR100–114 byte-for-byte, keeps115,
and assigns history116/pagehide117/span-schema118/controlled-build119. The merge
recognizes the old shared history100 as the same renamed116, leaving no duplicate
numeric prefixes. The source integration and baseline are separate evidence.

Current review-thread triage also identified real shared-oracle gaps. Successor
changes require all older GETs to complete successfully, await independently
signaled producer ownership, observe an actual successful composer POST to the
latest owner (guarding wrong/duplicate writes before dispatch), reject opacity0
on target/ancestors, require contiguous reconnect replay and exact acknowledged
chat authority, and restrict stock detail joins to successful completed fixture
GETs. Pending nonready requests remain intentionally pending; no positive oracle
was weakened. Red-first mutation tests preceded these changes. Independent
source-only shared review found no blocking defect; the successful baseline does
not claim this stronger new probe had already run.

Stop ownership is now explicit: controlled Python requests stop; stock's browser
already owns its stop and Python only awaits exit, avoiding a second socket
command during teardown. Standalone Cargo/Rayon/Rust worker caps are2, missing
physical/navigation Node tests are scheduled, phase bounds compose under a
90-minute job with cleanup/upload reserve, and outer witness420s contains the
230s browser bound +2s stop +65s exit +60s auxiliary reserve. The existing65s
normal drain bound is not increased. Frozen geometry reproduction now checks out
its exact191dac harness bundle alongside its original302a app/artifact98718;
it no longer imports moving source identity from the landing harness.

Parent independent cached validation at initial product integration1adc:
genuine composite tsc-b,689 tests18 app files,310 shared tests passing with2
existing optional-fixture skips. Python79 initially exposed9 errors:8 came from
its schema test's rsplit selecting a newly added Rust test string instead of the
actual migration; the fixture now extracts MIGRATION_091 explicitly. The ninth
was a real product-source preservation gate: donor b17 and independently
qualified35f are not byte-identical (10 file differences), and donor's cfg(test)
opening helper lacks the qualified password/home-permission packet. The product
writer is composing the entire qualified35f delta into the current-main source
before any hosted qualification. This is not papered over by changing the auth
assertion or inheriting green35f qualification for a different tree.

No successor hosted runner, local native/browser/build, production action or
merge has occurred at this entry. Overhead and native/device resume remain
separate unadopted decisions, not blockers to executable source integration.

A further bounded historical review found retained offline replay/source-check
issues, distinct from product/runtime behavior: normal non-JSON SSE ping parsed
as JSON before event filtering; contradictory duplicate IPC opening receipts
silently overwrote earlier authority; Global cache selftests implicitly required
ignored private artifacts at module evaluation. Successor fixes ignore only
unrelated event types before parsing, reject conflicting full-edge/client-key
duplicates with a fixed nondisclosing error, and require explicit opt-in paths
for artifact/source-backed groups while keeping pure unit tests portable.
Red-first failures and green25 tests (18 pass,7 explicit retained-input skips)
are preserved. Explicit invalid paths still fail rather than silently skipping.
Positive private-artifact groups were not requalified and are not represented as
runtime proof. The portable replay/cache suites are scheduled in successor CI.

### Qualified-source preservation closed; exact landing candidate prepared

Product merge0f6b75cfc059d11e273532c5eeb2222e6b99f6e6, tree
47d229c27705c4d216d3bd55f6bf31bdda474567, has exact parents1adc and qualified35f.
All10 donor-vs-qualified source paths were audited, not just the caught auth
helper. The cfg(test) opening helper is byte-identical to35f, including random
password/private home/create-new0600 credential. Existing main close-retirement,
bash and e2e hardening are byte-preserved. Exactly the two obsolete
probe-view-keepalive source/test paths (absent in both main and35f) are removed;
this is explicit qualified-source reconciliation, not silently deleted coverage.
The qualified Mermaid/reaction fixture timing corrections are retained. ADR
remaps/main byte-preservation and all15 main semantic resolutions remain intact.

The shared successor now merges this exact product ancestry and pins its clean
independent source branch qualification/history-joint-source-0f6b75cfc. Fresh
remote main remainsd074. Parent independently passed genuine composite tsc-b,
837 focused tests19 app files,79 Python lifecycle tests, and337 shared Node tests
(328 pass,9 explicitly optional retained-input skips), YAML and diff checks.
Independent final source-preservation review found no blockers; Rust compilation,
codegen, both actual UI builds and stronger actual native/browser qualification
remain hosted obligations, not inherited from greenedac/35f.

The successor PR targets main and the witness is explicitly scheduled for its
integration branch. No merge/deploy/production/device action is authorized by
preparing it. Overhead tolerance/capture and native/device resume remain distinct
unadopted decisions; full_acceptance/overhead_clearance remain false. The next
hosted run is one new exact-source causal integration qualification, not a blind
retry of the immutable baseline or a second concurrent runner.

### First current-main hosted run: typed fixture and ADR-shape composition repair

PR932 initial exact8425dc5/0f6 candidate run39005485369 is fully terminal failure
before browser execution. Rust/codegen/clippy found one new-main SseEvent::Message
test initializer missing required opening. UI/specs found the matching outer
opening omission in3 ConversationEventCard wire fixture cases, plus ADR119's
legacy Alternatives heading rather than required Options considered. E2E/task/
worker guards passed. Separate routine macOS run39005485335 succeeded; it is not
native/device resume acceptance. Artifact12090917594 (6,939,878 bytes) and full
failed job logs remain available. No unchanged rerun was requested.

Product fixfc39f4403b420d4dde33996c3cf131a96048c157, tree
2593aac43a5765130bdeabf1a6024d0c0818af0e, is an exact3-file child of0f6. Rust
fixture explicitly supplies openingNone and verifies null survives both redacted
and projected event presentation. TS positives use the generated message-wire
arm via satisfies, assert both opening and event presentation, and retain
missing-field/malformed negatives. ADR119 changes only the section heading;
point-in-time rationale is untouched. No runtime schema, generated type, product
policy, timeout or acceptance check was weakened.

Owner red-first reproduced all3 UI failures, then77 tests and composite tsc pass;
standalone Rust parse/format and exact source constructor audit are not Rust
compile evidence. Parent independently passed the same77, genuine composite
TSc-b,79 lifecycle tests, the actual full spEARS shape validator and diff check.
The clean immutable source is published qualification/history-joint-source-fc39f4403.
The shared landing successor composes this fix and atomically repins it for one
changed-candidate hosted qualification after the complete prior terminal result.

### Second current-main hosted run: real SQL matrix, lint and CLI composition gates

Run39006122848 at94eb/fc39 is fully terminal failure. UI/specs, E2E/tasks/workers
and routine macOS CI39006122822 passed. Genuine codegen, both isolated UI builds,
libtest and full pinned-app UI source qualification passed in the witness, but
browser launch did not occur: CLI still rejected --timeout420 because its
controlled ceiling was300. The budget source regression had checked arithmetic,
not the actual parser. The parser is now a side-effect-free function exercised
with the exact workflow timeout and negative missing-joint/above-cap inputs.
No output directory, executable, native service or browser is touched by this
regression. All79 Python tests pass; lower-level browser195s/launch230s and normal
exit65s remain unchanged.

Actual Rust execution exposed
late_hydration_matches_full_row_oracle_matrix: Database(SqliteError1,
"no such table: product_conversations") in its custom fixture. The composed SQL
now legitimately consults typed product kind/role; the source owner is repairing
the test schema/rows, not removing the product filter or weakening equality.
Nextest reported1160 pass/1 fail before cancellation of165, so no claim is made
that the remaining cases ran. Clippy reported the new coordinator fixture's
length, opening-helper import placement, view-capacity test length/style, and
SseWireEvent size after composing event presentation with opening. These are
explicit source obligations, not bypassed with blanket lint allows. The source
writer is preparing a bounded child packet with unchanged JSON/wire expectations.

Artifact12091277117 is96,607,629 bytes; exact failed Rust/clippy/witness logs and
terminal JSON are preserved. No unchanged or concurrent qualification was
started. The prior greenedac/35f baseline remains immutable; the two successor
failures are not retroactively qualified by it.

### Actual SQL/lint gate repair prepared without bypasses

Product aac4d9f05765d4e3c09ccb41bc3dbbb1742920bf, tree
7ab14388b4f638eb34108e0d6c93dd9c719c1846, is a clean4-file child offc39.
The isolated SQL matrix now supplies real product kind identity and traverses
both ordinary/coordinator role cases without changing production SQL or the
full-row15-column oracle. Parent independently replayed the extracted actual
SQL: priorfc39 fixture fails no-such-table;3150 current matrix comparisons match
optimized==full oracle with exact i64/tie/order boundaries. This is SQLite
execution, not Rust test execution.

Coordinator setup and capacity assertions are factored into bounded helpers;
52/32 capacity checks,44 fields/1030 key bytes, all8 marks, typed counters,
correlation/metadata and actual OTLP assertions remain. Message wire boxes its
large payload at its sole construction, preserving serde JSON and explicit TS
unknown shape; imports/closure/semicolon corrections address actual lint reports.
No blanket lint allowance or runtime schema relaxation was added. Independent
bounded static review found no blockers; immutable helper scope/type corrections
were included before commit. Parent independently passed tsc-b and79 lifecycle
regressions at the new pin. Genuine Rust/clippy/codegen remain hosted gates.

The source branch qualification/history-joint-source-aac4d9f05 and all three
shared pins select this exact clean tree; the composed CLI accepts the workflow's
420s outer bound under actual parser tests. One changed-candidate hosted run is
prepared after the previous whole terminal, with no duplicate runner, native
local build or expansion of overhead/device acceptance.

### Third successor run: current-main async creation and remaining source gates

At b934/aac4, run39007594676 passed the prior SQL-lint compilation/codegen gates
but witness117323187731 failed before browser with KeyError transcript_row_id.
The current-main create API returns HTTP202 acceptance, not a synchronous
published transcript. Shared Fixture.build had still indexed the acceptance as
an old published receipt. It now requires202, validates request/product identity,
polls the actual creation observation endpoint under a20s bound, accepts only
the server's published transcript/canonical route, and then retains the existing
runtime completion/persistence checks. Failed/cancelled/unknown or mismatched
observations fail closed; no transcript identity is synthesized. The main E2E
scenario uses the same contract; no additional observation mutation is required.

Nineteen compiler-free fixture tests and79 lifecycle tests pass, covering
acceptance→pending→publication, wrong identities/routes/rows/status, terminal
failure/cancellation and timeout. The tests use in-memory HTTP mocks, not a local
service. This fixes a concrete API composition error rather than weakening the
witness or retrying its unchanged candidate.

Other completed jobs exposed two remaining capacity assertion helpers over100
lines and a mounted conversation-stream test asserting its fake EventSource
before attachment. These source gates are commissioned separately. Rust is
still in progress at this entry; no next candidate is published until its whole
terminal state is established. Artifact12090904686 is96,845,740 bytes; bounded
native result/browser selections and completed-job logs are retained. No
browser scenario progress or source acceptance is claimed for this run yet.

Run39007594676 is now fully terminal failure with **actual Rust lane success**;
E2E/task/worker guards and routine macOS CI also passed. The remaining failures
are the two capacity helper line limits, one mounted-stream fixture timing case,
and the shared asynchronous-creation fixture error, all independently scoped.
No browser scenario ran in that failed witness.

Product ec710d65420c448b4a716e3e966a48d6aa74e934, tree
5ba7814e9695595779180b56165947c32ee6a081, is a clean2-file child ofaac4. Capacity
helpers are further factored while preserving22 base+8 marks+14 observations,
all44 attributes and real OTLP accounting. The UI test now awaits actual fake
stream attachment before emitting the same update, with unchanged waitFor
budget/no retry; no product behavior is changed. Parent reviewed the exact delta
and independently passed192 owning page tests, composite tsc-b and79 lifecycle
tests. The source owner also passed named/owning/lint/rustfmt; Clippy remains the
actual hosted gate rather than a physical source-line-count claim.

The exact source branch qualification/history-joint-source-ec710d654 is published
and all source pins select it. Shared19 creation regressions and79 lifecycle
regressions retain strict publication/identity/normal drain semantics. One next
changed-candidate normal qualification follows the complete preceding terminal
state; no unchanged/parallel witness is launched. Baselineedac/35f and all scope
boundaries remain unchanged.

### Stronger current-main witness passes; whole run blocked only by three semicolons

Run39009309679 at0873/ec710 is fully terminal failure **only in clippy** for
three trailing assertion semicolons. Actual Rust, UI/specs, E2E/tasks/workers,
routine macOS and the full isolated witness passed. Artifact12092068190 is
97,808,406 bytes; selected native/browser/drain/delivery receipts used90,737
range bytes, not a full local artifact download.

The actual stronger current-main witness passed all6 scenarios: ordinary/Global
history ready, pagehide incomplete, cancellation canceled. Both histories retain
2 successful completed older GETs under the original owner, strict physical
containment/four frames/source/canonical route, and actual latest-owner composer
POSTs with200 (not merely expected owner labels or independent streams).
Exactly6 browser reports=6 collector admissions=6 semantic exports; negative401
has no extra admissions/exports. Normal drain is true: acknowledged owned stop,
app exit0, owned home/socket removal before receiver close; drain errors empty.
This is successor controlled-functional proof, not full acceptance/overhead or
shipping/device proof. Original baseline remains unchanged and separate.

Product26d40cb8d61f3e0fe80fb520ad5a3e1af49abe84, tree
7d8eb4c05f1c50d39f0b90d76783f007101a7f34, is an exact one-file3-line child ofec710:
only String/I64/F64 assertion semicolons at the reported locations, with no
assertion/value/JSON/behavior change. Parent inspected the complete immutable
diff; standalone rustfmt and diff check pass,79 Python tests pass at its pin.
The source branch qualification/history-joint-source-26d40cb8d is clean/published.
The shared candidate receives one causal final normal qualification after the
entire preceding run, rather than treating the previous near-green as full green
or skipping Clippy. No duplicate/unchanged runner, local native/build or merge.

### Exact final source passes all functional/lint gates; full Rust process budget exhausted

Run39011013756 at0d34/26d is fully terminal failure only in Rust's outer cargo-test
1200s process budget. Clippy, UI/specs, E2E/tasks/workers, routine macOS and the
stronger six-scenario witness all passed. Artifact12093455564 is97,809,322 bytes;
92,025 range bytes retained the actual native/browser/drain/delivery/final facts.
Six reports/admissions/semantic exports,2 actual latest-owner POST200s, two older
GETs per historical route, negative401/no extras and actual normal app-first
drain all passed. Final raw capture reports16 batches/1,324,543 bytes/2472 spans.
This remains controlled-functional proof, not overhead/device/full acceptance.

Rust's actual terminal summary was4670/4809 tests run:4667 passed,3 failed by
SIGTERM from the outer timeout,184 skipped;139 not run due to cancellation. The
three canceled tests had only0.188s,0.490s,0.882s elapsed when terminated. There
was no assertion failure/panic or evidence of a stuck test: the expanded suite
was still making normal progress after1073.450s execution, with compilation
included in the1200s command budget. The previous exact product ancestry had
completed Rust successfully. It would be false to call this run wholly green,
or to treat externally canceled tests as three product assertion defects.

The causal successor changes only Rust CI execution budgeting:1500s cargo-test
under40min job, with600s non-test work+300s reserve enforced by an offline source
regression. The same full test selection, musl production check, worker settings,
assertions and individual test timeouts remain unchanged. No retries/skips/
continue-on-error or test threshold relaxation were added. This is a bounded
process-completion budget, not a numerical performance/overhead tolerance.
All80 lifecycle tests and YAML/diff checks pass. Product26d and its tree remain
immutable/unchanged. One changed-workflow qualification follows the entire
prior terminal state; no parallel or unchanged rerun is submitted.

Two isolated bounded final reviews used exact d074..0d34 and returned No
 actionable findings for selected product/qualification seams, explicitly not
exhaustive177-file proof and not external runtime confirmation. Their evidence
is retained separately; the budget-only successor is a different exact target
and does not silently inherit an identical-target label.

## Review-driven chapter-owner and shared-stream successor

Two new exact-head review findings supersede the earlier review-ready assessment
of shared1e042321. App52ec18b65bcf757ff40d8e5c0ed74982b2512afb,
treecdd9297ef0f5ed0577014b0b8c2f9a09b409ee51, is parent-linked directly to
qualified26d40cb8. Only NavStack source/tests and shared SSE handlers change.
Persisted aggregate chapter selections establish source_transcript even from a
hashless Global member route; unpinned ordinary fallback stays hash-only.
Shared SSE keeps token validation/subscription outside the existing bounded
three-attempt stable read, refreshes generation/messages/opening together, and
maps only the typed snapshot-change error into retry. Real persisted append,
mutation and three-attempt exhaustion regressions are authored; no runtime is
created by the share viewer.

The shared owner independently passed216 UI tests, cached tsc,80 Python lifecycle
checks with the required cached protobuf dependency,69 joint ownership guards,
YAML parse and diff checks. A first Python invocation without protobuf failed
its import and is retained separately, not treated as a source failure or pass.
Independent exact-parent source review found no actionable blocker; it did not
compile Rust or execute hosted jobs. New Rust regression execution, full CI and
rebuilt controlled-functional witness remain owed at the successor shared head.
Workflow/Python/JS app pins move atomically to52ec; earlier green1e04/26d and
PR908 baselines remain preserved. Separate native-capability artifact reuse and
read-only performance-fixture investigation do not amend this app packet or
provide overhead, physical-device or full acceptance. No numeric overhead
threshold is adopted; improved measurement quality proceeds without one.

## Orphaned synthetic keepalive workflow disposition

A later review found that the optional four-cell synthetic keepalive workflow
referenced probe sources absent from both current main and the pinned qualified
app lineage. The obsolete workflow is explicitly retired rather than restoring
removed legacy scripts or claiming a probe can execute. Its original source is
preserved in713c and earlier Git history; no baseline evidence, source workspace,
current native/codegen qualification or regression file is deleted. A focused
source-reference guard failed on both missing files before retirement; the full
81-test Python lifecycle suite passes after retirement. A future optional probe
workflow must ship its real sources together. This is a shared workflow/test
change only, not a product runtime or telemetry-acceptance change.

## Current-main owned Chrome teardown qualification candidate

App7906d436de340ea12e5e57bfb0b7459ec77d866d,
tree8ef968e711c2d0730de19dd588f6efc8128c0b81, composes main0f054466
with the preserved52ec chapter-owner/shared-stream repairs and a separately
reviewed native launch-group teardown candidate. Chromiumoxide's public API
is reused via native existing-executable reexec, not a dependency fork or
argument reconstruction. The root remains unreaped while exact owned group
signals/drain are authorized; group-marker deletion requires retained group
proof, and failed/unadopted launch profiles remain fail-closed. Escaped-writer
coverage explicitly demonstrates that group drain is not arbitrary descendant
closure. Actual supported Chrome layout is a qualification dependency, not a
claim inferred from that negative test or libtest RootOnly behavior.

The shared atomic source/tree/base pins and all emitted receipt bases move to
7906/8ef968/main0f054. Both existing Linux pinned-app qualification and existing
macOS workflow schedule exact group/marker/actual CLI+server shim tests plus
the real lost-Chrome test with required before/after native layout receipts.
Every exact test requires an actual one-pass result, server shim requires two,
and skipped Chrome cannot pass the layout-receipt guard. Full Rust/Clippy/UI
and rebuilt six-context history witness remain mandatory. No unchanged full-suite
retry, deletion retry, sleep-based cleanup assertion or threshold relaxation.

Parent compiler-free checks pass81 Python lifecycle tests,69 joint-pin guards,
YAML parse, shell syntax and diff checks; independent exact-source reviews
found no established native blocker and corrected stale auxiliary receipt-base
values before publication. No local Rust/Chrome candidate execution is claimed.
The earlier supplementary retained-binary macOS topology observed one group,
but is not exact Linux failure reproduction or candidate proof. Native compile,
Linux/macOS process behavior and all complete hosted gates remain owed at this
new shared head; prior1e04/26d,713c/52ec and PR908 evidence are not relabeled.
The orphaned optional keepalive workflow retirement is published in this
cohesive failure response rather than left indefinitely local. Separate reusable
observation benchmark development is not imported into the history packet.

## Native candidate compile and macOS exited-child correction

Sharedbcd8 finished both Linux and macOS qualification terminal failure before
any new actual Chrome layout/six-context witness could execute. The collected
causes were the missing direct IDE browser dependency, two server-test Cargo
binary identifier typos, two Clippy style diagnostics, unpinned macOS Actions,
and a real macOS native fixture failure reading exited-root birth identity.
All original assertions and failed receipts are retained.

Appa236ea6eba432d278739f6ec3d6cd144108c1173,
tree5ccd80b271a48cbfe40006abeeb17455f521b9d2, incorporates only those causal
source fixes. Cargo metadata generated the one required lockfile dependency
edge offline without a compiler, network or version update. The exited-root
repair binds every observation/signal/drain to the exclusive actual Tokio
Child handle and original PID; WNOWAIT precedes unavailable zombie proc info,
while live roots still require birth and isolated-group identity. Tests retain
repeated WNOWAIT observations and reject different/reaped child authority;
Drop never signals a numeric group without the child. This does not claim
protection from an external raw-libc reaper stealing an exclusively owned child,
nor does group drain establish escaped-descendant closure.

Independent exact-child source review found no established blocker, not an
execution pass. Parent reran81 Python lifecycle/69 joint guards/desktop Actions
security regressions successfully, plus YAML/shell/diff checks. All Actions
added to macOS qualification are pinned to resolved full commit SHAs. Source,
tree and all provenance pins move atomically to a236/5ccd; main0f054 remains
the base. Exactly one fresh causal full Linux/native/macOS qualification follows
after both preceding runs are terminal, with no ignored test, weakened assertion,
cleanup retry, synthetic lifecycle event, threshold or acceptance waiver.
