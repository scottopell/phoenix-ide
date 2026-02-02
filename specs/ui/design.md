# Web UI Design

## Technology Stack

- **React 18** with TypeScript
- **React Router v6** for client-side routing
- **Vite** for development and building
- **CSS Variables** for theming (no CSS framework)
- **localStorage** for persistence

## Architecture

```
ui/
  src/
    api.ts              # API client and types
    utils.ts            # Shared utilities
    types.ts            # Shared types
    App.tsx             # Router setup
    index.css           # Global styles
    pages/
      ConversationListPage.tsx
      ConversationPage.tsx
    components/
      StateBar.tsx      # Connection status header
      BreadcrumbBar.tsx # Activity breadcrumbs
      MessageList.tsx   # Message display
      InputArea.tsx     # Message composition
      ConversationList.tsx
      NewConversationModal.tsx
    hooks/
      useLocalStorage.ts
      useConnection.ts
      usePendingMessages.ts
```

## Message Delivery State Machine (REQ-UI-004)

```
                    +---> [sent] ✓
                    |
[draft] --send--> [sending] --success--+
                    |                   
                    +--error--> [failed] --retry--> [sending]
                    
[offline] --type--> [pending] --online--> [sending] ---> ...
```

### States

| State | Visual | Stored In | Behavior |
|-------|--------|-----------|----------|
| draft | (none) | localStorage | Persisted per conversation |
| sending | spinner | memory | Request in flight |
| sent | ✓ checkmark | server | Confirmed by API |
| failed | ⚠️ tap to retry | memory | Retryable |
| pending | clock icon | localStorage | Queued for when online |

### localStorage Schema (REQ-UI-011)

```typescript
// Draft message (one per conversation)
localStorage.setItem(`phoenix:draft:${convId}`, "partial message text");

// Pending messages (queued while offline)
localStorage.setItem(`phoenix:pending:${convId}`, JSON.stringify([
  { id: "local-uuid", text: "message 1", timestamp: 1234567890 },
  { id: "local-uuid", text: "message 2", timestamp: 1234567891 }
]));

// Last sequence ID (for reconnection)
localStorage.setItem(`phoenix:lastSeq:${convId}`, "42");
```

## Connection State Machine (REQ-UI-005, REQ-UI-006)

```
[disconnected] --connect--> [connecting] --open--> [connected]
                                 |                      |
                                 +--error--> [reconnecting]
                                                  |
                                 +--------<-------+
                                 |                |
                            (backoff)        (success)
                                 |                |
                                 v                v
                           [reconnecting]   [connected]
                                 |
                           (3+ failures)
                                 |
                                 v
                            [offline]
                                 |
                           (keep trying)
```

### Reconnection Backoff

```typescript
const BACKOFF_BASE = 1000;      // 1 second
const BACKOFF_MAX = 30000;      // 30 seconds
const OFFLINE_THRESHOLD = 3;    // Show "offline" after N failures

function getBackoffDelay(attempt: number): number {
  return Math.min(BACKOFF_BASE * Math.pow(2, attempt - 1), BACKOFF_MAX);
}

// Attempt 1: 1s
// Attempt 2: 2s
// Attempt 3: 4s (show offline banner)
// Attempt 4: 8s
// Attempt 5: 16s
// Attempt 6+: 30s
```

### Reconnection with Sequence Tracking

```typescript
// On each message event
function handleMessage(msg: Message) {
  lastSequenceId.current = msg.sequence_id;
  localStorage.setItem(`phoenix:lastSeq:${convId}`, String(msg.sequence_id));
  // ... update state
}

// On reconnect
function reconnect() {
  const after = lastSequenceId.current 
    || localStorage.getItem(`phoenix:lastSeq:${convId}`);
  const url = after 
    ? `/api/conversations/${convId}/stream?after=${after}`
    : `/api/conversations/${convId}/stream`;
  eventSource = new EventSource(url);
}

// Dedupe safety net
const seenIds = new Set<number>();
function handleMessage(msg: Message) {
  if (seenIds.has(msg.sequence_id)) return; // Already have this one
  seenIds.add(msg.sequence_id);
  // ... process message
}
```

## Offline Message Queue (REQ-UI-004)

```typescript
interface PendingMessage {
  localId: string;      // UUID generated client-side
  text: string;
  images: ImageData[];
  timestamp: number;
  retryCount: number;
}

class MessageQueue {
  private pending: PendingMessage[] = [];
  private storageKey: string;
  
  constructor(conversationId: string) {
    this.storageKey = `phoenix:pending:${conversationId}`;
    this.load();
  }
  
  private load() {
    const stored = localStorage.getItem(this.storageKey);
    this.pending = stored ? JSON.parse(stored) : [];
  }
  
  private save() {
    localStorage.setItem(this.storageKey, JSON.stringify(this.pending));
  }
  
  enqueue(text: string, images: ImageData[] = []) {
    this.pending.push({
      localId: crypto.randomUUID(),
      text,
      images,
      timestamp: Date.now(),
      retryCount: 0
    });
    this.save();
  }
  
  dequeue(localId: string) {
    this.pending = this.pending.filter(m => m.localId !== localId);
    this.save();
  }
  
  getAll(): PendingMessage[] {
    return [...this.pending];
  }
  
  async flush(sendFn: (msg: PendingMessage) => Promise<void>) {
    for (const msg of this.pending) {
      try {
        await sendFn(msg);
        this.dequeue(msg.localId);
      } catch (e) {
        msg.retryCount++;
        this.save();
        throw e; // Stop flush on first failure
      }
    }
  }
}
```

## Component Responsibilities

### ConnectionManager (new hook: useConnection)

Manages SSE connection lifecycle:
- Connect/reconnect with backoff
- Track connection state
- Integrate with `navigator.onLine`
- Emit events for state changes
- Track `lastSequenceId`

### MessageQueue (new hook: usePendingMessages)

Manages offline message queue:
- Persist to localStorage
- Expose pending messages for UI
- Flush on reconnection
- Handle send failures

### InputArea (enhanced)

- Save draft to localStorage on change
- Restore draft on mount
- Enqueue to MessageQueue if offline
- Show pending/sending/failed states

### StateBar (enhanced)

- Show reconnection attempt count
- Show offline banner with countdown
- Respond to navigator.onLine

## Visual States Reference

### Connection Indicator

| State | Dot | Text | Banner |
|-------|-----|------|--------|
| connected | 🟢 | "ready" | none |
| connecting | ⚪ | "connecting..." | none |
| reconnecting | 🟡 | "reconnecting (3)..." | none |
| offline | 🔴 | "offline" | "Reconnecting in 8s..." |

### Message States

| State | Icon | Interactive |
|-------|------|-------------|
| draft | (none) | editable |
| pending | 🕐 | shows in list |
| sending | ⏳ | non-interactive |
| sent | ✓ | normal message |
| failed | ⚠️ | tap to retry |
