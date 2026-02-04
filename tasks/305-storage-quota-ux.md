---
created: 2026-02-04
priority: p2
status: pending
---

# Improve Storage Quota Warning UX

## Summary

Currently, storage quota warnings are only logged to console. Need to show user-friendly warnings in the UI.

## Context

We implemented a 100MB warning threshold but it only logs to console. Users need clear visibility when approaching storage limits.

## Acceptance Criteria

- [ ] Show toast/banner when storage exceeds 100MB
- [ ] Display current usage in settings/about section
- [ ] Provide "Clear old data" button
- [ ] Show which conversations use most space
- [ ] Automatic cleanup suggestions
- [ ] Test quota exceeded scenarios

## Notes

- Current implementation: console.warn at 100MB
- Auto-cleanup runs at 7 days when quota exceeded
- Consider progressive warnings (75MB, 90MB, 100MB)
- Mobile browsers have smaller quotas
