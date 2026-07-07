# Fix CI gating and main-failure issue creation after PR #433

PR #433 (`eba3dae0 fix task naming conflict`) changed only one task filename:

- `tasks/59002-p1-done--deterministic-message-scroll-state-machine.md`
- renamed to `tasks/45002-p1-done--deterministic-message-scroll-state-machine.md`

That should have been a tiny validation run, but the current CI shape still starts the full four-job matrix and performs per-job setup before `./dev.py check` has a chance to skip lanes. This task fixes the three related problems surfaced by that PR.

## Findings from investigation

1. **PR #433 was task-only.** The merge commit has a single `R100` task-file rename and no Rust/UI/spec/e2e changes.
2. **`dev.py` path gating is too late for GitHub Actions cost.** `.github/workflows/ci.yml` always schedules all four matrix groups (`rust`, `clippy`, `e2e`, `ui`). `./dev.py check --lanes ...` can skip lanes inside those jobs, but the runners and setup steps have already started.
3. **The UI matrix group is forced for every PR.** `pkglock` is an always-on lane, and CI places it in the `ui` group. That means even a task-only change schedules the UI job and performs UI setup unless the workflow is made group-aware before setup.
4. **The desired “markdown format + task validate” path is not currently represented cleanly.** The current `fast` lane is `cargo fmt + task validation`, not markdown formatting. There is no obvious markdown-format lane in the current check implementation.
5. **Main failure issue creation needs direct verification.** The workflow has `notify-main-failure` and `close-main-failure` jobs, but they should be tested against real/main-run behavior and likely need hardened permissions/conditions plus regression coverage.
6. **Related drift noticed:** `dev.py` defines a `spec-shape` lane, but the CI `ui` lane list omits it (`tsc,ui-lint,vitest,ast-grep,allium,spec-anchors,pkglock`). That is adjacent to lane correctness and should be fixed or explicitly ruled out while touching this area.

## Plan

### 1. Reproduce and document the PR #433 lane decision

- Use `git diff --name-status HEAD^ HEAD` / equivalent fixture to preserve the fact that PR #433 was task-only.
- Add a regression test for changed paths containing only `tasks/*.md` / task filename renames.
- Assert the intended active lanes/groups explicitly so future workflow edits cannot reintroduce full-CI behavior for task-only changes.

### 2. Move gating before expensive GitHub Actions setup

- Add one lightweight CI planning job that checks out the repo and invokes a `dev.py` lane-decision/dry-run mode to compute the required lane groups for the PR.
- The planner should reuse the same lane registry/path-categorization logic as `./dev.py check`; do not duplicate lane decisions in workflow YAML.
- The planner must not run the actual check lanes. It should emit booleans or a JSON matrix for downstream jobs.
- Gate downstream jobs at the GitHub Actions job level, not only inside `./dev.py check`.
- Ensure a task-only PR does not start Rust, clippy, e2e, or UI toolchain setup jobs unless their lanes are actually required.
- Keep push-to-main behavior intentionally full-suite unless/until there is a separate, safe main policy.

### 3. Split or relocate always-on lanes so they do not force heavy groups

- Decide the minimal always-on set for task/markdown-only changes.
- Move `pkglock` out of the UI setup-heavy group, or make it conditional on lockfile/UI-relevant changes.
- Consider splitting `fast` so task validation and markdown formatting can run without requiring Rust/cargo setup, while `cargo fmt` remains Rust-related.
- Preserve correctness: no lane should silently skip when its input category changed.

### 4. Fix main CI failure issue creation

- Inspect recent main workflow runs with `gh` to determine why the issue was not opened/updated for the failing main run.
- Harden `notify-main-failure` conditions and permissions as needed.
- Add a small script/testable unit for the issue title lookup/body creation logic, or otherwise make the workflow behavior verifiable without waiting for a real main failure.
- Confirm `close-main-failure` still closes the deduplicated issue on the next green main run.

### 5. Fix lane inventory drift

- Reconcile `dev.py` lane definitions with `.github/workflows/ci.yml` lane groups.
- Include `spec-shape` in CI or deliberately classify it elsewhere with a test that compares the two lane inventories.
- Add regression coverage that every `dev.py` lane is either assigned to a CI group or intentionally local-only with an explicit allowlist.

## Acceptance criteria

- For a PR equivalent to #433, CI schedules only the lightweight planning job plus the minimal validation job(s); Rust/clippy/e2e/UI setup jobs are skipped at the Actions job level.
- `./dev.py check` still gates correctly locally and in CI, with tests covering task-only, spec-only, UI-only, Rust-only, e2e-only, and `dev.py`-changed cases.
- Main branch pushes still run the intended full validation suite.
- A failing main CI run opens or updates one deduplicated “CI failing on main” issue; the next green main run comments and closes it.
- CI lane groups and `dev.py` lane definitions cannot drift unnoticed.
