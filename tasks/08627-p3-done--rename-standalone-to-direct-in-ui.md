---
created: 2026-04-03
priority: p3
status: done
artifact: ui/src/components/ConversationList.tsx
---

# Rename Standalone to Direct in UI

## Summary

Backend enum says `Standalone`, UI badge says `SOLO`, tooltip says "Full access
(non-git directory)". Three names for one concept. "Direct" better communicates
the interaction style (direct file access, no plan/approve ceremony).

## What to change

- ConversationList badge: "SOLO" -> "Direct"
- Tooltip: "Full access (non-git directory)" -> "Direct mode (no git workflow)"
- Keep backend enum as `Standalone` (or rename if REQ-PROJ-018 lands first and
  introduces Direct as a new mode for git repos -- in that case, align the naming)

Note: coordinate with REQ-PROJ-018 (Direct mode for git repos). If that spec
lands, "Direct" will apply to both non-git and git-without-ceremony. The backend
may unify Standalone into Direct.

## Done when

- [ ] UI badge says "Direct" instead of "SOLO"
- [ ] Tooltip updated
- [ ] No backend enum changes (unless coordinated with REQ-PROJ-018)
