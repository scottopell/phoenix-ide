---
created: 2026-02-04
priority: p1
status: done
---

# Fix Remaining API Calls to Use Enhanced API

## Summary

Some API calls in ConversationPage still use the base `api` instead of `enhancedApi`, missing out on caching benefits.

## Context

During the investigation, we found that `sendMessage` and `cancelConversation` in ConversationPage.tsx are still using the regular `api` import instead of the enhanced API with caching.

## Acceptance Criteria

- [x] Replace all `api.` calls with `enhancedApi.` where appropriate
- [x] Ensure sendMessage uses enhancedApi
- [x] Ensure cancelConversation uses enhancedApi
- [x] Audit all components for remaining direct API usage
- [x] Update imports to remove unused `api` imports

## Notes

- Some operations (like mutations) might not benefit from caching
- Be careful not to cache operations that should always be fresh
- Test offline behavior after changes

## Implementation Notes (2026-02-04)

- Replaced all direct `api.` calls with `enhancedApi.` across 4 files
- Enhanced API properly handles mutations:
  - `sendMessage`, `cancelConversation` pass through without caching
  - `archiveConversation`, `deleteConversation`, etc. update local cache after mutation
- All components now benefit from two-tier caching (memory → IndexedDB)
- Removed unused `api` imports from components
- Verified caching works via console logs showing "Active: indexeddb"
