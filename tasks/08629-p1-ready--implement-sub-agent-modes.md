---
created: 2026-04-06
priority: p1
status: ready
artifact: src/tools/subagent.rs
---

# Implement sub-agent modes (REQ-PROJ-008)

## Summary

Design spec is complete in specs/projects/design.md. Implementation needed
for mode-aware sub-agents: explore (haiku, read-only) vs work (parent model,
full tools).

## Sub-items

1. Schema: add mode, model, max_turns to spawn_agents input
2. Tool registries: for_subagent_explore() and for_subagent_work()
3. Model selection: explore defaults haiku, work inherits parent
4. MCP access: both modes get deferred MCP tools
5. One-writer constraint: track active Work sub-agents per parent
6. Max turns limit: explore=20, work=50, supplements 5min timeout

## Done when

- [ ] spawn_agents accepts mode, model, max_turns per task
- [ ] Explore sub-agents get read-only tools + haiku
- [ ] Work sub-agents get full tools + parent model
- [ ] Only one Work sub-agent per parent at a time
- [ ] MCP tools available to sub-agents via tool search
- [ ] Max turns enforced, forced completion on limit
- [ ] ./dev.py check passes
