# PRIVATE ordinary observation attribution — explicit completion contract

## Boundary and qualification

Product-source baseline: `2702d8389c2fc78ac2e528c876c35a1a7040d692`.
This source-only contract change is atop `dfcd8b477468b4d94c9ec2830a1e92bf5bcdb62b`
on `bench/current-task-observation-attribution`. The exact exception blobs,
diffs and production-function inventory are in
[`conversation-observation-attribution.json`](conversation-observation-attribution.json).
This supplements the earlier composition packet. The deterministic-diff portability
follow-up preserved checkpoint `d5700c84f` and all its Rust/runtime sources. The
explicit private e34 backport atop `963307eb834fee0568fb8f991164d842d0ec68c2`
adds only the two approved ProductConversationPage runtime/test patches. All other
2702 UI bytes remain exact except the explicitly manifested source-parity guard;
composition documentation and UI exception metadata are separately enumerated.
Historical product identity remains 9ee, not e34 whole-UI parity.

**This source is UNCOMPILED. Its Rust tests are authored, UNRUN.** No Cargo,
build, codegen, installation, child/server executable, fixture/SQL operation,
network request, new GET, service, browser, production action or hosted dispatch
was performed. Cached Python/UI source tests and rustfmt are not Rust type or
runtime qualification. Roadmap issue #806 could not be refreshed under the
no-network authorization. The existing artifact and service are untouched.

## Completion, not span lifetime

Only these fixed events are completion authority:

| Target | Event name | Fields |
|---|---|---|
| `phoenix::observation::completion` | `observation.stage_complete` | `stage`, `outcome`, `elapsed_us` |
| `phoenix::observation::completion` | `observation.request_complete` | `stage = observation.request`, explicit numeric `request_id`, `outcome`, `elapsed_us`, optional numeric `status`, `response_ready` |

Each finite stage starts when its guarded future is first polled (authorization
is synchronous). It captures a monotonic `Instant`, its owning `Dispatch`, and
its current parent `Span`. The result wrapper awaits the unchanged operation,
then consumes the guard with the actual returned `Result`: `success` for `Ok`,
`error` for `Err`. The result and its data are returned unchanged; neither error
text nor return values are logged. An operation returning `Ok(None)` is a
successful operation even if its enclosing aggregate later rejects absence.

A guard dropped without a returned result emits exactly one `cancel` event.
**Cancel means the await/request was dropped, not that SQLx/SQLite worker work
ended.** It also does not distinguish cancellation from panic unwinding. An
unpolled future has not started a stage and owes no completion. Process abort,
SIGKILL, writer failure or a panicking subscriber cannot guarantee delivery.

The observation auth root owns an explicit process-local numeric request ID,
not one recovered from formatting or inferred from a raw route/reference. Its
terminal event is emitted under its captured owner dispatch **before returning
the Response** from auth or during cancellation destruction. HTTP 4xx/5xx
response readiness is an `error`; other returned statuses are `success`.
Canceled roots have `response_ready = false` and no status. IDs are not
cross-run identifiers. The root span retains the same ID for ancestry.

Stage events correlate via their captured parent and JSON `spans` ancestry to
that root ID; DB APIs do not accept a new context argument. IDE guards live in
`observation_startup`; the private DB read guard lives only in the existing
`product_conversation_read` module. Both accept finite enums, not arbitrary
stage names or payload fields. They do not implement a subscriber, forward
callbacks, discover ancestors, use task-local/global routing, instrument spawned
SQL workers, or introduce a cross-crate tracing engine. Ordinary async calls
retain root ancestry through the existing `Instrument` wrappers.

The private subscriber admits only the fixed target/event-name pair and the
explicit diagnostic span-name inventory. Other ordinary events (including SQL,
errors and access logs) are excluded. Existing `open.id` and `product.reference`
remain on existing **diagnostic spans**, not new event fields. JSON span context
can therefore still contain those selected diagnostic identifiers. New events
add no raw URI/reference, query/cursor, body, header, auth/forwarding value,
fixture path or error payload. Root span route metadata is a static MatchedPath
template, or `unmatched`.

`FmtSpan::CLOSE`, timestamps, `time.busy`, and `time.idle` remain **lifetime
DIAGNOSTICS ONLY**. They are not completion, cancellation, readiness or elapsed
wall-time authority. A retained child/parent reference can delay or prevent a
CLOSE record without withholding an explicit terminal event. No completion
depends on `on_close`, registry parent release, or parsing a close timestamp.

## Private dispatch lifetime

The ordinary observation entrypoint bypasses normal live logging. It constructs
a scoped JSON fmt subscriber writing to stderr, without global initialization,
OTLP, collector configuration, dependencies or live logging changes.
`WithSubscriber` scopes startup/shutdown polls. The existing private,
non-generic `AttributionRequest` wrapper is retained: it owns one pinned boxed
`Next::run` response future and restores the private dispatch for **both polls
and destruction**, then restores any absent or foreign ambient dispatcher.
Each completion guard additionally owns its dispatch and installs it while
emitting, with an explicit captured parent entered synchronously for JSON
ancestry. No entered span or dispatcher guard crosses an await.

No DB runtime API, query, fetch, bind, call order, topology loop, limit,
transaction boundary, rollback policy, response data, route or GET changes.
Shared DB/API guards and existing spans can be observed by an independently
enabled subscriber in a future live build; only the new private subscriber's
installation/output is observation-startup-specific. This is not a production
logging deployment or a request-context propagation promise for spawned tasks.

## Finite stage inventory and reading durations

| Region | Completion stages |
|---|---|
| Root and peer admission | `observation.request`, `observation.authorization` |
| Target/reference/member admission | `replay.admission`, `observation.resolve_ordinary`, `observation.get_aggregate`, `observation.require_member`, `observation.member.resolve_ordinary`, `observation.member.get_aggregate` |
| Existing aggregate subpasses (admission and snapshot) | `observation.db.product`, `.topology`, `.read_member`, `.handoff`, `.source` |
| Shared ordinary snapshot | `product_conversation.pool_wait`, `.begin_read`, `.resolve`, `.aggregate`, `.close`, `.message_page`, `.snapshot_read`, `.rollback_read` |
| Handler and projection | `product_conversation.detail_get`, `product_conversation.post_snapshot_projection` |

`elapsed_us` is monotonic inclusive wall time from guard creation to terminal
emission preparation (microseconds, integer, saturating at u64 max). It includes
await/scheduling/pool/SQLite delay and decoding within the wrapped operation,
not just entered-span/poll time. It is **not CPU time**. `read_member` includes
its existing SQL-plus-decode helper, not an isolated SQL measurement. Snapshot
read includes latest-opening work without a separate child. Projection includes
its existing metadata/enrichment awaits; no unsupported finer split is claimed.

The request root measures response-ready latency in auth: admission, handler
work and Axum JSON serialization before the Response returns. It excludes
routing/outer middleware before auth, outer compression, socket delivery and
body drain. Client/network completion minus this root is a **network/outer
residual**, not CPU or a pure network measurement. It also includes unmeasured
routing, compression, scheduling and observation-boundary differences.

Stages are nested and inclusive. Compare admission against detail/snapshot and
projection first, then compare sibling passes inside a parent. **Never sum a
parent with descendants** (e.g. aggregate plus its member reads, or snapshot
plus aggregate). Finite-loop member/handoff events may repeat; count them per
request and parent, not as duplicate request terminals. Preserve unattributed
residuals; snapshot/projection residuals are not automatically SQL or CPU.

Formatting/writing is synchronous. A child's event/CLOSE formatting and stderr
backpressure occur inside still-running parents and affect their elapsed time;
a child's own event formatting is after its elapsed sample. Guard/span creation,
result wrapping and diagnostic bookkeeping also cost time. Residuals therefore
include observer overhead; nothing here subtracts it or claims precise
serialization cost, latency percentiles or a bottleneck verdict.

## Failure history (not new qualification)

- Checkpoint `35e6c30f619cc898cff825516997a3b7484d6ace` and sample follow-up
  `f095d1ad7051c83e4806ab5b66af30336a57c82f` used span CLOSE as authority.
  Hosted run **39065608814** failed compilation: E0277 from an async admission
  borrow of `&Request<Body>` making auth non-Send, plus E0599 for nonexistent
  `Dispatch::is_none`. Failed-run artifacts are diagnostics, not binaries.
- Per parent report, `bef36e9e4` compiled and its success/error/denial case
  passed; cancellation failed to find the root close. Those results do not
  qualify this source or establish cancellation success.
- `dfcd8b477` retained owner dispatch during request destruction. Its source
  audit identified that WithDispatch scopes polls, not destruction, while
  registry child cleanup can release parent references through the ambient
  default dispatcher. It did not establish that span lifetime is completion.
- This explicitly authorized contract change makes monotonic guarded events
  authoritative. No sleep, yield, worker-completion claim or subscriber-forwarding
  repair is offered. Earlier CLOSE samples, if retained separately, are lifetime
  diagnostics only, not samples of the event contract. No new runtime sample
  has been generated or qualified.

The fourth qualification attempt, per parent report, stopped at the UI source
proof: **416 passed, 1 failed**. No Rust qualification ran in that attempt; it
was not a runtime/event failure. The exact-patch mismatch is consistent with
unpinned Git diff defaults. The runner's defaults/configuration were inferred,
not measured; this source-only follow-up does not assert a measured runner setting
or qualify a new runtime sample.

A held-out local source-only check of checkpoint `873e25042` measured two more
ambient configuration effects: `diff.interHunkContext=1000` changed **13 of 45**
canonical exception patches; `diff.suppressBlankEmpty=true` changed **10 of 45**.
These are reproduced local guard portability findings, **not evidence of the
hosted runner's configuration or the cause of its failure**. No hosted settings
or new runtime behavior were measured.

## Deterministic patch producer and consumers

Python `EXACT_DIFF_ARGS` is shared by `exact_diff` and the named
`regenerate_manifests` producer in `tests/devpy/test_observation_attribution_source.py`.
TypeScript `exactDiffArgs` pins the same explicit arguments for `exactDiff`:

```text
git -c diff.suppressBlankEmpty=false diff --diff-algorithm=myers --no-indent-heuristic --unified=3 --inter-hunk-context=0 --no-ext-diff --no-textconv --no-color --full-index --src-prefix=a/ --dst-prefix=b/ --no-index -- BEFORE AFTER
```

Both manifests are regenerated with these arguments, including **every** UI
exception (not just the reported failure) and all attribution exceptions. Both
manifests record `diff_args`, which both guards check against their explicit
constant. Only temporary operand paths are replaced with the manifest path; patch bodies are
never normalized or waived. Git blob hashes still pin exact bytes independently.
No user/repository Git configuration is written.

The finite order is: finish producer/guard/docs source, regenerate the UI exception
manifest, then regenerate the attribution manifest. The Python producer file is
hashed/diffed as an ordinary completed source entry. The attribution manifest does
not contain itself; its finite path inventory is enforced by both guards. The UI
manifest does not include either guard or itself; the attribution manifest pins
those files. No recursive hash fixed point is sought. A producer regression
compares both serialized outputs byte-for-byte without writing them.

Both helpers have an eight-case cross-product regression using subprocess-local
`GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n`: histogram/myers,
indent heuristic true/false, compaction heuristic true/false, plus hostile context
17 in every case. Two additional independent cases set `diff.interHunkContext=1000`
and `diff.suppressBlankEmpty=true`. The command-line `-c` precedes the `diff`
subcommand and overrides ambient blank-context suppression; the explicit
`--inter-hunk-context=0` prevents ambient hunk merging. Every case requires the
same canonical patches for every UI exception and every attribution exception.
An SQL-literal whitespace mutation
still differs in exact blob and exact patch; Python additionally rejects it through
the unchanged SQL-preserving token proof. Tests do not edit any SQL or product file.

## Static proof and authored regressions

The Python guard unwraps only the enumerated finite calls and original exact
instrumentation wrappers, the exact added import/member attribute and the DB
helper/test block, then compares entire owning-file Rust token streams to 2702.
The one rustfmt-added trailing comma on the unchanged latest-opening call is
normalized by its exact full call token sequence, not a general comma eraser.
SQL string tokens retain byte-for-byte whitespace. Query/helper/wrapper changes
and changed auth admission/denial/terminal sequences are negative test cases.
The exact helper code and all other exceptions remain pinned separately by
whole-file blobs/diffs. Live auth and all source outside the observation auth
branch remain byte-exact. Main/logging, schemas and generated types are
unchanged. UI exceptions are the enumerated source-parity guard and the two exact
approved e34-on-963 page patches; there are no observation-gate or event changes.

Authored Rust regressions (all **UNRUN**):

- Actual production router + observation application + compression: success,
  cursor-validation error, missing-reference admission error and forwarding
  denial, under explicit NONE and foreign defaults. Require one explicit terminal before Response/body handoff, exact
  stage counts/outcomes, numeric elapsed/status/readiness, request ancestry,
  snapshot-aggregate ancestry, privacy and ordinary response product identity.
  The one-member fixture expects no handoff stage; continuation loops are not
  separately exercised by this synthetic case.
- A retained-root regression requires the terminal event while the root span
  is still owned; diagnostic closure is not a precondition. Only completion-event
  counts, not diagnostic record counts, must remain stable after body drain.
- Pending request manually polled then synchronously dropped under NONE/foreign:
  one canceled projection and request, one successful authorization, no status,
  no ambient leak and post-drop sentinel restoration. The real auth future has
  a retained compile-time `Send` assertion.
- Notify/abort/await regression, without sleeps/yields: completion events are
  available after cancellation destruction. It does not claim worker shutdown.
- DB-specific result wrapper: unchanged Ok/Err values, one success, one error,
  one cancel per started pending await under NONE/foreign owner-switching.
  An unpolled future owes no event. This test does not query SQLite.
- Fixed event filter rejects unrelated events/target-name impostors and raw
  unmatched-path data. Assertions do not use root CLOSE as terminal authority.

Source-only validation actually run for this contract change:

- Python: **15 passed** (attribution source guard **7**, existing mocked
  observation launcher/source checks **8**). No real child or fixture opened.
- Cached Vitest, one worker: **18 passed** (source parity **4**,
  `PersistedObservation` **14**); fetch/EventSource are mocked, no browser/server.
- `rustfmt --check --edition 2021`: **passed**, all six changed Rust files.
- Cached ESLint: **passed**, the changed UI source guard, zero warnings.
- `git diff --check`: **passed**.

Rustfmt is syntax/formatting, **not compilation**. All new Rust tests remain
**UNRUN**. Full Rust/router/runtime qualification remains a parent-owned gate
requiring separate authorization; no retry or dispatch is implicitly authorized
by this packet.

## Portability follow-up reproduction (source-only)

From the repository root, using only cached Python/Node dependencies:

```sh
python3 tests/devpy/test_observation_attribution_source.py --regenerate-manifests
python3 -m unittest discover -s tests/devpy -p test_observation_attribution_source.py -v
python3 -m unittest discover -s tests/devpy -p test_observe.py -v
(cd ui && ./node_modules/.bin/vitest run tests/observationSourceParity.test.ts src/pages/PersistedObservation.test.tsx --maxWorkers=1)
(cd ui && ./node_modules/.bin/eslint tests/observationSourceParity.test.ts --report-unused-disable-directives --max-warnings 0)
git diff --exit-code d5700c84f -- crates ui/src
git diff --check
```

Executed follow-up results: **19 Python tests passed** (all **11** attribution
source guards, including producer idempotence, the eight-configuration matrix,
and the two additional hostile configurations, plus all **8** mocked observation
launcher/source guards); **28 UI tests passed** (the previous **18** plus **10**
hostile-config cases); strict ESLint **passed**
with zero warnings; both diff checks **passed**. These are cached, source-only
checks, not a new Rust/event/runtime qualification. The UI exception manifest was
fully regenerated; its 859 source identities and 34 patch bodies remain identical
to the checkpoint, with explicit diff-argument metadata added.

The Rust tree has no delta from d570; no rustfmt or Rust validation is needed for
this follow-up. These commands are not authorization for build, Cargo, install,
network, services, browser, SQL or hosted dispatch. Roadmap #806 remains unverified
under the no-network constraint. This source-only checkpoint requires independent review before any further hosted dispatch.

## Private e34 backport validation

Authorized source scope is only upstream
`e34ff8b85148bdb79f8c4164c5a7b067c930421a`'s two page-file patches applied to clean
`963307eb834fee0568fb8f991164d842d0ec68c2`, plus this documentation, exact manifests
and their guards. The product callback reads current owned Close state when the
notification arrives. The 35-line test-file delta adds a forced layout-commit
notification before passive resubscription, including its fixture hook; the
original empty-tail test is byte-unchanged. No b778 cursor/citation or other
later product changes are included. MessageList, ConversationNavStack,
ConversationNav, VirtualTranscript and other renderer files stay exact 9ee.
Observation capability gates and all Rust/SQL/completion-event sources stay exact
963. This is an explicit tiny new runtime exception to historical 9ee, **not a
whole-UI parity claim**.

The UI manifest retains historical `product = 9ee` and adds `approved_backport`
with immutable 963/e34/two-path identities. It includes exact historical-product
diffs for both page exceptions, including the bounded new test exception. The
attribution manifest retains its separate **2702 baseline**, extends `FUNCTIONS`
to both page diffs, and pins their complete bytes/diffs. Each source guard applies
the canonical exact upstream patch to 963 page bytes in scratch directories and
compares the entire result to current bytes. It rejects reverted, appended and
body-mutated candidates independently of manifest regeneration. This is not a
filename waiver. Canonical diff arguments and hostile-config regressions remain
in force; no Git configuration is changed.

The new genuine forced-window regression is distinct evidence from the unchanged
empty-tail test. Neither it nor source proof establishes the cause or repair of
the separately measured 963 failure. Existing failed logs remain retained and
must not be relabeled as passes or merged into the separate 2702 baseline.

Cached source-only reproduction from the repository root (no install/build):

```sh
python3 tests/devpy/test_observation_attribution_source.py --regenerate-manifests
python3 -m unittest discover -s tests/devpy -p test_observation_attribution_source.py -v
python3 -m unittest discover -s tests/devpy -p test_observe.py -v
(cd ui && ./node_modules/.bin/vitest run tests/observationSourceParity.test.ts src/pages/PersistedObservation.test.tsx src/App.alias.test.tsx src/App.markdownNavigation.test.tsx src/components/ConversationNavStack.test.tsx src/components/ConversationNavStack.readiness.test.tsx src/components/MessageList.test.tsx src/pages/CoordinatorPage.test.tsx src/pages/ProductConversationPage.test.tsx src/hooks/conversationViewLifecycle.test.ts src/hooks/useConversationViewJourney.test.tsx --maxWorkers=1)
(cd ui && ./node_modules/.bin/tsc -b --noEmit)
(cd ui && ./node_modules/.bin/eslint src/pages/ProductConversationPage.tsx src/pages/ProductConversationPage.test.tsx tests/observationSourceParity.test.ts --report-unused-disable-directives --max-warnings 0)
git diff --exit-code 963307eb834fee0568fb8f991164d842d0ec68c2 -- crates
git diff --check
```

Executed cached results: **428 UI tests passed across the exact 11 hosted suites**,
including the new forced-window regression and the existing provenance tests;
**11 Python attribution guards + 8 mocked launcher/source guards passed**.
`tsc -b --noEmit`, strict ESLint on both page files and the parity guard, empty
963-to-current `crates` diff, and `git diff --check` all passed. Passing UI fixtures
still emitted the previously recorded React act/render-update warnings and
`cacheDB.init is not a function`; these are not browser acceptance. An earlier worker
run used the finalization suite instead of ConversationNavStack.test; it passed
433 tests but was not the exact workflow inventory. The owner subsequently ran
the exact unchanged hosted inventory above: all 428 tests passed.


These commands bypass environment/bootstrap setup intentionally and use cached
binaries directly under the source-only authorization. Rust remains
uncompiled/unqualified by this backport; no install/build/Cargo, service, browser,
product GET, SQL operation, production action or hosted dispatch was performed.
No measured runtime/event or installed-artifact success is claimed. Roadmap #806
was read using `gh` per repository orientation guidance; that external metadata
read is not a product GET or qualification.
