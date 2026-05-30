---
name: spec-audit-bug-hunt
description: Re-run Phoenix spec executive-table inventory, prioritize high-ROI incomplete rows, close small spec-code gaps with matching executive updates, and spin out concrete follow-up tasks for ambiguous or significant gaps. Use when asked to audit specs, find spec-code drift, continue spec-audit bug hunting, or mine executive tables for actionable work.
allowed-tools: Bash(./dev.py audit-specs:*), Bash(./dev.py tasks:*), Bash(taskmd:*), Bash(grep:*), Bash(rg:*), Bash(git diff:*), Bash(git status:*)
---

# Spec Audit Bug Hunt

Use this workflow to turn stale or incomplete `specs/*/executive.md` rows into either shipped fixes, corrected status tables, or precise follow-up tasks.

## Workflow

1. **Refresh the inventory**
   ```bash
   for f in specs/*/executive.md; do
     n=$(grep -cE "(🚧|❌|Partial|Not Started|New|Replace|Rewrite|Extend)" "$f")
     [ "$n" -gt 0 ] && echo "$n $f"
   done | sort -rn
   ./dev.py audit-specs
   ```

2. **Prioritize high-ROI rows**
   - Start with 🚧 / partial rows and rows whose notes name a small concrete gap.
   - Prefer rows where code already appears implemented but the executive table is stale.
   - Defer broad ❌ rows that imply new product design or large backend/UI flows.

3. **For each candidate, verify before editing**
   - Read the requirement row, adjacent requirements, and any linked design/allium files.
   - Search implementation and tests for the cited requirement IDs and behavior.
   - Decide one of three outcomes:
     - **Small gap:** implement it, add/update tests, and mark the executive row complete in the same change.
     - **Already complete:** update the executive row/status note only, with implementation pointers.
     - **Ambiguous/significant:** do not guess; create a narrowed `taskmd new` follow-up describing evidence, options, and acceptance criteria.

4. **Keep changes reviewable**
   - One coherent spec gap per commit/PR unless multiple rows share the same implementation path.
   - Executive updates must match reality; don't flip status just to reduce the count.
   - If code and spec contradict each other, ask for the design decision before implementation.

5. **Validate**
   ```bash
   ./dev.py tasks validate
   ./dev.py audit-specs
   ./dev.py check
   ```
   Use the narrower validations while triaging; run the full check before handoff when code or generated types changed.

## Deliverable

A completed audit slice should leave every investigated row in one of these states:

- ✅ Complete with an accurate note pointing at code/tests/specs.
- Still incomplete, but backed by a concrete task file that a future implementer can pick up without rediscovery.
- Explicitly deferred or out-of-scope in the relevant spec, with the executive table aligned.
