---
created: 2026-02-07
priority: p2
status: ready
---

# Service Worker Cache Staleness and Edge Cases

## Summary

The service worker aggressively caches API responses and static assets. While we fixed the reload race condition on initial install, there may be other edge cases where stale cached data causes UI issues.

## Context

The blank white screen bug was caused by service worker's `controllerchange` triggering a reload during initial install. This was fixed, but the caching logic itself could cause other issues:

1. **Cache-first for static assets** - If assets change but cache isn't invalidated, old JS could run
2. **Network-first for API with cache fallback** - Cache metadata timestamps exist but TTL cleanup may not run
3. **No cache versioning in SW** - `CACHE_NAME = 'phoenix-ide-v10'` is hardcoded

## Potential Issues

- Stale conversation data shown after backend updates
- Old JavaScript running with new API responses
- Cache grows unbounded over time
- Offline mode may show very old data without indication

## Acceptance Criteria

- [ ] Add cache TTL enforcement (currently 5 min defined but not enforced)
- [ ] Show visual indicator when displaying cached/stale data
- [ ] Implement proper cache invalidation on SW update
- [ ] Add "force refresh" option for users experiencing stale data
- [ ] Log cache hit/miss stats for debugging

## Relevant Files

- `ui/public/service-worker.js`
- `ui/src/serviceWorkerRegistration.ts`
- `ui/src/enhancedApi.ts` - Shows "Loaded from: indexeddb/network"

## Notes

The "Loaded from: indexeddb" indicator at bottom of conversation page is good - consider making it more prominent when data is potentially stale.
