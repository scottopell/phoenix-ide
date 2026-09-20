# Integrate released Kache v0.26.0 into Phoenix development workflows

## Observed journey

Phoenix exposes `auto`, `kache`, `sccache`, and `none`, but `auto` selects sccache whenever both tools exist. Kache is selectable but not normally adopted. Deliver a qualified integration of released upstream Kache v0.26.0 on devmbp—not another recommendation-only audit and not a revival of Phoenix’s private patch stack.

This checkout is 42 commits behind the locally known `origin/main`; implementation must fetch and integrate current GitHub `origin/main` once before editing. Neither cache executable is currently on this sandboxed process’s `PATH`, so installation and provenance evidence are real execution work.

## Verified findings

- `dev.py::_configure_compiler_cache` is the framework merged from Phoenix PR #543: explicit-wrapper precedence, four backend values, Kache daemon startup, a private short socket keyed by `KACHE_CACHE_DIR`, separate sccache setup, and check profile/tracing metadata.
- Current `auto` is explicitly sccache-first; Kache is automatic only when sccache is absent (`test_auto_preserves_sccache_first_behavior`).
- Explicit Kache fails on missing binary or daemon error. Automatic Kache daemon failure clears generated environment and reports `none`. If `auto` becomes Kache-first, failure must honestly try qualified sccache rather than become no-cache or retain a Kache label.
- Cache setup occurs only in `cmd_check`. Ordinary `build_rust` paths (`up`, `restart`, seed/build helpers) and `prod_build` inherit ambient environment but do not select/setup a cache. This is the main normal build/deploy-preparation gap.
- `collect_doctor_results` has no cache install/version reporting. Existing guidance only says Kache must be on `PATH` or named by `PHOENIX_KACHE_BIN`.
- GitHub marks v0.26.0 (`af30b91`, 2026-09-19) as an immutable latest Kache release and publishes macOS archives, checksums, and release attestation. Use the official arm64 artifact (or another verifiably released installation route), verify it, and do not apply `patches/kache`.
- Maintainer PR #803 merged 2026-08-23 and fixes issue #540 through restored-artifact hash memoization; #804 stream hashing and #805 scan memoization also merged. Old patched-vs-stock data is historical context, not released Kache-vs-sccache evidence.
- No compiler-cache normative spec exists. REQ-COMP-001 forbids implied compatibility guarantees. Document tested bounds and unvalidated paths; do not expand REQ-PROJ-005A target cloning.

## Interaction map

Selection (`--compiler-cache`, `PHOENIX_COMPILER_CACHE`, or caller `RUSTC_WRAPPER`) → one cache resolver/setup seam → released executable/version plus daemon/socket readiness → inherited environment for check, ordinary Cargo builds, and non-activating production-build preparation → reporting that names the backend actually used.

Cache/socket state persists outside the process. Explicit selection must fail clearly; `auto` may try qualified candidates and then `none`, always recording the actual outcome. Benchmarks use isolated disposable paths, never active targets/caches or source mutations.

## Proposed scope

### 1. Current main and released provenance

- Fetch and integrate current `origin/main` once before implementation; record the exact base.
- On devmbp, install/use official Kache v0.26.0 without the old checkout or Phoenix patch stack. Verify release tag/commit, asset checksum, available GitHub attestation/provenance, executable architecture, and `kache --version == 0.26.0`. Record sccache and pinned Rust/Cargo versions.
- Add concise Phoenix installation/setup/version guidance. Normal `dev.py` commands must not auto-install arbitrary binaries.

### 2. Complete—not replace—the existing cache seam

- Reuse/refactor the existing selection, daemon, socket, environment, and reporting code so it covers `./dev.py check`, ordinary local build paths used by `up`/`restart`, and build-only production preparation. Do not create a second cache framework.
- Ensure explicit `PHOENIX_COMPILER_CACHE=kache` and equivalent CLI use the verified wrapper and release-compatible daemon/socket behavior. Preserve explicit `sccache`, `none`, and caller-owned `RUSTC_WRAPPER`.
- Report installed/selected backend and version through a clear setup/doctor/status surface. Never label fallback sccache/no-cache work as Kache.
- Explicit missing/incompatible Kache fails actionably. `auto` may fall through to the next qualified backend and ultimately `none`, reporting each reason and final selection. Keep backend environments separate.
- Choose and document the final `auto` order from v0.26.0 compatibility and measurements below. No blind global flip or performance guarantee.

### 3. Limited isolated devmbp comparison

Use owned cache/target/worktree paths; identical pinned source and Cargo inputs; sequential/interleaved order where practical; no concurrent local-Mac load; and no active cache/source mutation. Do not broadly purge, kill unrelated daemons, deploy, configure remote storage, use scheduler/manual build slots, or benchmark on the local Mac.

Compare released Kache v0.26.0 with released sccache for:

1. cold cache population into an empty isolated target;
2. normal same-worktree edit/rebuild;
3. cross-worktree restore into an empty isolated target at a different path;
4. actual Phoenix paths: representative/full check Cargo lanes and ordinary/build-only production preparation, bounded to avoid duplicate expensive runs.

Capture raw wall time, commands/results, native hit/miss/error/passthrough or restore data, and correctness (successful output/tests, no wrapper error/stale result). Use repeated samples; report range/distribution, ordering, cache state, and host noise without manufactured significance.

Measure **extra unique allocated storage** for isolated cache plus restored targets, not summed apparent `du`. Record APFS volume identity; quiesce/synchronize; take whole-volume allocated/free-block snapshots around each owned scenario; and cross-check tree allocated blocks (`du -sk`/`st_blocks`) plus Kache reflink reporting. APFS clone sharing and background allocation make volume deltas estimates. If exact extent accounting is unavailable without privileged/destructive tooling, state that and report bounded repeated estimates and limitations—not false precision.

Validate macOS arm64 on devmbp and deterministic Linux behavior in hosted CI. Explicitly list unvalidated executable-cache/debug-symbol or other OS paths rather than generalizing.

### 4. Deterministic tests, docs, and delivery

Add `tests/devpy` coverage for:

- CLI/environment/wrapper precedence and all backend values/`auto` ordering;
- exact version probes and actionable missing/incompatible diagnostics;
- v0.26.0 daemon command and private socket environment;
- explicit daemon failure versus honest `auto` fallback to sccache then none;
- reporting that cannot mislabel fallback;
- environment propagation through check, ordinary build, and production-build subprocesses without deployment;
- relevant macOS/Linux command-shape differences.

Document installation, overrides, selected default rationale, raw benchmark results/tradeoffs, and compatibility limits in the smallest suitable developer/spec/ADR surfaces. Add an ADR only if changing the default is a durable policy decision; do not invent a product worktree-cache requirement.

Run focused tests, full `./dev.py check`, and inspect the complete diff. Commit/push one branch and open one PR. Wait for hosted CI; request fresh Codex review at the exact final head; retain this owner through fixes; paginate review threads to cursor exhaustion and record exact head, checks, Codex status, total/unresolved thread counts, and pagination completion. Do not merge or deploy.

## Acceptance evidence

- Official evidence ties the installed arm64 executable to immutable v0.26.0/tag commit, checksum/attestation, architecture, and runtime version; no private patches.
- Daemon/socket smoke test and real wrapped compile prove explicit Kache is active; environment and Kache-native reporting agree.
- Tests prove precedence, diagnostics, daemon/socket handling, honest fallback/labels, and propagation through check/build/production-build seams.
- Explicit `kache`, `sccache`, and `none` work; explicit failures are actionable; `auto` follows the measured documented policy without mislabeling.
- All four benchmark classes include raw samples, protection/correctness evidence, timings, backend statistics, and an honest APFS unique-allocation estimate with limitations. Historical patched comparisons are not reused.
- Full `./dev.py check` and final-head hosted CI pass.
- Exact-head Codex review completes, findings are addressed, and review pagination is exhausted.
- One pushed branch/PR contains the integration; no merge/deploy.
- Final report separately states release/provenance evidence; functional integration; benchmark method/results/tradeoffs; installation/default story; exact PR head/CI/Codex/pagination state; and unvalidated OS paths.

## Explicit non-goals and safety limits

No declined upstream #543 resurrection, Phoenix patch stack, old patched-vs-stock comparison, target-tree sharing/cloning product feature, remote S3/paid setup, global purge, unrelated daemon termination, production deploy, scheduler/manual build slots, local-Mac benchmark load, performance guarantee, merge, or deploy.
