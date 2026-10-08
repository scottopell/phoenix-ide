# Rust compiler caches

Phoenix uses one compiler-cache selection path for Rust checks, ordinary development builds, and production build preparation. A cache is an optional accelerator: cache failure must not be reported as a successful cached build, and no wall-time improvement is guaranteed.

## Installation

Phoenix supports the qualified released Kache `1.0.0` command contract on macOS arm64. Download the archive for the host architecture from the immutable [Kache v1.0.0 release](https://github.com/kunobi-ninja/kache/releases/tag/v1.0.0), verify its adjacent SHA-256 file, and put `kache` on `PATH`. `PHOENIX_KACHE_BIN=/absolute/path/to/kache` selects a verified executable outside `PATH`. Kache's interactive `init` is not required because `dev.py` provides `RUSTC_WRAPPER`, starts the daemon, and supplies a short private socket when an isolated `KACHE_CACHE_DIR` is used.

`sccache` remains supported when its executable is on `PATH`. Run `./dev.py doctor` to see the detected cache tools and versions; both are optional development prerequisites.

## Selection

`PHOENIX_COMPILER_CACHE` accepts:

- `auto` (default): on macOS arm64, prefer compatible Kache, then usable sccache, then no cache; on other hosts, use usable sccache or no cache;
- `kache`: require compatible Kache and a working daemon on macOS arm64;
- `sccache`: require sccache;
- `none`: disable Phoenix's automatic wrapper.

`./dev.py check --compiler-cache …` overrides the environment for checks. A caller-provided `RUSTC_WRAPPER` always wins and is reported as `explicit`. Explicit unavailable or incompatible choices fail with an actionable error. Automatic fallback prints the reason and reports the backend that actually runs; it never labels sccache or uncached work as Kache.

`KACHE_DISABLED=1` makes Kache unavailable under the same rules. An sccache candidate must execute its version probe successfully before selection.

Automatic selection uses exact Kache 1.0.0 only on macOS arm64. That release contains the upstream correction that runs `dsymutil` from the Cargo profile root used by relative OSO records. Representative qualification restored Phoenix into a second clean source root: `phoenix_ide` and `phoenix_core` were local hits with zero compiler runs, the executable and dSYM UUIDs matched, and LLDB resolved one application and one dependency file/line location even though no standalone application `rcgu.o` files were restored. This clears the Kache 0.26.0 debug-fidelity blocker without expanding qualification to other releases, operating systems, or architectures.

Phoenix-generated cache variables are scoped to direct Cargo subprocesses and the E2E harness that owns a Cargo build, so starting Phoenix does not force agent-executed Cargo commands in other repositories through Phoenix's selected cache. Kache's nearest implicit `.kache.toml` is resolved from the invoking tree and pinned as an absolute `KACHE_CONFIG` before daemon startup or build-directory changes. Relative backend cache/socket/config/runtime/log-destination paths are normalized against the invoking directory before daemon startup. For sccache v0.18.0 this includes cache/config/error-log/GCS-key/startup-notify paths, extra-file/base-directory path lists, and filesystem UDS values; abstract UDS values remain literal. Kache `KACHE_LOG_FILE` is a tracing filter and wrapper opt-in, not a path; Phoenix preserves it literally and normalizes its `KACHE_LOG_FILE_PATH`, `KACHE_CONFIG`, and `KACHE_HOST_CONFIG` path values. Kache v1.0.0 cannot report a running daemon's effective environment. Phoenix-generated socket names therefore use a private digest covering the Kache binary, Cargo root, explicit `KACHE_*` inputs, implicit HOME/XDG config and cache roots, standard AWS inputs, and the contents of resolved Kache/AWS/Docker configuration files. A private ownership receipt binds that digest to the socket inode, so a subsequent automatic build with the same identity safely reuses the same daemon without writing environment values or credentials to disk. Operator-supplied sockets use the same receipt after Phoenix starts them; an unowned or replaced running socket fails closed instead of silently changing the selected backend. Phoenix never stops or restarts an existing daemon. The Kache daemon starts from the same Cargo working directory as its wrapper, so implicit project-local configuration cannot diverge across production-build worktrees.

Kache-specific settings (`KACHE_CACHE_DIR`, `KACHE_SOCKET_PATH`) and sccache-specific settings (`SCCACHE_DIR`, `SCCACHE_CACHE_SIZE`) remain separate. Phoenix does not install tools, purge caches, configure remotes, or replace either tool's garbage-collection policy.

Under pressure, retain the capped cache plus only targets for worktrees still in active use; inspect Kache's `stats`/`clean --dry-run` guidance or sccache's native stats before deleting anything. Do not sum clone-aware target `du` values to choose what to remove, and do not broadly purge active caches or whole target trees as routine setup.

## Why Kache is the qualified default on macOS arm64

A limited devmbp comparison used official arm64 releases Kache 0.26.0 and sccache 0.18.0, Rust/Cargo 1.95.0, macOS 26.4.1, and APFS. Three fresh-cache runs each executed `cargo check -p phoenix-core --locked` with `CARGO_INCREMENTAL=0`: cold population in source A, a touched same-worktree crate rebuild, then an empty-target restore in detached source B.

| Backend | Cold seconds, raw (median) | Edit seconds, raw (median) | Cross-worktree seconds, raw (median) |
|---|---|---|---|
| Kache 0.26.0 | 57.196, 49.783, 42.789 (**49.783**) | 1.011, 0.814, 0.862 (**0.862**) | 26.462, 21.200, 23.094 (**23.094**) |
| sccache 0.18.0 | 42.244, 35.524, 30.799 (**35.524**) | 1.033, 0.883, 0.905 (**0.905**) | 35.008, 29.643, 28.846 (**29.643**) |

Kache was slower to populate, tied for normal edits at this sample size, and faster for the intended empty-target cross-worktree restore. Its final sample reported 250 local hits, 253 misses, 0 errors/fallbacks, 49.7% hit rate, 251,559,534 restored bytes, and 100% zero-copy restore. sccache reported one Rust hit and 382 Rust misses across relocated sources (its 356 total hits were mostly C/C++/assembler), with no cache read/write errors or timeouts. These historical 0.26.0 results establish useful Kache cross-worktree behavior but are not a general performance or correctness promise. Automatic adoption additionally depends on the separate Kache 1.0.0 debug-fidelity qualification described above.

A later bounded physical-growth repeat at exact Phoenix source `be1dfae` used two fresh isolated runs per backend in interleaved order, official Kache 0.26.0 versus released sccache 0.18.0, 1 GiB cache caps, retained cache plus both targets, and the same cold/edit/cross-worktree `phoenix-core` workload:

| Backend | Cold seconds (2 runs; median) | Edit seconds (median) | Cross-worktree seconds (median) | Total APFS free-space loss (2 runs; median) | Retained tree allocated blocks (median) |
|---|---:|---:|---:|---:|---:|
| Kache 0.26.0 | 35.730, 34.056 (**34.893**) | 0.811, 0.795 (**0.803**) | 18.154, 18.231 (**18.193**) | 318,078,976, 338,120,704 (**328,099,840 bytes**) | 872,357,888 bytes |
| sccache 0.18.0 | 27.465, 28.677 (**28.071**) | 0.709, 0.714 (**0.712**) | 24.786, 24.686 (**24.736**) | 673,988,608, 676,515,840 (**675,252,224 bytes**) | 727,277,568 bytes |

Kache again traded slower cold population for faster cross-worktree restore. Its native final-run accounting reported a 267,583,057-byte store capped at 1,073,741,824 bytes, only 1,478,431 private store bytes, 266,104,626 bytes cloned into targets with full clone coverage, 250 local hits, 253 misses, and a 49.7% hit rate. sccache's final cache was 111,780,250 bytes under the 1 GiB cap and again recorded one Rust hit versus 382 Rust misses across relocated sources, with zero cache errors/read errors/write errors/timeouts. All twelve compile/check phases succeeded.

The retained-path block sum is deliberately not called unique physical use: it double-counts APFS shared extents, which is why Kache's value is larger even while its isolated volume delta is smaller. The APFS free-space measurement bracketed each phase with `sync` and a two-second settle; paired idle samples varied from -86,016 to +1,146,880 bytes, while unrelated host allocation can still enter a build-length bracket. The volume delta is the best available unprivileged estimate of **extra physical growth for the whole isolated workload**, not inode ownership or exact per-extent truth. Kache can still increase disk use—especially through retained targets—and neither backend fixes chronic pressure by itself.

A single isolated run through actual Phoenix entry points also succeeded for both backends. Fresh-cache `./dev.py check --lanes rust` took 607.14 seconds with Kache and 668.42 seconds with sccache; fresh-target `build_rust()` took 109.22 and 241.03 seconds respectively. These are order-sensitive single samples, not significance evidence. The full acceptance run, `PHOENIX_COMPILER_CACHE=kache ./dev.py check --all`, completed all 19 lanes in 1,051.4 seconds.

The closed standalone check-ROI audit at `e5ad1915:docs/audits/dev-check-roi.md` is retained only as historical methodology/context. It measured an earlier source revision, warned that its profiled wrapper materially changed Rust execution, treated a stale empty-target sample as non-current, attributed shared target trees once rather than once per lane, and distinguished allocated from apparent size on APFS. Those cautions informed this comparison’s isolation and allocation accounting; its 491.1-second warm check, stale 1,221.6-second cold check, and lane footprints are **not** Kache-vs-sccache results and are not used as current performance evidence.

### APFS allocation accounting

The benchmark isolated every cache and target pair. Summed file allocation (`st_blocks`, cross-checked with `du -sk`) was stable at 871,825,408 bytes median for Kache and 727,252,992 bytes for sccache. Those sums double-count shared APFS clone extents and therefore are **not unique allocation**. Whole-volume free-space deltas over each owned scenario were 260,374,528 bytes median for Kache (range 210,575,360–312,262,656) and 708,009,984 bytes for sccache (range 271,691,776–772,689,920). Kache's native accounting reported 267,578,617 stored reflink bytes and 251,442,744 restored reflink bytes in the final run.

macOS exposes no unprivileged per-extent unique-allocation total for an arbitrary set of APFS clone trees. Whole-volume deltas include unrelated filesystem activity, while tree block sums overcount shared extents. The volume deltas are consequently bounded estimates corroborated by native restore accounting—not exact unique-byte measurements. The evidence indicates Kache's APFS reflinks protect against physically duplicating restored Rust artifacts, but does not establish a universal disk-use ratio.

## Validation boundary

Validated for Kache 1.0.0 on macOS arm64: version probe, daemon startup over a short explicit socket, Rust compilation, APFS reflink restore, and a two-source run through `./dev.py check --all --lanes e2e --compiler-cache kache`. The clean second target restored representative Phoenix application and dependency crates with zero compiler runs. Its application and dependency file/line breakpoints resolved from the restored UUID-matched dSYM without standalone application object files. Earlier Kache 0.26.0 production-build and benchmark evidence remains historical input; it does not broaden the Kache 1.0.0 qualification.

Deterministic tests cover Linux command/environment shaping, and hosted Linux CI exercises the ordinary check suite without requiring either optional executable. Not validated by this comparison: Windows behavior; Linux Kache executable restore/signing/debug-symbol behavior with the released binary; macOS architectures other than arm64; every possible debugger/source shape beyond the representative Phoenix probes; cross-device filesystems without reflinks; remote/S3 caches; or production activation. Phoenix makes no compatibility or performance guarantee for those paths.

## Release provenance

The qualified Kache v1.0.0 release publishes `kache-aarch64-apple-darwin.tar.gz` with SHA-256 `6d1be0079d0689a85fa04b7fed7eaa94f7e08259cafb8bd361a5c25c4c98c4a3`. Its release tag peels to `7c27329c286e32c17a8dac05cff22442926ff719`, which contains upstream fixes #1168 and #1170. Phoenix's representative validation used that official arm64 release artifact and verified its published checksum before running the two-source journey. The release-tag source contains the corrected `dsymutil` path and dSYM content regression.

The historical comparison used Kache 0.26.0. The GitHub release API identified immutable release `v0.26.0`, published 2026-09-19, tag target commit `af30b9102f538f8ceebc974ec9dc827e2feef0d8`, and arm64 archive SHA-256 `422c59f988245575e6258629c58e0fc8c6d57639734f57d1b04bb5f015a1acdc`. The downloaded checksum file matched, the tag commit's GitHub signature verification was valid, the archive contained a Mach-O arm64 executable, and it reported `kache 0.26.0`. GitHub's release page advertised an attestation, but `gh attestation verify` returned HTTP 404 for the archive digest; no attestation-success claim is made.

Upstream maintainer changes corresponding to restored-artifact hash reuse (#803, fixing #540), streaming hashing (#804), and runtime scan memoization (#805) are in the released line. Phoenix's historical private patched-versus-stock measurements were not used for this decision.

Kache 1.0.0 contains the macOS debug-bundle cwd correction and content-level regression that clear the observed 0.26.0 blocker. Phoenix pins 1.0.0 exactly; adopting another release still requires fresh provenance, representative restored-archive/source-level debugger validation, and requalification of daemon/socket/cache behavior.
