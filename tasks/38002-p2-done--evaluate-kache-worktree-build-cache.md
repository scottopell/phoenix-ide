# Evaluate kache for worktree-local Rust build efficiency

## Goal

Integrate `kache` as an optional Rust compiler-cache backend in `dev.py`, measure it against the existing `sccache` behavior under Phoenix’s real multi-worktree build shape, and use the evidence to decide whether Phoenix should offer kache-backed Rust artifact reuse as a product feature.

The first deliverable is a concrete, reversible `dev.py` optimization and reproducible local measurements. Product-level worktree integration is a second stage, gated on those results.

## Existing behavior to preserve

- `dev.py check` automatically uses `sccache` for active Cargo lanes when it is installed and `RUSTC_WRAPPER` is not already set.
- An explicitly supplied `RUSTC_WRAPPER` always wins.
- The main Rust/test and e2e lanes share the workspace target directory; clippy uses `target/clippy` to avoid Cargo lock contention and fingerprint churn.
- `dev.py` reports Cargo target-lock wait time.
- Phoenix’s `project_opportunistic_build_warm` feature uses macOS `clonefile(2)` copy-on-write clones for a narrow allowlist of JS caches when creating worktrees. REQ-PROJ-005A intentionally excludes Cargo `target/` and forbids a large physical-copy fallback.

`kache` complements the existing prewarm feature: it restores content-addressed compiler outputs into independent target directories using reflinks on APFS (hardlinks or copies where necessary), rather than cloning an entire mutable Cargo target tree.

## Plan

### 1. Establish compatibility and benchmark methodology

- Record the installed Phoenix Rust/Cargo version, kache version, sccache version, filesystem/volume identity, and restore mechanism reported by kache.
- Attempt the straightforward Phoenix Rust upgrade from 1.94.x to 1.95 first: update `rust-toolchain.toml` and every workspace crate’s `rust-version`, regenerate lockfile/tool-generated effects if any, and run the full checks. Keep this upgrade in the task when it is limited to routine compatibility fixes. If Rust 1.95 exposes substantial unrelated migration work, defer the upgrade explicitly and use a prebuilt kache binary or an isolated 1.95 toolchain solely to install it.
- Define deterministic local scenarios representing Phoenix’s actual use:
  1. same-worktree rebuild after deleting the target directory;
  2. second Phoenix worktree at a different absolute path with an empty target directory;
  3. the dedicated clippy target directory, where compiler-cache reuse is already important;
  4. optionally a normal incremental rebuild as a regression check, because kache disables Cargo incremental compilation while wrapping rustc.
- Compare three backends: no wrapper, sccache, and kache. Use isolated backend cache directories and identical Cargo commands, features, environment, and target-directory shape.
- Run sequential repeated samples, retain raw per-run wall times, and report medians/ranges rather than relying on one run. Capture `sccache --show-stats`, `kache stats`/`kache report`, target/cache apparent size, physical disk usage where available, and Cargo lock-wait telemetry.

### 2. Add an explicit compiler-cache policy to `dev.py`

- Refactor the current inline sccache setup into a small, unit-testable compiler-cache selection helper.
- Support an explicit backend policy with at least `auto`, `kache`, `sccache`, and `none`; choose a narrowly scoped environment variable or CLI surface consistent with existing `dev.py` conventions after inspecting its option/config patterns.
- Preserve explicit user configuration: never overwrite `RUSTC_WRAPPER`.
- In `auto`, prefer the backend justified by the benchmark. Until evidence supports changing the default, preserve sccache-first behavior; do not silently switch every developer to preview software.
- A requested but unavailable backend must produce an actionable message rather than silently selecting a different backend. `auto` may degrade cleanly.
- Keep backend-specific settings typed/separate: do not set SCCACHE variables for kache or conflate their cache-size/GC semantics.
- Apply selection consistently to every Cargo path that currently benefits from the inherited wrapper, not only one check lane.
- Add focused `tests/devpy` coverage for precedence, availability, disabled caching, explicit `RUSTC_WRAPPER`, and backend-specific environment.

### 3. Validate real local efficiency

- Run the controlled scenarios before and after the `dev.py` integration.
- Verify cache correctness by requiring successful builds/tests and by inspecting wrapper miss/passthrough/error reporting.
- Determine whether kache provides material benefits in:
  - cross-worktree warm build time;
  - physical disk consumption on APFS;
  - clippy’s independent target directory;
  - target-lock avoidance when independent target directories are used.
- Explicitly quantify any regression from kache disabling incremental compilation in ordinary edit/build cycles.
- Save a concise, reproducible benchmark report in an appropriate project documentation/task location, including commands, raw samples, versions, filesystem, and limitations.

### 4. Decide the Phoenix product feature boundary

If measurements show a meaningful win, specify and implement a follow-up product capability that configures kache for Phoenix-created worktrees without copying or sharing a mutable Cargo `target/` tree. The design must:

- remain optional and best-effort when kache is unavailable;
- preserve worktree path isolation;
- avoid making worktree creation depend on successful cache setup;
- expose unsupported/fallback behavior in logs;
- avoid modifying global Cargo configuration without explicit user consent;
- define cache ownership, limits/GC, cleanup, filesystem fallback, and diagnostics;
- update `specs/projects/requirements.md` and executive coverage before changing product behavior.

If measurements do not show a worthwhile improvement, retain only useful explicit `dev.py` backend selection/benchmark support, or revert the integration, and document the evidence. Do not expand REQ-PROJ-005A to include Cargo `target/` cloning merely to force reuse.

## Acceptance criteria

- `dev.py` has deterministic, tested compiler-cache selection and respects an existing `RUSTC_WRAPPER`.
- Users can explicitly select kache, sccache, or no compiler cache; unavailable explicit selections fail clearly.
- Existing sccache behavior is preserved unless repeatable benchmark evidence supports a default change.
- A reproducible benchmark compares no wrapper, sccache, and kache across same-worktree and cross-worktree cold-target scenarios with raw samples and cache/disk statistics.
- The benchmark includes an incremental-edit regression check, and Phoenix is upgraded to Rust 1.95 when that remains a routine pin/compatibility update; otherwise the report records why an isolated/prebuilt kache installation was used.
- `./dev.py check` passes.
- Product-level integration is either backed by a normative spec update and evidence, or explicitly deferred with the measured reason.

## Results

Environment: macOS arm64 on APFS (`/System/Volumes/Data`), Rust/Cargo 1.95.0, kache 0.9.0 built from the supplied checkout, and sccache 0.15.0. Measurements used `cargo check -p phoenix-core --locked`, `CARGO_INCREMENTAL=0`, sequential execution, isolated backend caches, and empty target directories. Cross-worktree samples used a different absolute source and target path for every run.

### Cross-worktree empty-target samples

| Backend | Run 1 | Run 2 | Run 3 | Median | Final cache |
|---|---:|---:|---:|---:|---:|
| none | 19.616s | 19.620s | 18.880s | 19.616s | — |
| sccache | 25.336s | 21.040s | 19.979s | 21.040s | 170.7 MiB apparent |
| kache | 26.344s | 22.503s | 19.295s | 22.503s | 314.5 MiB apparent |

Each target directory was approximately 284 MiB apparent. kache reported 56.2% hits (342 local, 6 duplicate, 261 misses), 49.2% weighted by compile cost, 311.3 MiB physical blobs, and 2.6% deduplication savings. Its daemon remained offline, so this measured the local wrapper/store path only. sccache reported 0% Rust hits across changed absolute paths in this workload (573 Rust misses); its reported hits were C/C++ and assembler compilations.

### Same-path clean and edit-cycle probe

| Backend | Cold population | Empty same-path target | Touch workspace crate |
|---|---:|---:|---:|
| none | 19.233s | 23.168s | 1.904s |
| kache | 53.866s | 17.650s | 0.642s |

This small probe confirms that kache can reuse Rust outputs and materially improve a populated-cache rebuild, but cache population overhead is substantial. The cross-worktree median was slower than no wrapper and the three samples trend downward with OS/filesystem warming, so the result is directional rather than statistically conclusive.

### Decision

Keep kache as an explicit `dev.py check --compiler-cache kache` / `PHOENIX_COMPILER_CACHE=kache` opt-in. Preserve sccache-first behavior for `auto`; use kache automatically only when sccache is absent. Do not add Phoenix product-level worktree configuration or expand REQ-PROJ-005A: these local measurements do not show a consistent cross-worktree wall-time or disk-efficiency win sufficient to justify managing another external tool. The existing APFS copy-on-write prewarm remains complementary and intentionally limited to allowlisted JS caches rather than mutable Cargo target trees.

Phoenix was upgraded routinely to Rust 1.95.0 with no source compatibility changes required.
