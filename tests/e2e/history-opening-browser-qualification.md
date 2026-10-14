# Instrumented opening browser qualification

This harness is **instrumented runtime evidence, not shipping-binary proof**.
`history_browser.py` and `qualify-history-realserver.mjs` remain unchanged and
qualify the separate older shipping app. No new HTTP control endpoint exists.

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
  response headers and telemetry acknowledgments. No response-body read is
  attempted. Browser request payloads, including telemetry POSTs, remain in
  `browser-requests.jsonl`. Telemetry ack bodies are not owned history evidence;
  this harness imports no telemetry readiness or telemetry qualification claim.

Every API response saves immutable URL/method/status/content-type, request and
response times, request ordinal and navigation epochs **before** any body read.
Ordinals come from a WeakMap of actual Playwright Request objects and are labelled
local ordinals, never represented as CDP request IDs. Failures repeat their owning
metadata in `browser-capture-errors.jsonl` and response outcomes. Absent response
status/content type/time are explicitly null on missing-response/requestfailed
outcomes. Application `pageerror` events have a separate file and result array.

End-of-case seal removes the response/request producer before draining all
outstanding required tasks under their deadlines. `browser-capture-drain.jsonl`
records terminal outcomes and `partial:false`, even when qualification fails;
that field means receipt drain is complete, **not** that assertions passed.
Missing required responses fail. A real requestfailed transport outcome is not a
body-capture error; only an exact aggregate GET while the owned roster fault is
active is an expected exception, and the original failed-endpoint behavioral
assertion remains mandatory. A status200 body failure is never forgiven by that
exception. Both application errors and capture errors must be empty; cleanup
failure still forces failed status/exit. Controllable response mocks exist only
inside compiler-free selftests and invoke the actual installed producer.
