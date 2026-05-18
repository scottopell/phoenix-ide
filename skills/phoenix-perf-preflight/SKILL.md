---
name: phoenix-perf-preflight
description: Environment validation checklist for Phoenix React performance hunting. Run this FIRST when starting a new session to verify the dev server, run-scenario harness (including --self-test), seed data, git state, and stats helper are ready before any optimization work. The default transport is browser_profile (in-agent LLM tool); agent-browser is legacy/optional.
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
| `run-scenario` missing/not exec | `chmod +x skills/phoenix-perf-shared/scripts/run-scenario` |
| `run-scenario --self-test` failed | Check for regression in `skills/phoenix-perf-shared/scripts/run-scenario` |
| `BROWSER_PROFILE_CMD` not set (warning) | Normal in plain shell context; `browser_profile` tool is provided by the LLM runtime. Verify the tool is available before running a hunt. |
| `agent-browser` not found (warning) | Optional legacy transport. Only needed for `--transport agent-browser`. |
| DB has no conversations | `./dev.py seed` |
| `uv` not found | `brew install uv` (or system `python3` ≥ 3.11) |
| git user.name/email unset | `git config user.name/email` |
| working tree dirty | commit/stash before a hunt (baseline must be on a clean tree) |

# Why each check exists

- **Dev server + UI URL**: scenarios drive a real browser against the running
  app. No server → no scenario → no baseline.
- **run-scenario + --self-test**: the scenario harness must be present and its
  translation/mapping logic must pass self-tests. `browser_profile` is the default
  transport (Phoenix's in-agent tool); `agent-browser` is legacy/optional. Because
  a shell script cannot inspect LLM tool availability, `browser_profile` tool
  presence is a manual context check — confirm it is available before starting a
  hunt. `BROWSER_PROFILE_CMD` warns (not fails) if unset in the shell.
- **Seed data**: load scenarios need a deterministic many-message
  conversation; without it the scenario is not reproducible.
- **Clean git tree**: baseline must be captured against a known commit so the
  post-change diff is exactly the one change under test.
- **stats helper runnable**: the skill (not the harness) owns significance.

Report the UI URL to the user at the end so they can watch the hunt.
