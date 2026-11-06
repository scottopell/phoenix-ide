# Rust compiler caches

Phoenix uses one compiler-cache selection path for Rust checks, ordinary development builds, and production build preparation. A cache is an optional accelerator: cache failure must not be reported as a successful cached build, and no wall-time improvement is guaranteed.

## Installation

Phoenix selects only released Kache `1.2.2` on macOS arm64, using a strictly local-only Phoenix-generated configuration. Install the official arm64 release or point `PHOENIX_KACHE_BIN` at its executable; other reported versions, including `1.0.0`, do not qualify. The v1.2.2 release tag peels to commit `96052b4842239bb216f6c4812054203c21550788`; the macOS arm64 `kache-aarch64-apple-darwin.tar.gz` release archive has SHA-256 `10d09e146852a8df8796cf642ca58872107a7f391b7647ec8cc253cf289af0a9`. These pins identify the selected release; they do not establish native restored-archive qualification.

`sccache` remains supported when its executable is on `PATH`. Run `./dev.py doctor` to see each optional cache tool, executable, and version plus the effective compiler-cache request, read-only selected candidate, scope, and automatic fallback reasons. Doctor does not install tools, leave probe-created cache state, or start a daemon; its Kache version probe uses and removes an isolated temporary home. Kache runtime readiness is checked only when a Cargo command starts. A missing optional cache is advisory under `auto`, while an unusable backend explicitly requested through `PHOENIX_COMPILER_CACHE` fails doctor actionably. An explicit `RUSTC_WRAPPER` remains authoritative and is reported without executing an unknown custom wrapper; its scope is the caller-owned inherited environment rather than Phoenix's generated Cargo-only cache environment.

## Selection

`PHOENIX_COMPILER_CACHE` accepts:

- `auto` (default): prefer exact Kache 1.2.2 on macOS arm64 when its local-only daemon is ready, then usable sccache, then no cache;
- `kache`: require exact Kache 1.2.2 and its owned local-only daemon on macOS arm64; fail actionably rather than substitute another backend;
- `sccache`: require sccache;
- `none`: disable Phoenix's automatic wrapper.

`./dev.py check --compiler-cache …` overrides the environment for checks. A caller-provided `RUSTC_WRAPPER` always wins and is reported as `explicit`. Explicit unavailable or incompatible choices fail with an actionable error. Automatic fallback prints the reason and reports the backend that actually runs; it never labels sccache or uncached work as Kache.

`KACHE_DISABLED=1` makes Kache unavailable under the same rules. An sccache candidate must execute its version probe successfully before selection.

Historical Kache 1.0.0 debug-fidelity qualification on macOS arm64 restored Phoenix into a second clean source root: `phoenix_ide` and `phoenix_core` were local hits with zero compiler runs, the executable and dSYM UUIDs matched, and LLDB resolved one application and one dependency file/line location even though no standalone application `rcgu.o` files were restored. That release fixed the observed 0.26.0 `dsymutil` cwd blocker, but its trust-domain behavior made it ineligible for managed remote use. The v1.0.0 debugger evidence did **not** transfer to v1.2.2; the separate release-specific qualification is recorded below.

Phoenix-generated cache variables are scoped to direct Cargo subprocesses and the E2E harness that owns a Cargo build, so starting Phoenix does not force agent-executed Cargo commands in other repositories through Phoenix's selected cache. The selected Kache 1.2.2 path does not read the invoking tree's `.kache.toml` or accept caller-supplied Kache cache/socket/config paths. It generates one private local-only config, pins both `KACHE_CONFIG` and `KACHE_HOST_CONFIG` to it, and sets an owned socket. Phoenix derives socket identity from the Kache executable, selected version, and owned local paths; a private ownership receipt binds that identity to the socket inode. Phoenix launches the daemon as a detached foreground child and records ownership only if the ready serving process is that child. Reuse requires the matching receipt and socket incarnation; a bind-race winner or unowned/replaced running socket fails closed. Phoenix never stops or restarts an existing daemon. For sccache v0.18.0, relative cache/config/error-log/GCS-key/startup-notify paths, extra-file/base-directory path lists, and filesystem UDS values are normalized against the invoking directory; abstract UDS values remain literal.

Phoenix-managed Kache is strictly local-only: both `KACHE_CONFIG` and `KACHE_HOST_CONFIG` point to its generated configuration with `[cache] local_only = true`, `ignore_env = true`, and explicit Phoenix-owned `local_store` and `runtime_dir`. It clears ambient `KACHE_*` configuration inputs except operational controls before adding its own local-only settings; ambient Kache remote settings and credentials cannot activate remote caching. It does not configure remote backends, planners, or volumes. Remote caching is deferred to a separate contract and qualification, not declared obsolete. Kache-specific settings are owned by Phoenix for this selection; sccache-specific settings (`SCCACHE_DIR`, `SCCACHE_CACHE_SIZE`) remain separate. Phoenix does not install tools, purge caches, or replace either tool's garbage-collection policy.

Under pressure, retain the capped cache plus only targets for worktrees still in active use; inspect Kache's `stats`/`clean --dry-run` guidance or sccache's native stats before deleting anything. Do not sum clone-aware target `du` values to choose what to remove, and do not broadly purge active caches or whole target trees as routine setup.

## Kache v1.2.2 local-only qualification

The exact macOS arm64 release artifact identified above was qualified on 2026-11-01 with isolated, test-owned home, cache, runtime, config, socket, and target paths. A hostile ambient config named an S3 endpoint, credentials, a different cache root, and a volume shard; Phoenix replaced both config selectors with its generated strict local-only config, started and proved ownership of its private child, and did not create either ambient store path. The child reported version `1.2.2`, the requested private socket, and the generated config path, then stopped cleanly.

Two clean source roots containing an application and a local dependency were built into separate target directories with incremental compilation disabled. The first root produced two misses. The second root added two local hits with no additional misses and zero remote hits, so neither compiler executed for the restored application or dependency. Kache restored the executable's cached dSYM beside the second-root binary. `dwarfdump` reported matching binary/dSYM UUID `49FA89BE-6796-3147-9304-0DE8CA5C0794`; LLDB resolved and stopped at `main.rs:2`, and independently resolved `helper::doubled` at `lib.rs:3`.

The source-level configuration and child-lifecycle regressions remain in `tests/devpy/test_compiler_cache.py`. Native qualification used only disposable fixtures and the official release artifact; it did not read active credentials, contact a remote object store, reuse an existing Kache daemon, or inspect or mutate existing cache or upload-queue state.

## Historical Kache qualification evidence

A limited devmbp comparison used official arm64 releases Kache 0.26.0 and sccache 0.18.0, Rust/Cargo 1.95.0, macOS 26.4.1, and APFS. Three fresh-cache runs each executed `cargo check -p phoenix-core --locked` with `CARGO_INCREMENTAL=0`: cold population in source A, a touched same-worktree crate rebuild, then an empty-target restore in detached source B.

| Backend | Cold seconds, raw (median) | Edit seconds, raw (median) | Cross-worktree seconds, raw (median) |
|---|---|---|---|
| Kache 0.26.0 | 57.196, 49.783, 42.789 (**49.783**) | 1.011, 0.814, 0.862 (**0.862**) | 26.462, 21.200, 23.094 (**23.094**) |
| sccache 0.18.0 | 42.244, 35.524, 30.799 (**35.524**) | 1.033, 0.883, 0.905 (**0.905**) | 35.008, 29.643, 28.846 (**29.643**) |

Kache was slower to populate, tied for normal edits at this sample size, and faster for the intended empty-target cross-worktree restore. Its final sample reported 250 local hits, 253 misses, 0 errors/fallbacks, 49.7% hit rate, 251,559,534 restored bytes, and 100% zero-copy restore. sccache reported one Rust hit and 382 Rust misses across relocated sources (its 356 total hits were mostly C/C++/assembler), with no cache read/write errors or timeouts. These historical 0.26.0 results establish useful Kache cross-worktree behavior but are not a general performance or correctness promise. Those measurements and the separate v1.0.0 debug-fidelity evidence do not qualify v1.2.2 native restored archives.

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

Historical validation for official Kache 1.0.0 on macOS arm64 covered version probe, daemon startup over a short explicit socket, Rust compilation, APFS reflink restore, and a two-source run through `./dev.py check --all --lanes e2e --compiler-cache kache`. That evidence establishes debug fidelity for exact 1.0.0 only. Earlier Kache 0.26.0 production-build and benchmark evidence likewise remains historical input.

The exact v1.2.2 release received its own isolated local-only native qualification described above: owned daemon startup, clean-root application and dependency local hits without additional misses, zero remote hits, a UUID-matched restored dSYM, and resolved LLDB application and dependency file/line locations. Source-level tests separately cover selection, environment/configuration shaping, and child ownership. Not validated for v1.2.2: production activation, Windows or Linux restore/signing/debug-symbol behavior, other macOS architectures, cross-device filesystems without reflinks, or remote/S3 caches. Phoenix makes no compatibility or performance guarantee for those paths.

### Managed-remote qualification blocker in Kache v1.0.0

Historical Kache v1.0.0 could not qualify multiple managed daemon identities against one remote-enabled base store. Pinned release source `7c27329c286e32c17a8dac05cff22442926ff719` establishes two upstream boundaries:

- `Config::load_resolved` calls `isolate_cache_dir_for_trust_domain`; Kache's own `trust_domain_isolates_stores_and_fails_open` test requires an unusable domain path to return the unscoped base store. Distinct identities can therefore converge on one durable `upload-queue` when isolation cannot be created.
- `Config::cache_dir_for_path` returns configured volume-shard paths directly, and `Config::routed_for_path` installs that path without appending the trust domain. Distinct identities can therefore share configured shards even when their primary stores differ.

Local fork commit `dbdd84955453b45ab13367203c224d8345e5081d`, based on the pinned v1.0.0 release commit, makes trust-domain isolation fail closed and applies the domain to configured volume shards and the compiler-family probe cache. Executable regressions prove stable same-domain reuse, different-domain primary and shard separation, invalid and non-Unicode rejection, and production durable-upload intent replay only after returning to the originating domain. The exact head passed independent review. It remained local and unpublished and still reported `kache 1.0.0`, indistinguishable by version from the vulnerable official v1.0.0 artifact. This historical blocker explains why Phoenix does not use v1.0.0; it does not require a remote-isolation fix for the separately bounded v1.2.2 local-only milestone.

## Release provenance

The historically debug-fidelity-qualified Kache v1.0.0 release publishes `kache-aarch64-apple-darwin.tar.gz` with SHA-256 `6d1be0079d0689a85fa04b7fed7eaa94f7e08259cafb8bd361a5c25c4c98c4a3`. Its release tag peels to `7c27329c286e32c17a8dac05cff22442926ff719`, which contains upstream fixes #1168 and #1170. Phoenix's representative validation used that official arm64 release artifact and verified its published checksum before running the two-source journey. The release-tag source contains the corrected `dsymutil` path and dSYM content regression.

The historical comparison used Kache 0.26.0. The GitHub release API identified immutable release `v0.26.0`, published 2026-09-19, tag target commit `af30b9102f538f8ceebc974ec9dc827e2feef0d8`, and arm64 archive SHA-256 `422c59f988245575e6258629c58e0fc8c6d57639734f57d1b04bb5f015a1acdc`. The downloaded checksum file matched, the tag commit's GitHub signature verification was valid, the archive contained a Mach-O arm64 executable, and it reported `kache 0.26.0`. GitHub's release page advertised an attestation, but `gh attestation verify` returned HTTP 404 for the archive digest; no attestation-success claim is made.

Upstream maintainer changes corresponding to restored-artifact hash reuse (#803, fixing #540), streaming hashing (#804), and runtime scan memoization (#805) are in the released line. Phoenix's historical private patched-versus-stock measurements were not used for this decision.

Kache 1.0.0 contains the macOS debug-bundle cwd correction and content-level regression that cleared the observed 0.26.0 blocker, but Phoenix does not select that release because of its trust-domain isolation defects. The selected v1.2.2 local-only milestone is recorded in [ADR-225](../../specs/adrs/225_kache-v1-2-2-strict-local-only-selection.md); its distinct release identity and configuration boundary do not transfer v1.0.0 native debugger qualification to v1.2.2.
