---
created: 2026-02-05
priority: p3
status: ready
---

# Add Skeleton Loading to DirectoryPicker

## Summary

Show skeleton directory entries while the directory listing is loading in the NewConversationModal.

## Context

When users navigate directories in the DirectoryPicker, there's a brief moment where the list is empty while the API fetches the directory contents. On slower filesystems or with many entries, this delay is noticeable. A skeleton would provide better feedback.

## Current Behavior

1. User clicks a directory
2. List becomes empty (or shows previous entries)
3. API call completes
4. New entries appear

## Proposed Behavior

1. User clicks a directory
2. Show 3-4 skeleton directory entries with shimmer
3. API call completes
4. Skeleton fades to real entries

## Acceptance Criteria

- [ ] Add loading state to DirectoryPicker component
- [ ] Show `DirectoryEntrySkeleton` (3-4 items) while loading
- [ ] Skeleton matches directory entry structure (icon + name + arrow)
- [ ] Smooth transition from skeleton to real content
- [ ] Also show skeleton during initial load, not just navigation

## Technical Notes

- Add to `ui/src/components/Skeleton.tsx`:
  ```tsx
  export function DirectoryEntrySkeleton() { ... }
  export function DirectoryListSkeleton({ count = 4 }) { ... }
  ```
- Modify `DirectoryPicker.tsx` to track loading state for `loadEntries()`
- Current code in `useEffect` doesn't expose loading state

## See Also

- `ui/src/components/DirectoryPicker.tsx`
- `ui/src/components/Skeleton.tsx`
- `ui/src/components/NewConversationModal.tsx`
