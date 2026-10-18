# Conversation Identity — Executive Status

## Delivery boundary

This is staged v0.14 backend/spec work, not deployed behavior. Migration 129 is a new forward name-authority transformation; it does not expand or alter any qualified migration-128 production/incident registry, controller, recovery, or deployment admission. UI source integration is included. Compiled Rust qualification, genuine code generation, and rendered acceptance remain pending.

## Implementation

The root transcript `title` is the sole name authority. Migration 129 folds effective root names into it, drops persisted `chain_name`, releases all migrating root slugs before allocating in `(created_at, id)` order, and retains message and identity data. `conversation_name::allocate_slug_tx` allocates Unicode projections against shared transcript slugs and actual transcript/product identities. Ordinary rename, low-level root creation, legacy creation-shell publication, and product/fork insertion use the same projection rule in their write transaction. Creation claims and ordinary History/Close admission remain authoritative.

`canonical_route`, list projection, creation status, and ordinary Global reference output publish the encoded readable route. Product UUIDs remain API/cache identity. Exact transcript links remain explicitly pinned. Legacy chain-name endpoints write the same root title; the old response slot is null rather than another authority.

## Verification coverage

| Requirement | Authored coverage | Qualification |
|---|---|---|
| REQ-CID-001 | Legacy name round trips, prose generator tests, duplicate human-name rename/publication | Rust execution pending |
| REQ-CID-002 | Two-handle concurrent rename; deterministic numeric suffixes; actual ID collision; duplicate publication; low-level creation collision | Rust execution pending |
| REQ-CID-003 | Readable canonical list/snapshot/status routes and Unicode path-component encoding | Rust execution pending; UI acceptance pending |
| REQ-CID-004 | Stable ID aliases after rename, released-route reuse, existing exact-transcript tests | Rust execution pending |
| REQ-CID-005 | History/active Close fences, stale generated-name baseline, and stale creation-claim name/slug nonmutation | Rust execution pending |
| REQ-CID-006 | Effective-name migration, cross-root old-slug occupancy, column removal, stable message/continuation evidence; historical migration-005 checks | Rust execution pending |

Local `./dev.py check --lanes tsc,vitest` passes, including deferred rename-navigation regression coverage. Compiler-free `tests/devpy` discovery passes: 1,026 tests, one skipped. Hosted `./dev.py codegen` succeeded at source `041b35f89771cf7a9dddd482cb351f63022c2c31` (CI run 39169104230); its `ChainView.ts` output was collected from the generated-contract artifact without hand edits. The temporary branch-scoped collection step was then removed.

Independent review of that source found canonical-slug Global resolution selecting the root rather than current scope, and a suspended rename callback using stale route state. Both have source fixes and focused regressions; final exact-head hosted qualification and review remain pending. Initial hosted Clippy diagnostics and a UUID-only E2E route assertion have also been corrected. These checks do not substitute for rendered acceptance.

The local Allium executable reported parser errors in unchanged LLM specs; the hosted pinned Allium lane passed at the initial source, so this is a local-tool discrepancy rather than a naming-spec blocker. The incident controller remains hard-pinned to 128; historical unit fixtures isolate that contract and separately assert rejection of the feature registry.
