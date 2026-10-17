# PRIVATE ordinary observation attribution — source review packet

## Boundary and audit

Base: `2702d8389c2fc78ac2e528c876c35a1a7040d692`. Branch:
`bench/current-task-observation-attribution`, separate worktree
`.phoenix/current-task-attribution`. Source checkpoint
`35e6c30f619cc898cff825516997a3b7484d6ace` is preserved. Its sample-emission
follow-up `f095d1ad7051c83e4806ab5b66af30336a57c82f` failed hosted compilation
in run **39065608814**. Follow-up `bef36e9e4` fixed the reported E0277 auth-future
Send failure and E0599 nonexistent `Dispatch::is_none` call. Per the parent's
hosted results, bef36 compiled and the success/error/denial Rust case passed,
but cancellation failed to find the request close record. This uncommitted
source-only correction addresses the dispatch lifetime during request destruction;
it has not been compiled or dispatched for another hosted attempt.
This packet supplements, not rewrites, the prior composition packet. Its only
non-UI exceptions are the exact attribution changes in the separate
[`conversation-observation-attribution.json`](conversation-observation-attribution.json).
All UI bytes at 2702 are preserved except the explicitly manifested source guard.

`main::Invocation::Observation` calls `observation_startup::run` directly, bypassing
normal server logging initialization. The existing `replay.admission`, handler,
projection and database spans therefore have no observation-owned subscriber at
that entrypoint. This is a source audit, not evidence from deployed logs; no
service/log endpoint, production trace collector or fixture was contacted.
Roadmap context could not be refreshed under the no-network authorization.

## Scoped use of existing tracing

- The observation entrypoint creates only a `tracing_subscriber` JSON fmt layer,
  default timestamps and `FmtSpan::CLOSE`, writing to stderr. No global subscriber,
  OTLP/collector, live logging initialization, environment/config flag, tracing
  API, timer framework or dependency is added.
- `WithSubscriber` installs/restores its dispatch for each future poll. Startup
  and shutdown run under this scope. The private `with_attribution_dispatch`
  middleware-layer helper is reused by `serve` and the in-memory tests. Its
  request-specific, non-generic `AttributionRequest` owns a pinned boxed
  `Next::run` response future and installs that same dispatch for **both polling
  and destruction**. The future stays pinned; its optional slot is taken only
  during `Drop`. This adds one box per private request, not a reusable future
  engine. Tokio's spawned HTTP connection tasks do **not** inherit a thread-local
  subscriber automatically. An existing default/global subscriber is restored
  after each synchronous poll/drop scope, not permanently replaced.
- The fmt filter admits only the explicit span-name inventory in
  `attribution_subscriber`, and no ordinary events. Existing arbitrary error,
  SQL, payload and access-log events are not routed to this output. The selected
  existing fields are `open.id` and `product.reference`; new fields are a
  process-local monotonic numeric `request_id`, static `MatchedPath` route
  template (or `unmatched`), numeric status and boolean `response_ready`.
  No body, cursor/query text, raw URL/reference, auth/forwarding value or fixture
  path is added. Startup spans have no request parent and must not be attributed
  to a request.
- Each private-auth observation branch creates a root `observation.request`
  span. `observation.authorization` uses synchronous `Span::in_scope` around
  the existing peer/forwarding check, with no async borrow of `Request<Body>`.
  The root request still instruments the async handler future.
  The request span closes on response, error response or future drop; readiness
  remains false if no response was produced. False means unfinished, not proof
  of cancellation rather than panic. IDs are process-local, not cross-run IDs.
- New admission children cover ordinary resolution, aggregate lookup and
  `require_member`, including that method's ordinary resolution/aggregate reads.
  DB children cover each existing aggregate's product query, topology query,
  member SQL-plus-decode read, handoff read and source read. Repeated member and
  edge children reflect the existing finite topology loops; no IDs or payloads
  are attached. No extra SQL, cache, query reorder or new endpoint is introduced.

## Cancellation failure model — locked source audit

The missing-root failure has a concrete source-level mechanism. The following
anchors are in the cached sources for the versions pinned by `Cargo.lock`:

- `tracing-0.1.44/src/instrument.rs`, `WithDispatch<T>` and its `Future::poll`:
  the default dispatch guard exists only during `poll`; `WithDispatch` has no
  drop scope. `Instrumented<T>::PinnedDrop` enters its span while destroying its
  inner future, but does **not** install that span's dispatch as the default.
  `tracing-0.1.44/src/span.rs`, `Span::do_enter` and `Span::drop`, call the span's
  stored subscriber directly; entering a span is not a default-dispatch switch.
  `tracing-core-0.1.36/src/dispatcher.rs`, `Dispatch::try_close`, also delegates
  directly without installing a default dispatch.
- `tracing-subscriber-0.3.23/src/registry/sharded.rs`, `Registry::new_span` and
  `DataInner::clear`: each child retains a parent reference; clearing the child
  releases that reference through `dispatcher::get_default(Dispatch::clone)`
  and `subscriber.try_close(parent)`, not through the child's stored dispatch.
  Thus a pending request dropped outside its polling dispatch can emit the
  child's close through its stored subscriber yet release the parent through
  an absent or unrelated default. The observation registry's parent reference
  is not decremented, so dropping the request's own span cannot close the root.
  Adding a root clone does not repair the missed decrement.
- `tracing-subscriber-0.3.23/src/fmt/fmt_layer.rs`, `with_event_from_span!` and
  `Layer::on_close`, construct the close event with an **explicit** parent ID.
  `src/fmt/format/json.rs`, `Format<Json, N>::format_event`, selects `event.parent()`
  before falling back to `lookup_current()` for JSON `span`. Therefore the
  `request_record` predicate on `span.name` is retained: an entered child alone
  does not relabel an emitted root close as a child close. `SerializableContext`
  does use the current span for the JSON `spans` ancestry; the existing child
  association assertions remain necessary and unchanged.
- `tokio-1.52.3/src/runtime/task/harness.rs`, `cancel_task`, calls
  `core.drop_future_or_output()` before storing the cancellation result. Awaiting
  the cancelled `JoinHandle` is already the destruction boundary. No extra sleep,
  scheduling yield, timeout increase, deferred-drop claim or after-await polling
  is justified by this failure.

`AttributionRequest::Drop` now takes and destroys the entire inner request future
under its owning dispatch, so every descendant/parent release in that synchronous
unwind sees the correct registry. Polling uses the same dispatch. Neither auth
logic, the root/child span schema, filters nor JSON formatting changes.

The existing abort/await cancellation test now loops over explicitly absent and
unrelated outer dispatches. A test-only `AmbientRequest` installs that outer
(not expected/private) dispatch during both polling and abort destruction, then
emits a sentinel after dropping the actual router future. Using only
`WithDispatch` around this task would not exercise foreign-dispatch destruction,
because its scope ends after each poll. Immediately after `abort` and awaiting
the cancelled handle, assertions require exactly one private root with
`response_ready = false`, absent status, an associated DB child, no private
sentinel leak, and a sentinel only in the unrelated outer subscriber. There are
no added sleeps, yields or after-await polls. The private startup middleware must
still supply its own nested dispatch during destruction; the harness cannot
supply it.

A separate authored regression polls the actual middleware request once to
`Pending` with a no-op waker, then drops it synchronously under each of an
explicitly absent and an unrelated dispatch. It likewise requires immediate
root/child closure and a post-drop sentinel only in the unrelated subscriber.
Every request close lookup/count now includes the full captured records in its
failure message, including the actual-router root count; a recurring hosted
failure will retain the evidence absent from the prior plain `unwrap` panic.
These Rust changes are source-reviewed only; the hosted failure's exact captured
record set is unknown and the new fix is not runtime-verified.

## Reading durations without double counting

`close` records have a timestamp, `time.busy` and `time.idle`, with JSON span
context linking children to their request. Busy is time entered/polled, **not CPU
usage**; idle includes waiting between polls. Busy + idle is the span's inclusive
wall lifetime (including small construction/drop overhead). Awaited SQL/pool
wait is not lost by measuring only synchronous execution. Never hold an entered
span guard across `await`; the added futures use `Instrument` instead.

`observation.request` starts in private auth middleware and ends once `Next::run`
has returned the response, including Axum's `Json::into_response` body
serialization on these ordinary handlers. It excludes routing/outer middleware
before that point, compression (the compression layer is outside this scope),
network/socket delivery and response body drain. It is **response-ready latency,
not client completion**. The unmodified HTTP `TraceLayer` span can outlive body
drain and is deliberately excluded from the private fmt inventory.

Reuse `product_conversation.detail_get` and
`product_conversation.post_snapshot_projection`; there is no new parallel
handler/projection total. The audited 2702 source has no separate snapshot-view
metadata/enrichment timing spans: that work remains within the existing
post-snapshot projection parent, not separately measured. The existing snapshot
pool-wait, begin, resolve, aggregate, Close, message-page and rollback spans are
reused exactly. `snapshot_read` includes latest-opening work without a separate
child. Do not label all residual snapshot/projection time as SQL.

These are nested inclusive spans: admission includes member authorization, whose
aggregate includes member/handoff reads. Snapshot aggregate has analogous
children. **Do not sum parents and descendants**, or sum all busy/idle values as
an aggregate request attribution. Compare request response-ready wall first,
then sibling subpasses within their parent; retain unattributed residual time.
Concurrent/overlapping spans are not additive and this packet supplies no naive
aggregate attribution, percentiles, bottleneck verdict or performance claim.

The JSON fmt observer writes synchronously to stderr on span close. Formatting
and writing a child's close record happen inside its still-running parent's
lifetime; that cost is included in the parent's measured busy/wall time, while
that child's own close write is largely excluded from its already-captured
duration. Stderr backpressure can therefore affect parent timings and residuals.
Observer bookkeeping also has cost. Unattributed residual time is **not pure
application cost**, nor a precise serialization/enrichment estimate; this packet
does not subtract or benchmark observer overhead.

If executed successfully, the authored actual-router synthetic test would emit
one `observation_attribution_sample=` JSON record array only after privacy and
request/child correlation assertions pass. It would contain captured synthetic
span metadata, not fixture bodies. Such a sample would demonstrate layer shape,
not fixture latency or performance; none was produced by the failed run.

## Static semantic proof and regression evidence

The source guard strips **only** the fourteen exact new future wrappers (ten DB,
four route) plus the one skip-all member attribute/new trait import, then compares
Rust token streams of the *entire* owning files with pinned 2702. SQL string
whitespace remains significant. This proves the same query literals, bindings,
fetch/helper calls, ordering, branches, limits, snapshot boundaries, rollback and
response semantics in those files, modulo instrumentation/formatting. Negative
checks alter a query/helper/wrapper and require rejection. Other handler,
projection, enrichment, schema, generated type, UI, main/logging and launcher
sources remain exact. A separate auth guard proves all live auth/source outside
the observation branch is unchanged and normalizes only the exact token sequence
of the synchronous admission/root-future instrumentation to the pinned branch.
Negative cases reject borrowed async admission and changes to the peer, headers
or denial response; no arbitrary observation-branch replacement is accepted.
Exact changed-file blobs/diffs and the changed production-function inventory are
separately pinned in the manifest;
the original f389/product UI inventory remains unedited.

Auth instrumentation's response construction still calls the same existing
`next.run(req)` or denial `into_response` once. No response headers/body are
rewritten. Static proof does not replace the existing shared body-parity and
real startup/admission tests: those remain authoritative for later hosted
qualification of the exact integrated source.

The Rust cases reuse the exact startup dispatch-layer helper, not
an expected-subscriber wrapper around the test's `oneshot`. The actual
`create_router(ApplicationState::Observation(...))` ordinary product-detail GET
case uses the existing in-memory database/conversation/`for_target` builders and
production compression layer, without a listener or supplied fixture. It polls
worker tasks under an explicitly absent dispatch and a different existing
subscriber; expected output must arrive through the helper's `Next::run`
transfer. Assertions require the existing `product_conversation.detail_get`,
`snapshot_read`, projection and snapshot DB subpass close records, plus new DB
product/topology/member/source children, to associate with the root request ID.
Snapshot children must retain detail/aggregate ancestry. A sentinel after the
request checks restoration of the different default subscriber, and the ordinary
response's product ID is checked after closure-before-body-drain assertions.
The single-member synthetic case does not exercise a continuation handoff.

The synthetic-handler cases also use the startup helper. The existing
success/error/denial oneshot case now invokes the actual `auth_middleware`
through an Axum-provided `Next` and asserts that its exact future is `Send`
before awaiting it; no `Next` constructor or extra task is introduced. This is
a retained compile oracle; the parent reports that bef36 compiled and this
success/error/denial case passed in hosted execution. This drop-scope correction
and its new regression remain **uncompiled and unrun**. The actual
router's middleware bounds supply an additional compile oracle. Sample emission
uses the index of the explicitly enumerated absent/existing dispatches, not a
nonexistent dispatch-inspection method. Cases cover success,
handler error, authorization denial, awaited time plus deliberately slow JSON
serialization, cancellation RAII, metadata redaction and closure before body
drain. A synchronous scope test covers unmatched-route redaction and preservation
of a preexisting scoped subscriber. New synthetic in-memory test data is created
only when Rust tests are executed; source guards do not execute these Rust cases
or open a supplied fixture.

Source-only validation rerun for this correction:

- Python attribution source guard: **7 passed**, including exact token-sequence
  stripping, negative query/helper/wrapper and synchronous-auth normalization
  checks, the span-name allowlist, exact request poll/drop scopes, startup helper
  reuse/source-only test boundary, two-ambient abort/await cancellation and
  post-drop restoration assertions, retained Send oracle, captured-record
  diagnostic guards and all eight manifest entries.
- Prior packet only (not rerun for this correction): existing Python observation
  mock-launcher/source checks **8 passed**; no real child binary was launched and
  no supplied fixture was opened.
- Cached Vitest: **18 passed** across source parity (**4**) and
  `PersistedObservation` (**14**), using one worker.
- `rustfmt --check --edition 2021`: **passed** for all five changed Rust files,
  including the authored tests. `git diff --check`: **passed**.
- Prior packet only (not rerun): cached ESLint **passed** for
  `ui/tests/observationSourceParity.test.ts` with zero warnings.

Hosted run **39065608814** failed Rust compilation at the f095 checkpoint:
E0277 (`FromFn` could not accept the non-Send auth future because borrowed
`&Request<Body>` crossed an async admission wrapper), and E0599
(`Dispatch::is_none` does not exist). That run supplies **failure diagnostics,
not a qualified candidate binary**, executed attribution tests, a synthetic
sample or runtime/performance proof. Any retained f095 failed-run artifact is
**diagnostic only, not a binary**. The parent's later bef36 results establish
compilation and the first success/error/denial case only, not cancellation
success or a qualified runtime sample.

This corrected source remains **UNCOMPILED** and its Rust tests **UNRUN**;
rustfmt is formatting, not type-checking. Parent review and commit must precede
any separately and explicitly authorized hosted retry. No local Cargo, codegen,
build, installation, service/child binary, browser, network GET, fixture query,
new hosted dispatch or production operation was performed for this correction.
The existing artifact and service have not been modified. Shared DB helper
instrument wrappers would also be visible to an independently enabled subscriber
in a future live build of this branch; the new fmt subscriber itself is strictly
private-startup-only. This source packet is not a production deployment change.
