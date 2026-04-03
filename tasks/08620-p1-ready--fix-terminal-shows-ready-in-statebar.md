---
created: 2026-04-03
priority: p1
status: ready
artifact: ui/src/components/StateBar.tsx
---

# Fix terminal state showing "ready" in StateBar

## Summary

StateBar.tsx has a fall-through case: `case 'idle': case 'terminal':` both
resolve to a green dot with text "ready". A terminal conversation cannot accept
input -- showing "ready" is a lie.

## What to change

Separate the terminal case from idle. Terminal should show "completed" or
"closed" with a neutral (not green) dot.

## Done when

- [ ] Terminal conversations show "completed" (not "ready") in StateBar
- [ ] Dot color is muted/neutral (not green)
- [ ] Idle conversations still show green "ready"
