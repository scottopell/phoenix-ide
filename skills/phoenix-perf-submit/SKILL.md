---
name: phoenix-perf-submit
description: Full Phoenix React performance workflow with git branch, commit, and optional PR. Wraps /phoenix-perf-hunt with git automation.
allowed-tools: Bash Read Skill
---

# Phoenix perf submit

Wraps `/phoenix-perf-hunt` with git automation. Push/PR require explicit user
authorization (per AGENTS.md): commits are local and fine; pushes affect others.

## Phase 0 — Preflight

Run `/phoenix-perf-preflight`.

## Phase 1 — Clean git state

```bash
git status
```

**STOP if the working tree is dirty.** Baseline must be captured against a
known clean commit, else the post-change diff is not exactly the one change
under test. Commit/stash unrelated work first.

## Phase 2 — Hunt

Run `/phoenix-perf-hunt`. After it completes, **return here.** Hunt records the
verdict in `db.yaml`; it does not branch, commit, push, or PR. Those are this
skill's job.

If hunt recorded `blocked` (scenario unrunnable): STOP. Nothing to submit —
there is no measured improvement. Report the blocker to the user.

If hunt recorded `rejected`: STOP. Do not commit a rejected change; revert it
(`git checkout -- <file>`), leave the db record (it teaches future hunts).

## Phase 3 — Branch + commit (only on APPROVED)

```bash
git checkout -b perf/ui-<technique>-<short>
git add -A
```

Commit using `resources/commit-template.txt`. First line ≤ 50 chars. Put real
measured numbers (from review's report) in the body — no estimates.

## Phase 4 — Push + PR (only with explicit user OK)

```bash
git push -u origin perf/ui-<technique>-<short>
gh pr create --title "perf(ui): <short>" --body "$(cat <<'EOF'
## Summary
<what re-render / scripting cost was removed and why>

## Measured (run-scenario / agent-browser, scenario <name>, N=<n>)
- script_ms: <base> -> <opt> (<-%>, p=<p>)
- react_commit_count: <base> -> <opt> (<-%>, p=<p>)
- react_commit_ms: <base> -> <opt> (<-%>, p=<p>)
- js_heap_used: <base> -> <opt> (<-%>)

## Validation
- [x] ./dev.py check passes
- [x] 5-persona review unanimous (db: skills/phoenix-perf-hunt/resources/db/<id>.yaml)
- [x] No behavior change (rendered output / SSE / draft isolation unchanged)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
