---
created: 2026-02-05
priority: p4
status: ready
---

# Add Skeleton for StorageStatus Badge

## Summary

Show a skeleton placeholder for the storage status badge while storage info is being calculated.

## Context

The StorageStatus component returns `null` while loading storage info, causing a brief layout shift when it appears. A skeleton badge would provide consistent layout.

## Current Behavior

```tsx
if (!storageInfo) return null;  // Nothing shown during load
```

## Proposed Behavior

Show a skeleton badge with approximate dimensions:
```
[░░░░░░]  <- skeleton badge during load
[💾 2.3MB]  <- actual badge after load
```

## Acceptance Criteria

- [ ] Show skeleton badge while `storageInfo` is null
- [ ] Skeleton matches size of typical storage badge
- [ ] Smooth transition to real badge
- [ ] No layout shift when content loads

## Technical Notes

- Simple change in `StorageStatus.tsx`
- Badge is approximately 70-80px wide, 28px tall
- Could use inline skeleton or create `StorageStatusSkeleton`

## Priority

Low priority (p4) - this is a minor polish item. The storage check is usually fast.

## See Also

- `ui/src/components/StorageStatus.tsx`
- `ui/src/components/Skeleton.tsx`
