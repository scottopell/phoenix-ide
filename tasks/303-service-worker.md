---
created: 2026-02-04
priority: p2
status: pending
---

# Add Service Worker for Network-Level Caching

## Summary

Implement a service worker to cache API responses at the network level, providing an additional caching layer and enabling true offline functionality.

## Context

While we have IndexedDB caching, a service worker would provide network-level caching, making the app work even when fully offline (including initial load).

## Acceptance Criteria

- [ ] Create service worker with cache-first strategy
- [ ] Cache static assets (JS, CSS, images)
- [ ] Cache API responses with appropriate TTL
- [ ] Handle cache invalidation
- [ ] Show offline indicator when using cached responses
- [ ] Provide "Update available" notification
- [ ] Background sync for pending operations

## Notes

- Consider Workbox for easier implementation
- Need to handle cache versioning
- Coordinate with existing IndexedDB cache
- Be careful with SSE endpoints (don't cache)
