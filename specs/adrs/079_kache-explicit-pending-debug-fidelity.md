# ADR-079: Kache remains explicit pending restored debug fidelity

- **Status:** Accepted
- **Date:** 2026-10-04
- **Supersedes:** ADR-078
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-008

## Context

ADR-078 adopted released Kache 0.26.0 as the automatic compiler cache after measurements showed faster cross-worktree restoration and lower estimated APFS physical growth than sccache for a representative Phoenix check workload.

Qualification then exercised an actual native macOS production build. Restored dependency archives compiled successfully, Kache reported no cache errors, and the binary and `.dSYM` UUIDs matched. `dsymutil` nevertheless emitted unresolved-object warnings for restored archives. Those observations do not prove source-level symbol and breakpoint fidelity, and development binaries need that fidelity as much as production binaries.

The physical-growth and restore-time measurements remain valid for their bounded workload. They do not establish debug correctness and therefore cannot authorize a default switch.

## Options considered

1. **Keep Kache automatic outside production** — preserves measured development acceleration but exposes development debugging to the same unqualified archive behavior.
2. **Keep Kache explicit everywhere** — makes the qualified command/daemon/cache path usable while retaining sccache/no-cache defaults until source-level fidelity is proven.
3. **Disable Kache entirely** — avoids the fidelity risk but discards a useful explicit candidate and its measured APFS/cross-worktree benefits.

## Decision

Automatic compiler-cache selection uses a usable sccache executable and otherwise no cache. Kache 0.26.0 requires explicit operator selection for checks, development builds, and production build preparation.

Explicit Kache selection requires the exact qualified release, successful daemon startup, readiness on the configured socket, and honest backend reporting. It remains an opt-in candidate; it is not qualified for restored-archive source-level debug fidelity.

Automatic Kache adoption requires new evidence that a representative restored macOS archive preserves source-level symbols and debugger behavior, or an upstream release that fixes the behavior followed by deliberate release qualification. A newer upstream release is not adopted implicitly.

## Consequences

- **Positive:** no Phoenix default silently exchanges debug correctness for cache acceleration.
- **Negative:** automatic builds do not receive Kache's measured cross-worktree restore and APFS physical-growth advantages.
- **Neutral:** explicit Kache remains available for informed use; the fidelity blocker remains open and separately verifiable.

## References

- ADR-078
- `dev.py::_configure_compiler_cache`
- `docs/development/compiler-cache.md`
- Kache v0.26.0
