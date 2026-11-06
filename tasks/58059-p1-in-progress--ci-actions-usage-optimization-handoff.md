# CI handoff: reduce GitHub Actions usage and make CI reliably green

**Owner:** new CI owner. **Handed off from:** the PR #1005 session (Codex reset credits), which hit most of these problems first-hand.

## Goal

Cut GitHub Actions wall time, queue time and wasted runs, and stop CI from going red for reasons unrelated to the change under test. Lead with the work that removes failures and wasted re-runs, then shorten the critical path.

Constraints:

- The repo's PR rules forbid skipping, disabling or quarantining a test to get green.
- `./dev.py check` lane semantics, including the CI gating rule that `PHOENIX_CHECK_ALL` or `PHOENIX_CHECK_BASE` must be explicit (see AGENTS.md "Development"), must stay intact.

## What "usage" means here

`scottopell/phoenix-ide` is **public**, so minutes on standard GitHub-hosted runners are not billed. The real costs are:

- **Feedback latency.** `check (rust)` is the critical path at about 45–67 min.
- **Concurrency limits.** On free plans macOS concurrency is small, so macOS jobs queue behind each other.
- **Re-runs and cancellations.** Each lost rust run throws away about 50 runner-minutes.
- **Cache and artifact storage.** Both have repo-wide caps, and evictions cause cache misses.

If the repo ever goes private, macOS minutes bill at 10× Linux. The macOS items below then become direct cost.

## Baseline (measured 2026-10-30 04:31 → 2026-11-01 01:41 UTC, about 45 h)

The data covers 300 workflow runs from the public API, plus job durations sampled from 14 CI runs and 4–5 runs of each other workflow. Treat the figures as indicative, not exact.

| Workflow | Runs | Notes |
| --- | --- | --- |
| Reduce roadmap Issue | 136 | 130 `issue_comment` runs, all skipped (job `if:`); 6 scheduled |
| CI | 66 | PR: 21 success, 24 **cancelled**, 9 failure; push: 3 success, 5 cancelled, 1 failure |
| Deployment controller macOS | 38 | Mostly PRs. It now runs whenever `Cargo.toml`/`Cargo.lock` change |
| Bash launch settlement | 37 | 30 success, 6 failure. A macOS and an Ubuntu job on every matching PR |
| macOS app | 23 | All success |

Per-job durations (median / max across the sample):

| Job | Runner | Median | Max | Outcomes in sample |
| --- | --- | --- | --- | --- |
| CI `check (rust)` | ubuntu-latest | 48.8 m | 67.0 m | 7 ok, 4 fail, 1 cancel |
| Bash launch faults (macOS) | macos-latest | 18.4 m | 35.3 m | 5 ok |
| Bash launch faults (Ubuntu) | ubuntu-latest | 10.5 m | 12.9 m | 5 ok |
| CI `check (assistant finalization geometry)` | ubuntu-latest | 8.1 m | 8.4 m | 13 ok |
| Deployment controller (macOS) | macos-14 → now macos-15 | 6.0 m | 6.2 m | 5 ok |
| macOS app build-and-test | macos-latest | 5.9 m | 7.3 m | 4 ok |
| CI `check (ui/specs)` | ubuntu-latest | 4.2 m | 7.2 m | 10 ok, 3 fail |
| CI `check (e2e)` | ubuntu-latest | 3.9 m | 4.3 m | 11 ok, 1 fail |
| CI `check (clippy)` | ubuntu-latest | 2.7 m | 3.1 m | 10 ok, 2 fail |

Inside `check (rust)` on a normal runner (run 41230192024):

| Step | Duration |
| --- | --- |
| Build native bash launcher fixture | ≈ 5–6 min |
| `cargo test compile` | ≈ 4.5–5.5 min |
| `codegen` | ≈ 2.5 min |
| `cargo test` | ≈ 30 min (1802 s) |

`cargo test` runs nextest capped at **3 threads** on a 4-CPU runner (`cpus=4, mem_cap=10`).

## Progress (branch `ci/actions-usage-optimization`)

Findings from job logs (the agent proxy blocks the caches API, so `gh cache list` is still unverified):

- **P1-2 root cause (likely):** `ci-${{ github.ref }}` with `cancel-in-progress: true` also applied to `main`. The next merge cancelled the 50-min rust job before its post step, so the `check-rust` rust-cache was rarely saved (main push runs: 3 success, 5 cancelled, 1 failure in the window). The clippy job finishes in ~3 min and *does* get a full hit (`v1-rust-check-clippy-…`, ~405 MB, run 41260754169).
- **sccache "0 compile requests"** is a stats artifact: the server idles out (default 600 s) during the 30-min test step, and `--show-stats` then starts a fresh server. It does not show that sccache was unused.
- Run 41260754169 shows P2-1 directly: `ui/specs` failed after 9 s, while `rust` ran another 35 min and failed too.
- The `bash-launch.yml` Ubuntu job overlaps `check (rust)`, which builds the same native launcher and sets `PHOENIX_BASH_LAUNCH_TEST_REQUIRED=1`. That makes it a candidate for removal (product-owner call).

Done:

- [x] P1-1 stopgap: cargo-test budget 4800 s; step/job timeouts recomputed.
- [x] P1-2: main CI runs are no longer cancelled; `cache-on-failure` for the rust lane on main; `SCCACHE_IDLE_TIMEOUT=0` so stats become meaningful; macOS app saves rust-cache only on main.
- [x] Native fixture builds with default jobs instead of `-j 1`.
- [x] Bash launch settlement cancels superseded PR runs.
- [x] P3-2: the shallow-clone test fetches pinned commits on demand.
- [x] P3-3: Node 20 actions bumped (tag-referenced ones only; SHA pins untouched).

Next, once a few main runs have saved caches: compare PR rust-job `Cache Cargo` hits and sccache hit rates. Then decide P1-1 sharding, P2-1 gating and P2-2 macOS scope (open questions below).

## Prioritised issues

### P1-1: `check (rust)` times out on slow runners

**Evidence**

- `cargo test` has a 3000 s budget (`PHOENIX_CHECK_CARGO_TEST_TIMEOUT_SECS: "3000"`, `ci.yml` around line 82; job `timeout-minutes: 133`).
- On PR #1005 the lane went 47 min pass, 57 min **timeout**, 55 min pass, 66 min **timeout**, then 67 min pass on re-run. The code was the same or near-identical each time.
- In the timeouts, no test failed an assertion. The tests reported as failed exited with code 143 (SIGTERM at the budget), and 108–254 tests never ran.
- Per-test `rust-evidence-*` artifacts compare run 41248937970 (slow) with 41230192024 (normal) across 5775 shared tests:
  - Summed **CPU** time was 1.93× higher, summed wall time 1.82×, median per-test ratio 1.89×.
  - The runner image (`ubuntu-26.04`), CPU count and thread cap were identical.
  - The slowdown hit every test uniformly, including `phoenix-db` tests the PR never touched. The likely cause is slower host CPUs in the GitHub fleet.
- `main` at `98ef62e` took about 46 min (pass), so a slow runner turns any PR red.

**Options** (combine them)

1. **Quick stopgap:** raise the budget to about 4800 s, inside the existing 133-min job timeout. This stops false reds now, but slow runs still take about 70 min.
2. **Shard the lane:** run 2–3 matrix jobs with `cargo nextest run --partition hash:N/M` (or `count:`). The compile happens in each shard unless a shared build artifact is reused (nextest `archive` / `--archive-file`). This shortens the critical path and halves exposure to a slow host.
3. **Make the slowest tests cheaper.** Top CPU consumers, as normal → slow CPU seconds:
   - `phoenix-db lineage::tests::fresh_and_each_real_canonical_prefix_are_admitted` (38 → 71 s)
   - `conversation_goals::writer_tests::checkpoint::finalized::unprepared_report_guard_uses_outcome_not_output_across_db_entrypoints` (43 → 76 s)
   - `lineage::tests::goal_tools_tests::canonical_goal_tools_prefixes_upgrade_reopen_and_remain_idempotent` (29 → 51 s)
   - `svg_artifacts::tests::restart_recovers_only_committed_svg_invocation_after_staging_is_deleted` (18 → 37 s)

   Many of these replay the full migration chain. A cached, pre-migrated SQLite template database copied per test would likely cut this sharply. Verify before relying on it.
4. **Revisit the 3-thread cap** (`dev.py` memory/CPU heuristic) on 4-CPU runners. Measure peak RSS first.

**Done when:** the rust lane has not failed by timeout across 20+ consecutive runs, and its median is at or below the current one.

### P1-2: compiler and dependency caching looks ineffective on PRs (verify first)

**Evidence**

- `Swatinem/rust-cache@v2` (`shared-key: check-rust`, `save-if: main`, `prefix-key: v1-rust`) logged **"No cache found."** on all three sampled PR rust jobs.
- sccache (`mozilla-actions/sccache-action`, `SCCACHE_GHA_ENABLED`) post-job stats showed **1601 compile requests** on one run but **0** on two others (runs 41234330265 attempt 2 and 41248937970), with `RUSTC_WRAPPER=sccache` set. `dev.py` logs "Compiler cache: explicit" when `RUSTC_WRAPPER` is preset (`dev.py`, around line 5128).

Possible causes: repo cache eviction from the 10 GB cap (sccache GHA entries plus rust-cache), a key mismatch because PRs change `Cargo.lock`, or the stats being read from a different sccache server instance. **Unverified.**

**Do**

1. List caches (`gh cache list` or the API): sizes, keys and last-used dates.
2. Confirm whether `main` saves are evicted before PRs read them.
3. Check whether `dev.py` subprocesses actually use the preset wrapper.
4. Decide on one caching layer. sccache-GHA and target-dir caching compete for the same 10 GB.
5. Also covers: the deployment controller moved `macos-14` → `macos-15` in PR #1005 without bumping its rust-cache prefix. That is low risk, but it is the same cache-hygiene work.

**Payoff:** if caching works, it should save roughly 5–10 min per rust job (compile plus bash fixture build) and similar on the macOS jobs.

### P2-1: 36% of PR CI runs are cancelled

24 of 66 PR CI runs in the window were cancelled by `concurrency: cancel-in-progress` when a newer push landed. Cancellation is correct, but every cancelled rust lane wasted up to 50 min of a runner and of queue position.

Options:

- Cheaper early lanes, so pushes that will be superseded spend less.
- Encourage agents to batch pushes. This is a workflow norm, not CI config.
- Run the heavy rust lane only after the fast lanes (clippy/fast) pass on the same SHA, using `needs:`. Then a broken push never starts the 50-min lane.

### P2-2: macOS workflows trigger too broadly

- **Bash launch settlement.** It runs macOS (18–35 min) and Ubuntu jobs whenever its paths change. Those paths include hot files: `crates/phoenix-ide/src/runtime.rs`, `crates/phoenix-ide/src/main.rs`, `crates/phoenix-tools/src/bash.rs`, `tool_wire.rs` and `process_identity.rs`. It ran 37 times in 45 h, almost one per CI run.
- **Deployment controller macOS.** It triggers on any `Cargo.toml`/`Cargo.lock` change, so every dependency bump gets a macOS run.

Options:

- Narrow the `paths` filters to the code these jobs actually exercise.
- Gate macOS on a label, or run it on `main` pushes and on demand.
- Run macOS only after Linux passes.

Confirm with the product owner which macOS checks must block PR merge.

### P2-3: unexplained red lanes on `main` and PRs

- `main` at `98ef62e` failed `check (ui/specs)` (run 41195253665, job 124520083848). The cause was not investigated. `ui/specs` failed in 3 of 13 sampled runs, `clippy` in 2 and `e2e` in 1.
- **Do:** classify every non-rust failure in the last 2 weeks as this-PR's, a flake (name the test) or infra. File flakes as tasks with the failing test names.
- **Locally known flakes** from the PR #1005 session: under full-suite load, `ui/src/fixtures/productConversation/renderFixture.test.tsx` ("appends two touch-docked source-bound reactions…") timed out at 5 s, and `ui/src/pages/ProductConversationPage.test.tsx` failed a `findByRole('alert')` wait. Both pass when run alone.

### P3-1: Vitest log noise

The full UI suite prints about 580 `ECONNREFUSED 127.0.0.1:3000` lines from tests that make real, unmocked `fetch` calls (present on `main` before PR #1005). This hides real failures in CI logs.

**Fix:** a test-setup `fetch` guard that fails loudly, or default mocks for `api.*`.

### P3-2: local `./dev.py check` breaks in shallow clones

`tests/devpy/test_launchd_deploy.py::LaunchdControllerDurabilityTests::test_prepared114_receipt_candidate_gets_typed_evidence_from_exact_git_object` runs `git show 23badb8…:crates/phoenix-db/src/migrations.rs`. In `--depth` clones, such as the cloud agent sandboxes, that commit is missing, so the dev.py unit-test lane always fails there.

**Fix:** fetch the commit on demand, or skip with an explicit "object unavailable" reason. Don't fake a pass.

### P3-3: deprecated action runtimes

Run logs warn that `actions/checkout@v4`, `actions/upload-artifact@v4`, `astral-sh/setup-uv@v5` and `pnpm/action-setup@v4` target Node 20 and are being forced to run on Node 24. Bump them before GitHub removes the shim.

### P3-4: roadmap reducer noise

`roadmap-issue-reducer.yml` creates a skipped run for every comment on any issue: 130 of 136 runs in the window. That costs almost nothing, but it clutters the Actions UI. `issue_comment` can't be filtered by issue number at the trigger level, so leave it unless the noise matters.

## Tools and how to reproduce the measurements

- **Run list:** `GET /repos/scottopell/phoenix-ide/actions/runs?per_page=100&created=>=DATE`.
- **Jobs per run:** `GET /actions/runs/{id}/jobs`.
- **Per-test timing:** each CI rust job uploads `rust-evidence-<run>-<attempt>`. `check/rust-tests/process-*.json` holds `identity`, `wall_ms`, `total_cpu_ms`, `user_cpu_ms` and `returncode` per test. Comparing two runs by `identity` is how the CPU-slowdown evidence in P1-1 was produced.
- **Lane plan:** the `plan check lanes` job decides which lanes run (`dev.py` `_LANE_DEFS`, around line 4100). Spec-only PRs already skip heavy lanes; see the roadmap item "Spec-only changes skip dev.py tests".

## Open questions for the product owner

1. Which macOS workflows must block merge, and which can be post-merge or on demand? (Affects P2-2.)
2. Is any part of CI required to run on every PR push, or can heavy lanes wait for the fast lanes and the latest push? (Affects P2-1.)
3. Target for PR feedback latency (for example, rust lane ≤ 25 min)? This decides how far to take sharding.

## Local fixture work reduction (2026-11-02)

The exact-schema Python recovery classes use a deterministic shared capacity pool while dedicated APFS/shared-pool/independent-filesystem tests retain their topology assertions. Their executable Python fixtures use the active managed interpreter. A plain fixture mixin removes four accidentally inherited candidate-gate tests while retaining the original four and all seven distinct Linux cases.

The canonical-prefix admission test initializes prefix129 once and advances through every registered prefix, preserving each schema and non-mutation assertion. The Goal-report guard matrix initializes one database for all 20 entrypoint/outcome combinations, retains both attempts and global evidence checks, gives each case distinct durable identities, and verifies that later cases leave earlier scoped evidence unchanged.

Representative local samples on the same Mac (single before/after runs with some shared-host activity, not a full-suite estimate):

| Scope | Before | After |
| --- | ---: | ---: |
| Same 45 exact-schema Python tests | 145.265 s | 8.408 s |
| Canonical-prefix admission | 16.79 s | 1.99 s |
| Goal-report guard matrix | 19.38 s | 1.59 s |
| Candidate/Linux gate classes | 15 cases / 14.696 s | 11 cases / 10.678 s |

All 53 lineage/checkpoint tests and all 271 tests in the two affected Python modules pass. Focused fixture-cleanup/topology coverage also passes. These changes avoid a cross-process database-template cache; genuine migration, reopen, and topology coverage remains in its owning tests. Local logs are retained under `target/fixture-work-20261102/` in the Codex checkout. The broader CI handoff remains in progress.

## Bundled SQLite dev/test optimization (2026-11-02)

The SQLx package override optimizes the Rust facade; the bundled SQLite C engine was compiled without optimization. Setting `profile.dev.package.libsqlite3-sys.opt-level = 2` optimizes that dependency for dev and inherited test builds. Release profiles retain their existing settings. Test cases, SQLite features, schema, and runtime validation are unchanged.

Three paired local samples at source `b32b50e13240407ecdb491c562061c901f528e04`:

| Scope | Median test-body time before / after | Median CPU before / after |
| --- | ---: | ---: |
| Fresh DB/settings test | 1.03 / 0.51 s | 1.02 / 0.50 s |
| Eight-prefix upgrade/reopen test | 13.62 / 6.78 s | 13.77 / 7.01 s |

Runtime durations exclude process startup, as reported by Rust's test harness; CPU measurements include the full test process.

Three alternating fresh replays of the real bundled-library build script measured the C-stage tradeoff: median wall time 2.74 s at `-O0`, 14.10 s at `-O2`; user CPU 1.31 / 12.95 s. Replay objects byte-match Cargo's actual artifacts, and compiler commands differ only in optimization level. The archive grows by 3.84 MiB, primarily DWARF, while code size decreases 6.9%. Observed peak compiler RSS grows from about 294 to 918 MB. These are C-stage measurements with warm OS caches, excluding Rust rebuilding and linking; the affected-test samples are not a full-check speedup estimate.

Raw runtime samples remain under `target/post-1018-sqlite-profile-probe/`; cold-build parity, commands, and samples are under `target/sqlite-optimization-20261102/cold-c/` in the Codex checkout.
