# Conversation Identity — Executive Status

## Delivery boundary

The single-name implementation is delivered as source in [PR #951](https://github.com/scottopell/phoenix-ide/pull/951) for v0.14, not deployed behavior or a v0.13 release gate. Migration 129 is a new forward name-authority transformation; it does not expand any qualified migration-128 production/incident controller, recovery, or deployment admission. Backend/UI integration and genuine generated contracts are included. Final exact-head hosted qualification is tracked by PR checks; merge, release and rendered acceptance remain separate gates. The naming authority decision is [ADR-131](../adrs/131_single-conversation-name-and-allocated-slug.md); [ADR-132](../adrs/132_retire-duplicate-naming-mutations.md) supersedes its legacy mutation-delegation policy after caller audit.

## Implementation

The Coordinator-only rename adapter (REQ-CID-007, ADR-191) requires immutable product/root identity and the shared naming revision, then stores its historical receipt atomically with the title/slug update. Integrated qualification is pending; authored CAS, ABA, replay-after-later-rename, and revoked-Coordinator tests are not yet an integrated pass.

The root transcript `title` is the sole name authority. Migration 129 folds effective root names into it, drops persisted `chain_name`, releases migrating root slugs before deterministic reallocation, and retains messages and identities. The shared allocator checks transcript slugs and actual/reserved transcript/product identities. Rename, low-level creation, legacy shell metadata, approved-task handoff, product publication and fork insertion use that allocator within their write transactions. Generated naming captures an immutable target and name baseline. Creation claims and ordinary History/Close admission remain authoritative.

`canonical_route`, list/snapshot/status responses and ordinary Global navigation publish encoded readable routes. Product UUIDs remain API/cache identity. Exact evidence links use permanent identity plus explicit transcript/message selection. ProductConversation title mutation and name regeneration are the supported naming operations. Chain snapshot/read and Q&A compatibility surfaces remain; no legacy chain-name mutation route is supported. UI naming uses authoritative presentation; delayed rename completion preserves the live route, transcript pin, search and anchor.

## Naming caller inventory

The caller audit found one canonical naming surface: ordinary ProductConversation manual title changes and generated-name requests, both keyed by ProductConversation identity. The browser UI presents naming from the conversation list/sidebar and ProductConversation page; offline queued `rename` operations call `renameProductConversation` rather than a chain-specific API. `App.tsx` retains `/chains/:rootConvId` as a compatibility redirect, while lineage Q&A is hosted by `ChainQaColumn` within `ProductConversationPage`; there is no separate ChainPage naming caller. The backend registers ProductConversation title and `regenerate-name` operations. Its `/api/chains/:rootId` endpoints are read/Q&A/archive/delete compatibility surfaces, not naming routes; old chain-name mutation paths are not registered. Chain response's legacy `chain_name` slot remains null for wire compatibility. The spec preserves these read aliases, permanent ProductConversation UUID identity, and exact transcript/message evidence references.

This inventory describes callers and route registration in the current source tree; it does not claim that removing legacy naming delegation has passed cleanup-specific tests.

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

- Before the endpoint cleanup, local `./dev.py check --lanes tsc,ui-lint,vitest` and compiler-free `tests/devpy` discovery passed (1,056 Python tests, one skipped). Cleanup-specific local TypeScript, ESLint/stylelint and Vitest checks also pass, alongside spec-shape and spec-anchor validation. Cleanup and trailing-slash source `c1ed17600` passed exact normal CI39187737093 and macOS39187737161, including Rust/codegen and all applicable lanes; exact Codex review reported no major issues. The actual-main integration successor still requires its own exact-head checks/review.
- Hosted `./dev.py codegen` succeeded at source `041b35f89771cf7a9dddd482cb351f63022c2c31`, run 39169104230. Its generated `ChainView.ts` was collected without hand edits. The temporary collection step was removed.
- Independent integrated-source review of `95fca6cc8..7c9db305a` reported no actionable findings. Earlier findings in allocation, asynchronous naming, exact links and live navigation were corrected. Later changes repair test fixtures and remove diagnostic workflow scaffolding, without changing production naming behavior.
- Full Rust collection at `7c9db305a` ran 4,884 tests: 4,874 passed, ten failed, 185 skipped. Those failures exposed a stale title expectation, a delivery-visibility assumption, and eight uses of a synthetic schema missing a Coordinator-trigger prerequisite. Fixtures were corrected without weakening migration or lifecycle contracts. The focused owning workflow module and two naming regressions then passed (67 tests), run 39174656205. Final ordinary full-suite results are authoritative in PR checks; no failing run is represented as success.
- The local Allium executable rejected unchanged LLM-spec syntax; the hosted pinned Allium lane passed. This is a local-tool discrepancy, not a naming-spec acceptance claim.
- Historical controller fixtures explicitly isolate migration 128 and assert rejection of the feature registry. No production deployment, release, or merge was performed by this task.

- Ordinary CI run 39174994821 succeeded at `cbf2478b2805a271968399d0814ec343c43ee87a`, including Rust, codegen staleness and all other applicable lanes. A late review against the earlier source exposed Desktop Unicode double-encoding, value-baseline ABA, and legacy rename error mapping. Subsequent fixes decode the route boundary once, use an opaque immutable-owner/revision baseline, and retain typed admission errors; final qualification of that delta is tracked by PR checks.

## Actual installer/producer main integration

Main `f41de00825ef8993aa2a20d2fbb61d743ae3487a` legitimately contains installer ADR129 and producer ADR130 and is integrated into this branch; the prior normative-prefix dependency is cleared. Naming runtime/UI are unchanged from qualified `c1ed17600`. Newly merged installer prepared-source fixtures derive their synthetic qualified128 registry pin from retained historical fixture bytes rather than the feature's migration129 registry. Production admission remains fixed128 and rejects the identity registry; this does not repin an incident application or authorize host operations. Final exact-head results live in PR #951, not progress-only source updates. No production migration, artifact, release or deployment acceptance follows from this source integration.
