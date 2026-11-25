# ADR-285: Extend Kache v1.2.2 strict local-only qualification to Linux x86_64

- **Status:** Accepted
- **Date:** 2026-11-25
- **Extends:** [ADR-225](225_kache-v1-2-2-strict-local-only-selection.md) (macOS arm64 local-only selection remains unchanged)
- **Affects:** development methodology; REQ-COMP-001, REQ-COMP-008

## Context

[ADR-225](225_kache-v1-2-2-strict-local-only-selection.md) qualified exact Kache `1.2.2` behind a Phoenix-owned strict local-only boundary on macOS arm64 (`darwin/arm64`) only. Linux development hosts (`linux/x86_64`) remained rejected by `_kache_host_error()`, falling back to `sccache` or uncached builds.

The published Kache `v1.2.2` release tag (`94b6db79f5de652c8bd4b7ea30b1ee8dedacfadc`, peeling to commit `96052b4842239bb216f6c4812054203c21550788`) publishes an official static-pie Linux x86_64 archive `kache-x86_64-unknown-linux-musl.tar.gz` with published and verified SHA-256 `f2f6555dee50945c2a90a7fb793c2084a00cc6459d08ac2306d38d905064321b`.

Native qualification of that exact Linux x86_64 artifact ran on `sopell-worker-one` (Linux x86_64, Rust/Cargo `1.95.0`) using isolated disposable home, cache, runtime, config, socket, and target directories and a hostile ambient remote configuration naming an S3 endpoint, credentials, a separate store root, and a volume shard:

- Phoenix's version probe reported exact `kache 1.2.2` without leaving probe state.
- `_ensure_kache_daemon` replaced both `KACHE_CONFIG` and `KACHE_HOST_CONFIG` with Phoenix's generated strict local-only config (`local_only = true`, `ignore_env = true`), started a private foreground daemon child, verified its PID via `.state.json` and socket-inode receipt, reused that exact PID on re-entry, and never created either ambient path.
- Two clean isolated source roots containing an application and local dependency (`helper`) were built into separate target directories with `CARGO_INCREMENTAL=0`: root A recorded 2 local misses; root B recorded 2 local hits, 0 additional misses, and 0 remote hits (`hit_elapsed_ms: 26`, `miss_elapsed_ms: 229`, `copied_bytes: 4352681`).
- After deleting root A, the restored root B ELF binary (`NT_GNU_BUILD_ID: 9873b3d5a0daa401b6f7a0e04dd44f5a996c1e2f`) executed cleanly (`result=42`), and GDB resolved and stopped at both `app/src/main.rs:2` (`let value = helper::doubled(21);`) and `helper/src/lib.rs:3` (`x * 2`, `x = 21`).

## Options considered

1. **Keep Kache restricted to `darwin/arm64`** — leaves Linux x86_64 hosts without cross-worktree Rust compiler caching unless `sccache` is installed separately.
2. **Qualify Kache `1.2.2` on `linux/x86_64` under the same strict local-only boundary as ADR-225** — enables local-only compiler caching on verified Linux x86_64 hosts with native restored-artifact and GDB DWARF evidence while keeping every isolation, ownership, and non-goal invariant intact.
3. **Allow all Linux architectures (`linux/*`) without per-architecture qualification** — violates ADR-034's explicit compatibility boundary by claiming untested `linux/aarch64` support.

## Decision

Extend exact Kache `1.2.2` strict local-only selection in `REQ-COMP-008` and `dev.py` from `darwin/arm64` to **`darwin/arm64` and `linux/x86_64`**.

All ADR-225 boundaries remain unchanged:

- Exact reported version `kache 1.2.2` only.
- Phoenix-generated private config pinned via both `KACHE_CONFIG` and `KACHE_HOST_CONFIG` with `[cache] local_only = true` and `ignore_env = true`, explicit Phoenix-owned `local_store` and `runtime_dir`, and zero remote/planner/volume configuration.
- Exclusive Phoenix-owned socket, inode receipt, and child-PID ownership proof; fail closed on unowned/replaced sockets; never stop or restart existing daemons.
- Automatic selection falls through to usable `sccache` or `none` if Kache is unavailable; explicit `PHOENIX_COMPILER_CACHE=kache` fails actionably on unqualified hosts (`linux/aarch64`, `darwin/x86_64`, `win32/*`).

## Consequences

- Linux `x86_64` developers and worker hosts with Kache `1.2.2` installed automatically receive Phoenix-owned local-only Rust compilation caching across worktrees and target rebuilds.
- `linux/aarch64`, `darwin/x86_64`, Windows, remote/S3 caching, and production activation remain explicitly unqualified until separately verified and documented.

## References

- [REQ-COMP-008](../compatibility/requirements.md)
- [ADR-225](225_kache-v1-2-2-strict-local-only-selection.md)
- [ADR-034](034_explicit-compatibility-and-no-implicit-hardening.md)
- [Compiler cache development guide](../../docs/development/compiler-cache.md)
