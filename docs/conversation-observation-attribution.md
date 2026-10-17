# PRIVATE ordinary observation attribution — source review packet

## Boundary and audit

Base: `2702d8389c2fc78ac2e528c876c35a1a7040d692`. Branch:
`bench/current-task-observation-attribution`, separate worktree
`.phoenix/current-task-attribution`. Source checkpoint
`35e6c30f619cc898cff825516997a3b7484d6ace` is preserved. Its sample-emission
follow-up `f095d1ad7051c83e4806ab5b66af30336a57c82f` failed hosted compilation
in run **39065608814**. This uncommitted source-only correction addresses the
reported E0277 auth-future Send failure and E0599 nonexistent `Dispatch::is_none`
call; it has not been compiled or dispatched for another hosted attempt.
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
  and shutdown run under this scope. The tiny, non-generic private
  `with_attribution_dispatch` middleware-layer helper is reused by `serve` and
  the in-memory tests. It attaches that same dispatch to each `Next::run` request
  future: Tokio's spawned HTTP connection tasks do **not** inherit a thread-local
  subscriber automatically. An existing
  default/global subscriber is neither replaced nor an initialization error.
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

Authored Rust tests (UNRUN) reuse the exact startup dispatch-layer helper, not
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
an authored compile oracle, **not executed or type-checked evidence**. The actual
router's middleware bounds supply an additional compile oracle. Sample emission
uses the index of the explicitly enumerated absent/existing dispatches, not a
nonexistent dispatch-inspection method. Cases cover success,
handler error, authorization denial, awaited time plus deliberately slow JSON
serialization, cancellation RAII, metadata redaction and closure before body
drain. A synchronous scope test covers unmatched-route redaction and preservation
of a preexisting scoped subscriber. Synthetic in-memory test data would be
created only if Rust tests are later executed; source guards do not execute these
Rust cases or open a supplied fixture.

Source-only validation rerun for this correction:

- Python attribution source guard: **7 passed**, including exact token-sequence
  stripping, negative query/helper/wrapper and synchronous-auth normalization
  checks, the span-name allowlist, startup helper reuse/source-only test boundary,
  authored Send-oracle presence and all eight manifest entries.
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
sample or runtime/performance proof. Any retained failed-run artifact is
**diagnostic only, not a binary**.

This corrected source remains **UNCOMPILED** and its Rust tests **UNRUN**;
rustfmt is formatting, not type-checking. Parent review and commit must precede
any separately and explicitly authorized hosted retry. No local Cargo, codegen,
build, installation, service/child binary, browser, network GET, fixture query,
new hosted dispatch or production operation was performed for this correction.
The existing artifact and service have not been modified. Shared DB helper
instrument wrappers would also be visible to an independently enabled subscriber
in a future live build of this branch; the new fmt subscriber itself is strictly
private-startup-only. This source packet is not a production deployment change.
