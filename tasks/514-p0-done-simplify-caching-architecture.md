---
created: 2026-02-08
priority: p0
status: done
---

# Simplify Caching Architecture: Remove Service Worker, Eliminate Race Conditions

## Summary

The current caching architecture has three overlapping layers (Service Worker, IndexedDB, Memory Cache) that create race conditions, stale data bugs, and white-screen failures. This task refactors to a simple, correct-by-construction architecture with IndexedDB as the single source of truth for offline data.

## Problem Statement

### Symptoms Observed
1. **White screen on load** - Service worker serves corrupted/stale JS files
2. **Stale conversation lists** - Multiple cache layers get out of sync
3. **Race conditions** - Background refresh operations conflict with SSE updates and navigation
4. **Dev/prod inconsistency** - Service worker caches interfere with Vite HMR

### Root Causes Identified

1. **Service Worker caches JS/CSS files** with cache-first strategy, serving stale code even when Vite has new builds
2. **Three cache layers** (SW API cache, IndexedDB, Memory cache) with no coordination
3. **Background refresh pattern** (`if (stale) { backgroundRefresh() }`) creates races with SSE and navigation
4. **Memory cache** adds complexity without meaningful performance benefit over IndexedDB
5. **TTL-based staleness** is meaningless when SSE provides real-time updates anyway
6. **IndexedDB init blocking** can cause app to hang if init fails

## Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        React UI                             │
│   (orchestrates caching, decides when to refresh)           │
│                                                             │
│   ┌──────────────┐    ┌──────────────┐    ┌─────────────┐  │
│   │ List Page    │    │ Conv Page    │    │ New Page    │  │
│   │ cache→fetch  │    │ cache→SSE    │    │ (no cache)  │  │
│   └──────┬───────┘    └──────┬───────┘    └─────────────┘  │
│          │                   │                              │
└──────────┼───────────────────┼──────────────────────────────┘
           │                   │
           ▼                   ▼
    ┌─────────────────────────────────────┐
    │            cacheDB (IndexedDB)       │
    │  - conversations                     │
    │  - messages                          │
    │  - pendingOps (queued writes)        │
    │                                      │
    │  Simple operations: get/put only     │
    │  No TTL, no staleness, no background │
    └─────────────────────────────────────┘
           │                   │
           ▼                   ▼
    ┌─────────────────────────────────────┐
    │              baseApi                 │
    │  - Pure network calls                │
    │  - No caching logic                  │
    └─────────────────────────────────────┘
```

### Key Principles

1. **One source of truth per data type**: IndexedDB for offline data, Network for live data
2. **No background operations**: All cache reads/writes are explicit and synchronous from the caller's perspective
3. **UI controls the flow**: Pages decide when to show cached data vs fetch fresh
4. **Cache is dumb storage**: Simple get/put, no TTL logic, no automatic refresh
5. **Graceful offline**: Show cached data when offline, queue writes for later sync

## Data Flow Patterns

### ConversationListPage
```typescript
useEffect(() => {
  async function load() {
    // Step 1: Show cached data immediately
    const cached = await cacheDB.getAllConversations();
    if (cached.length > 0) {
      setConversations(cached);
      setLoading(false);
    }

    // Step 2: Fetch fresh if online (sequential, not background)
    if (navigator.onLine) {
      try {
        const fresh = await baseApi.listConversations();
        setConversations(fresh);
        await cacheDB.putConversations(fresh);
      } catch (err) {
        // Network failed, cached data still showing
      }
    }
    setLoading(false);
  }
  load();
}, []);
```

### ConversationPage
```typescript
useEffect(() => {
  async function load() {
    // Step 1: Show cached messages immediately
    const cached = await cacheDB.getConversationBySlug(slug);
    if (cached) {
      setMessages(cached.messages);
      setLoading(false);
    }

    // Step 2: Connect SSE for real-time updates
    // SSE 'init' event provides fresh data, replacing cached
    // SSE 'message' events provide real-time updates
    connectSSE(...);
  }
  load();
}, [slug]);
```

### Offline Message Sending (Subway Use Case)
```typescript
async function sendMessage(text: string) {
  const localId = crypto.randomUUID();
  
  // Optimistic UI update
  setMessages(prev => [...prev, { localId, text, status: 'pending' }]);
  
  // Queue the operation
  await cacheDB.addPendingOp({ type: 'send_message', conversationId, payload: { text, localId } });
  
  // Try to send if online
  if (navigator.onLine) {
    try {
      await baseApi.sendMessage(conversationId, text, localId);
      await cacheDB.removePendingOp(localId);
    } catch (err) {
      // Will retry when online
    }
  }
}
```

## Files to Modify

### DELETE
- `ui/src/memoryCache.ts` - Unnecessary layer
- `ui/src/enhancedApi.ts` - Over-engineered, causes races
- `ui/src/performance.ts` - Only used by enhancedApi
- `ui/public/service-worker.js` - Causes stale code issues
- `ui/src/components/ServiceWorkerUpdatePrompt.tsx` - No longer needed
- `ui/src/components/ServiceWorkerUpdatePrompt.css` - No longer needed

### MODIFY
- `ui/src/serviceWorkerRegistration.ts` - Change to unregister existing SWs only
- `ui/src/cache.ts` - Simplify: remove TTL/staleness, just get/put operations
- `ui/src/pages/ConversationListPage.tsx` - Direct cache + API pattern
- `ui/src/pages/ConversationPage.tsx` - Direct cache + SSE pattern  
- `ui/src/App.tsx` - Remove ServiceWorkerUpdatePrompt
- `ui/src/hooks/useAppMachine.ts` - Simplify: just init IndexedDB, fail clearly if it fails
- `ui/src/hooks/index.ts` - Update exports

### KEEP (no changes needed)
- `ui/src/api.ts` - Base API calls (already clean)
- `ui/src/hooks/useConnection.ts` - SSE connection management
- `ui/src/hooks/useMessageQueue.ts` - Pending operations UI
- `ui/src/syncQueue.ts` - Operation sync logic

## Acceptance Criteria

### Functional
- [ ] App loads without white screen issues
- [ ] Conversation list shows cached data immediately, then refreshes
- [ ] Conversation page shows cached messages, SSE provides updates
- [ ] Offline: can read cached conversations
- [ ] Offline: can queue messages for later sending
- [ ] Online: queued messages sync automatically
- [ ] No stale data issues after navigation

### Code Quality  
- [ ] No background refresh operations anywhere
- [ ] No TTL/staleness logic in cache layer
- [ ] Service worker completely removed
- [ ] Memory cache completely removed
- [ ] enhancedApi removed, pages use cache + api directly
- [ ] TypeScript compiles with no errors
- [ ] All existing tests pass (update as needed)

### Testing
- [ ] Test: Load app online, data displays correctly
- [ ] Test: Load app online, go offline, navigate - shows cached data
- [ ] Test: Send message offline, go online - message syncs
- [ ] Test: Vite HMR works without stale code issues
- [ ] Test: Clear browser data, load app - works without cache
- [ ] Test: IndexedDB init failure - shows clear error

## Implementation Order

### Phase 1: Remove Service Worker (unblock development)
1. Modify `serviceWorkerRegistration.ts` to only unregister existing SWs
2. Delete `public/service-worker.js`
3. Delete `ServiceWorkerUpdatePrompt.tsx` and `.css`
4. Update `App.tsx` to remove ServiceWorkerUpdatePrompt import
5. Test that app loads without white screen

### Phase 2: Simplify Cache Layer
1. Delete `memoryCache.ts`
2. Delete `enhancedApi.ts`
3. Delete `performance.ts`
4. Simplify `cache.ts`: remove `_meta.timestamp`, TTL checks, staleness logic
5. Add simple `putConversations()` batch method to cache.ts

### Phase 3: Update Data Flow in Pages
1. Update `ConversationListPage.tsx` to use direct cache + API pattern
2. Update `ConversationPage.tsx` to use direct cache + SSE pattern
3. Simplify `useAppMachine.ts` - just IndexedDB init with clear error handling
4. Update hook exports in `hooks/index.ts`

### Phase 4: Verify and Test
1. Run TypeScript compiler, fix any type errors
2. Run existing tests, update as needed
3. Manual testing of all acceptance criteria
4. Test on mobile viewport sizes

## Notes

### Why Remove Service Worker Entirely?

The SW was providing:
1. **Static asset caching** - Browser HTTP cache does this better with proper headers
2. **API response caching** - IndexedDB already does this
3. **Offline page** - Not needed if IndexedDB provides offline data

The SW was causing:
1. Stale JS/CSS served during development
2. Corrupted cache entries causing white screens
3. Complex cache coordination issues
4. Version skew between cached assets

### Why Remove Memory Cache?

The memory cache was premature optimization:
- IndexedDB reads are 5-20ms, imperceptible to users
- Memory cache created another layer to keep in sync
- Added ~200 lines of code for no real benefit
- Source of cache coherence bugs

### Why Remove TTL/Staleness?

- For ConversationPage: SSE provides real-time updates, TTL is meaningless
- For ConversationListPage: We always refresh when online anyway
- Offline: We show cached data regardless of age
- TTL logic added complexity without solving real problems

---

## Agent Prompt

The following prompt can be used to instruct an LLM agent to implement this task:

---

### Task: Simplify Phoenix IDE Caching Architecture

**Priority: P0 - Blocking development due to white screen and stale cache issues**

#### Context

You are refactoring the Phoenix IDE web UI's caching architecture. The current implementation has three overlapping cache layers (Service Worker, IndexedDB, Memory Cache) that cause race conditions, stale data, and white-screen failures. You will simplify to a single IndexedDB cache layer with explicit, synchronous data flow controlled by the UI components.

#### Working Directory
```
cd /home/exedev/phoenix-ide
```

#### Development Environment
```bash
./dev.py status  # Check if running
./dev.py up      # Start dev servers
# UI: http://localhost:5761
# API: http://localhost:8588
```

#### Implementation Steps

**Phase 1: Remove Service Worker**

1. Edit `ui/src/serviceWorkerRegistration.ts`:
   - Replace `register()` function to ONLY unregister existing service workers
   - Keep `unregister()` and `clearServiceWorkerCache()` functions
   - Remove `showUpdateNotification()` function

2. Delete files:
   - `ui/public/service-worker.js`
   - `ui/src/components/ServiceWorkerUpdatePrompt.tsx`
   - `ui/src/components/ServiceWorkerUpdatePrompt.css`

3. Edit `ui/src/App.tsx`:
   - Remove `ServiceWorkerUpdatePrompt` import and component

4. Verify: `cd ui && npx tsc --noEmit` should pass

**Phase 2: Simplify Cache Layer**

1. Delete files:
   - `ui/src/memoryCache.ts`
   - `ui/src/enhancedApi.ts`
   - `ui/src/performance.ts`

2. Edit `ui/src/cache.ts`:
   - Remove `CacheMeta` interface (no more timestamp/staleness)
   - Remove `CachedConversation` type (just use `Conversation` directly)
   - Simplify all methods to pure get/put operations
   - Add `putConversations(conversations: Conversation[])` batch method
   - Keep `PendingOperation` and related methods unchanged

**Phase 3: Update ConversationListPage**

Edit `ui/src/pages/ConversationListPage.tsx`:
- Remove `enhancedApi` import, use `api` from `../api` and `cacheDB` from `../cache`
- Remove `performanceMonitor` references
- Implement direct cache-then-fetch pattern:
  ```typescript
  useEffect(() => {
    async function load() {
      // Show cached immediately
      const cached = await cacheDB.getAllConversations();
      const active = cached.filter(c => !c.archived);
      const archived = cached.filter(c => c.archived);
      if (active.length > 0 || archived.length > 0) {
        setConversations(active);
        setArchivedConversations(archived);
        setLoading(false);
      }
      
      // Fetch fresh if online
      if (navigator.onLine) {
        try {
          const [freshActive, freshArchived] = await Promise.all([
            api.listConversations(),
            api.listArchivedConversations()
          ]);
          setConversations(freshActive);
          setArchivedConversations(freshArchived);
          // Update cache
          for (const conv of [...freshActive, ...freshArchived]) {
            await cacheDB.putConversation(conv);
          }
        } catch (err) {
          console.error('Failed to refresh:', err);
        }
      }
      setLoading(false);
    }
    load();
  }, []);
  ```
- Remove stale/fresh/source tracking - not needed
- Keep the rest of the component logic unchanged

**Phase 4: Update ConversationPage**

Edit `ui/src/pages/ConversationPage.tsx`:
- Remove `enhancedApi` import, use `api` from `../api` and `cacheDB` from `../cache`
- Update the data loading logic:
  ```typescript
  useEffect(() => {
    async function load() {
      // Show cached immediately
      const cached = await cacheDB.getConversationBySlug(slug);
      if (cached) {
        setConversation(cached);
        const messages = await cacheDB.getMessages(cached.id);
        setMessages(messages);
        setInitialLoadComplete(true);
        setConversationId(cached.id);
      }
      
      // Fetch fresh for SSE connection setup
      // SSE 'init' event will provide authoritative data
      if (navigator.onLine && !cached) {
        try {
          const result = await api.getConversationBySlug(slug);
          setConversation(result.conversation);
          setMessages(result.messages);
          setConversationId(result.conversation.id);
          // Cache it
          await cacheDB.putConversation(result.conversation);
          await cacheDB.putMessages(result.messages);
        } catch (err) {
          setError(err instanceof Error ? err.message : 'Failed to load');
        }
      }
      setInitialLoadComplete(true);
    }
    load();
  }, [slug]);
  ```
- Update SSE event handlers to update cache on new messages
- Remove `lastDataSource` tracking - not needed

**Phase 5: Simplify useAppMachine**

Edit `ui/src/hooks/useAppMachine.ts`:
- Remove references to sync status display for background refreshes
- Keep IndexedDB initialization
- Make init failure a clear error state (IndexedDB is required)
- Simplify the state machine - most states are no longer needed

**Phase 6: Update Exports**

Edit `ui/src/hooks/index.ts`:
- Remove any exports that no longer exist

**Phase 7: Verify**

1. TypeScript: `cd ui && npx tsc --noEmit`
2. Start dev server: `./dev.py up`
3. Test in browser at http://localhost:5761:
   - App loads without white screen
   - Conversation list displays
   - Can navigate to conversations
   - Messages display correctly
   - Vite HMR works (edit a component, see change)
4. Test offline:
   - Load app, then disable network in browser DevTools
   - Should still show cached conversations
   - Should be able to navigate between cached conversations

**Phase 8: Commit**

```bash
git add -A
git commit -m "refactor(ui): simplify caching architecture

- Remove service worker (caused stale code issues)
- Remove memory cache (unnecessary layer)
- Remove enhancedApi (background refresh races)
- Simplify IndexedDB cache to pure get/put
- Update pages to use direct cache-then-fetch pattern
- IndexedDB is now single source of truth for offline data

Fixes white-screen issues and stale data bugs."
```

#### Key Files Reference

Before making changes, review these files to understand current implementation:
- `ui/src/serviceWorkerRegistration.ts` - SW registration
- `ui/src/enhancedApi.ts` - Current caching logic (will be deleted)
- `ui/src/cache.ts` - IndexedDB implementation
- `ui/src/memoryCache.ts` - Memory cache (will be deleted)
- `ui/src/pages/ConversationListPage.tsx` - List page data loading
- `ui/src/pages/ConversationPage.tsx` - Conversation page data loading

#### What NOT to Change

- `ui/src/api.ts` - Base API client, already clean
- `ui/src/hooks/useConnection.ts` - SSE connection logic, works fine
- `ui/src/hooks/useMessageQueue.ts` - Pending message UI state
- `ui/src/syncQueue.ts` - Operation sync logic
- Any component rendering logic (just data loading)
- Any CSS/styling

#### Success Criteria

1. App loads reliably without white screens
2. No TypeScript errors
3. Conversation list shows data (cached first if available, then fresh)
4. Conversation page shows messages with real-time SSE updates
5. Offline mode shows cached data
6. Vite HMR works during development
7. Code is simpler - fewer files, no background operations, no race conditions

---

End of task file.
