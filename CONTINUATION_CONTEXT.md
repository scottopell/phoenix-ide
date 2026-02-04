# Phoenix IDE - Continuation Context for Follow-up Tasks

## Quick Start

You'll be working on performance and UX improvements for Phoenix IDE. The tasks are in `/home/exedev/phoenix-ide/tasks/` - start by reading the specific task file, then use this context to understand the codebase.

## Key Files & Architecture

### Frontend Structure
```
ui/src/
├── pages/
│   ├── ConversationListPage.tsx    # Main list view (check task #310)
│   └── ConversationPage.tsx        # Conversation detail view
├── components/
│   ├── MessageList.tsx             # Renders messages (needs virtual scrolling)
│   └── ConversationList.tsx        # List items (needs preload-on-hover)
├── hooks/
│   ├── useAppMachine.ts           # Global state machine
│   └── useConnection.ts           # SSE connection management
├── enhancedApi.ts                 # API wrapper with caching (some calls bypass this!)
├── cache.ts                       # IndexedDB persistence layer
├── memoryCache.ts                 # In-memory cache with TTL
└── performance.ts                 # Performance monitoring
```

### Backend Structure
```
src/
├── api/
│   └── handlers.rs               # HTTP endpoints (needs ETag support)
└── main.rs                       # Has compression middleware
```

## Core Concepts

### Caching Architecture
- **Two-tier cache**: Memory (5min TTL) → IndexedDB (persistent)
- **Stale-while-revalidate**: Serves old data immediately, refreshes in background
- **enhancedApi**: Wraps base API calls with caching logic
  - Problem: Some components still use `api.` directly (task #304)

### State Management
- **No Redux/Zustand**: Just React state + custom hooks
- **AppMachine**: XState-style state machine for global state
- **Offline-first**: All operations queue in IndexedDB when offline

### Performance Optimizations Done
- Compression (Brotli/gzip) reducing payloads by 85%
- Request deduplication
- SSE connection deferred by 100ms (prevents UI blocking)
- IndexedDB uses `getAll()` instead of cursor iteration

## Design Principles

1. **Honest UI**: Show real state (loading/error), not optimistic updates
2. **Offline-first**: Everything must work without network
3. **Fast navigation**: <50ms for cached content
4. **Single user, multiple devices**: Handle conflicts gracefully

## Common Patterns

### Adding Caching to an API Call
```typescript
// Before:
import { api } from '../api';
await api.someMethod();

// After:
import { enhancedApi } from '../enhancedApi';
await enhancedApi.someMethod();
```

### Performance Logging
```typescript
performanceMonitor.recordCacheHit('memory');
performanceMonitor.recordNetworkRequest(duration);
```

### Loading States
```typescript
// Correct order (prevents flash):
if (loading) return <Spinner />;
if (data.length === 0) return <EmptyState />;
return <DataList data={data} />;
```

## Testing Approach

1. **Manual**: Use `?debug=1` to see performance metrics
2. **Console**: Look for `[ConversationPage]`, `[IndexedDB]`, etc. logs
3. **Network**: Chrome DevTools → Offline mode for offline testing
4. **Storage**: Chrome DevTools → Application → IndexedDB

## Current Pain Points

- Large conversations (100+ messages) render slowly
- Storage quota warnings only in console
- No conflict resolution UI
- Bundle size growing (currently 226KB/71KB gzipped)
- Some API calls bypass cache layer

## Deployment

```bash
# Build and test locally
cd ui && npm run build

# Deploy to production
python3 dev.py prod deploy

# Production URL
https://meteor-rain.exe.xyz:7331/
```

Start with the task file in `tasks/`, use this context to understand the codebase, and remember the design principles. The user values offline functionality and honest UI states above all else.
