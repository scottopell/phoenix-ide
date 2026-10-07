# ADR-088: Qualified Kache v1 restores automatic compiler-cache selection

- **Status:** Accepted
- **Date:** 2026-10-07
- **Supersedes:** ADR-079
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-008

## Context

ADR-079 keeps Kache explicit because Kache 0.26.0 packages an incomplete macOS dSYM for a representative restored Phoenix executable. Its binary and dSYM UUIDs match, but the application file/line breakpoint does not resolve after restoration.

Kache 1.0.0 contains the upstream correction that runs `dsymutil` from the Cargo profile root used by relative OSO records, plus regression coverage that requires debug information for both a Rust target and its dependency. A two-source Phoenix qualification uses the real `dev.py check --all --lanes e2e --compiler-cache kache` entry point. The second clean target restores `phoenix_ide` and `phoenix_core` as local hits with zero compiler runs. Its restored executable has no standalone application `rcgu.o` files, yet its UUID-matched dSYM resolves one representative application file/line location and one dependency file/line location in LLDB.

The earlier performance and storage measurements still describe bounded workloads rather than guarantees. Kache remains an optional accelerator, and Phoenix qualifies only the exact release and host combination it exercised.

## Options considered

1. **Retain ADR-079's explicit-only policy** — avoids changing defaults, but preserves a temporary restriction after its stated fidelity gate has been cleared by an upstream release and representative qualification.
2. **Prefer exact Kache 1.0.0 on qualified hosts with fallback** — applies the measured cross-worktree benefit while retaining sccache, no-cache, and explicit-wrapper escape paths.
3. **Require Kache everywhere** — creates a new prerequisite and extends support to unqualified operating systems and architectures.

## Decision

Automatic compiler-cache selection prefers exact Kache 1.0.0 on macOS arm64 when its daemon becomes ready on the configured socket. If Kache is absent, disabled, incompatible, unsupported on the host, or its daemon cannot start, automatic selection falls through to a usable sccache and then no cache while reporting the reason and actual backend.

Explicit Kache and sccache selections continue to fail rather than substitute another backend. Explicit `none` and caller-owned `RUSTC_WRAPPER` remain authoritative. Support for another Kache release requires deliberate qualification rather than version-range inference.

## Consequences

- **Positive:** automatic Phoenix builds receive Kache's cross-worktree restoration behavior without sacrificing the representative macOS application and dependency debugger path.
- **Negative:** the first cache population can remain slower than sccache, and each supported Kache release requires a deliberate pin and qualification.
- **Neutral:** Phoenix does not install Kache, configure remotes, guarantee acceleration, or extend this qualification beyond macOS arm64.

## References

- ADR-079 and ADR-078
- `dev.py::_configure_compiler_cache`
- `specs/compatibility/requirements.md`
- `docs/development/compiler-cache.md`
- Kache v1.0.0; upstream fixes #1168 and #1170
