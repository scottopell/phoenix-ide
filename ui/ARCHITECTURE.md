# Phoenix IDE Frontend Architecture

## Caching Architecture

Phoenix IDE implements a sophisticated multi-tier caching system designed for offline-first functionality and optimal performance on slow networks.

### Cache Hierarchy

```
Network Request
      ↑
      | (miss)
┌─────┴──────┐
│   Memory   │ ← 5 minute TTL
│   Cache    │ ← Instant access
└─────┬──────┘
      | (miss)
┌─────┴──────┐
│  IndexedDB │ ← Persistent
│   Cache    │ ← Survives reload
└────────────┘
```

### Key Features

1. **Stale-While-Revalidate Pattern**
   - Serves stale data immediately
   - Refreshes in background if data >5 minutes old
   - User sees content instantly

2. **Request Deduplication**
   - Concurrent identical requests share same promise
   - Prevents redundant API calls
   - Especially useful during rapid navigation

3. **Offline Queue**
   - Operations stored in IndexedDB when offline
   - Automatic sync when connection returns
   - Exponential backoff for failed syncs

4. **Smart Invalidation**
   - Mutations update cache immediately
   - Event-based invalidation via SSE
   - 5-minute fallback for missed events

### Storage Management

- **Quota Handling**: Automatic cleanup when approaching limits
- **Retention Policy**: Conversations unused for 30 days are purged
- **Warning Threshold**: User warned at 100MB usage

### Performance Metrics

The app tracks:
- Cache hit rate (target: >90%)
- Average response time
- Network request count

Access metrics via `?debug=1` URL parameter.

## State Management

### AppMachine

Global state machine managing:
- Online/offline status
- Sync queue processing
- Error recovery with retry

### State Diagram

```
initializing → ready ←→ error
                ↓
              online ←→ offline
                ↓
              syncing
```

### Offline Behavior

1. **Message Queueing**
   - Messages saved to IndexedDB
   - UI shows pending state
   - Sent in order when online

2. **Visual Indicators**
   - Red banner when offline
   - Pending operation count
   - Sync progress bar

3. **Conflict Resolution**
   - API is source of truth
   - Last-write-wins for most fields
   - Local pending operations preserved

## Testing

### Automated Tests

1. **State Machine Properties**
   - No operations lost during transitions
   - No sync attempts while offline
   - Errors trigger retry with backoff

2. **Cache Behavior**
   - Correct cascade (memory → IndexedDB → network)
   - Background refresh for stale data
   - Proper invalidation on mutations

3. **Offline Scenarios**
   - Subway test: queue multiple messages offline
   - Intermittent connectivity handling
   - Sync failure recovery

### Manual Testing

1. **Performance Verification**
   ```bash
   # Enable debug dashboard
   open http://localhost:8000?debug=1
   
   # Monitor cache hit rate
   # Should see >90% after initial load
   ```

2. **Offline Testing**
   ```bash
   # Chrome DevTools
   # Network tab → Offline
   # Try sending messages
   # Go back online → verify sync
   ```

3. **Slow Network Testing**
   ```bash
   # Chrome DevTools  
   # Network tab → Slow 3G
   # Navigation should be instant
   ```

## Troubleshooting

### Common Issues

1. **"Quota Exceeded" Errors**
   - App automatically purges old conversations
   - Manual fix: Clear site data in browser settings

2. **Stale Data**
   - Click refresh button in UI
   - Hard refresh: Ctrl+Shift+R

3. **Sync Stuck**
   - Check browser console for errors
   - Verify network connectivity
   - App retries with exponential backoff

### Debug Tools

1. **Performance Dashboard**: `?debug=1`
2. **Console Logs**: `localStorage.debug = 'phoenix:*'`
3. **IndexedDB Inspector**: Chrome DevTools → Application
4. **Network Activity**: Chrome DevTools → Network

## Best Practices

1. **Always Handle Offline**
   - Check `isOnline` before network operations
   - Queue operations when offline
   - Show appropriate UI feedback

2. **Cache Responsibly**
   - Don't cache sensitive data
   - Respect cache headers from API
   - Implement proper invalidation

3. **Performance First**
   - Serve cached data immediately
   - Refresh in background
   - Minimize blocking operations
