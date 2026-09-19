# Audit current `./dev.py check` regression ROI

## Goal

Produce one concise, evidence-backed audit of the **actual current** `./dev.py check`: what each lane uniquely protects, its wall-time and incremental disk cost, how often it reruns, where CI overlaps it, and whether it should remain local, become conditional or CI-only, or be consolidated. This task changes no tests, lanes, gates, timeouts, or policy.

All instrumented commands and measurements must run on host `devmbp`, inside the managed isolated worktree under `/Users/sopell/git/phoenix-ide`. Abort rather than measure elsewhere. Preserve worker checkouts, processes, source, and caches; do not manufacture a cold result by purging shared state.

## Grounded starting point

- `dev.py::_LANE_DEFS` currently defines 13 lanes: `rust`, `cargo-fmt`, `clippy`, `tsc`, `ui-lint`, `vitest`, `ast-grep`, `allium`, `spec-shape`, `spec-anchors`, `e2e`, `task`, and `pkglock`.
- `cmd_check` runs active lanes sequentially, so lane wall times contribute directly to the local critical path. Path gating and Rust crate scoping make the default invocation change-dependent.
- The opt-in `--profile-work` path already records command, lane, step, and test timing/CPU evidence under ignored `target/check-profile/`; use it rather than inventing a benchmark platform.
- The current local Rust lane explicitly says it does **not** generate a Linux-musl target tree (`dev.py::lane_rust`). Musl presently appears as a separate `cargo check --target x86_64-unknown-linux-musl --features phoenix_ide/datadog-tracing` step in GitHub Actions job `check (rust)`, after `./dev.py check --lanes rust,cargo-fmt`.
- `.github/workflows/ci.yml` runs on every pull request and pushes to `main`; PR lane groups are path-gated through `plan check lanes`, while pushes to `main` force all groups. The audit must verify exact present job/trigger conditions rather than infer them from lane names.
- No reusable `target/check-profile` files are present in this worktree or other currently visible managed worktrees. The completed profiling task records historical figures, including an 8.6 s musl smoke, but those ignored raw artifacts are absent and the check topology has since changed; treat those figures as historical estimates, not current measurements.

## Deliverable

Add a compact Markdown audit under `docs/audits/` containing a per-lane table and a focused musl section. For every lane (and musl as a separate CI-only check surface), report:

1. exact command/purpose and the distinct regression or defect class protected, with test/script anchors;
2. **measured** cold, warm, and representative/default wall time where safely obtainable, raw samples, and share of the sequential local critical path;
3. observed rerun frequency from a bounded, disclosed sample of repository changes and, where available, GitHub Actions runs;
4. **extra unique allocated disk** beyond a declared shared baseline for target triples, profiles, features, build cache, and produced artifacts, with reuse/invalidation behavior;
5. overlap with other local lanes and the exact GitHub Actions workflow/job/step/triggers/path-gating that provide merge protection;
6. one recommendation: retain-local, conditional, CI-only, or consolidate, with quantified benefit, delayed-feedback cost, and regression-protection tradeoff.

Clearly distinguish measurements, historical observations, estimates, and unknowns. Do not sum shared dependency trees, compiler caches, hard links, or APFS-cloned/apparent bytes as though they were unique. Record both apparent and allocated sizes where useful, but base incremental claims on controlled before/after deltas in task-owned paths or another explicitly justified ownership-safe method.

## Bounded measurement method

1. Re-verify hostname and worktree path before every instrumented run. Inventory existing profiles/logs, target directories, toolchains, compiler-cache state, filesystem semantics, and current processes first.
2. Preserve the naturally clean state of this isolated worktree for the first safe cold observation. Snapshot allocated disk by relevant owned subtree before/after ordered lane runs; do not delete or reset caches to repeat cold measurements. If shared cache attribution cannot be isolated, report it as unknown or estimated.
3. Use a bounded matrix: one naturally cold ordered pass, at least one warm full/profiled pass, and representative default/path-gated plans or runs for disclosed change cohorts. Reuse `--profile-work` artifacts and ordinary check output. Additional repeats are allowed only to resolve material noise or a failure; retain raw samples and report reruns/failures.
4. Derive rerun frequency from a fixed recent commit/PR/run window using the current path classifier and CI plan. State sample size, dates, exclusions, cancellations, and whether the result is simulated from changed paths or observed in Actions.
5. For musl, independently measure/inventory the installed Rust target/toolchain and musl-specific artifacts if the host supports the exact command safely. Establish whether default local check invokes it, what failures it uniquely catches (target/linkage/production feature configuration), when CI actually schedules it, and whether CI-only execution preserves required merge protection with acceptable delayed feedback. Do not assume musl is waste or benefit merely from historical timing.
6. If any command fails, preserve diagnostics and classify product regression, environment limitation, or bounded tooling bug. Do not weaken a gate, inflate a timeout, rerun until favorable, or turn a tooling finding into a second implementation task.

## Acceptance evidence

- Audit is reproducible from listed commands, git SHA, host/tool versions, raw profile paths, disk snapshots, and disclosed history/Actions queries.
- Per-lane totals reconcile to measured sequential command time within explained setup/reporting overhead; percentages use the matching run rather than mixed samples.
- Disk claims isolate incremental physical allocation as far as APFS and shared caches permit and explicitly identify non-attributable shared bytes.
- Musl conclusions answer default-local inclusion, unique cost, unique protection, CI scheduling/merge coverage, and delayed-feedback implications with evidence.
- Recommendations are audit-only; no check/test/workflow behavior changes.
- Run proportionate artifact validation (at minimum task validation plus link/command/table consistency checks), commit the task and audit artifact, and push the owned branch. No merge or deploy.

## Non-goals

No test deletion or skipping; no gate or workflow change; no generalized benchmark platform, scheduler, policy framework, cache purge, timeout change, source cleanup, merge, or deployment.
