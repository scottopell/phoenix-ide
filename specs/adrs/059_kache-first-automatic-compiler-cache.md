# ADR-059: Automatic compiler caching prefers released Kache

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-006

## Context

Phoenix development commonly compiles the same Rust dependency graph in isolated Git worktrees and target directories. The existing cache selector supports Kache, sccache, no cache, and caller-owned wrappers, but automatic selection prefers sccache. Released Kache 0.26.0 contains the upstream socket, restored-output hash reuse, streaming hash, and scan-memoization work needed by Phoenix's worktree shape.

A controlled devmbp comparison shows a genuine tradeoff. Kache has higher cold-population cost than sccache, ordinary edit cycles are effectively tied in the limited sample, and Kache restores an empty target in another worktree faster while using APFS reflinks. Compiler caching remains an optional accelerator, so selection must not imply a performance or compatibility guarantee.

## Options considered

1. **Keep sccache first** — minimizes cold-population time but leaves relocated Rust worktrees with almost no Rust cache hits in the representative workload.
2. **Prefer compatible released Kache with explicit escapes** — targets Phoenix's cross-worktree workload while retaining `sccache`, `none`, and caller-owned `RUSTC_WRAPPER` choices.
3. **Require Kache** — simplifies the selected path but makes an optional accelerator a development prerequisite and removes a useful operational fallback.

## Decision

Automatic compiler-cache selection prefers Kache when the executable reports the qualified released `0.26.0` version and its daemon starts successfully. It otherwise falls through to sccache and then no cache, reporting the actual backend and fallback reason. Explicit Kache or sccache requests fail when unusable instead of changing backend. Explicit `none` and `RUSTC_WRAPPER` remain authoritative.

The same selector owns checks, ordinary development builds, and production build preparation. Phoenix does not install cache tools, configure remotes, purge storage, or promise an acceleration. Support for another Kache release requires deliberate qualification rather than assumed cross-version compatibility.

## Consequences

- **Positive:** Phoenix's default matches its isolated multi-worktree build shape, and fallback cannot be mislabeled as Kache.
- **Negative:** A first build can take longer with Kache, and each new Kache release needs explicit qualification.
- **Neutral:** sccache remains available explicitly and as automatic fallback; environments without either tool continue uncached.

## References

- `dev.py::_configure_compiler_cache`
- `dev.py::_ensure_kache_daemon`
- `docs/development/compiler-cache.md`
- ADR-034
- Kache release v0.26.0 and upstream changes #803, #804, and #805
