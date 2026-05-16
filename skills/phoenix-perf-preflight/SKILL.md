---
name: phoenix-perf-preflight
description: Environment validation checklist for Phoenix React performance hunting. Run this FIRST when starting a new session to verify the dev server, agent-browser harness, seed data, git state, and stats helper are ready before any optimization work.
allowed-tools: Bash
context: fork
---

# Run preflight checks

```bash
skills/phoenix-perf-preflight/scripts/preflight
```

**STOP on any `[X]` failure.** A failed required check means a later phase
produces an indeterminate baseline — which invalidates the whole hunt.

# Offer suggestions, then ask before acting

For each failed check show the fix and **ask before running it**. Do not
auto-execute fixes.

| Failed check | Fix |
|--------------|-----|
| dev server not running | `./dev.py up` (prints the UI URL — capture it) |
| Phoenix port not responding | `./dev.py status`, then `./dev.py restart` |
| `agent-browser` not found | It is a Claude skill / CLI; install per its docs. The scenario harness cannot run without it. |
| `run-scenario` missing/not exec | `chmod +x skills/phoenix-perf-shared/scripts/run-scenario` |
| DB has no conversations | `./dev.py seed` |
| `uv` not found | `brew install uv` (or system `python3` ≥ 3.11) |
| git user.name/email unset | `git config user.name/email` |
| working tree dirty | commit/stash before a hunt (baseline must be on a clean tree) |

# Why each check exists

- **Dev server + UI URL**: scenarios drive a real browser against the running
  app. No server → no scenario → no baseline.
- **agent-browser + run-scenario**: the measurement harness Claude Code uses
  (NOT Phoenix's in-agent `browser_profile`). Absent → scenarios cannot run.
- **Seed data**: load scenarios need a deterministic many-message
  conversation; without it the scenario is not reproducible.
- **Clean git tree**: baseline must be captured against a known commit so the
  post-change diff is exactly the one change under test.
- **stats helper runnable**: the skill (not the harness) owns significance.

Report the UI URL to the user at the end so they can watch the hunt.
