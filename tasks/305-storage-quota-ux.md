---
created: 2026-02-04
priority: p2
status: done
---

# Improve Storage Quota Warning UX

## Summary

Currently, storage quota warnings are only logged to console. Need to show user-friendly warnings in the UI.

## Context

We implemented a 100MB warning threshold but it only logs to console. Users need clear visibility when approaching storage limits.

## Acceptance Criteria

- [x] Show toast/banner when storage exceeds 100MB
- [x] Display current usage in settings/about section
- [x] Provide "Clear old data" button
- [x] Show which conversations use most space
- [x] Automatic cleanup suggestions
- [x] Test quota exceeded scenarios

## Notes

- Current implementation: console.warn at 100MB
- Auto-cleanup runs at 7 days when quota exceeded
- Consider progressive warnings (75MB, 90MB, 100MB)
- Mobile browsers have smaller quotas

## Implementation Notes (2026-02-04)

- Added StorageStatus component showing current usage in header
- Progressive color indicators: green (<75MB), orange (75-100MB), red (>100MB)
- Toast notifications via custom events when storage exceeds 100MB
- Storage details panel shows usage bar, percentage, and clear button
- Clear button removes conversations older than 7 days
- Quota exceeded events also trigger toast notifications
- Storage check runs every 5 minutes and on component mount
