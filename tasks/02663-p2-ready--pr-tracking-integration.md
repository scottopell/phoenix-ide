---
created: 2026-04-20
priority: p2
status: ready
artifact: ui/src/components/StateBar.tsx
---

# PR tracking integration in StateBar

## Summary

Show PR status (open, merged, CI) as a badge in the StateBar next to the
branch name. Detect via `gh pr list --head <branch>`. Color-coded: green
(checks passing), yellow (pending), red (failing), purple (merged).
Clickable to open PR URL.

## Context

User request: "super useful in claude code and other apps I use, see purple
badge with merged logo and skim right to the next one"

## Done When

Branch/Work conversations show PR badge when a PR exists for the branch.
Badge links to PR URL. CI status reflected in color.
