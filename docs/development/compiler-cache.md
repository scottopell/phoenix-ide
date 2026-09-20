# Rust compiler caches

Phoenix uses one compiler-cache selection path for Rust checks, ordinary development builds, and production build preparation. A cache is an optional accelerator: cache failure must not be reported as a successful cached build, and no wall-time improvement is guaranteed.

## Installation

Phoenix supports the released Kache `0.26.x` command contract. Download the archive for the host architecture from the immutable [Kache v0.26.0 release](https://github.com/kunobi-ninja/kache/releases/tag/v0.26.0), verify its adjacent SHA-256 file, and put `kache` on `PATH`. `PHOENIX_KACHE_BIN=/absolute/path/to/kache` selects a verified executable outside `PATH`. Kache's interactive `init` is not required because `dev.py` provides `RUSTC_WRAPPER`, starts the daemon, and supplies a short private socket when an isolated `KACHE_CACHE_DIR` is used.

`sccache` remains supported when its executable is on `PATH`. Run `./dev.py doctor` to see the detected cache tools and versions; both are optional development prerequisites.

## Selection

`PHOENIX_COMPILER_CACHE` accepts:

- `auto` (default): use compatible Kache first, otherwise sccache, otherwise no cache;
- `kache`: require compatible Kache and a working daemon;
- `sccache`: require sccache;
- `none`: disable Phoenix's automatic wrapper.

`./dev.py check --compiler-cache …` overrides the environment for checks. A caller-provided `RUSTC_WRAPPER` always wins and is reported as `explicit`. Explicit unavailable or incompatible choices fail with an actionable error. Automatic fallback prints the reason and reports the backend that actually runs; it never labels sccache or uncached work as Kache.

Kache-specific settings (`KACHE_CACHE_DIR`, `KACHE_SOCKET_PATH`) and sccache-specific settings (`SCCACHE_DIR`, `SCCACHE_CACHE_SIZE`) remain separate. Phoenix does not install tools, purge caches, configure remotes, or replace either tool's garbage-collection policy.

## Why `auto` prefers Kache

A limited devmbp comparison used official arm64 releases Kache 0.26.0 and sccache 0.18.0, Rust/Cargo 1.95.0, macOS 26.4.1, and APFS. Three fresh-cache runs each executed `cargo check -p phoenix-core --locked` with `CARGO_INCREMENTAL=0`: cold population in source A, a touched same-worktree crate rebuild, then an empty-target restore in detached source B.

| Backend | Cold seconds, raw (median) | Edit seconds, raw (median) | Cross-worktree seconds, raw (median) |
|---|---|---|---|
| Kache 0.26.0 | 57.196, 49.783, 42.789 (**49.783**) | 1.011, 0.814, 0.862 (**0.862**) | 26.462, 21.200, 23.094 (**23.094**) |
| sccache 0.18.0 | 42.244, 35.524, 30.799 (**35.524**) | 1.033, 0.883, 0.905 (**0.905**) | 35.008, 29.643, 28.846 (**29.643**) |

Kache was slower to populate, tied for normal edits at this sample size, and faster for the intended empty-target cross-worktree restore. Its final sample reported 250 local hits, 253 misses, 0 errors/fallbacks, 49.7% hit rate, 251,559,534 restored bytes, and 100% zero-copy restore. sccache reported one Rust hit and 382 Rust misses across relocated sources (its 356 total hits were mostly C/C++/assembler), with no cache read/write errors or timeouts. These results justify preferring Kache for Phoenix's multi-worktree shape while retaining explicit `sccache` and `none` escapes; they are not a general performance promise.

A single isolated run through actual Phoenix entry points also succeeded for both backends. Fresh-cache `./dev.py check --lanes rust` took 607.14 seconds with Kache and 668.42 seconds with sccache; fresh-target `build_rust()` took 109.22 and 241.03 seconds respectively. These are order-sensitive single samples, not significance evidence. The full acceptance run, `PHOENIX_COMPILER_CACHE=kache ./dev.py check --all`, completed all 19 lanes in 1,051.4 seconds.

### APFS allocation accounting

The benchmark isolated every cache and target pair. Summed file allocation (`st_blocks`, cross-checked with `du -sk`) was stable at 871,825,408 bytes median for Kache and 727,252,992 bytes for sccache. Those sums double-count shared APFS clone extents and therefore are **not unique allocation**. Whole-volume free-space deltas over each owned scenario were 260,374,528 bytes median for Kache (range 210,575,360–312,262,656) and 708,009,984 bytes for sccache (range 271,691,776–772,689,920). Kache's native accounting reported 267,578,617 stored reflink bytes and 251,442,744 restored reflink bytes in the final run.

macOS exposes no unprivileged per-extent unique-allocation total for an arbitrary set of APFS clone trees. Whole-volume deltas include unrelated filesystem activity, while tree block sums overcount shared extents. The volume deltas are consequently bounded estimates corroborated by native restore accounting—not exact unique-byte measurements. The evidence indicates Kache's APFS reflinks protect against physically duplicating restored Rust artifacts, but does not establish a universal disk-use ratio.

## Validation boundary

Validated locally: macOS arm64 release binary, version probe, daemon startup over a short explicit socket, Rust compilation, APFS reflink restore, normal development/check selection, and a successful native `./dev.py prod build` without activation. The production binary and its `.dSYM` had matching UUIDs. Kache's store-time `dsymutil` nevertheless emitted missing-intermediate-object warnings for dependency archives; source-level debug fidelity was not validated, so explicit `sccache` or `none` remains the conservative escape for debugging that output. This is reported rather than hidden because cache acceleration must not be confused with debug-symbol correctness.

Deterministic tests cover Linux command/environment shaping, and hosted Linux CI exercises the ordinary check suite without requiring either optional executable. Not validated by this comparison: Windows behavior; Linux Kache executable restore/signing/debug-symbol behavior with the released binary; source-level fidelity of restored macOS debug symbols; cross-device filesystems without reflinks; remote/S3 caches; or production activation. Phoenix makes no compatibility or performance guarantee for those paths.

## Release provenance used for the comparison

The GitHub release API identified immutable release `v0.26.0`, published 2026-09-19, tag target commit `af30b9102f538f8ceebc974ec9dc827e2feef0d8`, and arm64 archive SHA-256 `422c59f988245575e6258629c58e0fc8c6d57639734f57d1b04bb5f015a1acdc`. The downloaded checksum file matched, the tag commit's GitHub signature verification was valid, the archive contained a Mach-O arm64 executable, and it reported `kache 0.26.0`. GitHub's release page advertised an attestation, but `gh attestation verify` returned HTTP 404 for the archive digest; no attestation-success claim is made.

Upstream maintainer changes corresponding to restored-artifact hash reuse (#803, fixing #540), streaming hashing (#804), and runtime scan memoization (#805) are in the released line. Phoenix's historical private patched-versus-stock measurements were not used for this decision.
