---
name: phoenix-perf-hunt
description: Coordinates a Phoenix React performance attempt. Captures the scenario baseline (raw samples) BEFORE any change via the agent-browser harness, implements one focused fix, hands off to review, and records every outcome in db.yaml. Does not judge.
allowed-tools: Bash Read Write Edit Glob Grep Skill
---

# Role: coordinator and recorder — NOT judge

Hunt captures baselines, makes ONE change, hands to review, records the
outcome. Hunt does **not** run post-change benchmarks and does **not** decide
pass/fail. That is review's job. This separation kills self-grading bias.

## Phase 0 — Preflight

Run `/phoenix-perf-preflight`. STOP on failure. Capture the UI URL.

## Phase 1 — Find target

Run `/phoenix-perf-find-target`. Print its returned YAML.

## Phase 2 — Establish baseline (BEFORE any code change)

**CRITICAL: capture the baseline before touching code. A baseline measured
after the change, or reconstructed, is not a baseline — abort the hunt.**

1. Resolve scenario params: read the `scenario` file; fill `SLUG` /
   `SLUG_LARGE` / `UI_URL` from `./dev.py status` (DB path → query
   conversations for a slug) and the preflight UI URL.

2. Run the scenario through the harness with the scenario's `runs`/`warmup`:

   ```bash
   skills/phoenix-perf-shared/scripts/run-scenario \
     skills/phoenix-perf-find-target/<scenario> \
     --subst SLUG=<slug> --subst UI_URL=<url> \
     --out /tmp/phoenix-perf-baseline.json
   ```

3. `run-scenario` writes **raw per-run samples** to
   `/tmp/phoenix-perf-baseline.json` (never averaged). If it ever emits a
   reduced/averaged result, that is a non-conforming harness → STOP, fix the
   harness; do not proceed on reduced data.

4. If the scenario cannot run (server down, selector drift): fix it and
   re-baseline. Do NOT estimate. If genuinely unrunnable, record
   `status: blocked` / `verdict: pending` (Phase 5) with exact resume steps
   and STOP — a guess is never an acceptable baseline.

## Phase 3 — Implement ONE change

Single focused change applying `technique` to `target`. No scope creep.
Must pass before continuing:

```bash
./dev.py check
./dev.py restart   # Rust change; UI-only hot-reloads via Vite — still confirm
```

`./dev.py check` failing on a pre-existing unrelated issue → document, stop.
No "fix it while I'm here."

## Phase 4 — Hand off to review

```
/phoenix-perf-review <scenario> <file> <target> <technique>
```

Review re-runs the **same** scenario against `/tmp/phoenix-perf-baseline.json`,
computes significance from raw samples, runs the 5-persona panel, returns a
YAML report. Print it.

## Phase 5 — Record (every outcome: success, failure, blocked)

1. Write review's YAML report **verbatim** to
   `skills/phoenix-perf-hunt/resources/db/<id>.yaml`. Do not edit it.
2. Add an index entry to `skills/phoenix-perf-hunt/resources/db.yaml` per
   `resources/index.template.yaml`.

Every outcome is recorded — a rejected or blocked hunt still teaches the next
find-target run.
