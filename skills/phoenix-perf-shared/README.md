# Phoenix Frontend Performance Hunt Skills

Systematic React performance hunting for the Phoenix UI. Same scientific
method as the lading optimization skills (github.com/datadog/lading),
retargeted from Rust/Criterion/hyperfine to React + a browser harness.

The rigor is **not** in the tools. It is in the contract:

- **Reproducible scenario** (the "fingerprint") — a declarative step list.
- **Baseline captured BEFORE any code change** — never after, never reconstructed.
- **Raw per-run samples** — the harness never pre-averages. The skill owns
  mean / stddev / Welch significance (`phoenix-perf-review/scripts/stats.py`).
- **Significance thresholds** — sub-noise-floor changes are not optimizations.
- **Role separation** — finder ≠ coordinator/recorder ≠ judge. Review judges
  but does not record; hunt records but does not judge. Kills self-grading bias.
- **Persistent `db.yaml`** — every outcome (success, failure, blocked) recorded,
  so future hunts dedup and bias toward proven techniques.

## Harness: transport-abstracted (`browser_profile` default, `agent-browser` legacy)

`phoenix-perf-shared/scripts/run-scenario` is the scenario harness, supporting
two measurement transports selectable via `--transport`:

### `browser_profile` transport (default)

Delegates measurement to Phoenix's in-agent `browser_profile run_scenario` tool
via an environment-provided command bridge (`BROWSER_PROFILE_CMD`). The parent
LLM agent sets `BROWSER_PROFILE_CMD` to a command that forwards the
`run_scenario` request to the tool. Preferred when running inside Phoenix.

Metrics per run:

| Key | Meaning |
|-----|---------|
| `react_commits` | React commit count during the measured window |
| `react_actual_ms` | Σ React `actualDuration` during the window |
| `js_heap_used` | post-GC JSHeapUsedSize (bytes) |

### `agent-browser` transport (legacy, `--transport agent-browser`)

Drives the `agent-browser` CLI directly, reads metrics via `agent-browser eval`.
Requires `agent-browser` on PATH.

Metrics per run:

| Key | Meaning |
|-----|---------|
| `script_ms` | Σ longtask duration in the measured window |
| `long_tasks` | count of longtasks (>50ms) |
| `wall_ms` | `performance.now()` across the window |
| `js_heap_used` | `performance.memory.usedJSHeapSize` after readiness |
| `dom_nodes` | element count after readiness |
| `react_commit_count` / `react_commit_ms` | best-effort via DevTools hook; absent → omitted |

`stats.py` resolves both transport key sets via candidate path lists in `METRICS`.

## Skills

| Skill | Purpose |
|-------|---------|
| `/phoenix-perf-preflight` | Environment validation — run first every session |
| `/phoenix-perf-find-target` | Select and analyze one React perf target |
| `/phoenix-perf-hunt` | Baseline, implement, hand to review, record outcome |
| `/phoenix-perf-review` | 5-persona peer review backed by raw-sample stats |
| `/phoenix-perf-submit` | Git branch, commits, optional PR |

```
preflight --> find-target --> hunt --> [implement] --> review --> submit
                                                         |
                                                         v
                                            record in db.yaml (always)
```

## Significance thresholds

Below threshold = browser noise, not optimization. Browser is noisier than
Rust, so floors are higher than lading's 5/10/20.

| Metric | Threshold |
|--------|-----------|
| `script_ms` (Σ longtask) | ≥ 10% |
| `react_commit_count` | ≥ 20% fewer |
| `react_commit_ms` | ≥ 10% |
| `wall_ms` | ≥ 10% |
| `js_heap_used` | ≥ 15% |
| `long_tasks` | ≥ 1 fewer OR ≥ 20% total |

A win also requires Welch's t-test **p < 0.05** over the raw samples.
Threshold met but p ≥ 0.05 → noise → REJECT.

## Layout

Skills are tracked in `skills/` (in-repo) and symlinked into the gitignored
`.claude/skills/` so Claude Code discovers them. Shared harness + stats live
under `phoenix-perf-shared/` and `phoenix-perf-review/scripts/`.

```bash
grep streaming skills/phoenix-perf-hunt/resources/db.yaml
cat skills/phoenix-perf-hunt/resources/db/<id>.yaml
```

## Prerequisites

- Phoenix dev server running (`./dev.py up`) + seeded DB (`./dev.py seed`)
- `agent-browser` available
- `uv` (for the PEP-723 scripts) or Python ≥ 3.11
