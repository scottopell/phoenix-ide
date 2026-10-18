# Conversation Identity — Executive Status

## Delivery boundary

The single-name implementation is delivered as source in [PR #951](https://github.com/scottopell/phoenix-ide/pull/951) for v0.14, not deployed behavior or a v0.13 release gate. Migration 129 is a new forward name-authority transformation; it does not expand any qualified migration-128 production/incident controller, recovery, or deployment admission. Backend/UI integration and genuine generated contracts are included. Final exact-head hosted qualification is tracked by PR checks; merge, release and rendered acceptance remain separate gates. The design decision is [ADR-129](../adrs/129_single-conversation-name-and-allocated-slug.md).

## Implementation

The root transcript `title` is the sole name authority. Migration 129 folds effective root names into it, drops persisted `chain_name`, releases migrating root slugs before deterministic reallocation, and retains messages and identities. The shared allocator checks transcript slugs and actual/reserved transcript/product identities. Rename, low-level creation, legacy shell metadata, approved-task handoff, product publication and fork insertion use that allocator within their write transactions. Generated naming captures an immutable target and name baseline. Creation claims and ordinary History/Close admission remain authoritative.

`canonical_route`, list/snapshot/status responses and ordinary Global navigation publish encoded readable routes. Product UUIDs remain API/cache identity. Exact evidence links use permanent identity plus explicit transcript/message selection. Legacy chain-name endpoints write the same root title; their old response slot is null rather than another authority. UI naming uses authoritative presentation; delayed rename completion preserves the live route, transcript pin, search and anchor.

## Executable regression coverage

| Requirement | Coverage |
|---|---|
| REQ-CID-001 | Legacy name round trips, prose generator output, duplicate names, authoritative UI presentation |
| REQ-CID-002 | Concurrent allocation, deterministic suffixes, actual/reserved ID collisions, creation/publication and approved-task shell allocation |
| REQ-CID-003 | Readable canonical routes, Unicode encoding, emitted-route/current-scope round trip, delayed rename navigation |
| REQ-CID-004 | Permanent UUID resolution, released-slug reuse, exact transcript/message selection and stable evidence links |
| REQ-CID-005 | History/Close/claim fences; revision-fenced regeneration and valid-claim legacy metadata preserve newer human names, including A→B→A renames |
| REQ-CID-006 | Effective-name migration, release-before-reallocate ordering, column removal, stable message/continuation identities and historical schema fixtures |

## Qualification record

- Local `./dev.py check --lanes tsc,ui-lint,vitest` passes. Compiler-free `tests/devpy` discovery after main integration passes: 1,056 tests, one skipped.
- Hosted `./dev.py codegen` succeeded at source `041b35f89771cf7a9dddd482cb351f63022c2c31`, run 39169104230. Its generated `ChainView.ts` was collected without hand edits. The temporary collection step was removed.
- Independent integrated-source review of `95fca6cc8..7c9db305a` reported no actionable findings. Earlier findings in allocation, asynchronous naming, exact links and live navigation were corrected. Later changes repair test fixtures and remove diagnostic workflow scaffolding, without changing production naming behavior.
- Full Rust collection at `7c9db305a` ran 4,884 tests: 4,874 passed, ten failed, 185 skipped. Those failures exposed a stale title expectation, a delivery-visibility assumption, and eight uses of a synthetic schema missing a Coordinator-trigger prerequisite. Fixtures were corrected without weakening migration or lifecycle contracts. The focused owning workflow module and two naming regressions then passed (67 tests), run 39174656205. Final ordinary full-suite results are authoritative in PR checks; no failing run is represented as success.
- The local Allium executable rejected unchanged LLM-spec syntax; the hosted pinned Allium lane passed. This is a local-tool discrepancy, not a naming-spec acceptance claim.
- Historical controller fixtures explicitly isolate migration 128 and assert rejection of the feature registry. No production deployment, release, or merge was performed by this task.

- Ordinary CI run 39174994821 succeeded at `cbf2478b2805a271968399d0814ec343c43ee87a`, including Rust, codegen staleness and all other applicable lanes. A late review against the earlier source exposed Desktop Unicode double-encoding, value-baseline ABA, and legacy rename error mapping. Subsequent fixes decode the route boundary once, use an opaque immutable-owner/revision baseline, and retain typed admission errors; final qualification of that delta is tracked by PR checks.
