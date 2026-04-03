---
created: 2026-04-03
priority: p1
status: ready
artifact: ui/src/components/InputArea.tsx
---

# Explore mode InputArea hint

## Summary

InputArea looks identical in all modes. In Explore mode, the placeholder says
"Type a message..." with generic hints about @ and /. No indication that the
conversation is read-only or that the agent will propose tasks rather than
directly editing files.

## What to change

Pass `convModeLabel` to InputArea. When in Explore mode, change the default
placeholder to "Explore this codebase or describe a change to plan..."

Keep the rotating hints (@ for files, / for skills) but prepend the mode
context.

## Done when

- [ ] InputArea receives convModeLabel prop
- [ ] Explore mode shows mode-aware placeholder text
- [ ] Work and Standalone modes keep existing placeholder
- [ ] Placeholder doesn't conflict with voice input or other input states
