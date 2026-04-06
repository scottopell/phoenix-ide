---
created: 2026-04-06
priority: p1
status: ready
artifact: dev.py
---

# Deploy mode system changes to prod

## Summary

All the mode system improvements (StateBar indicator, terminal fix, input
hints, new-conv preview, propose_task rename, Direct naming, terminal banner)
are on dev but not deployed to prod. Also includes: prompt caching, taskmd-core
integration, context menu, file explorer improvements, auto-stash, and the
tool_use stripping fix for Explore->Work transitions.

## Done when

- [ ] ./dev.py prod deploy succeeds
- [ ] Prod at localhost:8031 shows mode indicators
- [ ] Existing conversations still work (no migration needed)
