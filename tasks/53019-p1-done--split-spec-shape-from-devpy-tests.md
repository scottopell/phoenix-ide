# Split cheap spec-shape validation from dev.py tests

## Observed journey

A change confined to an otherwise irrelevant spEARS artifact currently selects `spec-shape`, whose implementation first validates artifact shape and then runs the entire `tests/devpy` orchestration/deployment/check suite. The cheap validator therefore inherits the suite’s wall cost even though most spec content is not a dependency of those tests.

Admission was checked against GitHub `main` (`87606f42404d8d169b85cea2f6de3e6732a3e58f`), roadmap #651, all open PRs, and local non-done tasks. No active owner covers this split. Closed audit PR #794 is preserved evidence only. Open Kache PR #796 owns cache/time/unique-disk work and overlaps `dev.py`; release PR #675 remains under its existing hold.

## Verified findings

- `_LANE_DEFS`, `_categorize_changed_paths`, `_resolve_check_lanes`, `cmd_check_plan`, `_CI_LANE_GROUPS`, `_GRAPH_LANE_STEPS`, `cmd_check` reporting/profiling, and `.github/workflows/ci.yml` jointly define selection and execution.
- `check_spec_shape` records the cheap validation and then invokes `python -m unittest discover tests/devpy`; both are represented as one `spec-shape` lane.
- `tests/devpy` depends on more than `dev.py`: it directly exercises deployment/profile/timing helpers under `scripts/`, integration fixtures under `tests/integration/`, CI and release workflows, review-skill content, and toolchain/config inputs. Broadly declaring spec changes irrelevant would be unsound without an actual dependency audit.
- Current planning already force-runs all lanes for `dev.py`/workflow changes, `--all`, local main, and unresolved local bases, while CI requires explicit full mode or a resolvable explicit base and fails closed otherwise.

## Proposed scope

Separate spEARS artifact-shape validation from a distinctly named dev.py-test lane without adding a selector framework or changing test contents. Keep lane definitions, descriptions/graph, reporter output, profiling attribution, check-plan payloads, aliases/explicit filters, and hosted CI lane inventory/mappings internally consistent.

Before narrowing selection, audit every `tests/devpy` import, repository read, fixture, subprocess target, and asserted workflow/config surface. Add small conservative static path rules so changes to `dev.py`, `tests/devpy/**`, relevant `scripts/**`, integration fixtures, workflow files, deployment/check/profiler inputs, and any other verified dependency select the dev.py-test coverage. An unrelated spec-only change must select shape validation but not the whole Python suite; spec data that a test truly consumes must still select that suite. Renames and deletions must be handled through changed-path planning, not existence assumptions.

Preserve all existing gates and tests, explicit `--lanes` behavior, `--all`, local-main/CI force-all behavior, and fail-closed unknown-base behavior. Ensure every new lane has an explicit hosted CI group and command mapping; do not silently omit it. Do not increase timeouts.

## Regression and evidence plan

- Add deterministic classifier, lane-plan, execution/reporting/profile, and workflow-inventory tests covering irrelevant spec-only negatives; every audited positive dependency class; renamed/deleted paths; explicit filters; full runs; local/main CI behavior; and unknown/unresolvable bases.
- Run focused `tests/devpy` coverage, then full `./dev.py check` on the exact implementation head; inspect the final diff.
- On `devmbp`, capture fresh bounded before/after normal (unprofiled) wall-time samples under the same recorded source/toolchain/cache and quiet-host conditions. Record every raw sample, exit status, and selected work; preserve failures and do not rerun until green, purge broad caches, serialize global builds, or report profiling-wrapper time as normal execution.
- Put durable dependency justification, regression coverage, and relevant raw timing summary in the implementation PR rather than reviving or copying the point-in-time audit.
- Commit and push one focused PR; await hosted CI; request fresh exact-head Codex review; explicitly paginate advancing review cursors until exhausted; fix and requalify all actionable findings.

## Coordination and non-goals

Start implementation from then-current `main` on `devmbp`, not this stale Explore checkout. Coordinate semantic `dev.py` overlap with Kache PR #796 owner `06a49632` and avoid its worktree/global builds; keep release #675 untouched. Do not reopen, merge, copy wholesale, delete source from, or mutate PR #794 or its preserved branch. Do not change compiler caching, timing/unique-disk policy, tmux/watchdog behavior, musl placement, gate policy, deployment/configuration, or any other #794 recommendation; those are owned by #796, merged #781/#777 and harness follow-up, existing CI, or explicit non-commissioning. No merge or deploy.
