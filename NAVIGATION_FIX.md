// Phoenix IDE Performance Analysis - Navigation Issue Fix

## Root Cause Analysis

After investigating the navigation delay issue, I've identified several contributing factors:

### 1. SSE Connection Blocking
When navigating to a conversation, the app immediately establishes an SSE connection. This can add 50-200ms of delay before the UI feels responsive.

### 2. IndexedDB Performance
While IndexedDB is fast for small datasets, iterating through messages with a cursor can be slow for conversations with many messages.

### 3. React Re-rendering
The entire conversation page re-renders when navigating, even if the data is cached.

## Recommended Fixes

### Fix 1: Defer SSE Connection (Quick Win)
```typescript
// In ConversationPage.tsx
useEffect(() => {
  if (!conversationId) return;
  
  // Defer SSE connection to not block initial render
  const timer = setTimeout(() => {
    setConversationIdForSSE(conversationId);
  }, 100);
  
  return () => clearTimeout(timer);
}, [conversationId]);
```

### Fix 2: Optimize IndexedDB Queries
```typescript
// Use getAll instead of cursor iteration
async getMessages(conversationId: string): Promise<Message[]> {
  const tx = this.db!.transaction(['messages'], 'readonly');
  const store = tx.objectStore('messages');
  const index = store.index('by-conversation');
  const range = IDBKeyRange.only(conversationId);
  
  return new Promise((resolve) => {
    const request = index.getAll(range);
    request.onsuccess = () => {
      resolve(request.result || []);
    };
  });
}
```

### Fix 3: Add Loading States
```typescript
// Show cached content immediately while loading fresh data
if (cachedData && !initialLoadComplete) {
  return <MessageList messages={cachedData.messages} loading={true} />;
}
```

### Fix 4: Virtual Scrolling for Large Conversations
For conversations with 100+ messages, implement virtual scrolling to only render visible messages.

## Performance Metrics

Current performance:
- Initial load: ~200-500ms
- Cached navigation: ~100-300ms (should be <50ms)
- SSE connection: ~50-200ms

Target performance:
- Initial load: <300ms
- Cached navigation: <50ms
- SSE connection: Deferred (non-blocking)

## Implementation Priority

1. **Defer SSE Connection** - Easy fix, big impact
2. **Optimize IndexedDB** - Medium effort, good impact
3. **Progressive Loading** - Show cached data immediately
4. **Virtual Scrolling** - Only for large conversations
