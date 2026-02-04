---
created: 2026-02-04
priority: p2
status: done
---

# Add Service Worker for Network-Level Caching

## Summary

Implement a service worker to cache API responses at the network level, providing an additional caching layer and enabling true offline functionality.

## Context

While we have IndexedDB caching, a service worker would provide network-level caching, making the app work even when fully offline (including initial load).

## Acceptance Criteria

- [x] Create service worker with cache-first strategy
- [x] Cache static assets (JS, CSS, images)
- [x] Cache API responses with appropriate TTL
- [x] Handle cache invalidation
- [x] Show offline indicator when using cached responses
- [x] Provide "Update available" notification
- [x] Background sync for pending operations

## Notes

- Consider Workbox for easier implementation
- Need to handle cache versioning
- Coordinate with existing IndexedDB cache
- Be careful with SSE endpoints (don't cache)

## Implementation Notes (2026-02-04)

- Implemented custom service worker without Workbox for better control
- Cache-first strategy for static assets with background updates
- Network-first strategy for API calls with cache fallback
- SSE endpoints explicitly excluded from caching
- Two caches: CACHE_NAME for static, API_CACHE_NAME for API responses
- Update notification component shows when new version detected
- Automatic cache cleanup on activation
- Added X-From-Service-Worker-Cache header to cached responses
- Note: Service workers require HTTPS in production environments
