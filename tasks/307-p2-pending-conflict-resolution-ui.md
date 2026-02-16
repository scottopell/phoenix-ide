---
created: 2026-02-04
priority: p2
status: pending
---

# Add Conflict Resolution UI

## Summary

Implement UI for handling conflicts when the same conversation is modified on multiple devices.

## Context

We discussed "single user multiple devices" scenario and implemented basic CRDT-inspired merging, but there's no UI for handling conflicts when they occur.

## Acceptance Criteria

- [ ] Detect when local and remote state diverge
- [ ] Show conflict indicator in UI
- [ ] Allow user to choose resolution strategy
- [ ] "Their changes" / "My changes" / "Merge"
- [ ] Show diff of conflicting changes
- [ ] Test with multiple tabs/devices

## Notes

- Currently: last-write-wins
- Most conflicts will be in message ordering
- Consider three-way merge for complex conflicts
- Preserve unsent messages during conflict resolution
