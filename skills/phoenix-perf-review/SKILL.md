---
name: phoenix-perf-review
description: Reviews a Phoenix React performance patch with a 5-persona peer-review panel backed by raw-sample statistics. Requires unanimous approval and statistically significant improvement past the noise floor. Judges; does not record.
argument-hint: "[scenario] [file] [target] [technique]"
allowed-tools: Bash Read Glob Grep
context: fork
---

# Role: judge — does NOT record, does NOT create db files

Review re-measures, computes significance, runs the panel, returns a YAML
report to the caller. Hunt persists it. Review never writes the db.

## Arguments (all required; missing any → REJECT)

| Arg | Field | Used for |
|-----|-------|----------|
| `$0` | scenario | scenario file to re-run (same one as baseline) |
| `$1` | file | `ui/src` path — report + duplicate check |
| `$2` | target | Component/function — report |
| `$3` | technique | technique — report + duplicate check |

### Report id

`<file-stem>-<technique>` — e.g. `StreamingMessage-reparse-growing-buffer`.

## Phase 1 — Re-measure (identical methodology)

1. Read baseline raw samples `/tmp/phoenix-perf-baseline.json`. **Missing →
   REJECT** (baseline must precede the change; it cannot be reconstructed).
2. Re-run the **same** `$0` scenario via the same harness
   (`skills/phoenix-perf-shared/scripts/run-scenario`) with the **same**
   runs / warmup / subst. Persist raw samples
   `/tmp/phoenix-perf-optimized.json`. If `browser_profile` is available only
   as an LLM tool, use the same `--request-out` → tool call → `--response-in`
   flow used for the baseline.
3. Compute significance from raw samples — the skill owns stats, not the
   harness:

   ```bash
   uv run skills/phoenix-perf-review/scripts/stats.py \
     /tmp/phoenix-perf-baseline.json /tmp/phoenix-perf-optimized.json
   ```

   (`python3 scripts/stats.py ...` if no `uv`.) It prints, per metric:
   baseline mean±sd, optimized mean±sd, % change, Welch t-test p.

### Significance gate (both required per metric claimed as a win)

| Metric | Threshold |
|--------|-----------|
| `react_commits` / `react_commit_count` (legacy) | ≥ 20% fewer |
| `react_actual_ms` / `react_commit_ms` (legacy) | ≥ 10% |
| `js_heap_used` | ≥ 15% |
| `script_ms` (Σ longtask, legacy transport) | ≥ 10% |
| `wall_ms` (legacy transport) | ≥ 10% |
| `long_tasks` (>50ms, legacy transport) | ≥ 1 fewer OR ≥ 20% total |

AND Welch p < 0.05. Threshold met but p ≥ 0.05 → it is noise → REJECT.

### NO EXCEPTIONS

- "Theoretically faster" → REJECT. Prove it.
- "Obviously fewer renders" → REJECT. Measure it.
- "Benchmark later / tool flaky" → REJECT. Measure now or record blocked.
- Harness returned an averaged result instead of raw samples → REJECT
  (non-conforming harness; significance untestable).

## Phase 2 — Five-persona panel

### 1. Duplicate Hunter
- [ ] `$1`+`$3` combo not already in `db.yaml` (any verdict). Prior REJECT
      with no new angle → REJECT "DUPLICATE: see <id>".

### 2. Skeptic (demands proof)
- [ ] Baseline present and pre-change
- [ ] Same scenario, runs, warmup, throttle for both
- [ ] Hot path confirmed by profiling/commit attribution, not guessed
- [ ] Threshold met AND Welch p < 0.05
- [ ] Improvement exceeds run-to-run variance (sd), not a single lucky run

### 3. Conservative (guards correctness)
- [ ] `./dev.py check` passes (clippy, fmt, tests, codegen-stale, task validate)
- [ ] No behavior change: same rendered output, same SSE semantics, same draft
      isolation, same scroll behavior
- [ ] No new race (e.g. rAF/effect ordering) — escalate to timer-smell-hunter if unsure
- [ ] No bug introduced (found bug → REJECT with details)

### 4. React Expert (Phoenix patterns)
- [ ] No unnecessary re-render reintroduced elsewhere
- [ ] Deps arrays correct (no stale-closure, no missing dep)
- [ ] Memoization keys reflect real inputs (no over-memo masking a bug)
- [ ] Follows Phoenix UI conventions; no parallel state representation
- [ ] Worst case considered (longest stream, largest conversation), not just typical

### 5. Greybeard (simplicity)
- [ ] Readable without a paragraph of comments
- [ ] Complexity justified by the measured delta
- [ ] "Obviously fast", not a clever trick
- [ ] Minimal diff, no scope creep, 3-repeat rule respected

## Phase 3 — Decision

| Outcome | Votes | Action |
|---------|-------|--------|
| APPROVED | 5/5 approve | fill approved template |
| REJECTED | any reject | fill rejected template |

Duplicate, bug, correctness regression, missing/averaged baseline, or
sub-threshold/insignificant result are all rejections. State the specific
reason in `reason`.

## Phase 4 — Return report

Read the matching template from `resources/`, fill placeholders with real
numbers from `stats.py`, return the completed YAML to the caller. Do not
write any file.

| Verdict | Template |
|---------|----------|
| approved | `resources/approved.template.yaml` |
| rejected | `resources/rejected.template.yaml` |
