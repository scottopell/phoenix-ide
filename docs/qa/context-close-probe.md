# Stage 1: synthetic bare-Playwright context-close functional probe

## Question and authority

Can a response event joined to the **exact original Playwright Request object**
be observed after that owned context's `close()` promise resolves? A response
while `close()` is merely pending is a different observation. This is a bounded
functional question, **not** a performance test, reporter test, Phoenix collector
qualification, or explanation of a historical failure.

This packet is prepared from `5d2a62905`. No real probe result accompanies it.
`context-close-probe.template.json` is an **unexecuted sanitized shape template**:
its zero identities, empty cases, and placeholder revision are not observations;
it deliberately cannot pass `functionalPass`. Only a future hosted run's
`context-close-evidence/result.json` is runtime evidence. Fake-object unit tests
prove neither Playwright ordering nor response-after-close reachability.

The explicitly authorized isolated hosted job in the existing CI workflow is
`context-close-functional-probe`. Its entire job requires all of:

- `pull_request`, with head repository equal to this repository;
- exactly `prepare/exact-f16-stock-admission` as head branch;
- PR event label snapshot containing `offline-regression-only`.

The stock `qualify-history-realserver` job retains its exact **negative** label
guard and is **SKIPPED**, not passed, on that labeled PR. Other existing CI gates
are not waived or repaired here. The parent owns label verification, publication,
and hosted execution. Label before the next synchronize event: default PR
activity types do not start a run on label changes, and a later label cannot
retroactively cancel an already-started stock job. No local browser, server,
network/GET, install, build, Cargo, DB/SQL, or push is authorized by this packet.
Roadmap context was not refreshed because local network is prohibited.

## Hosted setup and isolation

The job checks out the exact PR head, disables checkout credential persistence,
uses Node 26 and `pnpm/action-setup@v4` reading the repository's pinned
`ui/package.json#packageManager` (`pnpm@11.0.8`), and installs with
`pnpm install --frozen-lockfile --ignore-scripts`. The existing lockfile resolves
Playwright **1.61.0**. A normal hosted `playwright install --with-deps chromium`
installs its Chromium; there is no local or protected browser connection. The CLI
checks both Playwright and playwright-core versions before launching. One fresh
owned headless browser contains four fresh nonpersistent contexts, used serially.
No Phoenix application, reporter, auth, production endpoint, telemetry sampling,
48-trial cohort, history fixture, or random sample decision participates. UUIDs
come from `crypto.randomUUID`, not `Math.random`.

Each context loads a synthetic page from a newly owned `127.0.0.1` HTTP server on
an OS-assigned ephemeral port. The page issues exactly one same-origin browser
POST to the held collector. Its JSON is exactly `{version: 1, runId, caseId}`;
both identifiers must be UUIDv4 and match on browser request and server receipt.
No auth or user content is involved. The response/failure observers join by
`response.request() === originalRequest` / exact failure Request identity, **not**
by URL or matching body alone. The body/URL and request objects stay in memory.
The collector's receipt barrier resolves after the complete small body validates.

## Four schedules and observed boundaries

All schedules await **both** the browser request event and the collector's full
request barrier before closing. The observer remains attached through owned
context and server cleanup. Cases run once each, with no retries.

| Case | Choreography | What is and is not guaranteed |
| --- | --- | --- |
| BEFORE | Release 204; await exact-request client response; then call and await context close. | Positive control requires observed client status 204 before `close_begin`. |
| DURING | Call context close; on the next explicit Promise continuation, record that its promise is still pending and release 204; await close. | Release must fall strictly between `close_begin` and `close_resolve`. Actual response delivery while closing is **not guaranteed**; retain its observed partial order or classified absence. A close already resolved at release fails the schedule barrier. |
| AFTER | Await context close resolution; then attempt release 204. | Socket may already be destroyed. Retain write availability, server finish/close, client response/failure independently. No response is an observation, not a failure alone. |
| NEVER | Await context close; force-dispose any remaining owned held response/socket; do not release a response. | No response is expected. Correct joined cancellation or explicit `no_response_observed` passes; any response/release/server finish fails. Disposal records an action, even if the socket already closed. |

Event names are closed: `request_emit`, `collector_received`, `close_begin`,
`response_release`, `server_response_finish`, `server_response_close`,
`client_response`, `request_failed`, `close_resolve`. `request_emit` means the
host's Playwright request callback, not a producer timestamp. All timestamps are
monotonic host `performance.now()` offsets from the case start; they are event
ordering facts, **not performance measurements**. At most 24 events per case
are retained; overflow is a functional failure, not silently accepted evidence.

`collector_received` **does not mean accepted 204**, client response, export,
or reporter success. `response_release` records an attempted server action.
`writeAvailable` is the server response/socket check at that action;
`server_response_finish` is a Node server write completion, **not** client
receipt. Only `client_response` plus the exact object join and `clientStatus: 204`
proves a Playwright-observed 204 in this synthetic exchange. Server `close` may
precede release, follow finish, or arrive during cleanup. No ordering between
browser callback and collector receipt is assumed beyond the two close barriers.

`response_after_close` means the exact client-response callback was observed
**after `close_resolve`**. `response_before_close_resolve` includes delivery
before closing or while closing; inspect `close_begin` to distinguish them.
`request_cancelled` means an exact-request `requestfailed` event, not a diagnosis
from raw failure text. `no_response_observed` is limited to this owned observation
window (through cleanup plus an explicit event-loop continuation); it is not a
universal impossibility claim. AFTER or DURING observing no response does not
fail just to force a desired outcome. Conversely, wrong ownership, multiple
POSTs, non-204 response, missing schedule barriers or hangs fail the real probe.

## Deadlines, errors, cleanup, safe evidence

Every case has a 10-second operation deadline, then independently bounded
one-second context cleanup and one-second server/socket cleanup attempts. The
case suite has a 48-second deadline and one-second browser cleanup; the CLI caps
launch plus suite at 60 seconds (launch itself at 10 seconds), then bounded
emergency browser cleanup. The hosted process has a 65-second TERM deadline,
five-second KILL grace and five-minute job limit covering dependency setup.
These are bounds, not measured durations. The server tracks all owned sockets,
destroys them at cleanup, and awaits server close. A stalled operation cannot
start a later case after suite deadline; late-acquired contexts/collectors are
closed rather than reused. A case cleanup failure stops further cases before
closing the owned browser. The CLI exits nonzero on any functional/cleanup error
or incomplete suite. If setup, a hard kill, or schema validation prevents a
result file, missing runtime evidence remains missing; schema/docs alone are not
success. No process logs are uploaded as a substitute.

The first semantic `primaryError` per case is preserved. Suite errors and cleanup
errors occupy separate fields; caught messages/stacks/URLs never enter them.
Only the fixed enums in `context-close-probe.schema.json` cross the boundary.
Result objects reject unknown keys, unbounded vectors, invalid UUIDs/provenance,
raw string error content and unexpected status values before serialization.
Browser POST count saturates at two (meaning multiple, a rejection). Exact
object identities are represented only by verified join booleans, never by
serialized handles. Result provenance contains Node/Playwright versions, SHA-256
of the frozen lockfile, Chromium manifest revision, and exact checked-out source
commit—not executable paths, environment dumps, credentials or browser logs.

The upload allowlist is only `result.json`, this document and the static JSON
schema. There are no screenshots, traces, HARs, raw bodies, network URLs, source
paths, database files, auth, stacks or arbitrary messages. The checked-in template
is not uploaded as a result. Schema validates shape; `functionalPass` separately
requires all four unique cases and the semantic barriers. Neither confers stock
runtime qualification, overhead clearance, universal-platform behavior, nor
retroactive attribution to the older terminal-delivery/cleanup failure.

## Compiler-free validation (local, no browser or server)

```sh
node --test ui/scripts/context-close-probe.test.mjs
node --check ui/scripts/context-close-probe.mjs
node --check ui/scripts/context-close-probe.cli.mjs
node --check ui/scripts/context-close-probe.test.mjs
python3 -B tests/e2e/history_stock_prepare.test.py
```

The unit suite uses injected fake browser/context/collector objects only. It
checks reducer schedules (including a modeled response after close), exact
request-object joins, payload ownership, duplicate rejection, safe schema,
first-error/cleanup separation, deadlines, inert real-probe/CLI imports, and
source guards for the exact positive probe gate and unchanged negative stock
gate. These are not actual Playwright observations. CI YAML and inline shell
syntax must also be checked without running shell commands embedded in it.

Stage 2 (actual reporter/Phoenix collector) is **out of scope**. Stage 1 results
may motivate a separately authorized reporter-specific test, but cannot
establish reporter lifecycle behavior, repair f16, explain the older incident,
or justify changes to its application source, module pins or lockfile.

### Preparation validation actually run

Offline on Node `v26.8.2`: **27/27** new probe controls and **17/17** existing
lifecycle controls pass (44 combined); stock-preparation **20/20**, publication
**12/12**, and artifact-admission **8/8** Python regressions pass. All three new
`.mjs` files pass `node --check`. Ruby parsed CI YAML; all **39** inline `run`
blocks pass `bash -n` after replacing GitHub expressions with a syntax placeholder.
`git diff --check` passes. The entire stock runtime job was compared byte-for-byte
with `5d2a62905` and is unchanged (SHA-256
`c6d1b37992d3c05b0ef2b058ec15e7a37d0b8e958a7bdb56628350d4f2fec210`).
Application source, `ui/package.json`, lockfile and existing module pins are
unchanged. Full `dev.py check`, hosted CI and actual Playwright were **not run**;
no local browser, listener, network/GET, install, build, DB/SQL or push occurred.
This is a clean preparation/validation packet, not Stage 1 runtime evidence.
