---
created: 2026-01-31
priority: p3
status: done
---

# Sub-agent Tree Visualization

## Summary

Show hierarchical view of sub-agents when parent spawns children, with individual status for each.

## Context

The backend supports sub-agents (REQ-API-011 will provide state data with pending/completed counts). Currently the UI just shows "waiting for N sub-agents" text. Users want to see which sub-agents are running, their tasks, and individual progress.

## Acceptance Criteria

- [ ] Show sub-agent list when in `awaiting_sub_agents` state
- [ ] Each sub-agent shows: task description, status (running/completed/failed)
- [ ] Expandable to show sub-agent's own tool execution progress
- [ ] Visual hierarchy (indentation or tree lines)
- [ ] Click to navigate to sub-agent conversation (if supported)

## Notes

- Blocked by task 020 (need granular state events first)
- Sub-agent conversations are separate entries in the database
- May need new API endpoint to fetch sub-agent details
- Consider mobile-friendly accordion UI vs desktop tree view
