---
name: phoenix-perf-find-target
description: Finds one valid React performance target in the Phoenix UI. Profiles scenarios via the run-scenario harness (default transport: browser_profile), scans for known React anti-patterns, learns from db.yaml history, ranks, and returns a filled target.yaml. Use before /phoenix-perf-hunt.
allowed-tools: Bash Read Glob Grep
context: fork
---

A component is **eligible** if a reproducible scenario exercises it. Scenarios
are the React analog of lading's Criterion benches + fingerprints, in
`skills/phoenix-perf-find-target/resources/scenarios/*.yaml`.

## Phase 1 — Enumerate scenarios

Glob `resources/scenarios/*.yaml`. Each is a declarative step list plus the
components it exercises (`covers:`). The `(scenario, covered_files)` set is the
eligible surface.

## Phase 2 — Profile scenario cost

For each scenario, run it through the harness with small N to see where the
cost is (not where you guessed):

```bash
skills/phoenix-perf-shared/scripts/run-scenario \
  skills/phoenix-perf-find-target/resources/scenarios/<name>.yaml \
  --runs 5 --warmup 1 --subst SLUG=<slug> --subst UI_URL=<url> \
  --out /tmp/pp-profile-<name>.json
```

Reduce each with `phoenix-perf-review/scripts/stats.py` against itself is not
meaningful; instead read the raw file and take the median `react_commits` /
`react_actual_ms` (browser_profile transport) or `script_ms` /
`react_commit_count` (agent-browser legacy transport) and `long_tasks` per
scenario. The harness returns **raw samples** — never a pre-average.

If a scenario cannot run (server down, selector drift), fix it or mark its
targets `profiled: false`; never invent numbers.

## Phase 3 — Learn from history

`Read skills/phoenix-perf-hunt/resources/db.yaml`; for each entry read its
detail file. Extract technique %-improvements, lessons, covered targets (skip
them). Past successful techniques are strong signals.

## Phase 4 — Find opportunities

Scan **every** covered component for **every** pattern in
`resources/patterns.md`. Exhaustive cross-product. Show the full
`component × pattern → hit count` matrix (0 = no hit). Each verified hot-path
hit is an opportunity `(pattern, technique, target, file, scenario, cost)`.

"Hot path" = per-token / per-keystroke / per-frame, OR scales with
conversation size. Off-hot-path hit = noise, drop it.

## Phase 5 — Filter

Drop if: (1) same component + equivalent technique already in `db.yaml` (any
verdict — a prior REJECT counts; no blind retry); (2) no scenario covers it.
Zero survive → STOP "No valid React perf targets found."

## Phase 6 — Rank

1. Technique impact — measured history in `db.yaml` ranks higher.
2. Profiled cost — higher `script_ms`/commit count ranks higher; if
   unprofiled, static severity: per-frame/per-token > per-keystroke >
   scales-with-size > other.

Tiebreaker: no prior db entry, then alphabetical file. Show the sorted table.

## Phase 7 — Return result

Top opportunity only. ONE fenced YAML block. Do NOT write disk, do NOT include
the matrix.

```yaml
pattern: "<the code pattern found, concretely with file:line>"
technique: "<technique from patterns.md>"
target: "<Component or Component::function>"
file: "<ui/src relative path>"
scenario: "resources/scenarios/<name>.yaml"
profiled: <true|false>
profiled_cost: "<median script_ms / commit count, or 'unprofiled: <why>'>"
```
