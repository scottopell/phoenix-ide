---
created: 2026-01-29
priority: p2
status: ready
---

# Implement sub-agent spawning (REQ-BED-008, REQ-BED-009)

## Summary

Sub-agent spawning and management features are specified but not implemented.

## Context

Discovered during QA validation. Requirements REQ-BED-008 and REQ-BED-009 define sub-agent capabilities:
- REQ-BED-008: Spawn sub-conversation with own working directory
- REQ-BED-009: Monitor and manage sub-agent lifecycle

These are marked as future features in the spec.

## Acceptance Criteria

- [ ] Design sub-agent data model (parent/child relationship)
- [ ] Implement spawn_agent tool
- [ ] Track sub-agent state in parent conversation
- [ ] Handle sub-agent completion/failure
- [ ] Add UI support for viewing sub-agents

## Notes

Requires:
- Database schema for parent_conversation_id (already exists)
- New tool definition
- State machine updates for sub-agent events
- UI components for sub-agent visualization
