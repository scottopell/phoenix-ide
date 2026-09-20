# `./dev.py check` regression-ROI audit

**Source audited:** `origin/main` `bed747b5bd231f495b8038163d63c72a9ebeb346` (the task commit was rebased above it as `da0fcf2d`). **Host:** `devmbp`, MacBookPro18,4, macOS 26.4.1, 10 logical CPUs, 64 GiB RAM, APFS. **Measured toolchain:** stable-aarch64 Rust with rustc/cargo 1.95.0, cargo-nextest 0.9.143, Node 26.7.0, pnpm 11.0.8, uv-selected Python 3.14.7 (the `dev.py`/spec-shape runner), system Python 3.9.6 (not the runner), ast-grep 0.45.1, and Allium 3.5.3. Both optional CLIs were present for the measured runs; neither was absent. **Captured:** 2026-09-19. No cache was purged. No lane, test, gate, timeout, or workflow was changed.

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
| `tsc` | `pnpm run typecheck` (`tsc -b --noEmit`); project-reference and `exactOptionalPropertyTypes` errors. | 0.5 s warm; profile 15.1 s, **1.4%**. Cold: unknown. | 27/50 (54%) | `--noEmit` emits no JS build tree, but composite mode persists `ui/node_modules/.tmp/{tsconfig.app,tsconfig.node}.tsbuildinfo` (422,879 + 51,919 bytes apparent in the measured state). These caches share `node_modules` and are invalidated/recomputed when TypeScript's tracked project inputs/options or the cache files change. | CI `check (ui/specs)`; ESLint/Vitest do not prove TS project-reference correctness. | **Conditional (retain).** The sub-second result is cache-assisted; protection remains unique. |
| `ui-lint` | ESLint then Stylelint; TS/React hook/static rules and CSS validity/conventions. | 8.4 s (6.7 + 1.7); profile 12.8 s, **1.2%**. Cold: unknown. | 27/50 (54%) | No material lane artifact; shares `ui/node_modules`. | CI `check (ui/specs)`; partially overlaps tsc syntax, but lint/CSS rule classes are distinct. | **Conditional (retain).** ~8 s for two unique static surfaces. Keep consolidated as one lane. |
| `vitest` | UI component/state/hook tests. | 39.5 s; profile 47.7 s, **4.4%**. Cold: unknown. | 27/50 (54%) | Transient workers only; shares `ui/node_modules`. | CI `check (ui/specs)`; overlaps code touched by tsc/lint, not asserted behavior. | **Conditional (retain).** CI-only saves ~40 s per UI change but loses the only broad UI behavioral feedback locally. |
| `ast-grep` | One structural-rule scan over Rust/UI, then a static changed-test synchronization-smell lint (`check_ast_grep`). The latter rejects newly introduced sleeps and unbounded event waits; it does not execute or time tests and therefore does not detect arbitrary slow tests. | 4.0 s (0.9 + 3.1); profile lane 5.2 s, **0.5%**. Cold: near cache-independent, not separately measured. | 42/50 (84%) | Negligible persistent output. | Local protection is conditional on the optional `ast-grep` CLI: when absent, `check_ast_grep` records a successful skip before both the structural scan and timing-smell lint. CI `check (ui/specs)` installs the CLI. | **Conditional (retain).** With the optional CLI installed, repository-specific structural and synchronization-smell protection costs ~4 s. Keep the two steps consolidated. |
| `allium` | Parse/analyse every Allium spec and reject new findings beyond the checked baseline. | 0.1 s; profile 0.2 s, **<0.1%**. Cold: tool install excluded. | 28/50 (56%) | None in worktree. | Local protection is conditional on the optional `allium` CLI; absence is a successful skipped lane. CI `check (ui/specs)` installs pinned `allium-cli` 3.5.0, where no other lane validates this grammar/finding baseline. | **Conditional (retain).** With the optional CLI installed, unique protection costs ~0.1 s. |
| `spec-shape` | Validate spEARS v2 artifact shape, then run all `tests/devpy` orchestration/deployment/check tests. | 34.7 s ordinary; profile 149.5 s, **13.9%**. Cold: unknown. | 28/50 (56%) | Python/uv caches are shared host state and not attributable; no material worktree output. | CI `check (ui/specs)`; shape validation is unique, while dev.py tests protect the check/deploy/task machinery itself. | **Conditional (retain).** The 35 s cost is dominated by distinct dev.py regression tests. A future split could improve attribution, but consolidation/removal has no proven protection-preserving saving. |
| `spec-anchors` | Cross-check code `REQ-*` references against declarations. | 0.6 s; profile 0.6 s, **0.1%**. Cold: cache-independent. | 46/50 (92%) | None. | CI `check (ui/specs)`; intentionally spans specs, Rust, and UI; shape validation cannot catch orphan code anchors. | **Conditional (retain).** Very frequent but sub-second and unique. |
| `e2e` | Build/run a real Phoenix binary and drive HTTP/SSE scenarios with isolated DB/mock model. | 48.8 s; profile 53.4 s, **5.0%**. Cold: unknown. | 32/50 (64%) | Reuses normal `target/debug` dependencies but must link non-test binary; incremental bytes cannot be separated safely from Rust lane in the shared tree. | Dedicated CI `check (e2e)`; overlaps user journeys but uniquely crosses binary/API/SSE/process boundaries. | **Conditional (retain).** CI-only saves ~49 s on Rust/E2E changes but removes the only local real-binary boundary gate. |
| `task` | Validate task filename grammar and global ID uniqueness. Always on. | 0.0 s (12 ms profile); **<0.1%**. | 50/50 (100%) | None. | CI `check (task validation)` plus roadmap reducer test. No substitute. | **Retain local always-on.** Negligible cost; it caught the intentionally plain approval brief until that proposal artifact was removed after approval. |
| `pkglock` | Run `git status --porcelain -- ui/pnpm-lock.yaml` and fail if that one file is dirty. This is only an uncommitted-lockfile tripwire: it does not validate package/lock consistency or predict whether a clean frozen install succeeds. | <0.1 s; profile 0.1 s, **<0.1%**. Cold: cache-independent. | 27/50 (54%) | None. | CI `check (ui/specs)` runs this same dirty-file predicate after checkout; its separate `pnpm install --frozen-lockfile` step provides package/lock consistency protection. | **Conditional (retain).** Near-zero cost for the exact dirty-file predicate. |

### Shared UI dependency footprint

The naturally empty stale worktree installed `ui/node_modules` once: **528,144 KiB allocated (516 MiB)** versus 434,740 KiB apparent. This is shared by `tsc`, `ui-lint`, and `vitest`; it must not be charged three times. The total includes TSC's persistent composite caches at `ui/node_modules/.tmp/`: `tsconfig.app.tsbuildinfo` (422,879 bytes apparent) and `tsconfig.node.tsbuildinfo` (51,919 bytes apparent). Lockfile changes invalidate package selection; TypeScript recomputes build-info when its tracked project inputs/options or cache files change, while unchanged inputs reuse it.

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

**Measured devmbp capability/cost:** the repository-declared Rust std target was already installed and occupies **222,708 KiB allocated (217.5 MiB)**. It is part of the audited host/toolchain baseline, not incremental local-lane cost. Neither `musl-gcc` nor `x86_64-linux-musl-gcc` was installed. One exact-command attempt failed honestly after **10.33 s** at `aws-lc-sys` because `x86_64-linux-musl-gcc` was absent. The failed attempt allocated **51,260 KiB (50.1 MiB)** under `target/x86_64-unknown-linux-musl` plus 107,208 KiB in shared `target/debug`; this is a lower bound, not successful-musl footprint. Incremental local cost is therefore a cross compiler plus isolated build artifacts (**>50.1 MiB observed**); successful current-source wall time and successful artifact cost are **unknown on devmbp**. Installing a cross compiler solely to complete this audit was not necessary and would have changed host-global state.

**Recommendation: CI-only (retain existing placement).** Removing it from local `check` saves **0 s** because it is already absent. Adding it locally would require a cross compiler and >50.1 MiB observed additional build artifacts; the pre-existing 217.5 MiB Rust target is not an incremental charge. CI provides relevant Linux tooling and observed pre-merge execution; the tradeoff is delayed feedback after Rust tests. Keep release builds as the stronger final-link protection. Do not claim the historical 8.6 s figure as current.

## CI mapping and recommendation summary

Exact workflow mapping at `bed747b5`:

- `check (rust)`: `rust,cargo-fmt`, then musl production-feature check.
- `check (clippy)`: `clippy`.
- `check (e2e)`: `e2e`.
- `check (ui/specs)`: `tsc,ui-lint,vitest,ast-grep,allium,spec-shape,spec-anchors,pkglock`.
- `check (task validation)`: `task` (and a separate roadmap reducer test).

All jobs are merge-time PR checks subject to the current path plan; all groups run on pushes to `main`. The current local design already captures the measurable ROI: an unchanged/docs-only branch paid 0.47 s instead of roughly 491 s, while relevant edits select their distinct gates. Recommended changes to gates: **none**. Potential follow-up investigations—not changes commissioned here—are load-sensitive tmux cleanup tests and whether `spec-shape` should be renamed/split solely for clearer ownership; neither has evidence supporting removal or CI-only conversion.

## Reproduction procedure

The raw files cited above are intentionally ignored, so the exact scripts used to produce their durable aggregate claims follow. Every instrumented command was preceded by:

```bash
set -euo pipefail
[ "$(hostname -s)" = devmbp ] || exit 1
case "$(git rev-parse --show-toplevel)" in
  /Users/sopell/git/phoenix-ide/.phoenix/worktrees/*) ;;
  *) exit 2 ;;
esac
mkdir -p target/check-roi-audit
```

### Plans, timed checks, and logs

`/usr/bin/time -lp` writes process wall/resource data into each named log through `tee`; `--profile-work` writes command/lane/step JSON under the named profile directory.

```bash
./dev.py check-plan --all --format json \
  > target/check-roi-audit/current-full-plan.json
./dev.py check-plan --format json \
  > target/check-roi-audit/current-default-plan.json
/usr/bin/time -lp ./dev.py check --all --profile-work \
  --profile-work-dir target/check-profile/roi-current-warm-1 2>&1 \
  | tee target/check-roi-audit/current-warm-1.log
/usr/bin/time -lp ./dev.py check --profile-work \
  --profile-work-dir target/check-profile/roi-current-default 2>&1 \
  | tee target/check-roi-audit/current-default.log
/usr/bin/time -lp ./dev.py check --all 2>&1 \
  | tee target/check-roi-audit/current-warm-normal.log
/usr/bin/time -lp env CARGO_INCREMENTAL=0 cargo check \
  --target x86_64-unknown-linux-musl \
  --features phoenix_ide/datadog-tracing 2>&1 \
  | tee target/check-roi-audit/musl-check.log
```

### Before/after disk snapshots

This exact function was called with distinct output files immediately before and after each owned run (for example, `disk-before-musl.tsv` and `disk-after-musl.tsv`). `du -sk` reports allocated KiB on this APFS host and `du -skA` reports apparent KiB. Deltas are compared by path; shared trees are reported once, not summed across lanes.

```bash
snapshot_disk() {
  out=$1
  {
    printf 'captured_utc=%s\n' "$(date -u +%FT%TZ)"
    printf 'source=%s\n' "$(git rev-parse HEAD)"
    for p in target target/debug target/clippy \
      target/x86_64-unknown-linux-musl ui/node_modules \
      "$HOME/.rustup/toolchains"; do
      if [ -e "$p" ]; then
        printf '%s\t' "$p"
        du -sk "$p" | awk '{printf "allocated_KiB=%s\\t",$1}'
        du -skA "$p" | awk '{printf "apparent_KiB=%s\\n",$1}'
      else
        printf '%s\tabsent\n' "$p"
      fi
    done
  } > "$out"
}
```

### Current-classifier simulation over 50 first-parent commits

This is the exact standalone simulation used for the table. It transcribes `_categorize_changed_paths`, `_LANE_INPUTS`, the `SELF` rule, and always-on `task` from audited `dev.py`; it evaluates each commit diff rather than PR aggregates.

```bash
python3 - <<'PY' > target/check-roi-audit/commit-frequency.json
import json, subprocess
lanes = {
  'rust': {'RUST'}, 'cargo-fmt': {'RUST'}, 'clippy': {'RUST'},
  'tsc': {'UI'}, 'ui-lint': {'UI'}, 'vitest': {'UI'},
  'ast-grep': {'UI', 'RUST', 'ASTGREP'}, 'allium': {'SPECS'},
  'spec-shape': {'SPECS'}, 'spec-anchors': {'SPECS', 'RUST', 'UI'},
  'e2e': {'RUST', 'E2E'}, 'task': None, 'pkglock': {'UI'},
}
def categories(paths):
  found = set()
  for p in paths:
    if p == 'dev.py' or p.startswith('.github/workflows/'): found.add('SELF')
    if (p.startswith('crates/') or p in ('Cargo.toml', 'Cargo.lock') or
        p.startswith('.cargo/') or p.startswith('rust-toolchain')): found.add('RUST')
    if p.startswith('ui/src/generated/'): found.add('RUST')
    if p.startswith('ui/') and not p.startswith('ui/dist/'): found.add('UI')
    if p.startswith('tasks/'): found.add('TASKS')
    if p.startswith('specs/') or p.startswith('tests/devpy/'): found.add('SPECS')
    if p == 'scripts/check_rust_test_timing.py': found.update(('ASTGREP', 'SPECS'))
    if p in {'scripts/check_profile_command.py', 'scripts/check_profile_report.py',
             'scripts/python_unittest_profile.py'}: found.add('SPECS')
    if p.startswith('ast-grep-rules/'): found.add('ASTGREP')
    if p.startswith('tests/e2e/') or p == 'phoenix-client.py': found.add('E2E')
  return found
shas = subprocess.check_output(
  ['git', 'rev-list', '--first-parent', '--max-count=50',
   'bed747b5bd231f495b8038163d63c72a9ebeb346'], text=True
).split()
counts = {lane: 0 for lane in lanes}; rows = []
for sha in shas:
  paths = subprocess.check_output(
    ['git', 'diff-tree', '--no-commit-id', '--name-only', '-r', sha], text=True
  ).splitlines()
  cats = categories(paths)
  active = (set(lanes) if 'SELF' in cats else
            {'task'} | {lane for lane, inputs in lanes.items() if inputs and inputs & cats})
  for lane in active: counts[lane] += 1
  rows.append({'sha': sha, 'paths': len(paths), 'categories': sorted(cats),
               'active_lanes': sorted(active)})
print(json.dumps({'method': 'current classifier per first-parent commit',
                  'sample_size': len(shas), 'tip': shas[0], 'oldest': shas[-1],
                  'counts': counts, 'rows': rows}, indent=2))
PY
```

### Latest 30 completed PR Actions runs

The sampled window is pinned to these exact 30 run IDs, ordered newest to oldest, from **2026-09-19T20:58:51Z through 2026-09-19T23:22:49Z**. A lightweight verification of representative run `35475964002` with the published REST mapping returned non-null `headSha` `8164d567798e43ded907c6dbbcbf739976faf8a6`. The query requests 100 jobs for each run; every sampled run had fewer than 100 jobs, so no job page was omitted. The first script persists immutable run metadata and each run's job names/outcomes. The second extracts the two named Rust steps. A `skipped` job is not counted as scheduled lane execution.

```bash
run_ids=(
  35475964002 35475484251 35475440124 35474960980 35474944626
  35474694703 35474651320 35474599742 35474532227 35474291809
  35474219249 35473989772 35473502338 35473451771 35472910524
  35472530750 35472389248 35471754617 35471686356 35471485123
  35471393512 35471178535 35470682089 35470276834 35470114295
  35470050709 35469704604 35469363717 35469198459 35469019544
)
printf '[]\n' > target/check-roi-audit/gh-pr-runs.json
for run_id in "${run_ids[@]}"; do
  gh api "repos/scottopell/phoenix-ide/actions/runs/$run_id" \
    --jq '{databaseId:.id,headSha:.head_sha,createdAt:.created_at,conclusion,url:.html_url}' \
    > target/check-roi-audit/run.json
  jq -s '.[0] + [.[1]]' target/check-roi-audit/gh-pr-runs.json \
    target/check-roi-audit/run.json > target/check-roi-audit/runs-next.json
  mv target/check-roi-audit/runs-next.json target/check-roi-audit/gh-pr-runs.json
done
python3 - <<'PY'
import json, subprocess
runs = json.load(open('target/check-roi-audit/gh-pr-runs.json'))
out = []
for run in runs:
  response = subprocess.run([
    'gh', 'api',
    f"repos/scottopell/phoenix-ide/actions/runs/{run['databaseId']}/jobs?per_page=100"
  ], capture_output=True, text=True)
  if response.returncode:
    run['jobs_error'] = response.stderr.strip()
  else:
    run['jobs'] = [{'name': job['name'], 'conclusion': job['conclusion']}
                   for job in json.loads(response.stdout)['jobs']]
  out.append(run)
open('target/check-roi-audit/gh-pr-jobs.json', 'w').write(
  json.dumps(out, indent=2) + '\n')
PY
python3 - <<'PY'
import json, subprocess
runs = json.load(open('target/check-roi-audit/gh-pr-runs.json'))
rows = []
for run in runs:
  raw = subprocess.check_output([
    'gh', 'api',
    f"repos/scottopell/phoenix-ide/actions/runs/{run['databaseId']}/jobs?per_page=100"
  ], text=True)
  for job in json.loads(raw)['jobs']:
    if job['name'] == 'check (rust)':
      rows.append({
        'run': run['databaseId'], 'createdAt': run['createdAt'],
        'job_conclusion': job['conclusion'], 'url': job['html_url'],
        'steps': [{'name': step['name'], 'status': step['status'],
                   'conclusion': step['conclusion']} for step in job['steps']
                  if step['name'] in ('./dev.py check (rust,cargo-fmt)',
                                      'Check production feature on musl')]
      })
open('target/check-roi-audit/gh-rust-musl-steps.json', 'w').write(
  json.dumps(rows, indent=2) + '\n')
PY
python3 - <<'PY'
import collections, json
runs = json.load(open('target/check-roi-audit/gh-pr-jobs.json'))
counts = collections.Counter(); outcomes = collections.Counter()
for run in runs:
  for job in run.get('jobs', []):
    if job['name'].startswith('check ('):
      counts[job['name']] += 1
      outcomes[(job['name'], job['conclusion'])] += 1
print(dict(counts)); print(dict(outcomes))
rows = json.load(open('target/check-roi-audit/gh-rust-musl-steps.json'))
print(collections.Counter(row['job_conclusion'] for row in rows))
print(collections.Counter(
  (step['status'], step['conclusion']) for row in rows for step in row['steps']
  if step['name'] == 'Check production feature on musl'))
PY
```
