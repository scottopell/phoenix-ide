# `./dev.py check` regression-ROI audit

**Source audited:** `origin/main` `bed747b5bd231f495b8038163d63c72a9ebeb346` (the task commit was rebased above it as `da0fcf2d`). **Host:** `devmbp`, MacBookPro18,4, macOS 26.4.1, 10 logical CPUs, 64 GiB RAM, APFS. **Captured:** 2026-09-19. No cache was purged. No lane, test, gate, timeout, or workflow was changed.

## Result

The current command has 13 path-gated, sequential lanes. The default invocation for this documentation-only branch selected only always-on task validation and took **0.47 s process wall / 0.14 s internal check wall**. A non-profiled, all-lane warm invocation took **491.1 s**; it retained one load-sensitive tmux cleanup failure and canceled 1,689 later Rust tests, so its Rust execution time is a lower bound. The profiler materially changes Rust execution (one wrapper per test), so its **1,073.8 s** full run is used only for complete per-lane shares and explicitly not as ordinary warm performance.

There is no current-source cold measurement: the only naturally empty target was consumed at stale head `bac41074c` before the source correction. That sample is retained below as historical context, not current evidence. The highest-ROI policy is the existing one: cheap task validation always on; every substantive lane conditional on relevant paths; CI runs the same groups. No gate should be removed from this evidence.

## Evidence and caveats

- Raw ignored artifacts: `target/check-roi-audit/` and `target/check-profile/{roi-cold,roi-current-warm-1,roi-current-default}/`.
- Current ordinary warm raw sample: `current-warm-normal.log`. Current profiled sample: `current-warm-1.log` and profile JSON. Representative default: `current-default.log`.
- `--profile-work` added substantial overhead to Rust test execution. Percentages below are internally consistent shares of that same serialized profiled run; ordinary warm times come from the non-profiled run.
- During the profiled run, two tmux tests failed at 10–15 s hook waits and passed in one corrected focused rerun (2.38 s and 7.68 s). The ordinary run later failed a different tmux test during watchdog cleanup. This is evidence of load-sensitive test/tooling behavior, not favorable-result rerun justification; the full failures remain the reported samples.
- The first focused diagnostic accidentally supplied libtest `--exact` without the module path and ran zero tests. It is excluded; corrected logs are retained. This is a bounded audit-command error, not a product gate bug.
- “Observed frequency” has two views: (a) simulation of the **current** classifier over 50 first-parent `origin/main` commits, 2026-08-24 through 2026-09-19; and (b) the latest 30 completed PR-triggered CI runs (2026-09-19 20:58–23:22 UTC). The latter instantiated all five job names in all 30 runs because conditional jobs appear as `skipped`; it is useful for outcomes, not lane scheduling frequency. Commit simulation is not PR-aggregate frequency.

## Per-lane inventory

Current profiled critical path was 1,073.8 s; lane records sum to 1,072.7 s (99.90%), leaving 1.1 s setup/report overhead. “Cold” is unknown for current source. Ordinary warm values marked `≥` are lower bounds because nextest stopped after a failure.

| Lane | Exact purpose and distinct regression protection | Current ordinary warm; profiled share | Observed classifier frequency | Extra unique allocated disk / reuse | Local and CI overlap | Recommendation |
|---|---|---:|---:|---|---|---|
| `rust` | Nextest compile; ts-rs export/staleness; workspace tests excluding duplicate export tests (`lane_rust`). Protects compile/type contracts, generated Rust→TS parity, and ~4,043 unit/integration behaviors. | **≥289.7 s**: compile 19.3, codegen 17.8, tests ≥252.6. Profile: 695.7 s, **64.8%**. Cold: unknown. | 31/50 (62%) | Shares `target/debug` with E2E/focused Cargo work; no honest per-lane split. Current-source compilation/profile activity added 6.52 GiB to debug. Reused until sources/features/toolchain invalidate Cargo fingerprints. | CI `check (rust)`, PR path-gated and all main pushes; codegen overlaps UI only at typed boundary, not behavior. | **Conditional (retain current).** Avoids ≥290 s on non-Rust changes while retaining the broadest protection. Investigate tmux load sensitivity separately; do not weaken this gate. |
| `cargo-fmt` | `cargo fmt --check`; syntactic formatting drift. | 2.8 s; profile 3.8 s, **0.4%**. Cold: effectively cache-independent but not separately measured. | 31/50 (62%) | Negligible persistent output. | Same `check (rust)` job; no behavioral-test substitute. | **Conditional (retain).** Only ~3 s when Rust changes and distinct deterministic protection. |
| `clippy` | `cargo clippy --all-targets -- -D warnings`; static/pedantic defects including tests. | 60.8 s; profile 88.6 s, **8.3%**. Cold: unknown. | 31/50 (62%) | Dedicated non-incremental `target/clippy`: stale natural build allocated **937,040 KiB (915 MiB)**; current refresh added 47,044 KiB. Isolation prevents clippy-driver invalidating Rust-test fingerprints; source/toolchain changes rebuild. | CI `check (clippy)`; overlaps compilation but catches lint classes tests do not. | **Conditional (retain).** CI-only would save ~61 s on Rust changes but delay static feedback; dedicated target trades ~915 MiB for avoiding cross-lane invalidation. |
| `tsc` | `pnpm run typecheck` (`tsc -b --noEmit`); project-reference and `exactOptionalPropertyTypes` errors. | 0.5 s warm; profile 15.1 s, **1.4%**. Cold: unknown. | 27/50 (54%) | No emitted build tree from `--noEmit`; shares installed UI dependencies. | CI `check (ui/specs)`; ESLint/Vitest do not prove TS project-reference correctness. | **Conditional (retain).** Warm cost is sub-second and protection is unique. |
| `ui-lint` | ESLint then Stylelint; TS/React hook/static rules and CSS validity/conventions. | 8.4 s (6.7 + 1.7); profile 12.8 s, **1.2%**. Cold: unknown. | 27/50 (54%) | No material lane artifact; shares `ui/node_modules`. | CI `check (ui/specs)`; partially overlaps tsc syntax, but lint/CSS rule classes are distinct. | **Conditional (retain).** ~8 s for two unique static surfaces. Keep consolidated as one lane. |
| `vitest` | UI component/state/hook tests. | 39.5 s; profile 47.7 s, **4.4%**. Cold: unknown. | 27/50 (54%) | Transient workers only; shares `ui/node_modules`. | CI `check (ui/specs)`; overlaps code touched by tsc/lint, not asserted behavior. | **Conditional (retain).** CI-only saves ~40 s per UI change but loses the only broad UI behavioral feedback locally. |
| `ast-grep` | One structural-rule scan over Rust/UI, then changed-test timing lint (`check_ast_grep`). Protects repository-specific forbidden shapes and newly introduced slow Rust tests. | 4.0 s (0.9 + 3.1); profile lane 5.2 s, **0.5%**. Cold: near cache-independent, not separately measured. | 42/50 (84%) | Negligible persistent output. | CI `check (ui/specs)` even for Rust changes; overlaps lint scanning but rules and timing-diff contract are unique. | **Conditional (retain).** High frequency but only ~4 s. Keep the two steps consolidated to avoid another job/lane. |
| `allium` | Parse/analyse every Allium spec and reject new findings beyond the checked baseline. | 0.1 s; profile 0.2 s, **<0.1%**. Cold: tool install excluded. | 28/50 (56%) | None in worktree. | CI `check (ui/specs)`; no other lane validates Allium grammar/finding baseline. | **Conditional (retain).** Unique protection at ~0.1 s. |
| `spec-shape` | Validate spEARS v2 artifact shape, then run all `tests/devpy` orchestration/deployment/check tests. | 34.7 s ordinary; profile 149.5 s, **13.9%**. Cold: unknown. | 28/50 (56%) | Python/uv caches are shared host state and not attributable; no material worktree output. | CI `check (ui/specs)`; shape validation is unique, while dev.py tests protect the check/deploy/task machinery itself. | **Conditional (retain).** The 35 s cost is dominated by distinct dev.py regression tests. A future split could improve attribution, but consolidation/removal has no proven protection-preserving saving. |
| `spec-anchors` | Cross-check code `REQ-*` references against declarations. | 0.6 s; profile 0.6 s, **0.1%**. Cold: cache-independent. | 46/50 (92%) | None. | CI `check (ui/specs)`; intentionally spans specs, Rust, and UI; shape validation cannot catch orphan code anchors. | **Conditional (retain).** Very frequent but sub-second and unique. |
| `e2e` | Build/run a real Phoenix binary and drive HTTP/SSE scenarios with isolated DB/mock model. | 48.8 s; profile 53.4 s, **5.0%**. Cold: unknown. | 32/50 (64%) | Reuses normal `target/debug` dependencies but must link non-test binary; incremental bytes cannot be separated safely from Rust lane in the shared tree. | Dedicated CI `check (e2e)`; overlaps user journeys but uniquely crosses binary/API/SSE/process boundaries. | **Conditional (retain).** CI-only saves ~49 s on Rust/E2E changes but removes the only local real-binary boundary gate. |
| `task` | Validate task filename grammar and global ID uniqueness. Always on. | 0.0 s (12 ms profile); **<0.1%**. | 50/50 (100%) | None. | CI `check (task validation)` plus roadmap reducer test. No substitute. | **Retain local always-on.** Negligible cost; it caught the intentionally plain approval brief until that proposal artifact was removed after approval. |
| `pkglock` | Fail when `ui/pnpm-lock.yaml` has uncommitted drift that a frozen deploy install would reject. | <0.1 s; profile 0.1 s, **<0.1%**. Cold: cache-independent. | 27/50 (54%) | None. | CI `check (ui/specs)`; CI frozen install also fails lock drift, but local tripwire gives immediate deploy-relevant feedback. | **Conditional (retain).** Near-zero cost. |

### Shared UI dependency footprint

The naturally empty stale worktree installed `ui/node_modules` once: **528,144 KiB allocated (516 MiB)** versus 434,740 KiB apparent. This is shared by `tsc`, `ui-lint`, and `vitest`; it must not be charged three times. Lockfile changes invalidate package selection; ordinary source changes reuse it.

### Historical, non-current cross-checks

- **Stale-topology devmbp natural cold, `bac41074c`:** 1,221.6 s profiled; all substantive lanes passed, proposal-filename task validation failed. It allocated 9.45 GiB in `target/debug`, 915 MiB in `target/clippy`, 516 MiB UI dependencies, and 18.7 MiB profile data. Because `origin/main` had advanced to `bed747b5`, none of these timings is a current cold claim.
- **Externally supplied deployment log, other MacBook, `df04ca49`, concurrent load:** normal `./dev.py check` 19/19 in 1,092.6 s; compile 196.0, cargo test 522.2, dev.py tests 132.4, clippy 82.2, E2E 53.4, Vitest 41.8, codegen 31.4, tsc 14.0, ESLint 7.2, timing lint 3.9, fmt 3.5, remainder <2 s. Source: `.phoenix/deploy/evidence-main-df04ca49/deploy.log` on that host. This only corroborates lane inventory/order and load sensitivity; it is neither controlled devmbp evidence nor current-source evidence.

## Musl finding

Musl is **not** part of default or `--all` local `./dev.py check`; `lane_rust` explicitly excludes a Linux-musl target tree. It is a separate step in `.github/workflows/ci.yml` job **`check (rust)`**:

```text
cargo check --target x86_64-unknown-linux-musl --features phoenix_ide/datadog-tracing
```

The workflow triggers on every PR and pushes to `main`. On PRs, `plan check lanes` schedules `check (rust)` for the `RUST` group; pushes force all groups. Musl runs only after local-equivalent `rust,cargo-fmt` succeeds. In the latest 30 completed PR CI runs, the Rust job was 18 success, 5 failure, 4 canceled, 3 path-skipped; the musl step itself reached **18 successes** and was recorded skipped 8 times (normally because the job/path or preceding Rust step did not reach it). Thus CI-only placement preserves pre-merge protection whenever the Rust job reaches the step, but feedback is delayed until after the much longer Rust suite.

**Unique protection:** compilation of the production-only `phoenix_ide/datadog-tracing` feature for Linux musl, target-specific `cfg`/dependency compatibility, and native C build configuration. Because this is `cargo check`, it does **not** prove final static linking or runtime behavior; release workflow builds both x86_64/aarch64 musl artifacts and supplies compilers.

**Measured devmbp capability/cost:** the Rust std target was already installed and occupies **222,708 KiB allocated (217.5 MiB)**. Neither `musl-gcc` nor `x86_64-linux-musl-gcc` was installed. One exact-command attempt failed honestly after **10.33 s** at `aws-lc-sys` because `x86_64-linux-musl-gcc` was absent. The failed attempt allocated **51,260 KiB (50.1 MiB)** under `target/x86_64-unknown-linux-musl` plus 107,208 KiB in shared `target/debug`; this is a lower bound, not successful-musl footprint. Successful current-source wall time and total unique build-tree cost are therefore **unknown on devmbp**. Installing a cross compiler solely to complete this audit was not necessary and would have changed host-global state.

**Recommendation: CI-only (retain existing placement).** Removing it from local `check` saves **0 s** because it is already absent. Adding it locally would impose at least 217.5 MiB toolchain plus >50.1 MiB artifacts and a cross-compiler prerequisite. CI provides relevant Linux tooling and observed pre-merge execution; the tradeoff is delayed feedback after Rust tests. Keep release builds as the stronger final-link protection. Do not claim the historical 8.6 s figure as current.

## CI mapping and recommendation summary

Exact workflow mapping at `bed747b5`:

- `check (rust)`: `rust,cargo-fmt`, then musl production-feature check.
- `check (clippy)`: `clippy`.
- `check (e2e)`: `e2e`.
- `check (ui/specs)`: `tsc,ui-lint,vitest,ast-grep,allium,spec-shape,spec-anchors,pkglock`.
- `check (task validation)`: `task` (and a separate roadmap reducer test).

All jobs are merge-time PR checks subject to the current path plan; all groups run on pushes to `main`. The current local design already captures the measurable ROI: an unchanged/docs-only branch paid 0.47 s instead of roughly 491 s, while relevant edits select their distinct gates. Recommended changes to gates: **none**. Potential follow-up investigations—not changes commissioned here—are load-sensitive tmux cleanup tests and whether `spec-shape` should be renamed/split solely for clearer ownership; neither has evidence supporting removal or CI-only conversion.

## Reproduction commands

Every instrumented command began with assertions for `hostname -s == devmbp` and a worktree root under `/Users/sopell/git/phoenix-ide/.phoenix/worktrees/`.

```bash
./dev.py check-plan --all --format json
./dev.py check-plan --format json
./dev.py check --all --profile-work --profile-work-dir target/check-profile/roi-current-warm-1
./dev.py check --profile-work --profile-work-dir target/check-profile/roi-current-default
./dev.py check --all
CARGO_INCREMENTAL=0 cargo check --target x86_64-unknown-linux-musl \
  --features phoenix_ide/datadog-tracing
```

Disk snapshots used `du -sk` (allocated blocks on this APFS host) and `du -skA` (apparent bytes) immediately before/after owned runs. Shared trees are reported once; APFS/apparent values are not summed as unique physical cost.
