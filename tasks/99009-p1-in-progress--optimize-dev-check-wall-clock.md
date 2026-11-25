# Optimize `./dev.py check --all` full-suite wall-clock time

## Objective

Reduce the wall-clock time of Phoenix IDE's complete supported `./dev.py check --all` suite on `sopell-worker-one` (16 vCPU, 61 GiB RAM) while strictly preserving every required validation lane, test assertion, failure propagation, and reproducibility contract.

## Supported Full-Suite Invocation

- Command: `./dev.py check --all` (equivalent to `PHOENIX_CHECK_ALL=1 ./dev.py check`).
- Active lanes (15 total, confirmed via `dev.py check-plan --all`):
  `rust`, `clippy`, `tsc`, `ui-lint`, `vitest`, `cargo-fmt`, `task`, `ast-grep`, `deps`, `allium`, `spec-shape`, `devpy`, `spec-anchors`, `pkglock`, `e2e`.
- Profiling command for bottleneck attribution: `./dev.py check --all --profile-work --profile-work-dir <empty-dir>`.

## Constraints & Guardrails

- Preserve every check/test, assertion strength, failure exit code, and `tests/devpy/test_check_plan.py` contract (including `PHOENIX_CHECK_WORKERS` override semantics, CI explicit gating, and fail-closed checks).
- Do not skip checks, weaken assertions, silently exclude tests, or misattribute warm-cache vs. cold-cache differences as intrinsic speedups.
- Clearly separate one-time environment/toolchain bootstrap cost from steady-state check wall time; record host load (`loadavg`), CPU count, and cache state (cold vs. warm) for every measurement.
- Work strictly inside this independent clone (`/home/bits/dev/phoenix-ide-optimize`) on a dedicated local branch; never modify `/home/bits/dev/phoenix-ide`, its configs, or unrelated repos. No push, merge, deploy, or release.

## Plan

1. **Bootstrap user-scoped prerequisites (measured separately)**
   - Install missing user-scoped tools required/expected by `./dev.py doctor` and the full 15-lane check without altering system/running Phoenix installations:
     - Rust `1.95.0` via `rustup` (`rustfmt`, `clippy`, `x86_64-unknown-linux-musl` target)
     - Node `>= 26` (per `.node-version`) + Corepack `pnpm@11.0.8` + `ui/node_modules`
     - `cargo-nextest`, `cargo-deny`, `allium-cli`, `@ast-grep/cli` (so optional lanes execute rather than silently skipping)
     - Chromium/Playwright browser binary if needed for browser tests (or explicitly document browser test status)
   - Verify `./dev.py doctor` and record bootstrap timings separately from check runs.

2. **Establish honest baseline & bottleneck breakdown**
   - Create a dedicated local optimization branch from `aeb32990e`.
   - Run clean cold/warm baseline measurements of `./dev.py check --all` and a `--profile-work` run to break down wall time and CPU time across all 15 lanes, steps, individual Rust tests, Vitest files, Python `devpy` tests, and E2E scenarios.
   - Identify concrete bottlenecks (e.g., lane execution / critical path, redundant Rust builds/links between `rust`/`e2e`/`devpy`, linker choice (`lld`), slow Rust/Python/UI tests or fixtures, `ensure_ui_deps` / Corepack overhead, per-step overhead).

3. **Implement evidence-driven optimizations**
   - Apply focused, cleanly split commits targeting the highest-leverage bottlenecks while respecting `AGENTS.md` principles and existing contracts (`tests/devpy/test_check_plan.py`, CI compatibility).
   - Verify each change preserves exact failure detection (including negative/fault checks where appropriate) and deterministic behavior.

4. **Trustworthy before/after measurement & full validation**
   - Re-run like-for-like steady-state measurements (same cache warmness, recorded `loadavg`, repeated runs where practical) comparing baseline `aeb32990e` vs. optimized HEAD.
   - Confirm full `./dev.py check --all` passes with zero skipped lanes (or explicitly report any truly unavailable external dependency).
   - Leave clean, review-ready local commits with full test evidence and recommendation.
