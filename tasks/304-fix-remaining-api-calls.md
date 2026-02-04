---
created: 2026-02-04
priority: p1
status: pending
---

# Fix Remaining API Calls to Use Enhanced API

## Summary

Some API calls in ConversationPage still use the base `api` instead of `enhancedApi`, missing out on caching benefits.

## Context

During the investigation, we found that `sendMessage` and `cancelConversation` in ConversationPage.tsx are still using the regular `api` import instead of the enhanced API with caching.

## Acceptance Criteria

- [ ] Replace all `api.` calls with `enhancedApi.` where appropriate
- [ ] Ensure sendMessage uses enhancedApi
- [ ] Ensure cancelConversation uses enhancedApi
- [ ] Audit all components for remaining direct API usage
- [ ] Update imports to remove unused `api` imports

## Notes

- Some operations (like mutations) might not benefit from caching
- Be careful not to cache operations that should always be fresh
- Test offline behavior after changes
